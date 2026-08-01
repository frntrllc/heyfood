//! Canonical JSON and digest contracts for native household state.
//!
//! Household persistence must never rely on incidental `serde_json` map or
//! number emission. This module first admits raw JSON through a bounded,
//! duplicate-detecting token scan and then emits RFC 8785 compatible bytes.

use std::{cell::Cell, cmp::Ordering, collections::BTreeSet, fmt};

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{DeserializeSeed, Error as _, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};

/// Domain label shared by every D2 canonical household digest.
pub const CANONICAL_BYTES_V1_CONTRACT: &str = "heyfood.household.canonical.v1";

/// Largest accepted safe I-JSON integer.
pub const MAX_SAFE_IJSON_INTEGER: i64 = 9_007_199_254_740_991;
/// Smallest accepted safe I-JSON integer.
pub const MIN_SAFE_IJSON_INTEGER: i64 = -9_007_199_254_740_991;
const MAX_COMPATIBILITY_STRING_BYTES: usize = 2_048;
const MAX_RAW_JSON_STRING_BYTES: usize = 2 + (6 * MAX_COMPATIBILITY_STRING_BYTES);

/// Recursive allocation limits for one compatibility JSON candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompatibilityJsonLimitsV1 {
    pub maximum_bytes: usize,
    pub maximum_depth: usize,
    pub maximum_object_keys: usize,
    pub maximum_array_entries: usize,
    pub maximum_nodes: usize,
}

impl CompatibilityJsonLimitsV1 {
    pub const PROFILE_DOCUMENT: Self = Self {
        maximum_bytes: 256 * 1024,
        maximum_depth: 8,
        maximum_object_keys: 128,
        maximum_array_entries: 256,
        maximum_nodes: 65_536,
    };

    pub const MIGRATION_CANDIDATE: Self = Self {
        maximum_bytes: 4 * 1024 * 1024,
        ..Self::PROFILE_DOCUMENT
    };

    pub const OWNER_SYNC_REQUEST: Self = Self {
        maximum_bytes: 524_288,
        ..Self::PROFILE_DOCUMENT
    };

    pub const VAULT_PLAINTEXT: Self = Self {
        maximum_bytes: 8 * 1024 * 1024,
        maximum_depth: 16,
        maximum_object_keys: 128,
        maximum_array_entries: 16_384,
        maximum_nodes: 262_144,
    };
}

/// Content-free canonicalization failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanonicalJsonError {
    InputTooLarge,
    InvalidSyntax,
    DuplicateObjectName,
    MaximumDepthExceeded,
    MaximumObjectKeysExceeded,
    MaximumArrayEntriesExceeded,
    MaximumNodesExceeded,
    UnsafeInteger,
    NonFiniteNumber,
    UnrepresentableNumber,
    ExpectedObject,
    Serialization,
    InvalidDigest,
    InvalidDomainLabel,
    DomainPartTooLarge,
}

impl fmt::Display for CanonicalJsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InputTooLarge => "canonical JSON input exceeds its byte limit",
            Self::InvalidSyntax => "canonical JSON input has invalid syntax",
            Self::DuplicateObjectName => "canonical JSON contains a duplicate object name",
            Self::MaximumDepthExceeded => "canonical JSON exceeds its depth limit",
            Self::MaximumObjectKeysExceeded => "canonical JSON exceeds its object-key limit",
            Self::MaximumArrayEntriesExceeded => "canonical JSON exceeds its array-entry limit",
            Self::MaximumNodesExceeded => "canonical JSON exceeds its node limit",
            Self::UnsafeInteger => "canonical JSON contains an unsafe integer",
            Self::NonFiniteNumber => "canonical JSON contains a non-finite number",
            Self::UnrepresentableNumber => {
                "canonical JSON contains a number outside the binary64 contract"
            }
            Self::ExpectedObject => "canonical JSON value must be an object",
            Self::Serialization => "canonical JSON serialization failed",
            Self::InvalidDigest => "canonical digest is invalid",
            Self::InvalidDomainLabel => "domain hash label must be nonempty ASCII",
            Self::DomainPartTooLarge => "domain hash part exceeds the u32 length contract",
        })
    }
}

impl std::error::Error for CanonicalJsonError {}

/// A SHA-256 digest serialized as exactly 64 lowercase hexadecimal characters.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CanonicalDigestV1([u8; 32]);

impl CanonicalDigestV1 {
    #[must_use]
    pub const fn from_bytes(value: [u8; 32]) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    #[must_use]
    pub fn to_lower_hex(self) -> String {
        encode_lower_hex(&self.0)
    }
}

impl fmt::Debug for CanonicalDigestV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("CanonicalDigestV1")
            .field(&self.to_lower_hex())
            .finish()
    }
}

impl Serialize for CanonicalDigestV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_lower_hex())
    }
}

impl<'de> Deserialize<'de> for CanonicalDigestV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        decode_lower_hex_32(&String::deserialize(deserializer)?)
            .map(Self)
            .map_err(D::Error::custom)
    }
}

/// Validated canonical JSON object. Its debug representation never emits data.
#[derive(Clone, Eq, PartialEq)]
pub struct CanonicalJsonObjectV1 {
    value: Map<String, Value>,
    canonical_sha256: CanonicalDigestV1,
    canonical_len: usize,
}

impl CanonicalJsonObjectV1 {
    pub fn parse(
        input: &[u8],
        limits: CompatibilityJsonLimitsV1,
    ) -> Result<Self, CanonicalJsonError> {
        let value = parse_bounded_json_object_v1(input, limits)?;
        Self::from_map(value, limits.maximum_bytes)
    }

    pub fn from_map(
        value: Map<String, Value>,
        maximum_bytes: usize,
    ) -> Result<Self, CanonicalJsonError> {
        let value = Value::Object(value);
        let mut nodes = 0;
        validate_value_shape(
            &value,
            1,
            &mut nodes,
            CompatibilityJsonLimitsV1 {
                maximum_bytes,
                ..CompatibilityJsonLimitsV1::PROFILE_DOCUMENT
            },
        )?;
        validate_value_numeric_domain(&value)?;
        let canonical = canonicalize_json_value_v1(&value)?;
        if canonical.len() > maximum_bytes {
            return Err(CanonicalJsonError::InputTooLarge);
        }
        let canonical_sha256 = sha256_bytes(&canonical);
        let Value::Object(value) = value else {
            unreachable!("value was constructed as an object");
        };
        Ok(Self {
            value,
            canonical_sha256,
            canonical_len: canonical.len(),
        })
    }

    #[must_use]
    pub fn as_map(&self) -> &Map<String, Value> {
        &self.value
    }

    #[must_use]
    pub const fn canonical_sha256(&self) -> CanonicalDigestV1 {
        self.canonical_sha256
    }

    #[must_use]
    pub const fn canonical_len(&self) -> usize {
        self.canonical_len
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        canonicalize_json_value_v1(&Value::Object(self.value.clone()))
    }
}

