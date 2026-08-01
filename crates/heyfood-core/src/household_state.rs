//! Versioned, I/O-free native household and profile domain.

use std::{cmp::Ordering, collections::BTreeSet, fmt};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use serde_json::{Map, Value};
use time::{Date, Month, OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};
use uuid::{Uuid, Variant, Version};

use crate::{
    AccountId, CommitId, OnboardingProfileInput,
    household_canonical::{
        CanonicalDigestV1, CanonicalJsonError, CanonicalJsonObjectV1, CanonicalJsonValueV1,
        CompatibilityJsonLimitsV1, canonical_sha256_v1, canonicalize_json_value_v1,
        parse_bounded_json_object_v1,
    },
};

pub const HOUSEHOLD_STATE_SCHEMA_VERSION: u16 = 1;
pub const HOUSEHOLD_PROFILE_DOCUMENT_SCHEMA_VERSION: u16 = 1;
pub const MAX_HOUSEHOLD_MEMBERS: usize = 256;
pub const MAX_HOUSEHOLD_SUBJECTS: usize = 257;
pub const MAX_HOUSEHOLD_PROFILES: usize = 257;
pub const MAX_HOUSEHOLD_OUTBOX_ENTRIES: usize = 1_024;
pub const MAX_APPLIED_COMMITS: usize = 16_384;
pub const MAX_LEGACY_APPLIED_MUTATION_IDS: usize = 100;
pub const MAX_IMPORTED_COMPATIBILITY_FIELDS: usize = 128;
pub const MAX_MIGRATION_DISPOSITIONS: usize = 128;
pub const MAX_LEGACY_REMOTE_PROFILE_REFERENCES: usize = MAX_HOUSEHOLD_SUBJECTS;
pub const MAX_LEGACY_TIMESTAMP_PROVENANCE: usize =
    (2 * MAX_HOUSEHOLD_SUBJECTS) + MAX_HOUSEHOLD_OUTBOX_ENTRIES + 1;
pub const MAX_COMPATIBILITY_JSON_DEPTH: usize = 8;
pub const MAX_COMPATIBILITY_OBJECT_KEYS: usize = 128;
pub const MAX_COMPATIBILITY_ARRAY_ENTRIES: usize = 256;
pub const MAX_COMPATIBILITY_JSON_NODES: usize = 65_536;
pub const MAX_OWNER_SYNC_REQUEST_BODY_BYTES: usize = 524_288;
pub const MAX_PROFILE_DOCUMENT_BYTES: usize = 256 * 1024;
pub const MAX_MIGRATION_CANDIDATE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_CANONICAL_VAULT_PLAINTEXT_BYTES: usize = 8 * 1024 * 1024;
pub const OWNER_SYNC_OUTBOX_PREFIX: &str = "owner-sync-v1:";

const SELF_COMPATIBILITY_SENTINEL: &str = "_self";
const EVERYONE_COMPATIBILITY_SENTINEL: &str = "__everyone__";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HouseholdStateError {
    InvalidSchemaVersion,
    InvalidIdentity,
    InvalidDisplayName,
    InvalidTimestamp,
    InvalidDate,
    InvalidRelationship,
    InvalidMinorStatus,
    InvalidProfileDocument,
    InvalidMigrationProvenance,
    InvalidRevision,
    RevisionOverflow,
    InvalidOwnerSyncIntent,
    InvalidOutbox,
    UnsortedCollection,
    DuplicateIdentity,
    OrphanReference,
    ArchivedActiveTarget,
    EveryoneRequiresTwoActiveSubjects,
    CardinalityExceeded,
    AppliedCommitLedgerFull,
    NonCanonicalEncoding,
    CanonicalJson(CanonicalJsonError),
}

impl fmt::Display for HouseholdStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSchemaVersion => "household schema version is invalid",
            Self::InvalidIdentity => "household identity is invalid",
            Self::InvalidDisplayName => "household display name is invalid",
            Self::InvalidTimestamp => "household timestamp is invalid",
            Self::InvalidDate => "household date is invalid",
            Self::InvalidRelationship => "household relationship is invalid",
            Self::InvalidMinorStatus => "household minor status is invalid",
            Self::InvalidProfileDocument => "household profile document is invalid",
            Self::InvalidMigrationProvenance => "household migration provenance is invalid",
            Self::InvalidRevision => "household revision is invalid",
            Self::RevisionOverflow => "household revision overflowed",
            Self::InvalidOwnerSyncIntent => "owner sync intent is invalid",
            Self::InvalidOutbox => "household outbox record is invalid",
            Self::UnsortedCollection => "household identity collection is not canonically sorted",
            Self::DuplicateIdentity => "household identity collection contains a duplicate",
            Self::OrphanReference => "household record contains an orphan reference",
            Self::ArchivedActiveTarget => "household active target is archived",
            Self::EveryoneRequiresTwoActiveSubjects => {
                "everyone scope requires at least two active subjects"
            }
            Self::CardinalityExceeded => "household collection exceeds its cardinality limit",
            Self::AppliedCommitLedgerFull => "household applied-commit ledger is full",
            Self::NonCanonicalEncoding => "household state bytes are not canonical",
            Self::CanonicalJson(error) => return error.fmt(formatter),
        })
    }
}

impl std::error::Error for HouseholdStateError {}

impl From<CanonicalJsonError> for HouseholdStateError {
    fn from(value: CanonicalJsonError) -> Self {
        Self::CanonicalJson(value)
    }
}

macro_rules! nonzero_revision {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(u64);

        impl $name {
            pub fn new(value: u64) -> Result<Self, HouseholdStateError> {
                (value != 0)
                    .then_some(Self(value))
                    .ok_or(HouseholdStateError::InvalidRevision)
            }

            #[must_use]
            pub const fn get(self) -> u64 {
                self.0
            }

            pub fn checked_next(self) -> Result<Self, HouseholdStateError> {
                self.0
                    .checked_add(1)
                    .map(Self)
                    .ok_or(HouseholdStateError::RevisionOverflow)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                Self::new(u64::deserialize(deserializer)?).map_err(D::Error::custom)
            }
        }
    };
}

nonzero_revision!(HouseholdRevision);
nonzero_revision!(ProfileRevision);
nonzero_revision!(OutboxRevision);

/// PostgreSQL `Integer`-compatible authoritative consent version.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ConsentVersionV1(u32);

impl ConsentVersionV1 {
    pub const MAXIMUM: u32 = 2_147_483_647;

    pub fn new(value: u32) -> Result<Self, HouseholdStateError> {
        (value != 0 && value <= Self::MAXIMUM)
            .then_some(Self(value))
            .ok_or(HouseholdStateError::InvalidRevision)
    }

    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl<'de> Deserialize<'de> for ConsentVersionV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u64::deserialize(deserializer)?;
        let value = u32::try_from(value).map_err(D::Error::custom)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MemberId(String);

impl MemberId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4().hyphenated().to_string())
    }

    /// Construct a canonical lowercase UUID-v4 identity for a native member.
    ///
    /// Legacy migration deliberately accepts bounded opaque identifiers
    /// through `parse_preserved`; new native household members must use this
    /// narrower constructor so the two identity authorities cannot be
    /// confused.
    pub fn from_native_uuid_v4(value: Uuid) -> Result<Self, HouseholdStateError> {
        require_uuid_v4(value)?;
        Ok(Self(value.hyphenated().to_string()))
    }

    #[must_use]
    pub fn is_native_uuid_v4(&self) -> bool {
        Uuid::parse_str(&self.0).is_ok_and(|value| {
            require_uuid_v4(value).is_ok() && value.hyphenated().to_string() == self.0
        })
    }

    pub fn parse_preserved(value: impl Into<String>) -> Result<Self, HouseholdStateError> {
        let value = value.into();
        validate_opaque_identity(&value, false)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for MemberId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for MemberId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MemberId([REDACTED])")
    }
}

impl Serialize for MemberId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for MemberId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse_preserved(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HouseholdOutboxId(String);

impl HouseholdOutboxId {
    pub fn parse_legacy(value: impl Into<String>) -> Result<Self, HouseholdStateError> {
        let value = value.into();
        validate_opaque_identity(&value, true)?;
        Ok(Self(value))
    }

    pub fn owner_sync(intent_id: Uuid) -> Result<Self, HouseholdStateError> {
        require_uuid_v4(intent_id)?;
        Ok(Self(format!("{OWNER_SYNC_OUTBOX_PREFIX}{intent_id}")))
    }

    pub fn deterministic_legacy(
        source_kind: LegacyOutboxSourceKindV1,
        source_digest: CanonicalDigestV1,
        source_key: &str,
        entry_digest: CanonicalDigestV1,
    ) -> Result<Self, HouseholdStateError> {
        let (contract, prefix) = match source_kind {
            LegacyOutboxSourceKindV1::PythonSubjectKeyedV1 => (
                "heyfood.household.legacy-outbox-id.python-subject-v1",
                "legacy-py-v1-",
            ),
            LegacyOutboxSourceKindV1::RustSubjectKeyedLocalContextV0 => (
                "heyfood.household.legacy-outbox-id.rust-subject-v0",
                "legacy-rust-subject-v0-",
            ),
            LegacyOutboxSourceKindV1::PythonSubjectKeyedPatchV0 => (
                "heyfood.household.legacy-outbox-id.python-patch-v0",
                "legacy-py-patch-v0-",
            ),
            LegacyOutboxSourceKindV1::RustMutationKeyedEmbeddedMemberV0 => {
                return Err(HouseholdStateError::InvalidOutbox);
            }
        };
        #[derive(Serialize)]
        struct Preimage<'a> {
            contract: &'a str,
            source_digest: String,
            source_kind: LegacyOutboxSourceKindV1,
            source_key: &'a str,
            entry_digest: String,
        }
        let digest = canonical_sha256_v1(&Preimage {
            contract,
            source_digest: source_digest.to_lower_hex(),
            source_kind,
            source_key,
            entry_digest: entry_digest.to_lower_hex(),
        })?;
        let value = format!("{prefix}{}", digest.to_lower_hex());
        if value.len()
            != match source_kind {
                LegacyOutboxSourceKindV1::PythonSubjectKeyedV1 => 77,
                LegacyOutboxSourceKindV1::RustSubjectKeyedLocalContextV0 => 87,
                LegacyOutboxSourceKindV1::PythonSubjectKeyedPatchV0 => 83,
                LegacyOutboxSourceKindV1::RustMutationKeyedEmbeddedMemberV0 => unreachable!(),
            }
        {
            return Err(HouseholdStateError::InvalidOutbox);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for HouseholdOutboxId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HouseholdOutboxId([REDACTED])")
    }
}

impl Serialize for HouseholdOutboxId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for HouseholdOutboxId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if let Some(uuid) = value.strip_prefix(OWNER_SYNC_OUTBOX_PREFIX) {
            let uuid = Uuid::parse_str(uuid).map_err(D::Error::custom)?;
            return Self::owner_sync(uuid).map_err(D::Error::custom);
        }
        if is_deterministic_outbox_id(&value) {
            return Ok(Self(value));
        }
        Self::parse_legacy(value).map_err(D::Error::custom)
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct DisplayName(String);

impl DisplayName {
    pub fn parse(value: impl Into<String>) -> Result<Self, HouseholdStateError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 320
            || value.chars().count() > 80
            || value.trim() != value
            || value.chars().any(forbidden_terminal_character)
        {
            return Err(HouseholdStateError::InvalidDisplayName);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for DisplayName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DisplayName([REDACTED])")
    }
}

impl Serialize for DisplayName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for DisplayName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
pub enum HouseholdSubjectId {
    Self_,
    Member(MemberId),
}

impl HouseholdSubjectId {
    #[must_use]
    pub const fn self_() -> Self {
        Self::Self_
    }

    #[must_use]
    pub fn member(member: MemberId) -> Self {
        Self::Member(member)
    }

    #[must_use]
    pub fn as_member(&self) -> Option<&MemberId> {
        match self {
            Self::Self_ => None,
            Self::Member(member) => Some(member),
        }
    }

    fn canonical_cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Self_, Self::Self_) => Ordering::Equal,
            (Self::Self_, Self::Member(_)) => Ordering::Less,
            (Self::Member(_), Self::Self_) => Ordering::Greater,
            (Self::Member(left), Self::Member(right)) => {
                left.as_str().as_bytes().cmp(right.as_str().as_bytes())
            }
        }
    }
}

impl fmt::Debug for HouseholdSubjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Self_ => formatter.write_str("HouseholdSubjectId::Self_"),
            Self::Member(_) => formatter.write_str("HouseholdSubjectId::Member([REDACTED])"),
        }
    }
}

impl Serialize for HouseholdSubjectId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Self_ => serializer.serialize_str("self"),
            Self::Member(member) => {
                #[derive(Serialize)]
                struct MemberSubject<'a> {
                    member: &'a MemberId,
                }
                MemberSubject { member }.serialize(serializer)
            }
        }
    }
}

impl<'de> Deserialize<'de> for HouseholdSubjectId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        match value {
            Value::String(value) if value == "self" => Ok(Self::self_()),
            Value::Object(mut value) if value.len() == 1 => match value.remove("member") {
                Some(Value::String(member)) => MemberId::parse_preserved(member)
                    .map(Self::member)
                    .map_err(D::Error::custom),
                _ => Err(D::Error::custom("typed household subject is invalid")),
            },
            _ => Err(D::Error::custom("typed household subject is invalid")),
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub enum HouseholdScope {
    Subject(HouseholdSubjectId),
    Everyone,
}

impl Serialize for HouseholdScope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Subject(subject) => subject.serialize(serializer),
            Self::Everyone => serializer.serialize_str("everyone"),
        }
    }
}

impl<'de> Deserialize<'de> for HouseholdScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        if value == Value::String("everyone".to_owned()) {
            Ok(Self::Everyone)
        } else {
            HouseholdSubjectId::deserialize(value)
                .map(Self::Subject)
                .map_err(D::Error::custom)
        }
    }
}

impl fmt::Debug for HouseholdScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Subject(subject) => formatter
                .debug_tuple("HouseholdScope::Subject")
                .field(subject)
                .finish(),
            Self::Everyone => formatter.write_str("HouseholdScope::Everyone"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipV1 {
    #[serde(rename = "self")]
    Self_,
    Spouse,
    Partner,
    Parent,
    Child,
    Sibling,
    Grandparent,
    Friend,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipSourceV1 {
    NativeDeclared,
    LegacyDeclared,
    LegacyMissing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MinorStatusV1 {
    Minor,
    Adult,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgeBandV1 {
    #[serde(rename = "under_13")]
    Under13,
    #[serde(rename = "age_13_17")]
    Age13_17,
    #[serde(rename = "age_18_plus")]
    Age18Plus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgeEvidenceSourceV1 {
    NativeDeclared,
    LegacyDateOfBirth,
    LegacyAgeBand,
}

#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CanonicalDateV1(String);

impl CanonicalDateV1 {
    pub fn parse(value: impl Into<String>) -> Result<Self, HouseholdStateError> {
        let value = value.into();
        parse_date(&value)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn as_date(&self) -> Result<Date, HouseholdStateError> {
        parse_date(&self.0)
    }
}

impl fmt::Debug for CanonicalDateV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CanonicalDateV1([REDACTED])")
    }
}

impl Serialize for CanonicalDateV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for CanonicalDateV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DateOfBirthV1(CanonicalDateV1);

impl DateOfBirthV1 {
    pub fn parse_for_evaluation(
        value: impl Into<String>,
        evaluated_on: &CanonicalDateV1,
    ) -> Result<Self, HouseholdStateError> {
        let value = CanonicalDateV1::parse(value)?;
        age_on(value.as_date()?, evaluated_on.as_date()?)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    fn as_date(&self) -> Result<Date, HouseholdStateError> {
        self.0.as_date()
    }
}

impl fmt::Debug for DateOfBirthV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DateOfBirthV1([REDACTED])")
    }
}

impl Serialize for DateOfBirthV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DateOfBirthV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Self(CanonicalDateV1::deserialize(deserializer)?))
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgeEvidenceV1 {
    pub date_of_birth: Option<DateOfBirthV1>,
    pub age_band: Option<AgeBandV1>,
    pub source: AgeEvidenceSourceV1,
}

impl fmt::Debug for AgeEvidenceV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgeEvidenceV1")
            .field("has_date_of_birth", &self.date_of_birth.is_some())
            .field("age_band", &self.age_band)
            .field("source", &self.source)
            .finish()
    }
}

pub fn derive_minor_status_v1(
    relationship: RelationshipV1,
    evidence: Option<&AgeEvidenceV1>,
    evaluated_on: &CanonicalDateV1,
) -> Result<MinorStatusV1, HouseholdStateError> {
    if relationship == RelationshipV1::Self_ {
        return Err(HouseholdStateError::InvalidRelationship);
    }
    let relationship_minor = relationship == RelationshipV1::Child;
    let mut affirmative_minor = relationship_minor;
    let mut affirmative_adult = false;
    if let Some(evidence) = evidence {
        if let Some(date_of_birth) = &evidence.date_of_birth {
            let age = age_on(date_of_birth.as_date()?, evaluated_on.as_date()?)?;
            affirmative_minor |= age < 18;
            affirmative_adult |= age >= 18;
        }
        if let Some(age_band) = evidence.age_band {
            match age_band {
                AgeBandV1::Under13 | AgeBandV1::Age13_17 => affirmative_minor = true,
                AgeBandV1::Age18Plus => affirmative_adult = true,
            }
        }
    }
    Ok(if affirmative_minor {
        MinorStatusV1::Minor
    } else if affirmative_adult {
        MinorStatusV1::Adult
    } else {
        MinorStatusV1::Unknown
    })
}

#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CanonicalTimestampV1(String);

