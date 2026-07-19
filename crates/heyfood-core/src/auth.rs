//! Secret-bearing authentication and session contracts.

use std::fmt;

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// A secret string that redacts both `Debug` and `Display` output.
#[derive(Clone)]
pub struct SensitiveString(SecretString);

impl SensitiveString {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(SecretString::from(value.into()))
    }

    /// Explicitly expose a secret only at a credential/service adapter boundary.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        self.0.expose_secret()
    }
}

impl fmt::Debug for SensitiveString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SensitiveString([REDACTED])")
    }
}

impl fmt::Display for SensitiveString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl PartialEq for SensitiveString {
    fn eq(&self, other: &Self) -> bool {
        self.expose_secret() == other.expose_secret()
    }
}

impl Eq for SensitiveString {}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccountId(String);

impl AccountId {
    pub fn parse(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err("account ID must not be empty");
        }
        if trimmed != value {
            return Err("account ID must not contain surrounding whitespace");
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct CredentialVersion(u64);

impl CredentialVersion {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

/// Credentials are intentionally not serializable. Persistence adapters must
/// opt in to exposure at their narrow boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionCredentials {
    pub account_id: AccountId,
    pub access_token: SensitiveString,
    pub refresh_token: SensitiveString,
    pub version: CredentialVersion,
    pub expires_at: OffsetDateTime,
}

impl SessionCredentials {
    /// Construct credentials from a wire-format Unix expiry without exposing
    /// the concrete time crate to application adapters and tests.
    pub fn from_unix_expiry(
        account_id: AccountId,
        access_token: SensitiveString,
        refresh_token: SensitiveString,
        version: CredentialVersion,
        expires_at_unix: i64,
    ) -> Result<Self, &'static str> {
        let expires_at = OffsetDateTime::from_unix_timestamp(expires_at_unix)
            .map_err(|_| "credential expiry is outside the supported range")?;
        Ok(Self {
            account_id,
            access_token,
            refresh_token,
            version,
            expires_at,
        })
    }

    /// Construct credentials from the RFC 3339 timestamp emitted by the
    /// Python `SessionResponse` and `CliSessionExchangeResponse` schemas.
    pub fn from_rfc3339_expiry(
        account_id: AccountId,
        access_token: SensitiveString,
        refresh_token: SensitiveString,
        version: CredentialVersion,
        access_expires_at: &str,
    ) -> Result<Self, &'static str> {
        let expires_at = OffsetDateTime::parse(access_expires_at, &Rfc3339)
            .map_err(|_| "credential expiry is not a valid RFC 3339 timestamp")?;
        Ok(Self {
            account_id,
            access_token,
            refresh_token,
            version,
            expires_at,
        })
    }

    #[must_use]
    pub fn expires_at_unix(&self) -> i64 {
        self.expires_at.unix_timestamp()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionSnapshot {
    pub credentials: SessionCredentials,
    pub reconciliation_required: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefreshRequest {
    pub account_id: AccountId,
    /// Python permits legacy/session-exchange documents without a refresh
    /// token and immediately falls back to the channel re-exchange path.
    pub refresh_token: Option<SensitiveString>,
    pub current_version: CredentialVersion,
}

impl From<&SessionCredentials> for RefreshRequest {
    fn from(credentials: &SessionCredentials) -> Self {
        Self {
            account_id: credentials.account_id.clone(),
            refresh_token: (!credentials.refresh_token.expose_secret().is_empty())
                .then(|| credentials.refresh_token.clone()),
            current_version: credentials.version,
        }
    }
}

/// Whether a refresh operation crossed the network dispatch boundary.
///
/// Cancellation and transport errors after dispatch are errors with uncertain
/// outcomes, not this value. This variant is reserved for a cancellation
/// observed before either refresh/re-exchange request was dispatched.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RefreshOutcome {
    Refreshed(RefreshResult),
    CancelledBeforeDispatch,
}

/// A validated server-accepted credential rotation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefreshResult {
    rotated: SessionCredentials,
}

impl RefreshResult {
    pub fn validated(
        request: &RefreshRequest,
        rotated: SessionCredentials,
    ) -> Result<Self, &'static str> {
        if rotated.account_id != request.account_id {
            return Err("refresh response account does not match the request");
        }
        if rotated.version <= request.current_version {
            return Err("refresh response credential version must advance");
        }
        Ok(Self { rotated })
    }

    #[must_use]
    pub fn rotated(&self) -> &SessionCredentials {
        &self.rotated
    }

    #[must_use]
    pub fn into_rotated(self) -> SessionCredentials {
        self.rotated
    }
}