impl fmt::Debug for CanonicalJsonObjectV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CanonicalJsonObjectV1")
            .field("keys", &self.value.len())
            .field("canonical_len", &self.canonical_len)
            .field("canonical_sha256", &self.canonical_sha256)
            .finish()
    }
}

impl Serialize for CanonicalJsonObjectV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.value.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CanonicalJsonObjectV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = deserialize_bounded_value_v1(
            deserializer,
            CompatibilityJsonLimitsV1::PROFILE_DOCUMENT,
        )?;
        let Value::Object(value) = value else {
            return Err(D::Error::custom(CanonicalJsonError::ExpectedObject));
        };
        Self::from_map(
            value,
            CompatibilityJsonLimitsV1::VAULT_PLAINTEXT.maximum_bytes,
        )
        .map_err(D::Error::custom)
    }
}

/// Validated canonical JSON value used for scalar-or-structured compatibility
/// archives. Its debug representation never emits the archived value.
#[derive(Clone, Eq, PartialEq)]
pub struct CanonicalJsonValueV1 {
    value: Value,
    canonical_sha256: CanonicalDigestV1,
    canonical_len: usize,
}

impl CanonicalJsonValueV1 {
    pub fn parse(
        input: &[u8],
        limits: CompatibilityJsonLimitsV1,
    ) -> Result<Self, CanonicalJsonError> {
        let value = parse_bounded_json_v1(input, limits)?;
        Self::from_value(value, limits.maximum_bytes)
    }

    pub fn from_value(value: Value, maximum_bytes: usize) -> Result<Self, CanonicalJsonError> {
        let mut nodes = 0;
        validate_value_shape(
            &value,
            1,
            &mut nodes,
            CompatibilityJsonLimitsV1 {
                maximum_bytes,
                ..CompatibilityJsonLimitsV1::PROFILE_DOCUMENT
            },
        )?;
        validate_value_numeric_domain(&value)?;
        let canonical = canonicalize_json_value_v1(&value)?;
        if canonical.len() > maximum_bytes {
            return Err(CanonicalJsonError::InputTooLarge);
        }
        Ok(Self {
            value,
            canonical_sha256: sha256_bytes(&canonical),
            canonical_len: canonical.len(),
        })
    }

    #[must_use]
    pub const fn as_value(&self) -> &Value {
        &self.value
    }

    #[must_use]
    pub const fn canonical_sha256(&self) -> CanonicalDigestV1 {
        self.canonical_sha256
    }

    #[must_use]
    pub const fn canonical_len(&self) -> usize {
        self.canonical_len
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        canonicalize_json_value_v1(&self.value)
    }
}

impl fmt::Debug for CanonicalJsonValueV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CanonicalJsonValueV1")
            .field("canonical_len", &self.canonical_len)
            .field("canonical_sha256", &self.canonical_sha256)
            .finish()
    }
}

impl Serialize for CanonicalJsonValueV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.value.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CanonicalJsonValueV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = deserialize_bounded_value_v1(
            deserializer,
            CompatibilityJsonLimitsV1::PROFILE_DOCUMENT,
        )?;
        Self::from_value(
            value,
            CompatibilityJsonLimitsV1::VAULT_PLAINTEXT.maximum_bytes,
        )
        .map_err(D::Error::custom)
    }
}

const SERDE_JSON_ARBITRARY_NUMBER_TOKEN: &str = "$serde_json::private::Number";

fn deserialize_bounded_value_v1<'de, D>(
    deserializer: D,
    limits: CompatibilityJsonLimitsV1,
) -> Result<Value, D::Error>
where
    D: Deserializer<'de>,
{
    let nodes = Cell::new(0);
    BoundedValueSeedV1 {
        depth: 1,
        nodes: &nodes,
        limits,
    }
    .deserialize(deserializer)
}

#[derive(Clone, Copy)]
struct BoundedValueSeedV1<'limits> {
    depth: usize,
    nodes: &'limits Cell<usize>,
    limits: CompatibilityJsonLimitsV1,
}

impl BoundedValueSeedV1<'_> {
    fn child(self) -> Self {
        Self {
            depth: self.depth + 1,
            ..self
        }
    }
}

impl<'de> DeserializeSeed<'de> for BoundedValueSeedV1<'_> {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        if self.depth > self.limits.maximum_depth {
            return Err(D::Error::custom(CanonicalJsonError::MaximumDepthExceeded));
        }
        let nodes = self
            .nodes
            .get()
            .checked_add(1)
            .ok_or_else(|| D::Error::custom(CanonicalJsonError::MaximumNodesExceeded))?;
        if nodes > self.limits.maximum_nodes {
            return Err(D::Error::custom(CanonicalJsonError::MaximumNodesExceeded));
        }
        self.nodes.set(nodes);
        deserializer.deserialize_any(BoundedValueVisitorV1 { seed: self })
    }
}

struct BoundedValueVisitorV1<'limits> {
    seed: BoundedValueSeedV1<'limits>,
}

struct RejectArrayOverflowSeedV1;

impl<'de> DeserializeSeed<'de> for RejectArrayOverflowSeedV1 {
    type Value = Value;

    fn deserialize<D>(self, _deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        Err(D::Error::custom(
            CanonicalJsonError::MaximumArrayEntriesExceeded,
        ))
    }
}

struct RejectObjectOverflowSeedV1;

impl<'de> DeserializeSeed<'de> for RejectObjectOverflowSeedV1 {
    type Value = String;

    fn deserialize<D>(self, _deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        Err(D::Error::custom(
            CanonicalJsonError::MaximumObjectKeysExceeded,
        ))
    }
}

