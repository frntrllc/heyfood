//! Provider-neutral grocery semantics. Final backend wire DTOs remain gated on
//! the deployed Grocery Phase-A contract freeze.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroceryConfirmationState {
    Proposed,
    Accepted,
    Cancelled,
    RejectedStale,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GroceryConfirmation {
    pub confirmation_id: GroceryEntityId,
    pub preconditions: FrozenGroceryPreconditions,
    pub state: GroceryConfirmationState,
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