impl CanonicalTimestampV1 {
    pub fn parse(value: impl Into<String>) -> Result<Self, HouseholdStateError> {
        let value = value.into();
        if value.len() != 24
            || value.as_bytes().get(10) != Some(&b'T')
            || value.as_bytes().get(19) != Some(&b'.')
            || value.as_bytes().get(23) != Some(&b'Z')
            || !value.as_bytes()[20..23].iter().all(u8::is_ascii_digit)
        {
            return Err(HouseholdStateError::InvalidTimestamp);
        }
        let parsed = OffsetDateTime::parse(&value, &Rfc3339)
            .map_err(|_| HouseholdStateError::InvalidTimestamp)?;
        if parsed.offset() != UtcOffset::UTC || parsed.nanosecond() % 1_000_000 != 0 {
            return Err(HouseholdStateError::InvalidTimestamp);
        }
        Ok(Self(value))
    }

    pub fn from_datetime(value: OffsetDateTime) -> Result<Self, HouseholdStateError> {
        let value = value.to_offset(UtcOffset::UTC);
        Self::parse(format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            value.year(),
            u8::from(value.month()),
            value.day(),
            value.hour(),
            value.minute(),
            value.second(),
            value.millisecond()
        ))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn as_datetime(&self) -> Result<OffsetDateTime, HouseholdStateError> {
        OffsetDateTime::parse(&self.0, &Rfc3339).map_err(|_| HouseholdStateError::InvalidTimestamp)
    }
}

impl fmt::Debug for CanonicalTimestampV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CanonicalTimestampV1([REDACTED])")
    }
}

impl Serialize for CanonicalTimestampV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for CanonicalTimestampV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyTimestampProvenanceV1 {
    pub normalized: CanonicalTimestampV1,
    pub source_precision: u8,
    pub truncated: bool,
    pub original_sha256: CanonicalDigestV1,
}