impl<'de> Visitor<'de> for BoundedValueVisitorV1<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded canonical JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if !(MIN_SAFE_IJSON_INTEGER..=MAX_SAFE_IJSON_INTEGER).contains(&value) {
            return Err(E::custom(CanonicalJsonError::UnsafeInteger));
        }
        Ok(Value::Number(value.into()))
    }

    fn visit_i128<E>(self, value: i128) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        let value =
            i64::try_from(value).map_err(|_| E::custom(CanonicalJsonError::UnsafeInteger))?;
        self.visit_i64(value)
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if value > u64::try_from(MAX_SAFE_IJSON_INTEGER).expect("positive safe integer") {
            return Err(E::custom(CanonicalJsonError::UnsafeInteger));
        }
        Ok(Value::Number(value.into()))
    }

    fn visit_u128<E>(self, value: u128) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        let value =
            u64::try_from(value).map_err(|_| E::custom(CanonicalJsonError::UnsafeInteger))?;
        self.visit_u64(value)
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if !value.is_finite() {
            return Err(E::custom(CanonicalJsonError::NonFiniteNumber));
        }
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom(CanonicalJsonError::UnrepresentableNumber))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if value.len() > MAX_COMPATIBILITY_STRING_BYTES {
            return Err(E::custom(CanonicalJsonError::InputTooLarge));
        }
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if value.len() > MAX_COMPATIBILITY_STRING_BYTES {
            return Err(E::custom(CanonicalJsonError::InputTooLarge));
        }
        Ok(Value::String(value))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        loop {
            if values.len() == self.seed.limits.maximum_array_entries {
                if sequence
                    .next_element_seed(RejectArrayOverflowSeedV1)?
                    .is_none()
                {
                    break;
                }
                unreachable!("overflow seed always rejects a present array element");
            }
            let Some(value) = sequence.next_element_seed(self.seed.child())? else {
                break;
            };
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let Some(first_key) = object.next_key::<String>()? else {
            return Ok(Value::Object(Map::new()));
        };
        if first_key == SERDE_JSON_ARBITRARY_NUMBER_TOKEN {
            let token = object.next_value::<String>()?;
            if object.next_key::<String>()?.is_some() {
                return Err(A::Error::custom(CanonicalJsonError::InvalidSyntax));
            }
            validate_number_token(&token).map_err(A::Error::custom)?;
            return token
                .parse::<serde_json::Number>()
                .map(Value::Number)
                .map_err(|_| A::Error::custom(CanonicalJsonError::UnrepresentableNumber));
        }

        let mut names = BTreeSet::new();
        let mut values = Map::new();
        validate_bounded_map_key::<A::Error>(&first_key)?;
        names.insert(first_key.clone());
        values.insert(first_key, object.next_value_seed(self.seed.child())?);
        loop {
            if values.len() == self.seed.limits.maximum_object_keys {
                if object.next_key_seed(RejectObjectOverflowSeedV1)?.is_none() {
                    break;
                }
                unreachable!("overflow seed always rejects a present object name");
            }
            let Some(key) = object.next_key::<String>()? else {
                break;
            };
            validate_bounded_map_key::<A::Error>(&key)?;
            if !names.insert(key.clone()) {
                return Err(A::Error::custom(CanonicalJsonError::DuplicateObjectName));
            }
            values.insert(key, object.next_value_seed(self.seed.child())?);
        }
        Ok(Value::Object(values))
    }
}

fn validate_bounded_map_key<E>(key: &str) -> Result<(), E>
where
    E: serde::de::Error,
{
    if key.len() > MAX_COMPATIBILITY_STRING_BYTES {
        Err(E::custom(CanonicalJsonError::InputTooLarge))
    } else {
        Ok(())
    }
}

/// Encode any serializable value using Canonical Bytes v1.
pub fn to_canonical_bytes_v1<T: Serialize + ?Sized>(
    value: &T,
) -> Result<Vec<u8>, CanonicalJsonError> {
    let value = serde_json::to_value(value).map_err(|_| CanonicalJsonError::Serialization)?;
    let mut output = Vec::new();
    encode_value(&value, &mut output)?;
    Ok(output)
}

/// Hash Canonical Bytes v1 with SHA-256.
pub fn canonical_sha256_v1<T: Serialize + ?Sized>(
    value: &T,
) -> Result<CanonicalDigestV1, CanonicalJsonError> {
    to_canonical_bytes_v1(value).map(|bytes| sha256_bytes(&bytes))
}

/// Emit an already parsed JSON value using RFC 8785 property ordering and
/// binary64 number formatting.
pub fn canonicalize_json_value_v1(value: &Value) -> Result<Vec<u8>, CanonicalJsonError> {
    validate_value_numeric_domain(value)?;
    let mut output = Vec::new();
    encode_value(value, &mut output)?;
    Ok(output)
}

/// Parse bounded raw JSON while rejecting duplicate names and unsafe numeric
/// tokens before generic JSON conversion.
pub fn parse_bounded_json_v1(
    input: &[u8],
    limits: CompatibilityJsonLimitsV1,
) -> Result<Value, CanonicalJsonError> {
    parse_bounded_json_internal(input, limits, true)
}

/// Parse a typed canonical document. Declared integer widths are validated by
/// its typed deserializer; recursively embedded `CanonicalJsonObjectV1`
/// values still enforce the safe compatibility-number domain on construction.
pub fn parse_bounded_typed_json_v1(
    input: &[u8],
    limits: CompatibilityJsonLimitsV1,
) -> Result<Value, CanonicalJsonError> {
    parse_bounded_json_internal(input, limits, false)
}

/// Scan a typed canonical document without materializing a generic JSON value.
///
/// `root_array_limits` applies exact schema-aware caps to named arrays on the
/// root object. The scan completes before either `serde_json::Value` or a
/// typed `Vec` can allocate from attacker-controlled collection lengths.
pub(crate) fn preflight_bounded_typed_json_v1(
    input: &[u8],
    limits: CompatibilityJsonLimitsV1,
    root_array_limits: &[(&str, usize)],
) -> Result<(), CanonicalJsonError> {
    preflight_json_internal(input, limits, false, root_array_limits)
}

fn parse_bounded_json_internal(
    input: &[u8],
    limits: CompatibilityJsonLimitsV1,
    enforce_safe_integer_domain: bool,
) -> Result<Value, CanonicalJsonError> {
    preflight_json_internal(input, limits, enforce_safe_integer_domain, &[])?;
    serde_json::from_slice(input).map_err(|_| CanonicalJsonError::InvalidSyntax)
}

fn preflight_json_internal(
    input: &[u8],
    limits: CompatibilityJsonLimitsV1,
    enforce_safe_integer_domain: bool,
    root_array_limits: &[(&str, usize)],
) -> Result<(), CanonicalJsonError> {
    if input.len() > limits.maximum_bytes {
        return Err(CanonicalJsonError::InputTooLarge);
    }
    let mut scanner = JsonScanner {
        input,
        cursor: 0,
        limits,
        nodes: 0,
        enforce_safe_integer_domain,
        root_array_limits,
    };
    scanner.scan_value(1)?;
    scanner.skip_whitespace();
    if scanner.cursor != input.len() {
        return Err(CanonicalJsonError::InvalidSyntax);
    }
    Ok(())
}

pub fn parse_bounded_json_object_v1(
    input: &[u8],
    limits: CompatibilityJsonLimitsV1,
) -> Result<Map<String, Value>, CanonicalJsonError> {
    match parse_bounded_json_v1(input, limits)? {
        Value::Object(value) => Ok(value),
        _ => Err(CanonicalJsonError::ExpectedObject),
    }
}

/// Length-delimited domain hash used by account/root/locator identities.
pub fn domain_hash_v1(
    label: &str,
    parts: &[&[u8]],
) -> Result<CanonicalDigestV1, CanonicalJsonError> {
    if label.is_empty() || !label.is_ascii() {
        return Err(CanonicalJsonError::InvalidDomainLabel);
    }
    let mut hash = Sha256::new();
    hash.update(label.as_bytes());
    hash.update([0]);
    for part in parts {
        let length =
            u32::try_from(part.len()).map_err(|_| CanonicalJsonError::DomainPartTooLarge)?;
        hash.update(length.to_be_bytes());
        hash.update(part);
    }
    Ok(CanonicalDigestV1::from_bytes(hash.finalize().into()))
}

