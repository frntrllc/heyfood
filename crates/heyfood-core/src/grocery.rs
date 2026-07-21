//! Provider-neutral grocery semantics. Final backend wire DTOs remain gated on
//! the deployed Grocery Phase-A contract freeze.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

const MAX_GROCERY_EDIT_BYTES: usize = 16 * 1024;
const MAX_GROCERY_EDIT_ENTRIES: usize = 64;
const MAX_GROCERY_EDIT_DEPTH: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GroceryCapability {
    Unavailable,
    V1,
    UnsupportedVersion(String),
}

impl GroceryCapability {
    #[must_use]
    pub fn from_advertised(value: Option<&str>) -> Self {
        match value {
            None => Self::Unavailable,
            Some("v1") => Self::V1,
            Some(value) => Self::UnsupportedVersion(value.to_owned()),
        }
    }

    #[must_use]
    pub fn is_usable(&self) -> bool {
        matches!(self, Self::V1)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct GroceryEntityId(Uuid);

impl GroceryEntityId {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        Uuid::parse_str(value)
            .map(Self)
            .map_err(|_| "grocery entity ID must be a UUID")
    }

    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl<'de> Deserialize<'de> for GroceryEntityId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct GroceryListVersion(u64);

impl GroceryListVersion {
    pub fn new(value: u64) -> Result<Self, &'static str> {
        (value > 0)
            .then_some(Self(value))
            .ok_or("grocery list version must be positive")
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for GroceryListVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(u64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct HouseholdContextHashVersion(i64);

impl HouseholdContextHashVersion {
    #[must_use]
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for HouseholdContextHashVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Self::new(i64::deserialize(deserializer)?))
    }
}

/// Server-minted replay identity from frozen C3. It is deliberately distinct
/// from logical/tracing `OperationId` and from every Grocery entity ID.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct GroceryIdempotencyKey(Uuid);

impl GroceryIdempotencyKey {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        let parsed = Uuid::parse_str(value)
            .map_err(|_| "grocery idempotency key must be a canonical UUID")?;
        if parsed.hyphenated().to_string() != value {
            return Err("grocery idempotency key must be a canonical lowercase UUID");
        }
        Ok(Self(parsed))
    }

    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

/// Server-minted pending-confirmation identity from frozen C3.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct GroceryConfirmationId(Uuid);

impl GroceryConfirmationId {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        let parsed = Uuid::parse_str(value)
            .map_err(|_| "grocery confirmation ID must be a canonical UUID")?;
        if parsed.hyphenated().to_string() != value {
            return Err("grocery confirmation ID must be a canonical lowercase UUID");
        }
        Ok(Self(parsed))
    }

    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl<'de> Deserialize<'de> for GroceryConfirmationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(&String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for GroceryIdempotencyKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(&String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ContextFingerprint(String);

impl ContextFingerprint {
    pub fn parse(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        if value.is_empty() || value.len() > 256 {
            return Err("context fingerprint has an invalid length");
        }
        if !value
            .bytes()
            .all(|value| value.is_ascii_hexdigit() || value == b'-')
        {
            return Err("context fingerprint has invalid characters");
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ContextFingerprint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrocerySafetyStatus {
    GenerallySafer,
    Risky,
    Avoid,
    UnableToEvaluate,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FrozenGroceryPreconditions {
    pub list_id: GroceryEntityId,
    pub list_version: GroceryListVersion,
    pub context_fingerprint: ContextFingerprint,
    /// Frozen C3 permits null for legacy pending confirmations. Grocery Phase
    /// A adapters must fail closed when their authoritative live snapshot does
    /// not provide the same value.
    pub household_context_hash_version: Option<HouseholdContextHashVersion>,
}

impl FrozenGroceryPreconditions {
    /// Compare only frozen semantic authority. Adapters remain responsible for
    /// read-only retrieval of the live values and must not create or replace a
    /// list while evaluating this check.
    pub fn validate_live(&self, live: &Self) -> Result<(), GroceryErrorCode> {
        if self.list_id != live.list_id {
            return Err(GroceryErrorCode::ListReplaced);
        }
        if self.list_version != live.list_version {
            return Err(GroceryErrorCode::ListVersionConflict);
        }
        if self.context_fingerprint != live.context_fingerprint
            || self.household_context_hash_version != live.household_context_hash_version
        {
            return Err(GroceryErrorCode::ContextChanged);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroceryConfirmationState {
    Proposed,
    Accepted,
    Cancelled,
    RejectedStale,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroceryConfirmation {
    pub confirmation_id: GroceryConfirmationId,
    pub idempotency_key: GroceryIdempotencyKey,
    pub preconditions: FrozenGroceryPreconditions,
    pub state: GroceryConfirmationState,
}

impl GroceryConfirmation {
    /// Normalize C3 decision semantics. An absent legacy decision is exactly an
    /// accept, while edits are accepted only after tool-specific validation
    /// has produced `GroceryValidatedEdits`.
    pub fn command(
        &self,
        decision: GroceryConfirmationDecision,
    ) -> Result<GroceryConfirmationCommand, GroceryErrorCode> {
        if self.state != GroceryConfirmationState::Proposed {
            return Err(match (&self.state, &decision) {
                (
                    GroceryConfirmationState::Cancelled,
                    GroceryConfirmationDecision::Accept { .. },
                ) => GroceryErrorCode::AlreadyCancelled,
                (GroceryConfirmationState::Cancelled, GroceryConfirmationDecision::Cancel) => {
                    GroceryErrorCode::CancelRejected
                }
                (GroceryConfirmationState::RejectedStale, _) => {
                    GroceryErrorCode::PreconditionFailed
                }
                _ => GroceryErrorCode::ConfirmationRejected,
            });
        }
        Ok(GroceryConfirmationCommand {
            confirmation_id: self.confirmation_id,
            idempotency_key: self.idempotency_key,
            preconditions: self.preconditions.clone(),
            decision,
        })
    }
}

/// Explicit C3 decision. `Cancel` cannot carry edits by construction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GroceryConfirmationDecision {
    Accept {
        edits: Option<GroceryValidatedEdits>,
    },
    Cancel,
}

impl GroceryConfirmationDecision {
    pub fn from_contract_fields(
        value: Option<&str>,
        edits: Option<GroceryValidatedEdits>,
    ) -> Result<Self, GroceryErrorCode> {
        match value {
            None | Some("accept") => Ok(Self::Accept { edits }),
            Some("cancel") if edits.is_none() => Ok(Self::Cancel),
            Some("cancel") => Err(GroceryErrorCode::EditInvalid),
            Some(_) => Err(GroceryErrorCode::ConfirmationRejected),
        }
    }

    #[must_use]
    pub const fn as_contract_value(&self) -> &'static str {
        match self {
            Self::Accept { .. } => "accept",
            Self::Cancel => "cancel",
        }
    }
}

/// A bounded object that has already passed the pending tool's edit schema.
/// Its values redact from diagnostics and are never model-reinterpreted here.
#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct GroceryValidatedEdits(Map<String, Value>);

impl GroceryValidatedEdits {
    pub fn new(values: Map<String, Value>) -> Result<Self, GroceryErrorCode> {
        let value = Value::Object(values);
        let mut entries = 0;
        validate_edit_value(&value, 0, &mut entries).map_err(|_| GroceryErrorCode::EditInvalid)?;
        if serde_json::to_vec(&value)
            .map_err(|_| GroceryErrorCode::EditInvalid)?
            .len()
            > MAX_GROCERY_EDIT_BYTES
        {
            return Err(GroceryErrorCode::EditInvalid);
        }
        let Value::Object(values) = value else {
            unreachable!("edit constructor starts with an object")
        };
        Ok(Self(values))
    }

    #[must_use]
    pub fn as_object(&self) -> &Map<String, Value> {
        &self.0
    }
}

impl fmt::Debug for GroceryValidatedEdits {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GroceryValidatedEdits([REDACTED])")
    }
}

impl<'de> Deserialize<'de> for GroceryValidatedEdits {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(Map::<String, Value>::deserialize(deserializer)?)
            .map_err(|_| serde::de::Error::custom("confirmation edits are invalid"))
    }
}

fn validate_edit_value(
    value: &Value,
    depth: usize,
    entries: &mut usize,
) -> Result<(), &'static str> {
    if depth > MAX_GROCERY_EDIT_DEPTH {
        return Err("confirmation edits exceed their nesting limit");
    }
    match value {
        Value::Array(values) => {
            *entries = entries.saturating_add(values.len());
            if *entries > MAX_GROCERY_EDIT_ENTRIES {
                return Err("confirmation edits exceed their entry limit");
            }
            for value in values {
                validate_edit_value(value, depth + 1, entries)?;
            }
        }
        Value::Object(values) => {
            *entries = entries.saturating_add(values.len());
            if *entries > MAX_GROCERY_EDIT_ENTRIES {
                return Err("confirmation edits exceed their entry limit");
            }
            for (key, value) in values {
                if key.is_empty() || key.len() > 128 || key.chars().any(char::is_control) {
                    return Err("confirmation edit key is invalid");
                }
                validate_edit_value(value, depth + 1, entries)?;
            }
        }
        Value::String(value) if value.len() > 4 * 1024 || value.chars().any(char::is_control) => {
            return Err("confirmation edit string is invalid");
        }
        _ => {}
    }
    Ok(())
}

/// Fully frozen C3 command passed through the provisional application port.
/// Replaying this value preserves the server-minted idempotency key exactly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroceryConfirmationCommand {
    pub confirmation_id: GroceryConfirmationId,
    pub idempotency_key: GroceryIdempotencyKey,
    pub preconditions: FrozenGroceryPreconditions,
    pub decision: GroceryConfirmationDecision,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroceryErrorCode {
    CapabilityUnavailable,
    UnsupportedCapability,
    ScopeRequired,
    ListMissing,
    ListReplaced,
    ListVersionConflict,
    ContextChanged,
    ConsentRevoked,
    ConfirmationExpired,
    ConfirmationRejected,
    AlreadyCancelled,
    CancelRejected,
    EditInvalid,
    PreconditionFailed,
    UnknownPrecondition,
    TemporarilyUnavailable,
    WriteFailed,
    OutcomeUncertain,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GroceryError {
    pub code: GroceryErrorCode,
    pub retryable: bool,
    pub requires_reauthentication: bool,
}

impl fmt::Display for GroceryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "grocery operation failed: {:?}", self.code)
    }
}

impl std::error::Error for GroceryError {}