pub fn normalize_legacy_timestamp_v1(
    value: &str,
    frozen_at: &CanonicalTimestampV1,
) -> Result<LegacyTimestampProvenanceV1, HouseholdStateError> {
    let (without_zone, zone_len) = if let Some(value) = value.strip_suffix('Z') {
        (value, 1)
    } else if let Some(value) = value.strip_suffix("+00:00") {
        (value, 6)
    } else {
        return Err(HouseholdStateError::InvalidTimestamp);
    };
    let (whole, fraction) = without_zone
        .split_once('.')
        .map_or((without_zone, ""), |(whole, fraction)| (whole, fraction));
    if whole.len() != 19
        || (!fraction.is_empty()
            && (fraction.len() > 6 || !fraction.bytes().all(|byte| byte.is_ascii_digit())))
        || (fraction.is_empty() && without_zone.contains('.'))
        || value.len() != without_zone.len() + zone_len
    {
        return Err(HouseholdStateError::InvalidTimestamp);
    }
    let mut milliseconds = fraction.chars().take(3).collect::<String>();
    while milliseconds.len() < 3 {
        milliseconds.push('0');
    }
    let normalized = CanonicalTimestampV1::parse(format!("{whole}.{milliseconds}Z"))?;
    if normalized.as_datetime()? > frozen_at.as_datetime()? {
        return Err(HouseholdStateError::InvalidTimestamp);
    }
    Ok(LegacyTimestampProvenanceV1 {
        normalized,
        source_precision: u8::try_from(fraction.len())
            .map_err(|_| HouseholdStateError::InvalidTimestamp)?,
        truncated: fraction.len() > 3,
        original_sha256: canonical_sha256_v1(&value)?,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HouseholdLifecycleV1 {
    Active,
    Archived,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HouseholdProfileStateV1 {
    Incomplete,
    LocalOnly,
    PendingSync,
    Synced,
    Conflicted,
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HouseholdOwnerV1 {
    pub display_name: DisplayName,
    pub relationship: RelationshipV1,
    pub profile_state: HouseholdProfileStateV1,
    pub created_at: CanonicalTimestampV1,
    pub updated_at: CanonicalTimestampV1,
}

impl HouseholdOwnerV1 {
    pub fn validate(&self) -> Result<(), HouseholdStateError> {
        if self.relationship != RelationshipV1::Self_
            || self.updated_at.as_datetime()? < self.created_at.as_datetime()?
        {
            return Err(HouseholdStateError::InvalidRelationship);
        }
        Ok(())
    }
}

impl fmt::Debug for HouseholdOwnerV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdOwnerV1")
            .field("relationship", &self.relationship)
            .field("profile_state", &self.profile_state)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HouseholdMemberV1 {
    pub member_id: MemberId,
    pub display_name: DisplayName,
    pub relationship: RelationshipV1,
    pub relationship_source: RelationshipSourceV1,
    pub minor_status: MinorStatusV1,
    pub age_evidence: Option<AgeEvidenceV1>,
    pub minor_status_evaluated_on: CanonicalDateV1,
    pub lifecycle: HouseholdLifecycleV1,
    pub profile_state: HouseholdProfileStateV1,
    pub created_at: CanonicalTimestampV1,
    pub updated_at: CanonicalTimestampV1,
}

impl HouseholdMemberV1 {
    pub fn validate(&self) -> Result<(), HouseholdStateError> {
        if self.relationship == RelationshipV1::Self_ {
            return Err(HouseholdStateError::InvalidRelationship);
        }
        if self.relationship_source == RelationshipSourceV1::LegacyMissing
            && self.relationship != RelationshipV1::Other
        {
            return Err(HouseholdStateError::InvalidRelationship);
        }
        if let Some(evidence) = &self.age_evidence {
            let source_is_consistent = match evidence.source {
                AgeEvidenceSourceV1::NativeDeclared => {
                    evidence.date_of_birth.is_some() || evidence.age_band.is_some()
                }
                AgeEvidenceSourceV1::LegacyDateOfBirth => {
                    evidence.date_of_birth.is_some() && evidence.age_band.is_none()
                }
                AgeEvidenceSourceV1::LegacyAgeBand => {
                    evidence.date_of_birth.is_none() && evidence.age_band.is_some()
                }
            };
            if !source_is_consistent {
                return Err(HouseholdStateError::InvalidMinorStatus);
            }
        }
        if derive_minor_status_v1(
            self.relationship,
            self.age_evidence.as_ref(),
            &self.minor_status_evaluated_on,
        )? != self.minor_status
        {
            return Err(HouseholdStateError::InvalidMinorStatus);
        }
        if self.updated_at.as_datetime()? < self.created_at.as_datetime()? {
            return Err(HouseholdStateError::InvalidTimestamp);
        }
        Ok(())
    }
}

impl fmt::Debug for HouseholdMemberV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdMemberV1")
            .field("relationship", &self.relationship)
            .field("minor_status", &self.minor_status)
            .field("lifecycle", &self.lifecycle)
            .field("profile_state", &self.profile_state)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HouseholdDeclaredProfileV1 {
    pub diet_style_ids: Vec<String>,
    pub custom_diet_styles: Vec<String>,
    pub allergy_ids: Vec<String>,
    pub custom_restrictions: Vec<String>,
    pub health_condition_ids: Vec<String>,
    pub custom_health_conditions: Vec<String>,
    pub avoid_ingredients: Vec<String>,
    pub activity_level: Option<String>,
    pub cuisine_preferences: Vec<String>,
    pub custom_cuisines: Vec<String>,
    pub severity_level: Option<u8>,
    pub notes: Option<String>,
}

impl HouseholdDeclaredProfileV1 {
    pub fn validate(&self) -> Result<Value, HouseholdStateError> {
        OnboardingProfileInput {
            diet_style_ids: self.diet_style_ids.clone(),
            custom_diet_styles: self.custom_diet_styles.clone(),
            allergy_ids: self.allergy_ids.clone(),
            custom_restrictions: self.custom_restrictions.clone(),
            health_condition_ids: self.health_condition_ids.clone(),
            custom_health_conditions: self.custom_health_conditions.clone(),
            avoid_ingredients: self.avoid_ingredients.clone(),
            activity_level: self.activity_level.clone(),
            cuisine_preferences: self.cuisine_preferences.clone(),
            custom_cuisines: self.custom_cuisines.clone(),
            severity_level: self.severity_level,
            notes: self.notes.clone(),
        }
        .profile_data()
        .map_err(|_| HouseholdStateError::InvalidProfileDocument)
    }
}

impl fmt::Debug for HouseholdDeclaredProfileV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdDeclaredProfileV1")
            .field("diet_style_count", &self.diet_style_ids.len())
            .field("allergy_count", &self.allergy_ids.len())
            .field("health_condition_count", &self.health_condition_ids.len())
            .field("avoid_ingredient_count", &self.avoid_ingredients.len())
            .field("has_notes", &self.notes.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DietaryProfileProjectionV1 {
    pub preferences: Option<Vec<String>>,
    pub restrictions: Option<Vec<String>>,
    pub avoid_ingredients: Option<Vec<String>>,
    pub medical_constraints: Option<Vec<String>>,
    pub cuisine_preferences: Option<Vec<String>>,
    pub health_condition_ids: Option<Vec<String>>,
    pub custom_health_conditions: Option<Vec<String>>,
    pub custom_diet_styles: Option<Vec<String>>,
    pub custom_restrictions: Option<Vec<String>>,
    pub custom_cuisines: Option<Vec<String>>,
    pub diet_style_ids: Option<Vec<String>>,
    pub allergy_ids: Option<Vec<String>>,
    pub additional_restriction_ids: Option<Vec<String>>,
    pub additional_medical_constraints: Option<Vec<String>>,
    pub preference_strictness: Option<Map<String, Value>>,
    pub restriction_handling: Option<Map<String, Value>>,
    pub condition_severity_levels: Option<Map<String, Value>>,
    pub notes: Option<String>,
    pub medical_condition_id: Option<String>,
    pub activity_level: Option<String>,
    pub severity_level: Option<i64>,
    pub selection_provenance_version: Option<i64>,
}

impl fmt::Debug for DietaryProfileProjectionV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = |values: &Option<Vec<String>>| values.as_ref().map_or(0, Vec::len);
        formatter
            .debug_struct("DietaryProfileProjectionV1")
            .field("preferences_count", &count(&self.preferences))
            .field("restrictions_count", &count(&self.restrictions))
            .field("avoid_ingredient_count", &count(&self.avoid_ingredients))
            .field(
                "medical_constraint_count",
                &count(&self.medical_constraints),
            )
            .field(
                "cuisine_preference_count",
                &count(&self.cuisine_preferences),
            )
            .field("health_condition_count", &count(&self.health_condition_ids))
            .field("has_notes", &self.notes.is_some())
            .field("has_activity_level", &self.activity_level.is_some())
            .field("has_severity", &self.severity_level.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileDocumentProvenanceV1 {
    NativeDeclared,
    LegacyLocalProjection,
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HouseholdProfileDocumentV1 {
    pub schema_version: u16,
    pub declared_profile: Option<HouseholdDeclaredProfileV1>,
    pub legacy_source_document: Option<CanonicalJsonObjectV1>,
    pub legacy_source_digest: Option<CanonicalDigestV1>,
    pub provenance: ProfileDocumentProvenanceV1,
}

impl HouseholdProfileDocumentV1 {
    pub fn native(
        declared_profile: HouseholdDeclaredProfileV1,
    ) -> Result<Self, HouseholdStateError> {
        declared_profile.validate()?;
        Ok(Self {
            schema_version: HOUSEHOLD_PROFILE_DOCUMENT_SCHEMA_VERSION,
            declared_profile: Some(declared_profile),
            legacy_source_document: None,
            legacy_source_digest: None,
            provenance: ProfileDocumentProvenanceV1::NativeDeclared,
        })
    }

    pub fn legacy_projection(input: &[u8]) -> Result<Self, HouseholdStateError> {
        let object =
            parse_bounded_json_object_v1(input, CompatibilityJsonLimitsV1::PROFILE_DOCUMENT)?;
        validate_profile_projection(&object)?;
        let document = CanonicalJsonObjectV1::from_map(object, MAX_PROFILE_DOCUMENT_BYTES)?;
        Ok(Self {
            schema_version: HOUSEHOLD_PROFILE_DOCUMENT_SCHEMA_VERSION,
            declared_profile: None,
            legacy_source_digest: Some(document.canonical_sha256()),
            legacy_source_document: Some(document),
            provenance: ProfileDocumentProvenanceV1::LegacyLocalProjection,
        })
    }

    pub fn validate(&self) -> Result<(), HouseholdStateError> {
        if self.schema_version != HOUSEHOLD_PROFILE_DOCUMENT_SCHEMA_VERSION {
            return Err(HouseholdStateError::InvalidSchemaVersion);
        }
        match self.provenance {
            ProfileDocumentProvenanceV1::NativeDeclared => {
                if self.legacy_source_document.is_some()
                    || self.legacy_source_digest.is_some()
                    || self
                        .declared_profile
                        .as_ref()
                        .ok_or(HouseholdStateError::InvalidProfileDocument)?
                        .validate()
                        .is_err()
                {
                    return Err(HouseholdStateError::InvalidProfileDocument);
                }
            }
            ProfileDocumentProvenanceV1::LegacyLocalProjection => {
                if self.declared_profile.is_some() {
                    return Err(HouseholdStateError::InvalidProfileDocument);
                }
                let document = self
                    .legacy_source_document
                    .as_ref()
                    .ok_or(HouseholdStateError::InvalidProfileDocument)?;
                if document.canonical_len() > MAX_PROFILE_DOCUMENT_BYTES
                    || self.legacy_source_digest != Some(document.canonical_sha256())
                {
                    return Err(HouseholdStateError::InvalidProfileDocument);
                }
                validate_profile_projection(document.as_map())?;
            }
        }
        Ok(())
    }

    pub fn legacy_projection_view(
        &self,
    ) -> Result<Option<DietaryProfileProjectionV1>, HouseholdStateError> {
        self.legacy_source_document
            .as_ref()
            .map(|document| {
                serde_json::from_value(Value::Object(document.as_map().clone()))
                    .map_err(|_| HouseholdStateError::InvalidProfileDocument)
            })
            .transpose()
    }

    pub fn effective_profile(&self) -> Result<Option<Value>, HouseholdStateError> {
        if let Some(declared) = &self.declared_profile {
            return declared.validate().map(Some);
        }
        let Some(document) = &self.legacy_source_document else {
            return Ok(None);
        };
        Ok(promoted_profile_projection(document.as_map()).map(Value::Object))
    }
}

impl fmt::Debug for HouseholdProfileDocumentV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdProfileDocumentV1")
            .field("schema_version", &self.schema_version)
            .field("provenance", &self.provenance)
            .field("has_declared_profile", &self.declared_profile.is_some())
            .field("legacy_source_digest", &self.legacy_source_digest)
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HouseholdProfileRecordV1 {
    pub subject: HouseholdSubjectId,
    pub profile_revision: ProfileRevision,
    pub document: HouseholdProfileDocumentV1,
}

impl fmt::Debug for HouseholdProfileRecordV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdProfileRecordV1")
            .field("profile_revision", &self.profile_revision)
            .field("document", &self.document)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyOutboxSourceKindV1 {
    PythonSubjectKeyedV1,
    RustMutationKeyedEmbeddedMemberV0,
    RustSubjectKeyedLocalContextV0,
    PythonSubjectKeyedPatchV0,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxPhaseV1 {
    PolicyBlockedLegacy,
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyProfileOutboxEntryV1 {
    pub target: HouseholdSubjectId,
    pub source_kind: LegacyOutboxSourceKindV1,
    pub source_key: String,
    pub source_digest: CanonicalDigestV1,
    pub payload: CanonicalJsonObjectV1,
    pub payload_digest: CanonicalDigestV1,
    pub phase: OutboxPhaseV1,
    pub updated_at: CanonicalTimestampV1,
}

impl LegacyProfileOutboxEntryV1 {
    fn validate(&self) -> Result<(), HouseholdStateError> {
        if self.phase != OutboxPhaseV1::PolicyBlockedLegacy
            || self.payload.canonical_len() > MAX_MIGRATION_CANDIDATE_BYTES
            || self.payload_digest != self.payload.canonical_sha256()
        {
            return Err(HouseholdStateError::InvalidOutbox);
        }
        let object = self.payload.as_map();
        let expected_target = match self.source_kind {
            LegacyOutboxSourceKindV1::RustMutationKeyedEmbeddedMemberV0 => {
                require_exact_keys(object, &["member_id", "repair"])?;
                if !object.get("repair").is_some_and(Value::is_boolean) {
                    return Err(HouseholdStateError::InvalidOutbox);
                }
                HouseholdOutboxId::parse_legacy(self.source_key.clone())?;
                parse_compatibility_subject(
                    object
                        .get("member_id")
                        .and_then(Value::as_str)
                        .ok_or(HouseholdStateError::InvalidOutbox)?,
                )?
            }
            LegacyOutboxSourceKindV1::PythonSubjectKeyedV1 => {
                require_exact_keys(
                    object,
                    &["fields", "local_context", "updated_at", "version"],
                )?;
                if object.get("version").and_then(Value::as_u64) != Some(1) {
                    return Err(HouseholdStateError::InvalidOutbox);
                }
                validate_bounded_object(object.get("fields"))?;
                validate_profile_projection(required_object(object.get("local_context"))?)?;
                let source_updated_at = object
                    .get("updated_at")
                    .and_then(Value::as_str)
                    .ok_or(HouseholdStateError::InvalidOutbox)?;
                if normalize_legacy_timestamp_v1(source_updated_at, &self.updated_at)?.normalized
                    != self.updated_at
                {
                    return Err(HouseholdStateError::InvalidOutbox);
                }
                parse_compatibility_subject(&self.source_key)?
            }
            LegacyOutboxSourceKindV1::RustSubjectKeyedLocalContextV0 => {
                require_exact_keys(object, &["local_context"])?;
                validate_profile_projection(required_object(object.get("local_context"))?)?;
                parse_compatibility_subject(&self.source_key)?
            }
            LegacyOutboxSourceKindV1::PythonSubjectKeyedPatchV0 => {
                require_exact_keys(object, &["fields", "local_context"])?;
                validate_patch_fields(required_object(object.get("fields"))?)?;
                validate_profile_projection(required_object(object.get("local_context"))?)?;
                parse_compatibility_subject(&self.source_key)?
            }
        };
        if expected_target != self.target {
            return Err(HouseholdStateError::InvalidOutbox);
        }
        Ok(())
    }
}

impl fmt::Debug for LegacyProfileOutboxEntryV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LegacyProfileOutboxEntryV1")
            .field("source_kind", &self.source_kind)
            .field("source_digest", &self.source_digest)
            .field("payload_digest", &self.payload_digest)
            .field("phase", &self.phase)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerSyncIntentPhaseV1 {
    NeedsConsentCheck,
    NeedsRemoteBase,
    ReadyToDispatch,
    DispatchingOutcomeUnknown,
    OutcomeUncertain,
    DefiniteFailure,
    Conflicted,
    LocalOnlyNoConsent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LastDefiniteOwnerSyncErrorV1 {
    ConsentAbsent,
    Unauthorized,
    Forbidden,
    Validation,
    VersionConflict,
    NotFound,
    PredispatchCancelled,
    ConsentVersionChangedRequiresNewSave,
    ConsentRevokedRegrantRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteProfileExistenceV1 {
    Absent,
    Present,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteProfileBaseV1 {
    pub existence: RemoteProfileExistenceV1,
    pub version: Option<u64>,
    pub profile_digest: Option<CanonicalDigestV1>,
}

impl RemoteProfileBaseV1 {
    fn validate(&self) -> Result<(), HouseholdStateError> {
        match self.existence {
            RemoteProfileExistenceV1::Absent
                if self.version.is_none() && self.profile_digest.is_none() =>
            {
                Ok(())
            }
            RemoteProfileExistenceV1::Present
                if self.version.is_some_and(|version| version != 0)
                    && self.profile_digest.is_some() =>
            {
                Ok(())
            }
            _ => Err(HouseholdStateError::InvalidOwnerSyncIntent),
        }
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerSyncIntentV1 {
    pub schema_version: u16,
    pub intent_id: Uuid,
    pub intent_revision: u64,
    pub phase: OwnerSyncIntentPhaseV1,
    pub subject: HouseholdSubjectId,
    pub local_household_revision: u64,
    pub local_profile_revision: u64,
    pub local_profile_digest: CanonicalDigestV1,
    pub remote_request_id: Uuid,
    pub consent_version: Option<ConsentVersionV1>,
    pub remote_base: Option<RemoteProfileBaseV1>,
    pub expected_remote_version: Option<u64>,
    pub request_method: Option<String>,
    pub request_path: Option<String>,
    pub request_body: Option<CanonicalJsonObjectV1>,
    pub request_body_digest: Option<CanonicalDigestV1>,
    pub attempt_count: u32,
    pub last_definite_error: Option<LastDefiniteOwnerSyncErrorV1>,
    pub created_at: CanonicalTimestampV1,
    pub updated_at: CanonicalTimestampV1,
}

impl OwnerSyncIntentV1 {
    pub fn validate(&self) -> Result<(), HouseholdStateError> {
        if self.schema_version != 1
            || self.intent_revision == 0
            || self.local_household_revision == 0
            || self.local_profile_revision == 0
            || self.subject != HouseholdSubjectId::self_()
            || self.intent_id != self.remote_request_id
            || require_uuid_v4(self.intent_id).is_err()
            || self.updated_at.as_datetime()? < self.created_at.as_datetime()?
        {
            return Err(HouseholdStateError::InvalidOwnerSyncIntent);
        }
        let request_group_all_null = self.request_method.is_none()
            && self.request_path.is_none()
            && self.request_body.is_none()
            && self.request_body_digest.is_none();
        let request_group_all_present = self.request_method.as_deref() == Some("PUT")
            && self.request_path.as_deref() == Some("/v1/profile/sync")
            && self.request_body.is_some()
            && self.request_body_digest.is_some();
        if !request_group_all_null && !request_group_all_present {
            return Err(HouseholdStateError::InvalidOwnerSyncIntent);
        }
        if let Some(body) = &self.request_body {
            if body.canonical_len() > MAX_OWNER_SYNC_REQUEST_BODY_BYTES
                || self.request_body_digest != Some(body.canonical_sha256())
            {
                return Err(HouseholdStateError::InvalidOwnerSyncIntent);
            }
            validate_owner_sync_request_body(
                body.as_map(),
                self.remote_base.as_ref(),
                self.local_profile_digest,
            )?;
        }
        if let Some(base) = &self.remote_base {
            base.validate()?;
            match base.existence {
                RemoteProfileExistenceV1::Absent if self.expected_remote_version.is_some() => {
                    return Err(HouseholdStateError::InvalidOwnerSyncIntent);
                }
                RemoteProfileExistenceV1::Present
                    if self.expected_remote_version != base.version =>
                {
                    return Err(HouseholdStateError::InvalidOwnerSyncIntent);
                }
                _ => {}
            }
        } else if self.expected_remote_version.is_some() {
            return Err(HouseholdStateError::InvalidOwnerSyncIntent);
        }
        let base_and_request_present = self.remote_base.is_some() && request_group_all_present;
        let base_and_request_absent = self.remote_base.is_none() && request_group_all_null;
        let valid = match self.phase {
            OwnerSyncIntentPhaseV1::NeedsConsentCheck => {
                self.consent_version.is_none()
                    && base_and_request_absent
                    && self.attempt_count == 0
                    && self.last_definite_error.is_none()
            }
            OwnerSyncIntentPhaseV1::NeedsRemoteBase => {
                self.consent_version.is_some()
                    && base_and_request_absent
                    && self.attempt_count == 0
                    && self.last_definite_error.is_none()
            }
            OwnerSyncIntentPhaseV1::ReadyToDispatch => {
                self.consent_version.is_some()
                    && base_and_request_present
                    && ((self.attempt_count == 0 && self.last_definite_error.is_none())
                        || (self.attempt_count >= 1
                            && matches!(
                                self.last_definite_error,
                                None | Some(LastDefiniteOwnerSyncErrorV1::PredispatchCancelled)
                            )))
            }
            OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown => {
                self.consent_version.is_some()
                    && base_and_request_present
                    && self.attempt_count >= 1
                    && self.last_definite_error.is_none()
            }
            OwnerSyncIntentPhaseV1::OutcomeUncertain => {
                self.consent_version.is_some()
                    && base_and_request_present
                    && self.attempt_count >= 1
                    && matches!(
                        self.last_definite_error,
                        None | Some(LastDefiniteOwnerSyncErrorV1::VersionConflict)
                    )
            }
            OwnerSyncIntentPhaseV1::DefiniteFailure => {
                self.consent_version.is_some()
                    && base_and_request_present
                    && matches!(
                        self.last_definite_error,
                        Some(
                            LastDefiniteOwnerSyncErrorV1::Unauthorized
                                | LastDefiniteOwnerSyncErrorV1::Forbidden
                                | LastDefiniteOwnerSyncErrorV1::Validation
                                | LastDefiniteOwnerSyncErrorV1::NotFound
                                | LastDefiniteOwnerSyncErrorV1::ConsentVersionChangedRequiresNewSave
                                | LastDefiniteOwnerSyncErrorV1::ConsentRevokedRegrantRequired
                        )
                    )
                    && (self.attempt_count >= 1
                        || matches!(
                            self.last_definite_error,
                            Some(
                                LastDefiniteOwnerSyncErrorV1::ConsentVersionChangedRequiresNewSave
                                    | LastDefiniteOwnerSyncErrorV1::ConsentRevokedRegrantRequired
                            )
                        ))
            }
            OwnerSyncIntentPhaseV1::Conflicted => {
                self.consent_version.is_some()
                    && base_and_request_present
                    && self.attempt_count >= 1
                    && self.last_definite_error
                        == Some(LastDefiniteOwnerSyncErrorV1::VersionConflict)
            }
            OwnerSyncIntentPhaseV1::LocalOnlyNoConsent => {
                self.consent_version.is_none()
                    && base_and_request_absent
                    && self.attempt_count == 0
                    && self.last_definite_error == Some(LastDefiniteOwnerSyncErrorV1::ConsentAbsent)
            }
        };
        valid
            .then_some(())
            .ok_or(HouseholdStateError::InvalidOwnerSyncIntent)
    }
}

impl fmt::Debug for OwnerSyncIntentV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OwnerSyncIntentV1")
            .field("schema_version", &self.schema_version)
            .field("intent_revision", &self.intent_revision)
            .field("phase", &self.phase)
            .field("local_household_revision", &self.local_household_revision)
            .field("local_profile_revision", &self.local_profile_revision)
            .field("local_profile_digest", &self.local_profile_digest)
            .field("attempt_count", &self.attempt_count)
            .field("last_definite_error", &self.last_definite_error)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HouseholdProfileOutboxEntryV1 {
    Legacy {
        version: u16,
        target: HouseholdSubjectId,
        legacy: LegacyProfileOutboxEntryV1,
    },
    OwnerSync {
        version: u16,
        target: HouseholdSubjectId,
        intent: OwnerSyncIntentV1,
    },
}

impl HouseholdProfileOutboxEntryV1 {
    #[must_use]
    pub fn target(&self) -> &HouseholdSubjectId {
        match self {
            Self::Legacy { target, .. } | Self::OwnerSync { target, .. } => target,
        }
    }

    fn validate(&self) -> Result<(), HouseholdStateError> {
        match self {
            Self::Legacy {
                version,
                target,
                legacy,
            } if *version == 1 && target == &legacy.target && legacy.validate().is_ok() => Ok(()),
            Self::OwnerSync {
                version,
                target,
                intent,
            } if *version == 1
                && target == &HouseholdSubjectId::self_()
                && target == &intent.subject =>
            {
                intent.validate()
            }
            _ => Err(HouseholdStateError::InvalidOutbox),
        }
    }
}

impl fmt::Debug for HouseholdProfileOutboxEntryV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Legacy {
                version,
                target,
                legacy,
            } => formatter
                .debug_struct("HouseholdProfileOutboxEntryV1::Legacy")
                .field("version", version)
                .field("target_kind", &subject_kind(target))
                .field("legacy", legacy)
                .finish(),
            Self::OwnerSync {
                version,
                target,
                intent,
            } => formatter
                .debug_struct("HouseholdProfileOutboxEntryV1::OwnerSync")
                .field("version", version)
                .field("target_kind", &subject_kind(target))
                .field("intent", intent)
                .finish(),
        }
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HouseholdOutboxRecordV1 {
    pub outbox_id: HouseholdOutboxId,
    pub outbox_revision: OutboxRevision,
    pub entry: HouseholdProfileOutboxEntryV1,
}

impl HouseholdOutboxRecordV1 {
    fn validate(&self) -> Result<(), HouseholdStateError> {
        self.entry.validate()?;
        match &self.entry {
            HouseholdProfileOutboxEntryV1::OwnerSync { intent, .. } => {
                let expected = HouseholdOutboxId::owner_sync(intent.intent_id)?;
                if self.outbox_id != expected
                    || self.outbox_revision.get() != intent.intent_revision
                {
                    return Err(HouseholdStateError::InvalidOwnerSyncIntent);
                }
            }
            HouseholdProfileOutboxEntryV1::Legacy { legacy, .. } => {
                if self
                    .outbox_id
                    .as_str()
                    .starts_with(OWNER_SYNC_OUTBOX_PREFIX)
                {
                    return Err(HouseholdStateError::InvalidOutbox);
                }
                let expected = match legacy.source_kind {
                    LegacyOutboxSourceKindV1::RustMutationKeyedEmbeddedMemberV0 => {
                        HouseholdOutboxId::parse_legacy(legacy.source_key.clone())?
                    }
                    source_kind => HouseholdOutboxId::deterministic_legacy(
                        source_kind,
                        legacy.source_digest,
                        &legacy.source_key,
                        legacy.payload_digest,
                    )?,
                };
                if self.outbox_id != expected {
                    return Err(HouseholdStateError::InvalidOutbox);
                }
            }
        }
        Ok(())
    }
}

impl fmt::Debug for HouseholdOutboxRecordV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdOutboxRecordV1")
            .field("outbox_revision", &self.outbox_revision)
            .field("entry", &self.entry)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppliedCommitOutcomeV1 {
    Initialized,
    Committed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppliedCommitRecordV1 {
    pub commit_id: CommitId,
    pub fingerprint: CanonicalDigestV1,
    pub resulting_revision: HouseholdRevision,
    pub outcome: AppliedCommitOutcomeV1,
    pub committed_at: CanonicalTimestampV1,
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedCompatibilityFieldV1 {
    pub field_name: String,
    pub value: CanonicalJsonValueV1,
    pub source_digest: CanonicalDigestV1,
}

impl fmt::Debug for ImportedCompatibilityFieldV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImportedCompatibilityFieldV1")
            .field("source_digest", &self.source_digest)
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyRemoteProfileReferenceV1 {
    pub subject: HouseholdSubjectId,
    pub source_digest: CanonicalDigestV1,
}

impl fmt::Debug for LegacyRemoteProfileReferenceV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LegacyRemoteProfileReferenceV1")
            .field("subject_kind", &subject_kind(&self.subject))
            .field("source_digest", &self.source_digest)
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LegacyTimestampDispositionV1 {
    Normalized {
        provenance: LegacyTimestampProvenanceV1,
    },
    LegacyMissingTime {
        normalized: CanonicalTimestampV1,
    },
}

impl fmt::Debug for LegacyTimestampDispositionV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Normalized { provenance } => formatter
                .debug_struct("LegacyTimestampDispositionV1::Normalized")
                .field("source_precision", &provenance.source_precision)
                .field("truncated", &provenance.truncated)
                .field("original_sha256", &provenance.original_sha256)
                .finish(),
            Self::LegacyMissingTime { .. } => {
                formatter.write_str("LegacyTimestampDispositionV1::LegacyMissingTime")
            }
        }
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyTimestampRecordV1 {
    pub field_path: String,
    pub disposition: LegacyTimestampDispositionV1,
}

impl fmt::Debug for LegacyTimestampRecordV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LegacyTimestampRecordV1")
            .field("disposition", &self.disposition)
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedCompatibilityStateV1 {
    pub fields: Vec<ImportedCompatibilityFieldV1>,
    pub legacy_python_applied_mutation_ids: Vec<String>,
    pub legacy_python_applied_mutation_ids_digest: Option<CanonicalDigestV1>,
    pub legacy_remote_profile_references: Vec<LegacyRemoteProfileReferenceV1>,
    pub legacy_timestamp_provenance: Vec<LegacyTimestampRecordV1>,
}

impl fmt::Debug for ImportedCompatibilityStateV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImportedCompatibilityStateV1")
            .field("field_count", &self.fields.len())
            .field(
                "legacy_applied_mutation_count",
                &self.legacy_python_applied_mutation_ids.len(),
            )
            .field(
                "legacy_applied_mutation_digest",
                &self.legacy_python_applied_mutation_ids_digest,
            )
            .field(
                "legacy_remote_profile_reference_count",
                &self.legacy_remote_profile_references.len(),
            )
            .field(
                "legacy_timestamp_provenance_count",
                &self.legacy_timestamp_provenance.len(),
            )
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationDispositionKindV1 {
    Migrated,
    Retired,
    ReauthenticationRequired,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationDispositionV1 {
    pub field_name: String,
    pub disposition: MigrationDispositionKindV1,
    pub destination_schema: Option<String>,
    pub source_digest: Option<CanonicalDigestV1>,
    pub destination_digest: Option<CanonicalDigestV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationDispositionManifestV1 {
    pub dispositions: Vec<MigrationDispositionV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LegacySourceIdentityV1 {
    Present {
        source_kind: String,
        source_digest: CanonicalDigestV1,
    },
    NoSource {
        source_set_fingerprint: CanonicalDigestV1,
    },
}

/// Content-free authority for retiring one exact released Python snapshot
/// after the authenticated native state has committed.
///
/// The locator digest prevents retargeting retirement to another path, while
/// the content digest prevents deleting bytes that appeared or changed after
/// migration. The pair is persisted inside authenticated canonical household
/// state so crash recovery never needs to re-read legacy config or keyring
/// sources after a committed vault generation exists.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyPythonSnapshotProvenanceV1 {
    pub locator_digest: CanonicalDigestV1,
    pub content_digest: CanonicalDigestV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationProvenanceV1 {
    pub source_identity: LegacySourceIdentityV1,
    pub legacy_python_snapshot: Option<LegacyPythonSnapshotProvenanceV1>,
    pub migration_id: Uuid,
    pub initialization_id: Uuid,
    pub initial_commit_id: CommitId,
    pub migration_frozen_at: CanonicalTimestampV1,
}

#[derive(Clone, Eq, PartialEq, Serialize)]
pub struct HouseholdStateV1 {
    pub schema_version: u16,
    pub account_binding: AccountId,
    pub revision: HouseholdRevision,
    pub owner: HouseholdOwnerV1,
    pub active_scope: HouseholdScope,
    pub members: Vec<HouseholdMemberV1>,
    pub profiles: Vec<HouseholdProfileRecordV1>,
    pub outbox: Vec<HouseholdOutboxRecordV1>,
    pub bounded_applied_commits: Vec<AppliedCommitRecordV1>,
    pub imported_compatibility: ImportedCompatibilityStateV1,
    pub migration_dispositions: MigrationDispositionManifestV1,
    pub migration_provenance: MigrationProvenanceV1,
    pub updated_at: CanonicalTimestampV1,
}

impl HouseholdStateV1 {
    pub fn validate(&self) -> Result<(), HouseholdStateError> {
        if self.schema_version != HOUSEHOLD_STATE_SCHEMA_VERSION {
            return Err(HouseholdStateError::InvalidSchemaVersion);
        }
        self.owner.validate()?;
        if self.members.len() > MAX_HOUSEHOLD_MEMBERS
            || self.members.len() + 1 > MAX_HOUSEHOLD_SUBJECTS
            || self.profiles.len() > MAX_HOUSEHOLD_PROFILES
            || self.outbox.len() > MAX_HOUSEHOLD_OUTBOX_ENTRIES
            || self.bounded_applied_commits.len() > MAX_APPLIED_COMMITS
            || self.imported_compatibility.fields.len() > MAX_IMPORTED_COMPATIBILITY_FIELDS
            || self
                .imported_compatibility
                .legacy_python_applied_mutation_ids
                .len()
                > MAX_LEGACY_APPLIED_MUTATION_IDS
            || self
                .imported_compatibility
                .legacy_remote_profile_references
                .len()
                > MAX_LEGACY_REMOTE_PROFILE_REFERENCES
            || self
                .imported_compatibility
                .legacy_timestamp_provenance
                .len()
                > MAX_LEGACY_TIMESTAMP_PROVENANCE
            || self.migration_dispositions.dispositions.len() > MAX_MIGRATION_DISPOSITIONS
        {
            return Err(HouseholdStateError::CardinalityExceeded);
        }
        validate_sorted_unique_by(&self.members, |left, right| {
            left.member_id
                .as_str()
                .as_bytes()
                .cmp(right.member_id.as_str().as_bytes())
        })?;
        for member in &self.members {
            member.validate()?;
        }
        validate_sorted_unique_by(&self.profiles, |left, right| {
            left.subject.canonical_cmp(&right.subject)
        })?;
        validate_sorted_unique_by(&self.outbox, |left, right| {
            left.outbox_id
                .as_str()
                .as_bytes()
                .cmp(right.outbox_id.as_str().as_bytes())
        })?;
        validate_sorted_unique_by(&self.bounded_applied_commits, |left, right| {
            left.commit_id
                .as_uuid()
                .as_bytes()
                .cmp(right.commit_id.as_uuid().as_bytes())
        })?;
        validate_sorted_unique_by(&self.imported_compatibility.fields, |left, right| {
            left.field_name.as_bytes().cmp(right.field_name.as_bytes())
        })?;
        validate_sorted_unique_by(
            &self.imported_compatibility.legacy_remote_profile_references,
            |left, right| left.subject.canonical_cmp(&right.subject),
        )?;
        validate_sorted_unique_by(
            &self.imported_compatibility.legacy_timestamp_provenance,
            |left, right| left.field_path.as_bytes().cmp(right.field_path.as_bytes()),
        )?;
        validate_sorted_unique_by(&self.migration_dispositions.dispositions, |left, right| {
            left.field_name.as_bytes().cmp(right.field_name.as_bytes())
        })?;

        let mut profile_usability = Vec::with_capacity(self.profiles.len());
        for profile in &self.profiles {
            self.validate_subject_reference(&profile.subject)?;
            profile.document.validate()?;
            profile_usability.push((
                profile.subject.clone(),
                profile.document.effective_profile()?.is_some(),
            ));
        }
        let mut owner_sync_count = 0_usize;
        for record in &self.outbox {
            record.validate()?;
            self.validate_subject_reference(record.entry.target())?;
            if let HouseholdProfileOutboxEntryV1::OwnerSync { intent, .. } = &record.entry {
                owner_sync_count += 1;
                self.validate_owner_sync_record(intent)?;
            }
        }
        if owner_sync_count > 1 {
            return Err(HouseholdStateError::InvalidOwnerSyncIntent);
        }
        let owner_has_usable_profile = profile_usability
            .iter()
            .any(|(subject, usable)| subject == &HouseholdSubjectId::self_() && *usable);
        let owner_legacy_context_count =
            self.distinct_legacy_context_count(&HouseholdSubjectId::self_())?;
        let owner_has_context_conflict = owner_legacy_context_count > 1;
        match self.owner.profile_state {
            HouseholdProfileStateV1::Incomplete
                if !owner_has_usable_profile
                    && owner_legacy_context_count == 0
                    && owner_sync_count == 0 => {}
            HouseholdProfileStateV1::LocalOnly | HouseholdProfileStateV1::Synced
                if owner_has_usable_profile && !owner_has_context_conflict => {}
            HouseholdProfileStateV1::PendingSync
                if owner_has_usable_profile
                    && !owner_has_context_conflict
                    && owner_sync_count == 1 => {}
            HouseholdProfileStateV1::Conflicted
                if owner_has_context_conflict
                    || (owner_has_usable_profile && owner_sync_count == 1) => {}
            _ => return Err(HouseholdStateError::InvalidProfileDocument),
        }
        for member in &self.members {
            let subject = HouseholdSubjectId::member(member.member_id.clone());
            let has_usable_profile = profile_usability
                .iter()
                .any(|(candidate, usable)| candidate == &subject && *usable);
            let legacy_context_count = self.distinct_legacy_context_count(&subject)?;
            let has_context_conflict = legacy_context_count > 1;
            let valid = match member.profile_state {
                HouseholdProfileStateV1::Incomplete => {
                    !has_usable_profile && legacy_context_count == 0
                }
                HouseholdProfileStateV1::LocalOnly => has_usable_profile && !has_context_conflict,
                HouseholdProfileStateV1::Conflicted => has_context_conflict,
                HouseholdProfileStateV1::PendingSync | HouseholdProfileStateV1::Synced => false,
            };
            if !valid {
                return Err(HouseholdStateError::InvalidProfileDocument);
            }
        }
        for reference in &self.imported_compatibility.legacy_remote_profile_references {
            self.validate_subject_reference(&reference.subject)?;
        }
        match &self.active_scope {
            HouseholdScope::Subject(subject) => {
                self.validate_subject_reference(subject)?;
                if subject.as_member().is_some_and(|member_id| {
                    self.members.iter().any(|member| {
                        &member.member_id == member_id
                            && member.lifecycle == HouseholdLifecycleV1::Archived
                    })
                }) {
                    return Err(HouseholdStateError::ArchivedActiveTarget);
                }
            }
            HouseholdScope::Everyone => {
                if !self
                    .members
                    .iter()
                    .any(|member| member.lifecycle == HouseholdLifecycleV1::Active)
                {
                    return Err(HouseholdStateError::EveryoneRequiresTwoActiveSubjects);
                }
            }
        }
        require_uuid_v4(self.migration_provenance.migration_id)?;
        require_uuid_v4(self.migration_provenance.initialization_id)?;
        require_uuid_v4(self.migration_provenance.initial_commit_id.as_uuid())?;
        let migration_frozen_at = self
            .migration_provenance
            .migration_frozen_at
            .as_datetime()?;
        let household_updated_at = self.updated_at.as_datetime()?;
        let invalid_household_time = if self.revision.get() == 1 {
            // A migrated initial state may preserve the normalized historical
            // Python household timestamp. Clean initialization uses equality.
            household_updated_at > migration_frozen_at
        } else {
            // Every native mutation is frozen after initialization, so a
            // later revision cannot move the household clock behind the
            // durable migration/initialization boundary.
            household_updated_at < migration_frozen_at
        };
        if invalid_household_time {
            return Err(HouseholdStateError::InvalidTimestamp);
        }
        for provenance in &self.imported_compatibility.legacy_timestamp_provenance {
            if provenance.field_path.is_empty()
                || provenance.field_path.len() > 512
                || provenance.field_path.trim() != provenance.field_path
                || provenance
                    .field_path
                    .chars()
                    .any(forbidden_terminal_character)
            {
                return Err(HouseholdStateError::InvalidTimestamp);
            }
            match &provenance.disposition {
                LegacyTimestampDispositionV1::Normalized { provenance } => {
                    if provenance.source_precision > 6
                        || provenance.truncated != (provenance.source_precision > 3)
                        || provenance.normalized.as_datetime()?
                            > self
                                .migration_provenance
                                .migration_frozen_at
                                .as_datetime()?
                    {
                        return Err(HouseholdStateError::InvalidTimestamp);
                    }
                }
                LegacyTimestampDispositionV1::LegacyMissingTime { normalized } => {
                    if normalized != &self.migration_provenance.migration_frozen_at {
                        return Err(HouseholdStateError::InvalidTimestamp);
                    }
                }
            }
        }
        for commit in &self.bounded_applied_commits {
            require_uuid_v4(commit.commit_id.as_uuid())?;
            if commit.resulting_revision > self.revision {
                return Err(HouseholdStateError::InvalidRevision);
            }
        }
        for field in &self.imported_compatibility.fields {
            if !is_valid_metadata_label(&field.field_name, 128)
                || field.value.canonical_len() > MAX_MIGRATION_CANDIDATE_BYTES
                || field.source_digest != field.value.canonical_sha256()
            {
                return Err(HouseholdStateError::InvalidProfileDocument);
            }
        }
        for mutation_id in &self
            .imported_compatibility
            .legacy_python_applied_mutation_ids
        {
            validate_opaque_identity(mutation_id, true)?;
        }
        let expected_legacy_digest = if self
            .imported_compatibility
            .legacy_python_applied_mutation_ids
            .is_empty()
        {
            None
        } else {
            Some(canonical_sha256_v1(
                &self
                    .imported_compatibility
                    .legacy_python_applied_mutation_ids,
            )?)
        };
        if self
            .imported_compatibility
            .legacy_python_applied_mutation_ids_digest
            != expected_legacy_digest
        {
            return Err(HouseholdStateError::InvalidProfileDocument);
        }
        for disposition in &self.migration_dispositions.dispositions {
            if !is_valid_metadata_label(&disposition.field_name, 128)
                || disposition
                    .destination_schema
                    .as_ref()
                    .is_some_and(|value| !is_valid_metadata_label(value, 256))
            {
                return Err(HouseholdStateError::InvalidProfileDocument);
            }
        }
        if let LegacySourceIdentityV1::Present { source_kind, .. } =
            &self.migration_provenance.source_identity
            && !is_valid_metadata_label(source_kind, 128)
        {
            return Err(HouseholdStateError::InvalidIdentity);
        }
        if matches!(
            (
                &self.migration_provenance.source_identity,
                &self.migration_provenance.legacy_python_snapshot,
            ),
            (LegacySourceIdentityV1::NoSource { .. }, Some(_))
        ) {
            return Err(HouseholdStateError::InvalidMigrationProvenance);
        }
        Ok(())
    }

    pub fn ensure_commit_capacity(&self) -> Result<(), HouseholdStateError> {
        if self.bounded_applied_commits.len() >= MAX_APPLIED_COMMITS {
            Err(HouseholdStateError::AppliedCommitLedgerFull)
        } else {
            Ok(())
        }
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, HouseholdStateError> {
        self.validate()?;
        let bytes = crate::household_canonical::to_canonical_bytes_v1(self)?;
        if bytes.len() > MAX_CANONICAL_VAULT_PLAINTEXT_BYTES {
            return Err(HouseholdStateError::CardinalityExceeded);
        }
        Ok(bytes)
    }

    fn validate_subject_reference(
        &self,
        subject: &HouseholdSubjectId,
    ) -> Result<(), HouseholdStateError> {
        match subject {
            HouseholdSubjectId::Self_ => Ok(()),
            HouseholdSubjectId::Member(member)
                if self
                    .members
                    .binary_search_by(|candidate| {
                        candidate
                            .member_id
                            .as_str()
                            .as_bytes()
                            .cmp(member.as_str().as_bytes())
                    })
                    .is_ok() =>
            {
                Ok(())
            }
            HouseholdSubjectId::Member(_) => Err(HouseholdStateError::OrphanReference),
        }
    }

    fn validate_owner_sync_record(
        &self,
        intent: &OwnerSyncIntentV1,
    ) -> Result<(), HouseholdStateError> {
        if intent.local_household_revision > self.revision.get() {
            return Err(HouseholdStateError::InvalidOwnerSyncIntent);
        }
        let profile = self
            .profiles
            .iter()
            .find(|profile| profile.subject == HouseholdSubjectId::self_())
            .ok_or(HouseholdStateError::InvalidOwnerSyncIntent)?;
        if profile.profile_revision.get() != intent.local_profile_revision {
            return Err(HouseholdStateError::InvalidOwnerSyncIntent);
        }
        let effective_profile = profile
            .document
            .effective_profile()?
            .ok_or(HouseholdStateError::InvalidOwnerSyncIntent)?;
        if canonical_sha256_v1(&effective_profile)? != intent.local_profile_digest {
            return Err(HouseholdStateError::InvalidOwnerSyncIntent);
        }
        let expected_state = match intent.phase {
            OwnerSyncIntentPhaseV1::LocalOnlyNoConsent => HouseholdProfileStateV1::LocalOnly,
            OwnerSyncIntentPhaseV1::Conflicted => HouseholdProfileStateV1::Conflicted,
            OwnerSyncIntentPhaseV1::DefiniteFailure
                if intent.last_definite_error
                    == Some(LastDefiniteOwnerSyncErrorV1::ConsentRevokedRegrantRequired) =>
            {
                HouseholdProfileStateV1::LocalOnly
            }
            _ => HouseholdProfileStateV1::PendingSync,
        };
        if self.owner.profile_state != expected_state {
            return Err(HouseholdStateError::InvalidOwnerSyncIntent);
        }
        Ok(())
    }

    fn distinct_legacy_context_count(
        &self,
        subject: &HouseholdSubjectId,
    ) -> Result<usize, HouseholdStateError> {
        let mut digests = BTreeSet::new();
        for record in &self.outbox {
            let HouseholdProfileOutboxEntryV1::Legacy { target, legacy, .. } = &record.entry else {
                continue;
            };
            if target != subject {
                continue;
            }
            let Some(local_context) = legacy.payload.as_map().get("local_context") else {
                continue;
            };
            let local_context = local_context
                .as_object()
                .ok_or(HouseholdStateError::InvalidOutbox)?;
            if promoted_profile_projection(local_context).is_some() {
                digests.insert(canonical_sha256_v1(&Value::Object(local_context.clone()))?);
            }
        }
        Ok(digests.len())
    }
}

impl fmt::Debug for HouseholdStateV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdStateV1")
            .field("schema_version", &self.schema_version)
            .field("revision", &self.revision)
            .field("active_scope_kind", &scope_kind(&self.active_scope))
            .field("member_count", &self.members.len())
            .field("profile_count", &self.profiles.len())
            .field("outbox_count", &self.outbox.len())
            .field("applied_commit_count", &self.bounded_applied_commits.len())
            .field(
                "compatibility_field_count",
                &self.imported_compatibility.fields.len(),
            )
            .finish()
    }
}

impl<'de> Deserialize<'de> for HouseholdStateV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            schema_version: u16,
            account_binding: AccountId,
            revision: HouseholdRevision,
            owner: HouseholdOwnerV1,
            active_scope: HouseholdScope,
            members: Vec<HouseholdMemberV1>,
            profiles: Vec<HouseholdProfileRecordV1>,
            outbox: Vec<HouseholdOutboxRecordV1>,
            bounded_applied_commits: Vec<AppliedCommitRecordV1>,
            imported_compatibility: ImportedCompatibilityStateV1,
            migration_dispositions: MigrationDispositionManifestV1,
            migration_provenance: MigrationProvenanceV1,
            updated_at: CanonicalTimestampV1,
        }
        let raw = Raw::deserialize(deserializer)?;
        let state = Self {
            schema_version: raw.schema_version,
            account_binding: raw.account_binding,
            revision: raw.revision,
            owner: raw.owner,
            active_scope: raw.active_scope,
            members: raw.members,
            profiles: raw.profiles,
            outbox: raw.outbox,
            bounded_applied_commits: raw.bounded_applied_commits,
            imported_compatibility: raw.imported_compatibility,
            migration_dispositions: raw.migration_dispositions,
            migration_provenance: raw.migration_provenance,
            updated_at: raw.updated_at,
        };
        state.validate().map_err(D::Error::custom)?;
        Ok(state)
    }
}

/// Decode vault plaintext only when it is already exact Canonical Bytes v1.
/// This catches duplicate names before conversion and rejects alternate UUID,
/// timestamp, object-order, number, and optional-field spellings.
pub fn decode_canonical_household_state_v1(
    input: &[u8],
) -> Result<HouseholdStateV1, HouseholdStateError> {
    crate::household_canonical::preflight_bounded_typed_json_v1(
        input,
        CompatibilityJsonLimitsV1::VAULT_PLAINTEXT,
        &[
            ("members", MAX_HOUSEHOLD_MEMBERS),
            ("profiles", MAX_HOUSEHOLD_PROFILES),
            ("outbox", MAX_HOUSEHOLD_OUTBOX_ENTRIES),
        ],
    )?;
    let state: HouseholdStateV1 =
        serde_json::from_slice(input).map_err(|_| HouseholdStateError::InvalidProfileDocument)?;
    let canonical = state.canonical_bytes()?;
    if canonical != input {
        return Err(HouseholdStateError::NonCanonicalEncoding);
    }
    Ok(state)
}

pub fn classify_legacy_outbox_v1(
    source_digest: CanonicalDigestV1,
    source_key: &str,
    input: &[u8],
    frozen_at: &CanonicalTimestampV1,
) -> Result<(HouseholdOutboxId, LegacyProfileOutboxEntryV1), HouseholdStateError> {
    let object =
        parse_bounded_json_object_v1(input, CompatibilityJsonLimitsV1::MIGRATION_CANDIDATE)?;
    let payload = CanonicalJsonObjectV1::from_map(object.clone(), MAX_MIGRATION_CANDIDATE_BYTES)?;
    let payload_digest = payload.canonical_sha256();
    let (source_kind, target, updated_at) = if object.contains_key("member_id") {
        require_exact_keys(&object, &["member_id", "repair"])?;
        if !object.get("repair").is_some_and(Value::is_boolean) {
            return Err(HouseholdStateError::InvalidOutbox);
        }
        let member = object
            .get("member_id")
            .and_then(Value::as_str)
            .ok_or(HouseholdStateError::InvalidOutbox)?;
        (
            LegacyOutboxSourceKindV1::RustMutationKeyedEmbeddedMemberV0,
            parse_compatibility_subject(member)?,
            frozen_at.clone(),
        )
    } else if exact_keys(
        &object,
        &["fields", "local_context", "updated_at", "version"],
    ) && object.get("version").and_then(Value::as_u64) == Some(1)
    {
        validate_bounded_object(object.get("fields"))?;
        let local_context = required_object(object.get("local_context"))?;
        validate_profile_projection(local_context)?;
        let updated_at = object
            .get("updated_at")
            .and_then(Value::as_str)
            .ok_or(HouseholdStateError::InvalidOutbox)?;
        (
            LegacyOutboxSourceKindV1::PythonSubjectKeyedV1,
            parse_compatibility_subject(source_key)?,
            normalize_legacy_timestamp_v1(updated_at, frozen_at)?.normalized,
        )
    } else if exact_keys(&object, &["local_context"]) {
        validate_profile_projection(required_object(object.get("local_context"))?)?;
        (
            LegacyOutboxSourceKindV1::RustSubjectKeyedLocalContextV0,
            parse_compatibility_subject(source_key)?,
            frozen_at.clone(),
        )
    } else if exact_keys(&object, &["fields", "local_context"]) {
        validate_patch_fields(required_object(object.get("fields"))?)?;
        validate_profile_projection(required_object(object.get("local_context"))?)?;
        (
            LegacyOutboxSourceKindV1::PythonSubjectKeyedPatchV0,
            parse_compatibility_subject(source_key)?,
            frozen_at.clone(),
        )
    } else {
        return Err(HouseholdStateError::InvalidOutbox);
    };
    let outbox_id = if source_kind == LegacyOutboxSourceKindV1::RustMutationKeyedEmbeddedMemberV0 {
        HouseholdOutboxId::parse_legacy(source_key)?
    } else {
        HouseholdOutboxId::deterministic_legacy(
            source_kind,
            source_digest,
            source_key,
            payload_digest,
        )?
    };
    Ok((
        outbox_id,
        LegacyProfileOutboxEntryV1 {
            target,
            source_kind,
            source_key: source_key.to_owned(),
            source_digest,
            payload,
            payload_digest,
            phase: OutboxPhaseV1::PolicyBlockedLegacy,
            updated_at,
        },
    ))
}

fn validate_opaque_identity(
    value: &str,
    reject_owner_sync_prefix: bool,
) -> Result<(), HouseholdStateError> {
    if value.is_empty()
        || value.len() > 128
        || value.trim() != value
        || value.chars().any(forbidden_terminal_character)
        || value.contains(['/', '\\'])
        || matches!(
            value,
            "." | ".." | SELF_COMPATIBILITY_SENTINEL | EVERYONE_COMPATIBILITY_SENTINEL
        )
        || (reject_owner_sync_prefix && value.starts_with(OWNER_SYNC_OUTBOX_PREFIX))
    {
        return Err(HouseholdStateError::InvalidIdentity);
    }
    Ok(())
}

fn subject_kind(subject: &HouseholdSubjectId) -> &'static str {
    match subject {
        HouseholdSubjectId::Self_ => "self",
        HouseholdSubjectId::Member(_) => "member",
    }
}

fn scope_kind(scope: &HouseholdScope) -> &'static str {
    match scope {
        HouseholdScope::Subject(subject) => subject_kind(subject),
        HouseholdScope::Everyone => "everyone",
    }
}

fn validate_owner_sync_request_body(
    body: &Map<String, Value>,
    remote_base: Option<&RemoteProfileBaseV1>,
    local_profile_digest: CanonicalDigestV1,
) -> Result<(), HouseholdStateError> {
    let profile_data = body
        .get("profile_data")
        .and_then(Value::as_object)
        .ok_or(HouseholdStateError::InvalidOwnerSyncIntent)?;
    if body.get("member_id").and_then(Value::as_str) != Some(SELF_COMPATIBILITY_SENTINEL)
        || canonical_sha256_v1(&Value::Object(profile_data.clone()))? != local_profile_digest
    {
        return Err(HouseholdStateError::InvalidOwnerSyncIntent);
    }
    match remote_base {
        Some(RemoteProfileBaseV1 {
            existence: RemoteProfileExistenceV1::Absent,
            ..
        }) if exact_keys(body, &["member_id", "profile_data"]) => Ok(()),
        Some(RemoteProfileBaseV1 {
            existence: RemoteProfileExistenceV1::Present,
            version: Some(version),
            ..
        }) if exact_keys(body, &["expected_version", "member_id", "profile_data"])
            && body.get("expected_version").and_then(Value::as_u64) == Some(*version) =>
        {
            Ok(())
        }
        _ => Err(HouseholdStateError::InvalidOwnerSyncIntent),
    }
}

fn forbidden_terminal_character(value: char) -> bool {
    value.is_control() || value == '\u{1b}' || matches!(value as u32, 0x80..=0x9f)
}

fn is_valid_metadata_label(value: &str, maximum_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum_bytes
        && value.trim() == value
        && !value.chars().any(forbidden_terminal_character)
}

fn is_deterministic_outbox_id(value: &str) -> bool {
    [
        ("legacy-py-v1-", 77_usize),
        ("legacy-rust-subject-v0-", 87),
        ("legacy-py-patch-v0-", 83),
    ]
    .into_iter()
    .any(|(prefix, length)| {
        value.len() == length
            && value.strip_prefix(prefix).is_some_and(|digest| {
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
    })
}

fn require_uuid_v4(value: Uuid) -> Result<(), HouseholdStateError> {
    if value.get_version() == Some(Version::Random)
        && value.get_variant() == Variant::RFC4122
        && value.hyphenated().to_string() == value.to_string()
    {
        Ok(())
    } else {
        Err(HouseholdStateError::InvalidIdentity)
    }
}

fn parse_date(value: &str) -> Result<Date, HouseholdStateError> {
    if value.len() != 10
        || value.as_bytes().get(4) != Some(&b'-')
        || value.as_bytes().get(7) != Some(&b'-')
        || value
            .bytes()
            .enumerate()
            .any(|(index, byte)| !matches!(index, 4 | 7) && !byte.is_ascii_digit())
    {
        return Err(HouseholdStateError::InvalidDate);
    }
    let year = value[0..4]
        .parse::<i32>()
        .map_err(|_| HouseholdStateError::InvalidDate)?;
    let month = value[5..7]
        .parse::<u8>()
        .ok()
        .and_then(|value| Month::try_from(value).ok())
        .ok_or(HouseholdStateError::InvalidDate)?;
    let day = value[8..10]
        .parse::<u8>()
        .map_err(|_| HouseholdStateError::InvalidDate)?;
    Date::from_calendar_date(year, month, day).map_err(|_| HouseholdStateError::InvalidDate)
}

fn age_on(date_of_birth: Date, evaluated_on: Date) -> Result<u16, HouseholdStateError> {
    if date_of_birth > evaluated_on {
        return Err(HouseholdStateError::InvalidDate);
    }
    let birthday_passed =
        (evaluated_on.month(), evaluated_on.day()) >= (date_of_birth.month(), date_of_birth.day());
    let years = evaluated_on.year() - date_of_birth.year() - i32::from(!birthday_passed);
    if !(0..=130).contains(&years) {
        return Err(HouseholdStateError::InvalidDate);
    }
    u16::try_from(years).map_err(|_| HouseholdStateError::InvalidDate)
}

const KNOWN_PROJECTION_KEYS: &[&str] = &[
    "preferences",
    "restrictions",
    "avoid_ingredients",
    "medical_constraints",
    "cuisine_preferences",
    "health_condition_ids",
    "custom_health_conditions",
    "custom_diet_styles",
    "custom_restrictions",
    "custom_cuisines",
    "diet_style_ids",
    "allergy_ids",
    "additional_restriction_ids",
    "additional_medical_constraints",
    "preference_strictness",
    "restriction_handling",
    "condition_severity_levels",
    "notes",
    "medical_condition_id",
    "activity_level",
    "severity_level",
    "selection_provenance_version",
];

fn compatibility_value_is_semantically_empty(value: &Value) -> bool {
    value.is_null()
        || value.as_array().is_some_and(Vec::is_empty)
        || value.as_object().is_some_and(Map::is_empty)
        || value.as_str().is_some_and(str::is_empty)
}

fn promoted_profile_projection(object: &Map<String, Value>) -> Option<Map<String, Value>> {
    let promoted = object
        .iter()
        .filter(|(key, _)| KNOWN_PROJECTION_KEYS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Map<_, _>>();
    if promoted.iter().all(|(key, value)| {
        key == "selection_provenance_version" || compatibility_value_is_semantically_empty(value)
    }) {
        None
    } else {
        Some(promoted)
    }
}

fn validate_profile_projection(object: &Map<String, Value>) -> Result<(), HouseholdStateError> {
    for (key, value) in object {
        if !KNOWN_PROJECTION_KEYS.contains(&key.as_str()) {
            continue;
        }
        match key.as_str() {
            "preferences"
            | "restrictions"
            | "medical_constraints"
            | "cuisine_preferences"
            | "health_condition_ids"
            | "diet_style_ids"
            | "allergy_ids"
            | "additional_restriction_ids"
            | "additional_medical_constraints" => validate_string_array(value, 256, 2_048)?,
            "avoid_ingredients" => validate_string_array(value, 20, 40)?,
            "custom_health_conditions" | "custom_restrictions" => {
                validate_custom_string_array(value, 10, 60)?;
            }
            "custom_diet_styles" | "custom_cuisines" => {
                validate_custom_string_array(value, 10, 40)?;
            }
            "preference_strictness" | "restriction_handling" => {
                validate_string_map(value)?;
            }
            "condition_severity_levels" => validate_integer_map(value, 1, 5)?,
            "notes" => validate_nullable_string(value, 280)?,
            "medical_condition_id" | "activity_level" => {
                validate_nullable_string(value, 2_048)?;
            }
            "severity_level" => validate_nullable_integer(value, 1, 5)?,
            "selection_provenance_version" => {
                if value.as_i64().is_none() {
                    return Err(HouseholdStateError::InvalidProfileDocument);
                }
            }
            _ => unreachable!(),
        }
    }
    let canonical = canonicalize_json_value_v1(&Value::Object(object.clone()))?;
    if canonical.len() > MAX_PROFILE_DOCUMENT_BYTES {
        return Err(HouseholdStateError::InvalidProfileDocument);
    }
    Ok(())
}

fn validate_string_array(
    value: &Value,
    maximum_entries: usize,
    maximum_scalars: usize,
) -> Result<(), HouseholdStateError> {
    let values = value
        .as_array()
        .ok_or(HouseholdStateError::InvalidProfileDocument)?;
    if values.len() > maximum_entries {
        return Err(HouseholdStateError::InvalidProfileDocument);
    }
    for value in values {
        let value = value
            .as_str()
            .ok_or(HouseholdStateError::InvalidProfileDocument)?;
        if value.len() > 2_048
            || value.chars().count() > maximum_scalars
            || value.chars().any(char::is_control)
        {
            return Err(HouseholdStateError::InvalidProfileDocument);
        }
    }
    Ok(())
}

fn validate_custom_string_array(
    value: &Value,
    maximum_entries: usize,
    maximum_scalars: usize,
) -> Result<(), HouseholdStateError> {
    validate_string_array(value, maximum_entries, maximum_scalars)?;
    if value.as_array().is_none_or(|values| {
        values
            .iter()
            .any(|value| value.as_str().is_none_or(|value| value.trim().is_empty()))
    }) {
        return Err(HouseholdStateError::InvalidProfileDocument);
    }
    Ok(())
}

fn validate_string_map(value: &Value) -> Result<(), HouseholdStateError> {
    let values = value
        .as_object()
        .ok_or(HouseholdStateError::InvalidProfileDocument)?;
    if values.len() > MAX_COMPATIBILITY_OBJECT_KEYS
        || values.iter().any(|(key, value)| {
            key.len() > 2_048 || value.as_str().is_none_or(|value| value.len() > 2_048)
        })
    {
        return Err(HouseholdStateError::InvalidProfileDocument);
    }
    Ok(())
}

fn validate_integer_map(
    value: &Value,
    minimum: i64,
    maximum: i64,
) -> Result<(), HouseholdStateError> {
    let values = value
        .as_object()
        .ok_or(HouseholdStateError::InvalidProfileDocument)?;
    if values.len() > MAX_COMPATIBILITY_OBJECT_KEYS
        || values.iter().any(|(key, value)| {
            key.len() > 2_048
                || value
                    .as_i64()
                    .is_none_or(|value| !(minimum..=maximum).contains(&value))
        })
    {
        return Err(HouseholdStateError::InvalidProfileDocument);
    }
    Ok(())
}

fn validate_nullable_string(
    value: &Value,
    maximum_scalars: usize,
) -> Result<(), HouseholdStateError> {
    if value.is_null() {
        return Ok(());
    }
    let value = value
        .as_str()
        .ok_or(HouseholdStateError::InvalidProfileDocument)?;
    if value.len() > 2_048
        || value.chars().count() > maximum_scalars
        || value.chars().any(char::is_control)
    {
        return Err(HouseholdStateError::InvalidProfileDocument);
    }
    Ok(())
}

fn validate_nullable_integer(
    value: &Value,
    minimum: i64,
    maximum: i64,
) -> Result<(), HouseholdStateError> {
    if value.is_null() {
        return Ok(());
    }
    if value
        .as_i64()
        .is_none_or(|value| !(minimum..=maximum).contains(&value))
    {
        return Err(HouseholdStateError::InvalidProfileDocument);
    }
    Ok(())
}

fn validate_bounded_object(value: Option<&Value>) -> Result<(), HouseholdStateError> {
    let object = required_object(value)?;
    let canonical = canonicalize_json_value_v1(&Value::Object(object.clone()))?;
    if object.len() > MAX_COMPATIBILITY_OBJECT_KEYS || canonical.len() > MAX_PROFILE_DOCUMENT_BYTES
    {
        return Err(HouseholdStateError::InvalidOutbox);
    }
    Ok(())
}

fn validate_patch_fields(object: &Map<String, Value>) -> Result<(), HouseholdStateError> {
    const PATCH_KEYS: &[&str] = &[
        "restrictions",
        "preferences",
        "avoid_ingredients",
        "medical_condition_id",
    ];
    if object.keys().any(|key| !PATCH_KEYS.contains(&key.as_str())) {
        return Err(HouseholdStateError::InvalidOutbox);
    }
    for (key, value) in object {
        match key.as_str() {
            "restrictions" | "preferences" => validate_string_array(value, 256, 2_048)?,
            "avoid_ingredients" => validate_string_array(value, 20, 40)?,
            "medical_condition_id" => validate_nullable_string(value, 2_048)?,
            _ => unreachable!(),
        }
    }
    Ok(())
}

fn required_object(value: Option<&Value>) -> Result<&Map<String, Value>, HouseholdStateError> {
    value
        .and_then(Value::as_object)
        .ok_or(HouseholdStateError::InvalidOutbox)
}

fn exact_keys(object: &Map<String, Value>, keys: &[&str]) -> bool {
    object.len() == keys.len() && keys.iter().all(|key| object.contains_key(*key))
}

fn require_exact_keys(
    object: &Map<String, Value>,
    keys: &[&str],
) -> Result<(), HouseholdStateError> {
    exact_keys(object, keys)
        .then_some(())
        .ok_or(HouseholdStateError::InvalidOutbox)
}

fn parse_compatibility_subject(value: &str) -> Result<HouseholdSubjectId, HouseholdStateError> {
    if value == SELF_COMPATIBILITY_SENTINEL {
        Ok(HouseholdSubjectId::self_())
    } else {
        MemberId::parse_preserved(value).map(HouseholdSubjectId::member)
    }
}

fn validate_sorted_unique_by<T>(
    values: &[T],
    compare: impl Fn(&T, &T) -> Ordering,
) -> Result<(), HouseholdStateError> {
    for pair in values.windows(2) {
        match compare(&pair[0], &pair[1]) {
            Ordering::Less => {}
            Ordering::Equal => return Err(HouseholdStateError::DuplicateIdentity),
            Ordering::Greater => return Err(HouseholdStateError::UnsortedCollection),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn timestamp() -> CanonicalTimestampV1 {
        CanonicalTimestampV1::parse("2026-07-30T12:00:00.000Z").unwrap()
    }

    fn minimal_state() -> HouseholdStateV1 {
        let golden: Value = serde_json::from_str(include_str!(
            "../../../schemas/v1/household-canonical-v1.golden.json"
        ))
        .unwrap();
        decode_canonical_household_state_v1(
            golden["state"]["canonical_utf8"]
                .as_str()
                .unwrap()
                .as_bytes(),
        )
        .unwrap()
    }

    fn member(id: String) -> HouseholdMemberV1 {
        HouseholdMemberV1 {
            member_id: MemberId::parse_preserved(id).unwrap(),
            display_name: DisplayName::parse("Private name").unwrap(),
            relationship: RelationshipV1::Other,
            relationship_source: RelationshipSourceV1::NativeDeclared,
            minor_status: MinorStatusV1::Unknown,
            age_evidence: None,
            minor_status_evaluated_on: CanonicalDateV1::parse("2026-07-30").unwrap(),
            lifecycle: HouseholdLifecycleV1::Active,
            profile_state: HouseholdProfileStateV1::Incomplete,
            created_at: timestamp(),
            updated_at: timestamp(),
        }
    }

    fn usable_profile(subject: HouseholdSubjectId, restriction: &str) -> HouseholdProfileRecordV1 {
        HouseholdProfileRecordV1 {
            subject,
            profile_revision: ProfileRevision::new(1).unwrap(),
            document: HouseholdProfileDocumentV1::legacy_projection(
                serde_json::to_string(&json!({"restrictions":[restriction]}))
                    .unwrap()
                    .as_bytes(),
            )
            .unwrap(),
        }
    }

    fn legacy_context_record(
        source_key: &str,
        source_marker: u8,
        restriction: &str,
    ) -> HouseholdOutboxRecordV1 {
        let input = serde_json::to_string(&json!({"local_context":{"restrictions":[restriction]}}))
            .unwrap();
        let (outbox_id, legacy) = classify_legacy_outbox_v1(
            CanonicalDigestV1::from_bytes([source_marker; 32]),
            source_key,
            input.as_bytes(),
            &timestamp(),
        )
        .unwrap();
        HouseholdOutboxRecordV1 {
            outbox_id,
            outbox_revision: OutboxRevision::new(1).unwrap(),
            entry: HouseholdProfileOutboxEntryV1::Legacy {
                version: 1,
                target: legacy.target.clone(),
                legacy,
            },
        }
    }

    fn legacy_mutation_record(index: usize) -> HouseholdOutboxRecordV1 {
        let payload = CanonicalJsonObjectV1::parse(
            br#"{"member_id":"_self","repair":true}"#,
            CompatibilityJsonLimitsV1::PROFILE_DOCUMENT,
        )
        .unwrap();
        let source_key = format!("mutation-{index:04}");
        HouseholdOutboxRecordV1 {
            outbox_id: HouseholdOutboxId::parse_legacy(source_key.clone()).unwrap(),
            outbox_revision: OutboxRevision::new(1).unwrap(),
            entry: HouseholdProfileOutboxEntryV1::Legacy {
                version: 1,
                target: HouseholdSubjectId::self_(),
                legacy: LegacyProfileOutboxEntryV1 {
                    target: HouseholdSubjectId::self_(),
                    source_kind: LegacyOutboxSourceKindV1::RustMutationKeyedEmbeddedMemberV0,
                    source_key,
                    source_digest: CanonicalDigestV1::from_bytes([1; 32]),
                    payload_digest: payload.canonical_sha256(),
                    payload,
                    phase: OutboxPhaseV1::PolicyBlockedLegacy,
                    updated_at: timestamp(),
                },
            },
        }
    }

    #[test]
    fn generated_member_ids_are_canonical_uuid_v4() {
        let member = MemberId::new();
        assert_eq!(member.as_str().len(), 36);
        let uuid = Uuid::parse_str(member.as_str()).unwrap();
        assert_eq!(uuid.get_version(), Some(Version::Random));
        assert_eq!(uuid.hyphenated().to_string(), member.as_str());
        assert!(member.is_native_uuid_v4());
        assert_eq!(
            MemberId::from_native_uuid_v4(uuid).unwrap().as_str(),
            member.as_str()
        );
        assert!(
            MemberId::from_native_uuid_v4(
                Uuid::parse_str("aaaaaaaa-aaaa-1aaa-8aaa-aaaaaaaaaaaa").unwrap()
            )
            .is_err()
        );
        assert!(
            !MemberId::parse_preserved("AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA")
                .unwrap()
                .is_native_uuid_v4()
        );
    }

    #[test]
    fn legacy_python_snapshot_provenance_is_an_exact_present_source_pair() {
        let snapshot = LegacyPythonSnapshotProvenanceV1 {
            locator_digest: CanonicalDigestV1::from_bytes([8; 32]),
            content_digest: CanonicalDigestV1::from_bytes([9; 32]),
        };
        let mut state = minimal_state();
        state.migration_provenance.legacy_python_snapshot = Some(snapshot.clone());
        assert_eq!(
            state.validate(),
            Err(HouseholdStateError::InvalidMigrationProvenance)
        );

        state.migration_provenance.source_identity = LegacySourceIdentityV1::Present {
            source_kind: "legacy_python_source_bundle_v1".to_owned(),
            source_digest: CanonicalDigestV1::from_bytes([7; 32]),
        };
        state.validate().unwrap();
        let canonical = state.canonical_bytes().unwrap();
        let decoded = decode_canonical_household_state_v1(&canonical).unwrap();
        assert_eq!(
            decoded.migration_provenance.legacy_python_snapshot,
            Some(snapshot)
        );
    }

    #[test]
    fn legacy_identity_and_name_boundaries_preserve_exact_bytes() {
        let exact = "é".repeat(64);
        assert_eq!(exact.len(), 128);
        assert_eq!(
            MemberId::parse_preserved(exact.clone()).unwrap().as_str(),
            exact
        );
        assert!(MemberId::parse_preserved("a".repeat(129)).is_err());
        for invalid in [
            "",
            " member",
            "member ",
            "_self",
            "__everyone__",
            ".",
            "..",
            "a/b",
            "a\\b",
            "\u{1b}[31m",
        ] {
            assert!(MemberId::parse_preserved(invalid).is_err(), "{invalid:?}");
        }
        assert!(HouseholdOutboxId::parse_legacy("owner-sync-v1:anything").is_err());

        assert!(DisplayName::parse("a".repeat(80)).is_ok());
        assert!(DisplayName::parse("a".repeat(81)).is_err());
        assert!(DisplayName::parse("é".repeat(80)).is_ok());
        assert!(DisplayName::parse("é".repeat(161)).is_err());
        assert!(DisplayName::parse("😀".repeat(80)).is_ok());
        assert_eq!("😀".repeat(80).len(), 320);
        assert!(DisplayName::parse("😀".repeat(81)).is_err());
        assert!(HouseholdOutboxId::parse_legacy("x").is_ok());
        assert!(HouseholdOutboxId::parse_legacy("x".repeat(128)).is_ok());
        assert!(HouseholdOutboxId::parse_legacy("x".repeat(129)).is_err());
    }

    #[test]
    fn typed_subjects_never_treat_compatibility_sentinels_as_members() {
        assert_eq!(
            serde_json::to_value(HouseholdSubjectId::self_()).unwrap(),
            json!("self")
        );
        let member = HouseholdSubjectId::member(MemberId::parse_preserved("member-1").unwrap());
        assert_eq!(
            serde_json::to_value(member).unwrap(),
            json!({"member":"member-1"})
        );
        assert_eq!(
            serde_json::to_value(HouseholdScope::Everyone).unwrap(),
            json!("everyone")
        );
        assert_eq!(
            serde_json::to_value(RelationshipV1::Self_).unwrap(),
            json!("self")
        );
        assert_eq!(
            serde_json::to_value(AgeBandV1::Under13).unwrap(),
            json!("under_13")
        );
        assert_eq!(
            serde_json::to_value(AgeBandV1::Age13_17).unwrap(),
            json!("age_13_17")
        );
        assert_eq!(
            serde_json::to_value(AgeBandV1::Age18Plus).unwrap(),
            json!("age_18_plus")
        );

        let golden: Value = serde_json::from_str(include_str!(
            "../../../schemas/v1/household-canonical-v1.golden.json"
        ))
        .unwrap();
        let wire = &golden["wire_enums"];
        assert_eq!(
            serde_json::to_value([
                RelationshipV1::Self_,
                RelationshipV1::Spouse,
                RelationshipV1::Partner,
                RelationshipV1::Parent,
                RelationshipV1::Child,
                RelationshipV1::Sibling,
                RelationshipV1::Grandparent,
                RelationshipV1::Friend,
                RelationshipV1::Other,
            ])
            .unwrap(),
            wire["relationship"]
        );
        assert_eq!(
            serde_json::to_value([
                RelationshipSourceV1::NativeDeclared,
                RelationshipSourceV1::LegacyDeclared,
                RelationshipSourceV1::LegacyMissing,
            ])
            .unwrap(),
            wire["relationship_source"]
        );
        assert_eq!(
            serde_json::to_value([
                MinorStatusV1::Minor,
                MinorStatusV1::Adult,
                MinorStatusV1::Unknown,
            ])
            .unwrap(),
            wire["minor_status"]
        );
        assert_eq!(
            serde_json::to_value([
                AgeBandV1::Under13,
                AgeBandV1::Age13_17,
                AgeBandV1::Age18Plus,
            ])
            .unwrap(),
            wire["age_band"]
        );
        assert_eq!(
            serde_json::to_value([
                AgeEvidenceSourceV1::NativeDeclared,
                AgeEvidenceSourceV1::LegacyDateOfBirth,
                AgeEvidenceSourceV1::LegacyAgeBand,
            ])
            .unwrap(),
            wire["age_evidence_source"]
        );
        assert_eq!(
            serde_json::to_value([HouseholdLifecycleV1::Active, HouseholdLifecycleV1::Archived,])
                .unwrap(),
            wire["lifecycle"]
        );
        assert_eq!(
            serde_json::to_value([
                HouseholdProfileStateV1::Incomplete,
                HouseholdProfileStateV1::LocalOnly,
                HouseholdProfileStateV1::PendingSync,
                HouseholdProfileStateV1::Synced,
                HouseholdProfileStateV1::Conflicted,
            ])
            .unwrap(),
            wire["profile_state"]
        );
        assert_eq!(
            serde_json::to_value([
                ProfileDocumentProvenanceV1::NativeDeclared,
                ProfileDocumentProvenanceV1::LegacyLocalProjection,
            ])
            .unwrap(),
            wire["profile_document_provenance"]
        );
        assert_eq!(
            serde_json::to_value([
                LegacyOutboxSourceKindV1::PythonSubjectKeyedV1,
                LegacyOutboxSourceKindV1::RustMutationKeyedEmbeddedMemberV0,
                LegacyOutboxSourceKindV1::RustSubjectKeyedLocalContextV0,
                LegacyOutboxSourceKindV1::PythonSubjectKeyedPatchV0,
            ])
            .unwrap(),
            wire["legacy_outbox_source_kind"]
        );
        assert_eq!(
            serde_json::to_value([OutboxPhaseV1::PolicyBlockedLegacy]).unwrap(),
            wire["legacy_outbox_phase"]
        );
        assert_eq!(
            serde_json::to_value([
                OwnerSyncIntentPhaseV1::NeedsConsentCheck,
                OwnerSyncIntentPhaseV1::NeedsRemoteBase,
                OwnerSyncIntentPhaseV1::ReadyToDispatch,
                OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown,
                OwnerSyncIntentPhaseV1::OutcomeUncertain,
                OwnerSyncIntentPhaseV1::DefiniteFailure,
                OwnerSyncIntentPhaseV1::Conflicted,
                OwnerSyncIntentPhaseV1::LocalOnlyNoConsent,
            ])
            .unwrap(),
            wire["owner_sync_phase"]
        );
        assert_eq!(
            serde_json::to_value([
                LastDefiniteOwnerSyncErrorV1::ConsentAbsent,
                LastDefiniteOwnerSyncErrorV1::Unauthorized,
                LastDefiniteOwnerSyncErrorV1::Forbidden,
                LastDefiniteOwnerSyncErrorV1::Validation,
                LastDefiniteOwnerSyncErrorV1::VersionConflict,
                LastDefiniteOwnerSyncErrorV1::NotFound,
                LastDefiniteOwnerSyncErrorV1::PredispatchCancelled,
                LastDefiniteOwnerSyncErrorV1::ConsentVersionChangedRequiresNewSave,
                LastDefiniteOwnerSyncErrorV1::ConsentRevokedRegrantRequired,
            ])
            .unwrap(),
            wire["owner_sync_error"]
        );
        assert_eq!(
            serde_json::to_value([
                RemoteProfileExistenceV1::Absent,
                RemoteProfileExistenceV1::Present,
            ])
            .unwrap(),
            wire["remote_profile_existence"]
        );
        assert_eq!(
            serde_json::to_value([
                AppliedCommitOutcomeV1::Initialized,
                AppliedCommitOutcomeV1::Committed,
            ])
            .unwrap(),
            wire["applied_commit_outcome"]
        );
        assert_eq!(
            serde_json::to_value([
                MigrationDispositionKindV1::Migrated,
                MigrationDispositionKindV1::Retired,
                MigrationDispositionKindV1::ReauthenticationRequired,
            ])
            .unwrap(),
            wire["migration_disposition"]
        );
    }

    #[test]
    fn minor_policy_is_deterministic_and_conflict_favors_minor() {
        let evaluated = CanonicalDateV1::parse("2026-07-30").unwrap();
        let adult = AgeEvidenceV1 {
            date_of_birth: Some(
                DateOfBirthV1::parse_for_evaluation("1980-01-01", &evaluated).unwrap(),
            ),
            age_band: Some(AgeBandV1::Age18Plus),
            source: AgeEvidenceSourceV1::NativeDeclared,
        };
        assert_eq!(
            derive_minor_status_v1(RelationshipV1::Other, Some(&adult), &evaluated).unwrap(),
            MinorStatusV1::Adult
        );
        assert_eq!(
            derive_minor_status_v1(RelationshipV1::Child, Some(&adult), &evaluated).unwrap(),
            MinorStatusV1::Minor
        );
        assert_eq!(
            derive_minor_status_v1(RelationshipV1::Friend, None, &evaluated).unwrap(),
            MinorStatusV1::Unknown
        );
        assert!(DateOfBirthV1::parse_for_evaluation("1890-01-01", &evaluated).is_err());
        assert!(DateOfBirthV1::parse_for_evaluation("2026-08-01", &evaluated).is_err());

        let mut invalid_provenance = member("relationship-source".into());
        invalid_provenance.relationship = RelationshipV1::Friend;
        invalid_provenance.relationship_source = RelationshipSourceV1::LegacyMissing;
        assert_eq!(
            invalid_provenance.validate(),
            Err(HouseholdStateError::InvalidRelationship)
        );
    }

    #[test]
    fn legacy_profile_round_trips_unknown_extensions_with_equal_digest() {
        let raw =
            br#"{"restrictions":["glutenFree"],"unknown":{"n":4.50,"safe":9007199254740991}}"#;
        let document = HouseholdProfileDocumentV1::legacy_projection(raw).unwrap();
        document.validate().unwrap();
        let encoded = document
            .legacy_source_document
            .as_ref()
            .unwrap()
            .canonical_bytes()
            .unwrap();
        assert_eq!(
            String::from_utf8(encoded).unwrap(),
            r#"{"restrictions":["glutenFree"],"unknown":{"n":4.5,"safe":9007199254740991}}"#
        );
        assert_eq!(
            document.legacy_source_digest,
            document
                .legacy_source_document
                .as_ref()
                .map(CanonicalJsonObjectV1::canonical_sha256)
        );
        assert!(
            HouseholdProfileDocumentV1::legacy_projection(
                br#"{"selection_provenance_version":9007199254740992}"#
            )
            .is_err()
        );
        let unknown_only =
            HouseholdProfileDocumentV1::legacy_projection(br#"{"unknown":"encrypted-only"}"#)
                .unwrap();
        assert_eq!(unknown_only.effective_profile().unwrap(), None);
    }

    #[test]
    fn deterministic_outbox_ids_have_frozen_lengths_and_disjoint_domains() {
        let golden: Value = serde_json::from_str(include_str!(
            "../../../schemas/v1/household-canonical-v1.golden.json"
        ))
        .unwrap();
        let mut ids = Vec::new();
        for vector in golden["deterministic_outbox_ids"].as_array().unwrap() {
            let source_kind: LegacyOutboxSourceKindV1 =
                serde_json::from_value(vector["source_kind"].clone()).unwrap();
            let source_digest = CanonicalDigestV1::from_bytes(
                crate::decode_lower_hex_32(vector["source_digest"].as_str().unwrap()).unwrap(),
            );
            let entry_digest = CanonicalDigestV1::from_bytes(
                crate::decode_lower_hex_32(vector["entry_digest"].as_str().unwrap()).unwrap(),
            );
            let id = HouseholdOutboxId::deterministic_legacy(
                source_kind,
                source_digest,
                vector["source_key"].as_str().unwrap(),
                entry_digest,
            )
            .unwrap();
            assert_eq!(id.as_str(), vector["outbox_id"].as_str().unwrap());
            assert_eq!(
                id.as_str().len(),
                usize::try_from(vector["length"].as_u64().unwrap()).unwrap()
            );
            let preimage = json!({
                "contract": match source_kind {
                    LegacyOutboxSourceKindV1::PythonSubjectKeyedV1 =>
                        "heyfood.household.legacy-outbox-id.python-subject-v1",
                    LegacyOutboxSourceKindV1::RustSubjectKeyedLocalContextV0 =>
                        "heyfood.household.legacy-outbox-id.rust-subject-v0",
                    LegacyOutboxSourceKindV1::PythonSubjectKeyedPatchV0 =>
                        "heyfood.household.legacy-outbox-id.python-patch-v0",
                    LegacyOutboxSourceKindV1::RustMutationKeyedEmbeddedMemberV0 => unreachable!(),
                },
                "source_digest": source_digest.to_lower_hex(),
                "source_kind": source_kind,
                "source_key": vector["source_key"].as_str().unwrap(),
                "entry_digest": entry_digest.to_lower_hex(),
            });
            assert_eq!(
                String::from_utf8(canonicalize_json_value_v1(&preimage).unwrap()).unwrap(),
                vector["preimage"].as_str().unwrap()
            );
            ids.push(id);
        }
        assert_eq!(ids.len(), 3);
        assert_ne!(ids[0], ids[1]);
        assert_ne!(ids[0], ids[2]);
    }

    #[test]
    fn all_four_outbox_classifiers_are_disjoint() {
        let golden: Value = serde_json::from_str(include_str!(
            "../../../schemas/v1/household-canonical-v1.golden.json"
        ))
        .unwrap();
        let frozen = timestamp();
        for vector in golden["legacy_outbox_classifier_vectors"]
            .as_array()
            .unwrap()
        {
            let source = CanonicalDigestV1::from_bytes(
                crate::decode_lower_hex_32(vector["source_digest"].as_str().unwrap()).unwrap(),
            );
            let (outbox_id, entry) = classify_legacy_outbox_v1(
                source,
                vector["source_key"].as_str().unwrap(),
                vector["source"].as_str().unwrap().as_bytes(),
                &frozen,
            )
            .unwrap();
            let expected_kind: LegacyOutboxSourceKindV1 =
                serde_json::from_value(vector["source_kind"].clone()).unwrap();
            assert_eq!(entry.source_kind, expected_kind);
            assert_eq!(
                serde_json::to_value(entry.target).unwrap(),
                vector["target"]
            );
            assert_eq!(
                entry.payload_digest.to_lower_hex(),
                vector["entry_digest"].as_str().unwrap()
            );
            assert_eq!(outbox_id.as_str(), vector["outbox_id"].as_str().unwrap());
        }

        let source = CanonicalDigestV1::from_bytes([3; 32]);
        assert!(
            classify_legacy_outbox_v1(
                source,
                "member-1",
                br#"{"local_context":{},"extra":true}"#,
                &frozen
            )
            .is_err()
        );
    }

    #[test]
    fn legacy_outbox_record_binds_shape_target_and_deterministic_identity() {
        let source = CanonicalDigestV1::from_bytes([3; 32]);
        let (outbox_id, legacy) = classify_legacy_outbox_v1(
            source,
            "_self",
            br#"{"version":1,"fields":{},"local_context":{"restrictions":["x"]},"updated_at":"2026-07-30T11:59:00Z"}"#,
            &timestamp(),
        )
        .unwrap();
        let expected_id = outbox_id.clone();
        let mut state = minimal_state();
        state.owner.profile_state = HouseholdProfileStateV1::LocalOnly;
        state.profiles = vec![usable_profile(HouseholdSubjectId::self_(), "x")];
        state.outbox = vec![HouseholdOutboxRecordV1 {
            outbox_id,
            outbox_revision: OutboxRevision::new(1).unwrap(),
            entry: HouseholdProfileOutboxEntryV1::Legacy {
                version: 1,
                target: HouseholdSubjectId::self_(),
                legacy,
            },
        }];
        state.validate().unwrap();
        let raw = state.canonical_bytes().unwrap();
        assert_eq!(decode_canonical_household_state_v1(&raw).unwrap(), state);

        state.outbox[0].outbox_id = HouseholdOutboxId::parse_legacy("forged-id").unwrap();
        assert_eq!(state.validate(), Err(HouseholdStateError::InvalidOutbox));
        state.outbox[0].outbox_id = expected_id;

        let HouseholdProfileOutboxEntryV1::Legacy { legacy, .. } = &mut state.outbox[0].entry
        else {
            unreachable!()
        };
        legacy.target =
            HouseholdSubjectId::member(MemberId::parse_preserved("member-forged").unwrap());
        assert_eq!(state.validate(), Err(HouseholdStateError::InvalidOutbox));
    }

    #[test]
    fn consent_version_rejects_non_integer_and_out_of_range_json() {
        for invalid in ["true", "1.0", "\"1\"", "0", "-1", "2147483648"] {
            assert!(
                serde_json::from_str::<ConsentVersionV1>(invalid).is_err(),
                "{invalid}"
            );
        }
        assert_eq!(
            serde_json::from_str::<ConsentVersionV1>("2147483647")
                .unwrap()
                .get(),
            ConsentVersionV1::MAXIMUM
        );
    }

    #[test]
    fn legacy_timestamp_normalization_is_frozen_and_never_rounds() {
        let frozen = timestamp();
        let normalized =
            normalize_legacy_timestamp_v1("2026-07-30T11:59:59.123999+00:00", &frozen).unwrap();
        assert_eq!(normalized.normalized.as_str(), "2026-07-30T11:59:59.123Z");
        assert_eq!(normalized.source_precision, 6);
        assert!(normalized.truncated);
        assert!(normalize_legacy_timestamp_v1("2026-07-30T12:00:01Z", &frozen).is_err());
        assert!(normalize_legacy_timestamp_v1("2026-07-30T11:00:00-01:00", &frozen).is_err());
    }

    #[test]
    fn initial_migration_accepts_historical_household_time_but_rejects_future_time() {
        let historical = CanonicalTimestampV1::parse("2026-07-30T11:59:59.123Z").unwrap();
        let mut state = minimal_state();
        state.updated_at = historical;
        state.validate().unwrap();

        let future = CanonicalTimestampV1::parse("2026-07-30T12:00:00.001Z").unwrap();
        state.updated_at = future;
        assert_eq!(state.validate(), Err(HouseholdStateError::InvalidTimestamp));
    }

    #[test]
    fn post_initialization_revision_cannot_move_before_frozen_time() {
        let historical = CanonicalTimestampV1::parse("2026-07-30T11:59:59.999Z").unwrap();
        let mut state = minimal_state();
        state.revision = HouseholdRevision::new(2).unwrap();
        state.updated_at = historical;
        assert_eq!(state.validate(), Err(HouseholdStateError::InvalidTimestamp));

        let later = CanonicalTimestampV1::parse("2026-07-30T12:00:00.001Z").unwrap();
        state.updated_at = later;
        state.validate().unwrap();
    }

    #[test]
    fn compatibility_archive_preserves_scalar_and_remote_time_provenance() {
        let mut state = minimal_state();
        let scalar = CanonicalJsonValueV1::parse(
            br#""2026-07-30T11:00:00Z""#,
            CompatibilityJsonLimitsV1::PROFILE_DOCUMENT,
        )
        .unwrap();
        state.imported_compatibility.fields = vec![ImportedCompatibilityFieldV1 {
            field_name: "welcomed_at".into(),
            source_digest: scalar.canonical_sha256(),
            value: scalar,
        }];
        state
            .imported_compatibility
            .legacy_remote_profile_references = vec![LegacyRemoteProfileReferenceV1 {
            subject: HouseholdSubjectId::self_(),
            source_digest: CanonicalDigestV1::from_bytes([9; 32]),
        }];
        state.imported_compatibility.legacy_timestamp_provenance = vec![LegacyTimestampRecordV1 {
            field_path: "owner.created_at".into(),
            disposition: LegacyTimestampDispositionV1::LegacyMissingTime {
                normalized: timestamp(),
            },
        }];
        state.validate().unwrap();

        state.owner.profile_state = HouseholdProfileStateV1::LocalOnly;
        assert_eq!(
            state.validate(),
            Err(HouseholdStateError::InvalidProfileDocument)
        );
        state.profiles = vec![usable_profile(
            HouseholdSubjectId::self_(),
            "local-profile-with-remote-reference-provenance",
        )];
        state.validate().unwrap();
    }

    #[test]
    fn canonical_state_decode_rejects_noncanonical_and_duplicate_input() {
        let state = minimal_state();
        let canonical = state.canonical_bytes().unwrap();
        assert_eq!(
            decode_canonical_household_state_v1(&canonical).unwrap(),
            state
        );
        let mut whitespace = canonical.clone();
        whitespace.push(b'\n');
        assert_eq!(
            decode_canonical_household_state_v1(&whitespace),
            Err(HouseholdStateError::NonCanonicalEncoding)
        );
        let text = String::from_utf8(canonical).unwrap();
        let duplicate = format!("{{\"schema_version\":1,{}", &text[1..]);
        assert!(matches!(
            decode_canonical_household_state_v1(duplicate.as_bytes()),
            Err(HouseholdStateError::CanonicalJson(
                CanonicalJsonError::DuplicateObjectName
            ))
        ));
    }

    #[test]
    fn raw_state_decode_preflights_exact_member_profile_and_outbox_caps() {
        let member_values = (0..=MAX_HOUSEHOLD_MEMBERS)
            .map(|index| member(format!("member-{index:03}")))
            .collect::<Vec<_>>();
        for count in [MAX_HOUSEHOLD_MEMBERS - 1, MAX_HOUSEHOLD_MEMBERS] {
            let mut state = minimal_state();
            state.members = member_values[..count].to_vec();
            let raw = state.canonical_bytes().unwrap();
            assert_eq!(decode_canonical_household_state_v1(&raw).unwrap(), state);
        }
        let mut member_overflow = minimal_state();
        member_overflow.members = member_values;
        let member_overflow_raw = crate::to_canonical_bytes_v1(&member_overflow).unwrap();
        assert_eq!(
            decode_canonical_household_state_v1(&member_overflow_raw),
            Err(HouseholdStateError::CanonicalJson(
                CanonicalJsonError::MaximumArrayEntriesExceeded
            ))
        );

        let profile_members = (0..MAX_HOUSEHOLD_MEMBERS)
            .map(|index| member(format!("member-{index:03}")))
            .collect::<Vec<_>>();
        let empty_document = HouseholdProfileDocumentV1::legacy_projection(b"{}").unwrap();
        let mut profile_values = vec![HouseholdProfileRecordV1 {
            subject: HouseholdSubjectId::self_(),
            profile_revision: ProfileRevision::new(1).unwrap(),
            document: empty_document.clone(),
        }];
        profile_values.extend(
            profile_members
                .iter()
                .map(|member| HouseholdProfileRecordV1 {
                    subject: HouseholdSubjectId::member(member.member_id.clone()),
                    profile_revision: ProfileRevision::new(1).unwrap(),
                    document: empty_document.clone(),
                }),
        );
        for count in [MAX_HOUSEHOLD_PROFILES - 1, MAX_HOUSEHOLD_PROFILES] {
            let mut state = minimal_state();
            state.members = profile_members.clone();
            state.profiles = profile_values[..count].to_vec();
            let raw = state.canonical_bytes().unwrap();
            assert_eq!(decode_canonical_household_state_v1(&raw).unwrap(), state);
        }
        let mut profile_overflow = minimal_state();
        profile_overflow.members = profile_members;
        profile_overflow.profiles = profile_values;
        profile_overflow
            .profiles
            .push(profile_overflow.profiles[0].clone());
        let profile_overflow_raw = crate::to_canonical_bytes_v1(&profile_overflow).unwrap();
        assert_eq!(
            decode_canonical_household_state_v1(&profile_overflow_raw),
            Err(HouseholdStateError::CanonicalJson(
                CanonicalJsonError::MaximumArrayEntriesExceeded
            ))
        );

        let outbox_values = (0..=MAX_HOUSEHOLD_OUTBOX_ENTRIES)
            .map(legacy_mutation_record)
            .collect::<Vec<_>>();
        for count in [
            MAX_HOUSEHOLD_OUTBOX_ENTRIES - 1,
            MAX_HOUSEHOLD_OUTBOX_ENTRIES,
        ] {
            let mut state = minimal_state();
            state.outbox = outbox_values[..count].to_vec();
            let raw = state.canonical_bytes().unwrap();
            let typed: Result<HouseholdStateV1, _> = serde_json::from_slice(&raw);
            assert!(
                typed.is_ok(),
                "typed outbox boundary decode failed: {:?}",
                typed.unwrap_err()
            );
            assert_eq!(decode_canonical_household_state_v1(&raw).unwrap(), state);
        }
        let mut outbox_overflow = minimal_state();
        outbox_overflow.outbox = outbox_values;
        let outbox_overflow_raw = crate::to_canonical_bytes_v1(&outbox_overflow).unwrap();
        assert_eq!(
            decode_canonical_household_state_v1(&outbox_overflow_raw),
            Err(HouseholdStateError::CanonicalJson(
                CanonicalJsonError::MaximumArrayEntriesExceeded
            ))
        );
    }

    #[test]
    fn canonical_record_arrays_reject_unsorted_duplicates_and_orphans() {
        let mut state = minimal_state();
        state.members = vec![member("member-b".into()), member("member-a".into())];
        assert_eq!(
            state.validate(),
            Err(HouseholdStateError::UnsortedCollection)
        );
        state.members = vec![member("member-a".into()), member("member-a".into())];
        assert_eq!(
            state.validate(),
            Err(HouseholdStateError::DuplicateIdentity)
        );
        state.members = vec![member("member-a".into()), member("member-b".into())];
        let document = HouseholdProfileDocumentV1::legacy_projection(b"{}").unwrap();
        state.profiles = vec![
            HouseholdProfileRecordV1 {
                subject: HouseholdSubjectId::self_(),
                profile_revision: ProfileRevision::new(1).unwrap(),
                document: document.clone(),
            },
            HouseholdProfileRecordV1 {
                subject: HouseholdSubjectId::member(MemberId::parse_preserved("member-a").unwrap()),
                profile_revision: ProfileRevision::new(1).unwrap(),
                document,
            },
        ];
        state.validate().unwrap();
        state.profiles.swap(0, 1);
        assert_eq!(
            state.validate(),
            Err(HouseholdStateError::UnsortedCollection)
        );
        state.profiles = vec![HouseholdProfileRecordV1 {
            subject: HouseholdSubjectId::member(
                MemberId::parse_preserved("member-missing").unwrap(),
            ),
            profile_revision: ProfileRevision::new(1).unwrap(),
            document: HouseholdProfileDocumentV1::legacy_projection(b"{}").unwrap(),
        }];
        assert_eq!(state.validate(), Err(HouseholdStateError::OrphanReference));
    }

    #[test]
    fn unresolved_distinct_legacy_contexts_require_conflicted_even_with_materialized_profile() {
        let mut one_context_state = minimal_state();
        one_context_state.outbox = vec![legacy_context_record("_self", 9, "single-owner-context")];
        assert_eq!(
            one_context_state.validate(),
            Err(HouseholdStateError::InvalidProfileDocument),
            "one usable context must be materialized instead of remaining incomplete"
        );
        one_context_state.profiles = vec![usable_profile(
            HouseholdSubjectId::self_(),
            "single-owner-context",
        )];
        one_context_state.owner.profile_state = HouseholdProfileStateV1::LocalOnly;
        one_context_state.validate().unwrap();

        let mut owner_state = minimal_state();
        owner_state.profiles = vec![usable_profile(HouseholdSubjectId::self_(), "owner-local")];
        owner_state.outbox = vec![
            legacy_context_record("_self", 1, "owner-context-a"),
            legacy_context_record("_self", 2, "owner-context-b"),
        ];
        owner_state.outbox.sort_by(|left, right| {
            left.outbox_id
                .as_str()
                .as_bytes()
                .cmp(right.outbox_id.as_str().as_bytes())
        });
        for invalid_state in [
            HouseholdProfileStateV1::LocalOnly,
            HouseholdProfileStateV1::Synced,
            HouseholdProfileStateV1::PendingSync,
        ] {
            owner_state.owner.profile_state = invalid_state;
            assert_eq!(
                owner_state.validate(),
                Err(HouseholdStateError::InvalidProfileDocument)
            );
        }
        owner_state.owner.profile_state = HouseholdProfileStateV1::Conflicted;
        owner_state.validate().unwrap();
        assert!(
            owner_state.profiles[0]
                .document
                .effective_profile()
                .unwrap()
                .is_some(),
            "conflicted state retains encrypted profile material for later resolution"
        );

        let member_id = MemberId::parse_preserved("member-conflict").unwrap();
        let subject = HouseholdSubjectId::member(member_id.clone());
        let mut member_state = minimal_state();
        member_state.members = vec![member("member-conflict".into())];
        member_state.profiles = vec![usable_profile(subject, "member-local")];
        member_state.outbox = vec![
            legacy_context_record(member_id.as_str(), 3, "member-context-a"),
            legacy_context_record(member_id.as_str(), 4, "member-context-b"),
        ];
        member_state.outbox.sort_by(|left, right| {
            left.outbox_id
                .as_str()
                .as_bytes()
                .cmp(right.outbox_id.as_str().as_bytes())
        });
        for invalid_state in [
            HouseholdProfileStateV1::LocalOnly,
            HouseholdProfileStateV1::Synced,
        ] {
            member_state.members[0].profile_state = invalid_state;
            assert_eq!(
                member_state.validate(),
                Err(HouseholdStateError::InvalidProfileDocument)
            );
        }
        member_state.members[0].profile_state = HouseholdProfileStateV1::Conflicted;
        member_state.validate().unwrap();
        assert!(
            member_state.profiles[0]
                .document
                .effective_profile()
                .unwrap()
                .is_some(),
            "member conflict retains encrypted profile material without resolving personalization"
        );
    }

    #[test]
    fn legacy_custom_profile_arrays_reject_empty_and_whitespace_only_values() {
        for field in [
            "custom_health_conditions",
            "custom_diet_styles",
            "custom_restrictions",
            "custom_cuisines",
        ] {
            for invalid in ["", " ", "\t", "\u{2003}"] {
                let input = serde_json::to_vec(&json!({field:[invalid]})).unwrap();
                assert_eq!(
                    HouseholdProfileDocumentV1::legacy_projection(&input),
                    Err(HouseholdStateError::InvalidProfileDocument),
                    "{field} admitted an empty or whitespace-only compatibility value"
                );
            }
            let valid = serde_json::to_vec(&json!({field:[" preserved custom value "]})).unwrap();
            HouseholdProfileDocumentV1::legacy_projection(&valid).unwrap();
        }
    }

    #[test]
    fn owner_sync_initial_phase_is_bound_to_profile_and_record_identity() {
        let mut state = minimal_state();
        let document =
            HouseholdProfileDocumentV1::legacy_projection(br#"{"restrictions":["x"]}"#).unwrap();
        let effective = document.effective_profile().unwrap().unwrap();
        let profile_digest = canonical_sha256_v1(&effective).unwrap();
        state.owner.profile_state = HouseholdProfileStateV1::PendingSync;
        state.profiles = vec![HouseholdProfileRecordV1 {
            subject: HouseholdSubjectId::self_(),
            profile_revision: ProfileRevision::new(1).unwrap(),
            document,
        }];
        let intent_id = Uuid::parse_str("dddddddd-dddd-4ddd-8ddd-dddddddddddd").unwrap();
        let intent = OwnerSyncIntentV1 {
            schema_version: 1,
            intent_id,
            intent_revision: 1,
            phase: OwnerSyncIntentPhaseV1::NeedsConsentCheck,
            subject: HouseholdSubjectId::self_(),
            local_household_revision: 1,
            local_profile_revision: 1,
            local_profile_digest: profile_digest,
            remote_request_id: intent_id,
            consent_version: None,
            remote_base: None,
            expected_remote_version: None,
            request_method: None,
            request_path: None,
            request_body: None,
            request_body_digest: None,
            attempt_count: 0,
            last_definite_error: None,
            created_at: timestamp(),
            updated_at: timestamp(),
        };
        state.outbox = vec![HouseholdOutboxRecordV1 {
            outbox_id: HouseholdOutboxId::owner_sync(intent_id).unwrap(),
            outbox_revision: OutboxRevision::new(1).unwrap(),
            entry: HouseholdProfileOutboxEntryV1::OwnerSync {
                version: 1,
                target: HouseholdSubjectId::self_(),
                intent: intent.clone(),
            },
        }];
        state.validate().unwrap();
        let initial_raw = state.canonical_bytes().unwrap();
        assert_eq!(
            decode_canonical_household_state_v1(&initial_raw).unwrap(),
            state
        );
        let HouseholdProfileOutboxEntryV1::OwnerSync { intent, .. } = &mut state.outbox[0].entry
        else {
            unreachable!()
        };
        intent.phase = OwnerSyncIntentPhaseV1::ReadyToDispatch;
        assert_eq!(
            state.validate(),
            Err(HouseholdStateError::InvalidOwnerSyncIntent)
        );

        let request_body = CanonicalJsonObjectV1::parse(
            br#"{"member_id":"_self","profile_data":{"restrictions":["x"]}}"#,
            CompatibilityJsonLimitsV1::OWNER_SYNC_REQUEST,
        )
        .unwrap();
        let request_body_digest = request_body.canonical_sha256();
        let HouseholdProfileOutboxEntryV1::OwnerSync { intent, .. } = &mut state.outbox[0].entry
        else {
            unreachable!()
        };
        intent.consent_version = Some(ConsentVersionV1::new(3).unwrap());
        intent.remote_base = Some(RemoteProfileBaseV1 {
            existence: RemoteProfileExistenceV1::Absent,
            version: None,
            profile_digest: None,
        });
        intent.request_method = Some("PUT".into());
        intent.request_path = Some("/v1/profile/sync".into());
        intent.request_body = Some(request_body);
        intent.request_body_digest = Some(request_body_digest);
        state.validate().unwrap();
        let request_body_raw = state.canonical_bytes().unwrap();
        assert_eq!(
            decode_canonical_household_state_v1(&request_body_raw).unwrap(),
            state
        );
    }

    #[test]
    fn owner_sync_phase_nullability_and_error_table_is_closed() {
        let intent_id = Uuid::parse_str("dddddddd-dddd-4ddd-8ddd-dddddddddddd").unwrap();
        let local_profile = json!({"restrictions":["x"]});
        let local_profile_digest = canonical_sha256_v1(&local_profile).unwrap();
        let request_body = CanonicalJsonObjectV1::parse(
            br#"{"member_id":"_self","profile_data":{"restrictions":["x"]}}"#,
            CompatibilityJsonLimitsV1::OWNER_SYNC_REQUEST,
        )
        .unwrap();
        let request_body_digest = request_body.canonical_sha256();
        let base = OwnerSyncIntentV1 {
            schema_version: 1,
            intent_id,
            intent_revision: 1,
            phase: OwnerSyncIntentPhaseV1::ReadyToDispatch,
            subject: HouseholdSubjectId::self_(),
            local_household_revision: 1,
            local_profile_revision: 1,
            local_profile_digest,
            remote_request_id: intent_id,
            consent_version: Some(ConsentVersionV1::new(3).unwrap()),
            remote_base: Some(RemoteProfileBaseV1 {
                existence: RemoteProfileExistenceV1::Absent,
                version: None,
                profile_digest: None,
            }),
            expected_remote_version: None,
            request_method: Some("PUT".into()),
            request_path: Some("/v1/profile/sync".into()),
            request_body: Some(request_body),
            request_body_digest: Some(request_body_digest),
            attempt_count: 0,
            last_definite_error: None,
            created_at: timestamp(),
            updated_at: timestamp(),
        };
        let mut valid = Vec::new();
        valid.push(base.clone());

        let mut ready_after_cancel = base.clone();
        ready_after_cancel.attempt_count = 1;
        ready_after_cancel.last_definite_error =
            Some(LastDefiniteOwnerSyncErrorV1::PredispatchCancelled);
        valid.push(ready_after_cancel);

        let mut dispatching = base.clone();
        dispatching.phase = OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown;
        dispatching.attempt_count = 1;
        valid.push(dispatching);

        let mut uncertain = base.clone();
        uncertain.phase = OwnerSyncIntentPhaseV1::OutcomeUncertain;
        uncertain.attempt_count = 1;
        valid.push(uncertain.clone());
        uncertain.last_definite_error = Some(LastDefiniteOwnerSyncErrorV1::VersionConflict);
        valid.push(uncertain);

        let mut definite_http = base.clone();
        definite_http.phase = OwnerSyncIntentPhaseV1::DefiniteFailure;
        definite_http.attempt_count = 1;
        definite_http.last_definite_error = Some(LastDefiniteOwnerSyncErrorV1::Unauthorized);
        valid.push(definite_http);

        let mut definite_consent = base.clone();
        definite_consent.phase = OwnerSyncIntentPhaseV1::DefiniteFailure;
        definite_consent.last_definite_error =
            Some(LastDefiniteOwnerSyncErrorV1::ConsentVersionChangedRequiresNewSave);
        valid.push(definite_consent);

        let mut conflicted = base.clone();
        conflicted.phase = OwnerSyncIntentPhaseV1::Conflicted;
        conflicted.attempt_count = 1;
        conflicted.last_definite_error = Some(LastDefiniteOwnerSyncErrorV1::VersionConflict);
        valid.push(conflicted);

        let without_authority = |phase, consent_version, last_definite_error| OwnerSyncIntentV1 {
            phase,
            consent_version,
            remote_base: None,
            expected_remote_version: None,
            request_method: None,
            request_path: None,
            request_body: None,
            request_body_digest: None,
            attempt_count: 0,
            last_definite_error,
            ..base.clone()
        };
        valid.push(without_authority(
            OwnerSyncIntentPhaseV1::NeedsConsentCheck,
            None,
            None,
        ));
        valid.push(without_authority(
            OwnerSyncIntentPhaseV1::NeedsRemoteBase,
            Some(ConsentVersionV1::new(3).unwrap()),
            None,
        ));
        valid.push(without_authority(
            OwnerSyncIntentPhaseV1::LocalOnlyNoConsent,
            None,
            Some(LastDefiniteOwnerSyncErrorV1::ConsentAbsent),
        ));

        for intent in valid {
            intent.validate().unwrap();
        }

        let mut invalid = base;
        invalid.phase = OwnerSyncIntentPhaseV1::NeedsRemoteBase;
        assert_eq!(
            invalid.validate(),
            Err(HouseholdStateError::InvalidOwnerSyncIntent)
        );
    }

    #[test]
    fn exact_collection_limits_accept_limit_and_reject_limit_plus_one() {
        assert_eq!(MAX_HOUSEHOLD_MEMBERS, 256);
        assert_eq!(MAX_HOUSEHOLD_SUBJECTS, 257);
        assert_eq!(MAX_HOUSEHOLD_PROFILES, 257);
        assert_eq!(MAX_HOUSEHOLD_OUTBOX_ENTRIES, 1_024);
        assert_eq!(MAX_APPLIED_COMMITS, 16_384);
        assert_eq!(MAX_LEGACY_APPLIED_MUTATION_IDS, 100);
        assert_eq!(MAX_IMPORTED_COMPATIBILITY_FIELDS, 128);
        assert_eq!(MAX_MIGRATION_DISPOSITIONS, 128);
        assert_eq!(MAX_LEGACY_REMOTE_PROFILE_REFERENCES, 257);
        assert_eq!(MAX_LEGACY_TIMESTAMP_PROVENANCE, 1_539);
        assert_eq!(MAX_COMPATIBILITY_JSON_DEPTH, 8);
        assert_eq!(MAX_COMPATIBILITY_OBJECT_KEYS, 128);
        assert_eq!(MAX_COMPATIBILITY_ARRAY_ENTRIES, 256);
        assert_eq!(MAX_COMPATIBILITY_JSON_NODES, 65_536);
        assert_eq!(MAX_OWNER_SYNC_REQUEST_BODY_BYTES, 524_288);

        let members = (0..MAX_HOUSEHOLD_MEMBERS)
            .map(|index| member(format!("member-{index:03}")))
            .collect::<Vec<_>>();
        let mut state = minimal_state();
        state.members = members.clone();
        state.validate().unwrap();
        state.members.pop();
        state.validate().unwrap();
        state.members = members;
        state.members.push(member("member-over-limit".into()));
        assert_eq!(
            state.validate(),
            Err(HouseholdStateError::CardinalityExceeded)
        );

        let mut profile_state = minimal_state();
        profile_state.members = (0..MAX_HOUSEHOLD_MEMBERS)
            .map(|index| member(format!("member-{index:03}")))
            .collect();
        let document = HouseholdProfileDocumentV1::legacy_projection(b"{}").unwrap();
        profile_state.profiles.push(HouseholdProfileRecordV1 {
            subject: HouseholdSubjectId::self_(),
            profile_revision: ProfileRevision::new(1).unwrap(),
            document: document.clone(),
        });
        profile_state
            .profiles
            .extend(
                profile_state
                    .members
                    .iter()
                    .map(|member| HouseholdProfileRecordV1 {
                        subject: HouseholdSubjectId::member(member.member_id.clone()),
                        profile_revision: ProfileRevision::new(1).unwrap(),
                        document: document.clone(),
                    }),
            );
        profile_state.validate().unwrap();
        profile_state.profiles.push(HouseholdProfileRecordV1 {
            subject: HouseholdSubjectId::self_(),
            profile_revision: ProfileRevision::new(1).unwrap(),
            document: document.clone(),
        });
        assert_eq!(
            profile_state.validate(),
            Err(HouseholdStateError::CardinalityExceeded)
        );

        let payload = CanonicalJsonObjectV1::parse(
            br#"{"member_id":"_self","repair":true}"#,
            CompatibilityJsonLimitsV1::PROFILE_DOCUMENT,
        )
        .unwrap();
        let legacy = |index: usize| {
            let source_key = format!("mutation-{index:04}");
            HouseholdOutboxRecordV1 {
                outbox_id: HouseholdOutboxId::parse_legacy(source_key.clone()).unwrap(),
                outbox_revision: OutboxRevision::new(1).unwrap(),
                entry: HouseholdProfileOutboxEntryV1::Legacy {
                    version: 1,
                    target: HouseholdSubjectId::self_(),
                    legacy: LegacyProfileOutboxEntryV1 {
                        target: HouseholdSubjectId::self_(),
                        source_kind: LegacyOutboxSourceKindV1::RustMutationKeyedEmbeddedMemberV0,
                        source_key,
                        source_digest: CanonicalDigestV1::from_bytes([1; 32]),
                        payload: payload.clone(),
                        payload_digest: payload.canonical_sha256(),
                        phase: OutboxPhaseV1::PolicyBlockedLegacy,
                        updated_at: timestamp(),
                    },
                },
            }
        };
        let mut outbox_state = minimal_state();
        outbox_state.outbox = (0..MAX_HOUSEHOLD_OUTBOX_ENTRIES).map(legacy).collect();
        outbox_state.validate().unwrap();
        outbox_state.outbox.pop();
        outbox_state.validate().unwrap();
        outbox_state.outbox = (0..=MAX_HOUSEHOLD_OUTBOX_ENTRIES).map(legacy).collect();
        assert_eq!(
            outbox_state.validate(),
            Err(HouseholdStateError::CardinalityExceeded)
        );

        let commit = |index: usize| AppliedCommitRecordV1 {
            commit_id: CommitId::from_uuid(Uuid::from_u128(
                0x00000000000040008000000000000000_u128 + u128::try_from(index).unwrap(),
            )),
            fingerprint: CanonicalDigestV1::from_bytes([2; 32]),
            resulting_revision: HouseholdRevision::new(1).unwrap(),
            outcome: AppliedCommitOutcomeV1::Committed,
            committed_at: timestamp(),
        };
        let mut ledger_state = minimal_state();
        ledger_state.bounded_applied_commits = (0..MAX_APPLIED_COMMITS).map(commit).collect();
        ledger_state.validate().unwrap();
        assert_eq!(
            ledger_state.ensure_commit_capacity(),
            Err(HouseholdStateError::AppliedCommitLedgerFull)
        );
        ledger_state.bounded_applied_commits.pop();
        ledger_state.validate().unwrap();
        ledger_state.ensure_commit_capacity().unwrap();
        ledger_state.bounded_applied_commits = (0..=MAX_APPLIED_COMMITS).map(commit).collect();
        assert_eq!(
            ledger_state.validate(),
            Err(HouseholdStateError::CardinalityExceeded)
        );

        let mut compatibility_state = minimal_state();
        compatibility_state
            .imported_compatibility
            .legacy_python_applied_mutation_ids = (0..MAX_LEGACY_APPLIED_MUTATION_IDS)
            .map(|index| format!("legacy-mutation-{index:03}"))
            .collect();
        compatibility_state
            .imported_compatibility
            .legacy_python_applied_mutation_ids_digest = Some(
            canonical_sha256_v1(
                &compatibility_state
                    .imported_compatibility
                    .legacy_python_applied_mutation_ids,
            )
            .unwrap(),
        );
        compatibility_state.validate().unwrap();
        compatibility_state
            .imported_compatibility
            .legacy_python_applied_mutation_ids
            .pop();
        compatibility_state
            .imported_compatibility
            .legacy_python_applied_mutation_ids_digest = Some(
            canonical_sha256_v1(
                &compatibility_state
                    .imported_compatibility
                    .legacy_python_applied_mutation_ids,
            )
            .unwrap(),
        );
        compatibility_state.validate().unwrap();
        compatibility_state
            .imported_compatibility
            .legacy_python_applied_mutation_ids = (0..=MAX_LEGACY_APPLIED_MUTATION_IDS)
            .map(|index| format!("legacy-mutation-{index:03}"))
            .collect();
        assert_eq!(
            compatibility_state.validate(),
            Err(HouseholdStateError::CardinalityExceeded)
        );

        let compatibility_value = CanonicalJsonValueV1::from_value(Value::Null, 16).unwrap();
        let compatibility_field = |index: usize| ImportedCompatibilityFieldV1 {
            field_name: format!("field-{index:03}"),
            value: compatibility_value.clone(),
            source_digest: compatibility_value.canonical_sha256(),
        };
        let mut compatibility_fields_state = minimal_state();
        compatibility_fields_state.imported_compatibility.fields = (0
            ..MAX_IMPORTED_COMPATIBILITY_FIELDS)
            .map(compatibility_field)
            .collect();
        compatibility_fields_state.validate().unwrap();
        compatibility_fields_state
            .imported_compatibility
            .fields
            .push(compatibility_field(MAX_IMPORTED_COMPATIBILITY_FIELDS));
        assert_eq!(
            compatibility_fields_state.validate(),
            Err(HouseholdStateError::CardinalityExceeded)
        );

        let disposition = |index: usize| MigrationDispositionV1 {
            field_name: format!("field-{index:03}"),
            disposition: MigrationDispositionKindV1::Retired,
            destination_schema: None,
            source_digest: None,
            destination_digest: None,
        };
        let mut disposition_state = minimal_state();
        disposition_state.migration_dispositions.dispositions =
            (0..MAX_MIGRATION_DISPOSITIONS).map(disposition).collect();
        disposition_state.validate().unwrap();
        disposition_state
            .migration_dispositions
            .dispositions
            .push(disposition(MAX_MIGRATION_DISPOSITIONS));
        assert_eq!(
            disposition_state.validate(),
            Err(HouseholdStateError::CardinalityExceeded)
        );

        let mut reference_state = minimal_state();
        reference_state.members = (0..MAX_HOUSEHOLD_MEMBERS)
            .map(|index| member(format!("member-{index:03}")))
            .collect();
        reference_state
            .imported_compatibility
            .legacy_remote_profile_references
            .push(LegacyRemoteProfileReferenceV1 {
                subject: HouseholdSubjectId::self_(),
                source_digest: CanonicalDigestV1::from_bytes([3; 32]),
            });
        reference_state
            .imported_compatibility
            .legacy_remote_profile_references
            .extend(
                reference_state
                    .members
                    .iter()
                    .map(|member| LegacyRemoteProfileReferenceV1 {
                        subject: HouseholdSubjectId::member(member.member_id.clone()),
                        source_digest: CanonicalDigestV1::from_bytes([3; 32]),
                    }),
            );
        reference_state.validate().unwrap();
        reference_state
            .imported_compatibility
            .legacy_remote_profile_references
            .push(LegacyRemoteProfileReferenceV1 {
                subject: HouseholdSubjectId::self_(),
                source_digest: CanonicalDigestV1::from_bytes([3; 32]),
            });
        assert_eq!(
            reference_state.validate(),
            Err(HouseholdStateError::CardinalityExceeded)
        );

        let timestamp_provenance = |index: usize| LegacyTimestampRecordV1 {
            field_path: format!("field-{index:04}"),
            disposition: LegacyTimestampDispositionV1::LegacyMissingTime {
                normalized: timestamp(),
            },
        };
        let mut timestamp_state = minimal_state();
        timestamp_state
            .imported_compatibility
            .legacy_timestamp_provenance = (0..MAX_LEGACY_TIMESTAMP_PROVENANCE)
            .map(timestamp_provenance)
            .collect();
        timestamp_state.validate().unwrap();
        timestamp_state
            .imported_compatibility
            .legacy_timestamp_provenance
            .push(timestamp_provenance(MAX_LEGACY_TIMESTAMP_PROVENANCE));
        assert_eq!(
            timestamp_state.validate(),
            Err(HouseholdStateError::CardinalityExceeded)
        );
    }

    #[test]
    fn sensitive_debug_is_redacted() {
        let member = member("canary-member-id".into());
        let profile =
            HouseholdProfileDocumentV1::legacy_projection(br#"{"notes":"canary-profile"}"#)
                .unwrap();
        let projection = profile.legacy_projection_view().unwrap().unwrap();
        let state = minimal_state();
        for rendered in [
            format!("{member:?}"),
            format!("{profile:?}"),
            format!("{projection:?}"),
            format!("{state:?}"),
        ] {
            assert!(!rendered.contains("canary-member-id"));
            assert!(!rendered.contains("Private name"));
            assert!(!rendered.contains("canary-profile"));
            assert!(!rendered.contains("Owner"));
        }
    }
}