#[must_use]
pub fn encode_lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

pub fn decode_lower_hex_32(value: &str) -> Result<[u8; 32], CanonicalJsonError> {
    if value.len() != 64
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
    {
        return Err(CanonicalJsonError::InvalidDigest);
    }
    let mut decoded = [0_u8; 32];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        decoded[index] = (hex_nibble(chunk[0])? << 4) | hex_nibble(chunk[1])?;
    }
    Ok(decoded)
}

fn hex_nibble(value: u8) -> Result<u8, CanonicalJsonError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(CanonicalJsonError::InvalidDigest),
    }
}

fn sha256_bytes(bytes: &[u8]) -> CanonicalDigestV1 {
    CanonicalDigestV1::from_bytes(Sha256::digest(bytes).into())
}

fn validate_value_numeric_domain(value: &Value) -> Result<(), CanonicalJsonError> {
    match value {
        Value::Null | Value::Bool(_) | Value::String(_) => Ok(()),
        Value::Number(number) => validate_number_token(number.as_str()).map(|_| ()),
        Value::Array(values) => values.iter().try_for_each(validate_value_numeric_domain),
        Value::Object(values) => values.values().try_for_each(validate_value_numeric_domain),
    }
}

fn validate_value_shape(
    value: &Value,
    depth: usize,
    nodes: &mut usize,
    limits: CompatibilityJsonLimitsV1,
) -> Result<(), CanonicalJsonError> {
    if depth > limits.maximum_depth {
        return Err(CanonicalJsonError::MaximumDepthExceeded);
    }
    *nodes = nodes
        .checked_add(1)
        .ok_or(CanonicalJsonError::MaximumNodesExceeded)?;
    if *nodes > limits.maximum_nodes {
        return Err(CanonicalJsonError::MaximumNodesExceeded);
    }
    match value {
        Value::String(value) if value.len() > MAX_COMPATIBILITY_STRING_BYTES => {
            Err(CanonicalJsonError::InputTooLarge)
        }
        Value::Array(values) => {
            if values.len() > limits.maximum_array_entries {
                return Err(CanonicalJsonError::MaximumArrayEntriesExceeded);
            }
            values
                .iter()
                .try_for_each(|value| validate_value_shape(value, depth + 1, nodes, limits))
        }
        Value::Object(values) => {
            if values.len() > limits.maximum_object_keys {
                return Err(CanonicalJsonError::MaximumObjectKeysExceeded);
            }
            if values
                .keys()
                .any(|key| key.len() > MAX_COMPATIBILITY_STRING_BYTES)
            {
                return Err(CanonicalJsonError::InputTooLarge);
            }
            values
                .values()
                .try_for_each(|value| validate_value_shape(value, depth + 1, nodes, limits))
        }
        _ => Ok(()),
    }
}

fn encode_value(value: &Value, output: &mut Vec<u8>) -> Result<(), CanonicalJsonError> {
    match value {
        Value::Null => output.extend_from_slice(b"null"),
        Value::Bool(true) => output.extend_from_slice(b"true"),
        Value::Bool(false) => output.extend_from_slice(b"false"),
        Value::String(value) => {
            let escaped =
                serde_json::to_string(value).map_err(|_| CanonicalJsonError::Serialization)?;
            output.extend_from_slice(escaped.as_bytes());
        }
        Value::Number(number) => {
            output.extend_from_slice(canonical_number(number.as_str())?.as_bytes());
        }
        Value::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                encode_value(value, output)?;
            }
            output.push(b']');
        }
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_by(|left, right| compare_utf16(left, right));
            output.push(b'{');
            for (index, key) in keys.into_iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                let escaped =
                    serde_json::to_string(key).map_err(|_| CanonicalJsonError::Serialization)?;
                output.extend_from_slice(escaped.as_bytes());
                output.push(b':');
                encode_value(&values[key], output)?;
            }
            output.push(b'}');
        }
    }
    Ok(())
}

fn compare_utf16(left: &str, right: &str) -> Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

fn canonical_number(token: &str) -> Result<String, CanonicalJsonError> {
    if !token.contains(['.', 'e', 'E']) {
        return Ok(if token == "-0" { "0" } else { token }.to_owned());
    }
    let number = validate_number_token(token)?;
    if number == 0.0 {
        return Ok("0".to_owned());
    }
    format_binary64(number)
}

fn validate_number_token(token: &str) -> Result<f64, CanonicalJsonError> {
    if !token.contains(['.', 'e', 'E']) {
        validate_safe_integer_token(token)?;
    }
    let value = token
        .parse::<f64>()
        .map_err(|_| CanonicalJsonError::UnrepresentableNumber)?;
    if !value.is_finite() {
        return Err(CanonicalJsonError::NonFiniteNumber);
    }
    if value == 0.0
        && token
            .bytes()
            .any(|byte| byte.is_ascii_digit() && byte != b'0')
    {
        return Err(CanonicalJsonError::UnrepresentableNumber);
    }
    Ok(value)
}

fn validate_safe_integer_token(token: &str) -> Result<(), CanonicalJsonError> {
    let digits = token.strip_prefix('-').unwrap_or(token);
    let maximum = "9007199254740991";
    if digits.len() > maximum.len() || (digits.len() == maximum.len() && digits > maximum) {
        return Err(CanonicalJsonError::UnsafeInteger);
    }
    Ok(())
}

fn format_binary64(value: f64) -> Result<String, CanonicalJsonError> {
    let source = value.to_string();
    let (negative, unsigned) = source
        .strip_prefix('-')
        .map_or((false, source.as_str()), |value| (true, value));
    let (mantissa, explicit_exponent) =
        unsigned
            .split_once(['e', 'E'])
            .map_or((unsigned, 0_i32), |(mantissa, exponent)| {
                (
                    mantissa,
                    exponent
                        .parse::<i32>()
                        .expect("Rust binary64 formatter emits a bounded exponent"),
                )
            });
    let decimal_index = mantissa.find('.').unwrap_or(mantissa.len());
    let mut digits = mantissa
        .bytes()
        .filter(|byte| *byte != b'.')
        .map(char::from)
        .collect::<String>();
    let mut decimal_position = i32::try_from(decimal_index)
        .map_err(|_| CanonicalJsonError::Serialization)?
        + explicit_exponent;
    let leading_zeroes = digits.bytes().take_while(|byte| *byte == b'0').count();
    digits.drain(..leading_zeroes);
    decimal_position -=
        i32::try_from(leading_zeroes).map_err(|_| CanonicalJsonError::Serialization)?;
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
    }
    if digits.is_empty() {
        return Ok("0".to_owned());
    }
    let scientific_exponent = decimal_position - 1;
    let mut output = String::new();
    if negative {
        output.push('-');
    }
    if (-6..=20).contains(&scientific_exponent) {
        if decimal_position <= 0 {
            output.push_str("0.");
            output.extend(std::iter::repeat_n(
                '0',
                usize::try_from(-decimal_position)
                    .map_err(|_| CanonicalJsonError::Serialization)?,
            ));
            output.push_str(&digits);
        } else {
            let decimal_position =
                usize::try_from(decimal_position).map_err(|_| CanonicalJsonError::Serialization)?;
            if decimal_position >= digits.len() {
                output.push_str(&digits);
                output.extend(std::iter::repeat_n(
                    '0',
                    decimal_position.saturating_sub(digits.len()),
                ));
            } else {
                output.push_str(&digits[..decimal_position]);
                output.push('.');
                output.push_str(&digits[decimal_position..]);
            }
        }
    } else {
        output.push(char::from(digits.as_bytes()[0]));
        if digits.len() > 1 {
            output.push('.');
            output.push_str(&digits[1..]);
        }
        output.push('e');
        if scientific_exponent >= 0 {
            output.push('+');
        }
        output.push_str(&scientific_exponent.to_string());
    }
    Ok(output)
}

struct JsonScanner<'input, 'limits> {
    input: &'input [u8],
    cursor: usize,
    limits: CompatibilityJsonLimitsV1,
    nodes: usize,
    enforce_safe_integer_domain: bool,
    root_array_limits: &'limits [(&'limits str, usize)],
}

impl JsonScanner<'_, '_> {
    fn scan_value(&mut self, depth: usize) -> Result<(), CanonicalJsonError> {
        self.scan_value_with_array_limit(depth, None)
    }

    fn scan_value_with_array_limit(
        &mut self,
        depth: usize,
        exact_array_limit: Option<usize>,
    ) -> Result<(), CanonicalJsonError> {
        if depth > self.limits.maximum_depth {
            return Err(CanonicalJsonError::MaximumDepthExceeded);
        }
        self.nodes = self
            .nodes
            .checked_add(1)
            .ok_or(CanonicalJsonError::MaximumNodesExceeded)?;
        if self.nodes > self.limits.maximum_nodes {
            return Err(CanonicalJsonError::MaximumNodesExceeded);
        }
        self.skip_whitespace();
        match self.peek() {
            Some(b'{') => self.scan_object(depth),
            Some(b'[') => self.scan_array(
                depth,
                exact_array_limit
                    .unwrap_or(self.limits.maximum_array_entries)
                    .min(self.limits.maximum_array_entries),
            ),
            Some(b'"') => self.scan_string().map(|_| ()),
            Some(b't') => self.scan_literal(b"true"),
            Some(b'f') => self.scan_literal(b"false"),
            Some(b'n') => self.scan_literal(b"null"),
            Some(b'-' | b'0'..=b'9') => self.scan_number(),
            _ => Err(CanonicalJsonError::InvalidSyntax),
        }
    }

    fn scan_object(&mut self, depth: usize) -> Result<(), CanonicalJsonError> {
        self.cursor += 1;
        self.skip_whitespace();
        if self.take(b'}') {
            return Ok(());
        }
        let mut names = BTreeSet::new();
        let mut entries = 0_usize;
        loop {
            self.skip_whitespace();
            if self.peek() != Some(b'"') {
                return Err(CanonicalJsonError::InvalidSyntax);
            }
            let name = self.scan_string()?;
            let exact_array_limit = (depth == 1)
                .then(|| {
                    self.root_array_limits
                        .iter()
                        .find_map(|(field_name, maximum)| {
                            (*field_name == name.as_str()).then_some(*maximum)
                        })
                })
                .flatten();
            entries += 1;
            if entries > self.limits.maximum_object_keys {
                return Err(CanonicalJsonError::MaximumObjectKeysExceeded);
            }
            if !names.insert(name) {
                return Err(CanonicalJsonError::DuplicateObjectName);
            }
            self.skip_whitespace();
            if !self.take(b':') {
                return Err(CanonicalJsonError::InvalidSyntax);
            }
            self.scan_value_with_array_limit(depth + 1, exact_array_limit)?;
            self.skip_whitespace();
            if self.take(b'}') {
                return Ok(());
            }
            if !self.take(b',') {
                return Err(CanonicalJsonError::InvalidSyntax);
            }
        }
    }

    fn scan_array(
        &mut self,
        depth: usize,
        maximum_entries: usize,
    ) -> Result<(), CanonicalJsonError> {
        self.cursor += 1;
        self.skip_whitespace();
        if self.take(b']') {
            return Ok(());
        }
        let mut entries = 0_usize;
        loop {
            entries += 1;
            if entries > maximum_entries {
                return Err(CanonicalJsonError::MaximumArrayEntriesExceeded);
            }
            self.scan_value(depth + 1)?;
            self.skip_whitespace();
            if self.take(b']') {
                return Ok(());
            }
            if !self.take(b',') {
                return Err(CanonicalJsonError::InvalidSyntax);
            }
        }
    }

    fn scan_string(&mut self) -> Result<String, CanonicalJsonError> {
        let start = self.cursor;
        self.cursor += 1;
        let mut escaped = false;
        while let Some(byte) = self.peek() {
            self.cursor += 1;
            if escaped {
                escaped = false;
                continue;
            }
            match byte {
                b'\\' => escaped = true,
                b'"' => {
                    if self.cursor - start > MAX_RAW_JSON_STRING_BYTES {
                        return Err(CanonicalJsonError::InputTooLarge);
                    }
                    let decoded: String = serde_json::from_slice(&self.input[start..self.cursor])
                        .map_err(|_| CanonicalJsonError::InvalidSyntax)?;
                    if decoded.len() > MAX_COMPATIBILITY_STRING_BYTES {
                        return Err(CanonicalJsonError::InputTooLarge);
                    }
                    return Ok(decoded);
                }
                0x00..=0x1f => return Err(CanonicalJsonError::InvalidSyntax),
                _ => {}
            }
        }
        Err(CanonicalJsonError::InvalidSyntax)
    }

    fn scan_number(&mut self) -> Result<(), CanonicalJsonError> {
        let start = self.cursor;
        if self.take(b'-') && self.peek().is_none() {
            return Err(CanonicalJsonError::InvalidSyntax);
        }
        match self.peek() {
            Some(b'0') => {
                self.cursor += 1;
                if self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                    return Err(CanonicalJsonError::InvalidSyntax);
                }
            }
            Some(b'1'..=b'9') => {
                self.cursor += 1;
                while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                    self.cursor += 1;
                }
            }
            _ => return Err(CanonicalJsonError::InvalidSyntax),
        }
        if self.take(b'.') {
            let before = self.cursor;
            while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                self.cursor += 1;
            }
            if self.cursor == before {
                return Err(CanonicalJsonError::InvalidSyntax);
            }
        }
        if self.peek().is_some_and(|byte| matches!(byte, b'e' | b'E')) {
            self.cursor += 1;
            if self.peek().is_some_and(|byte| matches!(byte, b'+' | b'-')) {
                self.cursor += 1;
            }
            let before = self.cursor;
            while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                self.cursor += 1;
            }
            if self.cursor == before {
                return Err(CanonicalJsonError::InvalidSyntax);
            }
        }
        let token = std::str::from_utf8(&self.input[start..self.cursor])
            .map_err(|_| CanonicalJsonError::InvalidSyntax)?;
        if self.enforce_safe_integer_domain {
            validate_number_token(token).map(|_| ())
        } else if token.contains(['.', 'e', 'E']) {
            validate_number_token(token).map(|_| ())
        } else {
            Ok(())
        }
    }

    fn scan_literal(&mut self, literal: &[u8]) -> Result<(), CanonicalJsonError> {
        if self.input.get(self.cursor..self.cursor + literal.len()) == Some(literal) {
            self.cursor += literal.len();
            Ok(())
        } else {
            Err(CanonicalJsonError::InvalidSyntax)
        }
    }

    fn skip_whitespace(&mut self) {
        while self
            .peek()
            .is_some_and(|byte| matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
        {
            self.cursor += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.cursor).copied()
    }

    fn take(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.cursor += 1;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonicalizes_rfc_8785_number_and_key_vectors() {
        let value = parse_bounded_json_v1(
            br#"{"z":4.50,"a":2e-3,"large":1e30,"small":1e-7,"zero":-0.0}"#,
            CompatibilityJsonLimitsV1::PROFILE_DOCUMENT,
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(canonicalize_json_value_v1(&value).unwrap()).unwrap(),
            r#"{"a":0.002,"large":1e+30,"small":1e-7,"z":4.5,"zero":0}"#
        );

        let non_ascii = json!({"\u{20ac}": 1, "\r": 2, "\u{1f600}": 3});
        assert_eq!(
            String::from_utf8(canonicalize_json_value_v1(&non_ascii).unwrap()).unwrap(),
            "{\"\\r\":2,\"€\":1,\"😀\":3}"
        );
    }

    #[test]
    fn raw_parser_rejects_duplicates_and_unsafe_numbers_before_conversion() {
        let limits = CompatibilityJsonLimitsV1::PROFILE_DOCUMENT;
        assert_eq!(
            parse_bounded_json_v1(br#"{"outer":{"x":1,"x":2}}"#, limits),
            Err(CanonicalJsonError::DuplicateObjectName)
        );
        assert!(serde_json::from_str::<CanonicalJsonObjectV1>(r#"{"x":1,"x":2}"#).is_err());
        assert!(serde_json::from_str::<CanonicalJsonValueV1>(r#"{"x":1,"x":2}"#).is_err());
        assert!(
            serde_json::from_str::<CanonicalJsonObjectV1>(r#"{"outer":{"x":1,"x":2}}"#).is_err()
        );
        assert_eq!(
            parse_bounded_json_v1(b"9007199254740992", limits),
            Err(CanonicalJsonError::UnsafeInteger)
        );
        assert_eq!(
            parse_bounded_json_v1(b"1e999", limits),
            Err(CanonicalJsonError::NonFiniteNumber)
        );
        assert_eq!(
            parse_bounded_json_v1(b"1e-999", limits),
            Err(CanonicalJsonError::UnrepresentableNumber)
        );
        assert!(parse_bounded_json_v1(b"9007199254740991", limits).is_ok());
        assert!(parse_bounded_json_v1(b"-9007199254740991", limits).is_ok());

        let golden: Value = serde_json::from_str(include_str!(
            "../../../schemas/v1/household-canonical-v1.golden.json"
        ))
        .unwrap();
        assert_eq!(
            golden["numeric_admission"]["safe_integer_minimum"].as_i64(),
            Some(MIN_SAFE_IJSON_INTEGER)
        );
        assert_eq!(
            golden["numeric_admission"]["safe_integer_maximum"].as_i64(),
            Some(MAX_SAFE_IJSON_INTEGER)
        );
        for token in golden["numeric_admission"]["accepted_fraction_examples"]
            .as_array()
            .unwrap()
        {
            assert!(parse_bounded_json_v1(token.as_str().unwrap().as_bytes(), limits).is_ok());
        }
        for token in golden["numeric_admission"]["rejected_tokens"]
            .as_array()
            .unwrap()
        {
            assert!(parse_bounded_json_v1(token.as_str().unwrap().as_bytes(), limits).is_err());
        }
    }

    #[test]
    fn bounded_value_visitor_survives_tagged_buffering_without_losing_contract_checks() {
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        enum Buffered {
            Object { value: CanonicalJsonObjectV1 },
        }

        let valid: Buffered = serde_json::from_str(
            r#"{"kind":"object","value":{"fraction":1.5,"nested":{"safe":9007199254740991}}}"#,
        )
        .unwrap();
        let Buffered::Object { value } = valid;
        assert_eq!(
            value.as_map()["fraction"].as_f64(),
            Some(1.5),
            "arbitrary-precision number buffering must reconstruct a number"
        );
        assert!(
            serde_json::from_str::<Buffered>(
                r#"{"kind":"object","value":{"outer":{"x":1,"x":2}}}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<Buffered>(
                r#"{"kind":"object","value":{"unsafe":9007199254740992}}"#
            )
            .is_err()
        );
        for invalid in ["1e-999", "1e999"] {
            let encoded = format!(r#"{{"kind":"object","value":{{"invalid":{invalid}}}}}"#);
            assert!(serde_json::from_str::<Buffered>(&encoded).is_err());
        }
    }

    #[test]
    fn parser_enforces_depth_keys_entries_and_nodes_before_generic_parse() {
        let base = CompatibilityJsonLimitsV1 {
            maximum_bytes: 1024,
            maximum_depth: 2,
            maximum_object_keys: 1,
            maximum_array_entries: 1,
            maximum_nodes: 2,
        };
        assert!(parse_bounded_json_v1(br#"{"a":1}"#, base).is_ok());
        assert_eq!(
            parse_bounded_json_v1(br#"{"a":{"b":1}}"#, base),
            Err(CanonicalJsonError::MaximumDepthExceeded)
        );
        assert_eq!(
            parse_bounded_json_v1(br#"{"a":1,"b":2}"#, base),
            Err(CanonicalJsonError::MaximumObjectKeysExceeded)
        );
        assert_eq!(
            parse_bounded_json_v1(br#"[1,2]"#, base),
            Err(CanonicalJsonError::MaximumArrayEntriesExceeded)
        );

        let maximum_key = format!("{{\"{}\":null}}", "k".repeat(2_048));
        assert!(
            parse_bounded_json_v1(
                maximum_key.as_bytes(),
                CompatibilityJsonLimitsV1::PROFILE_DOCUMENT
            )
            .is_ok()
        );
        let oversized_key = format!("{{\"{}\":null}}", "k".repeat(2_049));
        assert_eq!(
            parse_bounded_json_v1(
                oversized_key.as_bytes(),
                CompatibilityJsonLimitsV1::PROFILE_DOCUMENT
            ),
            Err(CanonicalJsonError::InputTooLarge)
        );
    }

    #[test]
    fn canonical_compatibility_value_preserves_scalars_without_disclosure() {
        let value = CanonicalJsonValueV1::parse(
            br#""retired-value""#,
            CompatibilityJsonLimitsV1::PROFILE_DOCUMENT,
        )
        .unwrap();
        assert_eq!(value.canonical_bytes().unwrap(), br#""retired-value""#);
        assert!(!format!("{value:?}").contains("retired-value"));

        let integer = CanonicalJsonValueV1::parse(
            b"9007199254740991",
            CompatibilityJsonLimitsV1::PROFILE_DOCUMENT,
        )
        .unwrap();
        assert_eq!(integer.canonical_bytes().unwrap(), b"9007199254740991");
    }

    #[test]
    fn domain_hash_is_length_delimited() {
        let joined = domain_hash_v1("test", &[b"ab", b"c"]).unwrap();
        let split = domain_hash_v1("test", &[b"a", b"bc"]).unwrap();
        assert_ne!(joined, split);
        assert_eq!(joined.to_lower_hex().len(), 64);
    }

    #[test]
    fn published_golden_vectors_recompute_from_the_core_contract() {
        let golden: Value = serde_json::from_str(include_str!(
            "../../../schemas/v1/household-canonical-v1.golden.json"
        ))
        .unwrap();
        for vector in golden["canonical_json"].as_array().unwrap() {
            let source = vector["source"].as_str().unwrap().as_bytes();
            let parsed =
                parse_bounded_json_v1(source, CompatibilityJsonLimitsV1::PROFILE_DOCUMENT).unwrap();
            let canonical = canonicalize_json_value_v1(&parsed).unwrap();
            assert_eq!(
                String::from_utf8(canonical.clone()).unwrap(),
                vector["canonical_utf8"].as_str().unwrap()
            );
            assert_eq!(
                encode_lower_hex(&canonical),
                vector["canonical_hex"].as_str().unwrap()
            );
            assert_eq!(
                sha256_bytes(&canonical).to_lower_hex(),
                vector["sha256"].as_str().unwrap()
            );
        }

        for vector in golden["full_width_identity_vectors"].as_array().unwrap() {
            let account = vector["account_id"].as_str().unwrap().as_bytes();
            let platform = vector["platform"].as_str().unwrap().as_bytes();
            let root = vector["native_root"].as_str().unwrap().as_bytes();
            let account_digest =
                domain_hash_v1("heyfood.household.account-digest.v1", &[account]).unwrap();
            let root_digest = domain_hash_v1(
                "heyfood.household.native-root-instance.v1",
                &[platform, root],
            )
            .unwrap();
            let locator_digest = domain_hash_v1(
                "heyfood.household.account-locator.v1",
                &[root_digest.as_bytes(), account_digest.as_bytes()],
            )
            .unwrap();
            assert_eq!(
                account_digest.to_lower_hex(),
                vector["account_digest"].as_str().unwrap()
            );
            assert_eq!(
                root_digest.to_lower_hex(),
                vector["native_root_instance_digest"].as_str().unwrap()
            );
            assert_eq!(
                locator_digest.to_lower_hex(),
                vector["account_locator_digest"].as_str().unwrap()
            );
        }

        let envelopes = &golden["envelope_vectors"];
        assert_eq!(
            envelopes["generation_header_hex"].as_str().unwrap().len(),
            84 * 2
        );
        assert_eq!(
            envelopes["journal_header_hex"].as_str().unwrap().len(),
            84 * 2
        );
        assert!(
            envelopes["generation_aad_hex"]
                .as_str()
                .unwrap()
                .ends_with(envelopes["generation_header_hex"].as_str().unwrap())
        );
        assert!(
            envelopes["journal_aad_hex"]
                .as_str()
                .unwrap()
                .ends_with(envelopes["journal_header_hex"].as_str().unwrap())
        );
    }

    #[test]
    fn published_household_schemas_are_valid_and_freeze_core_limits() {
        let schema_limits = CompatibilityJsonLimitsV1 {
            maximum_bytes: 8 * 1024 * 1024,
            maximum_depth: 64,
            maximum_object_keys: 128,
            maximum_array_entries: 16_384,
            maximum_nodes: 262_144,
        };
        let profile = parse_bounded_typed_json_v1(
            include_bytes!("../../../schemas/v1/household-profile-document.schema.json"),
            schema_limits,
        )
        .unwrap();
        let state = parse_bounded_typed_json_v1(
            include_bytes!("../../../schemas/v1/household-state.schema.json"),
            schema_limits,
        )
        .unwrap();
        assert_eq!(
            profile["x-heyfood-canonical-byte-limit"].as_u64(),
            Some(256 * 1024)
        );
        assert_eq!(profile["x-heyfood-max-json-depth"].as_u64(), Some(8));
        for definition in ["custom40Array", "custom60Array"] {
            assert_eq!(
                profile["$defs"][definition]["items"]["x-heyfood-not-unicode-whitespace-only"]
                    .as_bool(),
                Some(true)
            );
            assert_eq!(
                profile["$defs"][definition]["items"]["x-heyfood-no-controls"].as_bool(),
                Some(true)
            );
        }
        assert_eq!(
            state["properties"]["members"]["maxItems"].as_u64(),
            Some(256)
        );
        assert_eq!(
            state["properties"]["profiles"]["maxItems"].as_u64(),
            Some(257)
        );
        assert_eq!(
            state["properties"]["outbox"]["maxItems"].as_u64(),
            Some(1_024)
        );
        let legacy_payload = &state["$defs"]["legacyOutbox"]["properties"]["payload"];
        for (annotation, expected) in [
            ("x-heyfood-canonical-byte-limit", 4 * 1024 * 1024),
            ("x-heyfood-max-json-depth", 8),
            ("x-heyfood-max-json-nodes", 65_536),
            ("x-heyfood-max-object-properties", 128),
            ("x-heyfood-max-array-items", 256),
            ("x-heyfood-max-string-utf8-bytes", 2_048),
        ] {
            assert_eq!(legacy_payload[annotation].as_u64(), Some(expected));
        }
        assert_eq!(
            state["properties"]["bounded_applied_commits"]["maxItems"].as_u64(),
            Some(16_384)
        );
        assert_eq!(
            state["$defs"]["ownerSyncIntent"]["properties"]["consent_version"]["oneOf"][0]
                ["maximum"]
                .as_u64(),
            Some(2_147_483_647)
        );
        assert_eq!(
            state["$defs"]["ownerSyncIntent"]["allOf"]
                .as_array()
                .map(Vec::len),
            Some(10)
        );
        assert_eq!(
            profile["$defs"]["dietStyleIds"]["maxItems"].as_u64(),
            Some(24)
        );
        assert_eq!(
            profile["$defs"]["allergyIds"]["maxItems"].as_u64(),
            Some(28)
        );
        assert_eq!(
            profile["$defs"]["healthConditionIds"]["maxItems"].as_u64(),
            Some(31)
        );
        assert_eq!(
            profile["$defs"]["cuisineIds"]["maxItems"].as_u64(),
            Some(28)
        );
        let schema_ids = |definition: &str| {
            profile["$defs"][definition]["items"]["enum"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            schema_ids("dietStyleIds"),
            crate::diet_options()
                .iter()
                .map(|option| option.id.as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            schema_ids("allergyIds"),
            crate::allergy_options()
                .iter()
                .map(|option| option.id.as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            schema_ids("healthConditionIds"),
            crate::condition_options()
                .iter()
                .map(|option| option.id.as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            schema_ids("cuisineIds"),
            crate::cuisine_options()
                .iter()
                .map(|option| option.id.as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            profile["$defs"]["declaredProfile"]["properties"]["activity_level"]["oneOf"][0]["enum"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap())
                .collect::<Vec<_>>(),
            crate::activity_options()
                .iter()
                .map(|option| option.id.as_str())
                .collect::<Vec<_>>()
        );
    }

    fn canonical_object_with_array_strings(target_bytes: usize, entries: usize) -> String {
        let overhead = 7 + (3 * entries);
        let mut remaining = target_bytes.checked_sub(overhead).unwrap();
        assert!(remaining <= entries * 2_048);
        let mut values = Vec::with_capacity(entries);
        for _ in 0..entries {
            let length = remaining.min(2_048);
            values.push(format!("\"{}\"", "a".repeat(length)));
            remaining -= length;
        }
        assert_eq!(remaining, 0);
        let value = format!("{{\"x\":[{}]}}", values.join(","));
        assert_eq!(value.len(), target_bytes);
        value
    }

    fn object_with_keys(keys: usize) -> String {
        let fields = (0..keys)
            .map(|index| format!("\"k{index:03}\":null"))
            .collect::<Vec<_>>();
        format!("{{{}}}", fields.join(","))
    }

    fn array_with_entries(entries: usize) -> String {
        format!("[{}]", vec!["null"; entries].join(","))
    }

    fn node_tree(child_arrays: usize, last_entries: usize) -> String {
        let children = (0..child_arrays)
            .map(|index| {
                let entries = if index + 1 == child_arrays {
                    last_entries
                } else {
                    256
                };
                array_with_entries(entries)
            })
            .collect::<Vec<_>>();
        format!("[{}]", children.join(","))
    }

    #[test]
    fn exact_recursive_and_byte_caps_cover_limit_neighbors() {
        let profile_limit = CompatibilityJsonLimitsV1::PROFILE_DOCUMENT;
        for keys in [127, 128] {
            assert!(
                parse_bounded_json_v1(object_with_keys(keys).as_bytes(), profile_limit).is_ok()
            );
        }
        assert_eq!(
            parse_bounded_json_v1(object_with_keys(129).as_bytes(), profile_limit),
            Err(CanonicalJsonError::MaximumObjectKeysExceeded)
        );
        for entries in [255, 256] {
            assert!(
                parse_bounded_json_v1(array_with_entries(entries).as_bytes(), profile_limit)
                    .is_ok()
            );
        }
        assert_eq!(
            parse_bounded_json_v1(array_with_entries(257).as_bytes(), profile_limit),
            Err(CanonicalJsonError::MaximumArrayEntriesExceeded)
        );
        for arrays in [6, 7] {
            let nested = format!("{}null{}", "[".repeat(arrays), "]".repeat(arrays));
            assert!(parse_bounded_json_v1(nested.as_bytes(), profile_limit).is_ok());
        }
        let too_deep = format!("{}null{}", "[".repeat(8), "]".repeat(8));
        assert_eq!(
            parse_bounded_json_v1(too_deep.as_bytes(), profile_limit),
            Err(CanonicalJsonError::MaximumDepthExceeded)
        );

        let migration_limit = CompatibilityJsonLimitsV1::MIGRATION_CANDIDATE;
        assert!(parse_bounded_json_v1(node_tree(255, 255).as_bytes(), migration_limit).is_ok());
        assert!(parse_bounded_json_v1(node_tree(255, 256).as_bytes(), migration_limit).is_ok());
        assert_eq!(
            parse_bounded_json_v1(node_tree(256, 0).as_bytes(), migration_limit),
            Err(CanonicalJsonError::MaximumNodesExceeded)
        );

        for target in [MAX_PROFILE_BYTES - 1, MAX_PROFILE_BYTES] {
            let object = canonical_object_with_array_strings(target, 128);
            assert_eq!(
                CanonicalJsonObjectV1::parse(&object.into_bytes(), profile_limit)
                    .unwrap()
                    .canonical_len(),
                target
            );
        }
        let profile_too_large = canonical_object_with_array_strings(MAX_PROFILE_BYTES + 1, 128);
        assert_eq!(
            CanonicalJsonObjectV1::parse(profile_too_large.as_bytes(), profile_limit),
            Err(CanonicalJsonError::InputTooLarge)
        );

        let owner_limit = CompatibilityJsonLimitsV1::OWNER_SYNC_REQUEST;
        for target in [owner_limit.maximum_bytes - 1, owner_limit.maximum_bytes] {
            let object = canonical_object_with_array_strings(target, 256);
            assert_eq!(
                CanonicalJsonObjectV1::parse(object.as_bytes(), owner_limit)
                    .unwrap()
                    .canonical_len(),
                target
            );
        }
        let owner_too_large =
            canonical_object_with_array_strings(owner_limit.maximum_bytes + 1, 256);
        assert_eq!(
            CanonicalJsonObjectV1::parse(owner_too_large.as_bytes(), owner_limit),
            Err(CanonicalJsonError::InputTooLarge)
        );

        for target in [
            migration_limit.maximum_bytes - 1,
            migration_limit.maximum_bytes,
        ] {
            let mut source = vec![b' '; target - 4];
            source.extend_from_slice(b"null");
            assert!(parse_bounded_json_v1(&source, migration_limit).is_ok());
        }
        let mut too_large = vec![b' '; migration_limit.maximum_bytes - 3];
        too_large.extend_from_slice(b"null");
        assert_eq!(
            parse_bounded_json_v1(&too_large, migration_limit),
            Err(CanonicalJsonError::InputTooLarge)
        );
    }

    const MAX_PROFILE_BYTES: usize = 256 * 1024;
}
