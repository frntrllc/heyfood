//! Secret-bearing authentication and session contracts.

use std::fmt;

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Deserializer, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

pub const GROCERY_READ_SCOPE: &str = "grocery:read";
pub const GROCERY_WRITE_SCOPE: &str = "grocery:write";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroceryScopeAuthority {
    Read,
    ReadWrite,
}

/// Build a replacement grant in the authorization server's canonical order.
/// Existing non-Grocery scopes are preserved, but Grocery authority is reduced
/// to exactly what the requested operation requires. Optional scopes are never
/// inferred from application capabilities or a failed metadata request.
pub fn negotiate_grocery_scopes(
    current_scopes: &[String],
    supported_scopes: &[String],
    authority: GroceryScopeAuthority,
) -> Result<Vec<String>, &'static str> {
    if supported_scopes.is_empty() {
        return Err("authorization metadata did not publish any scopes");
    }
    let required = match authority {
        GroceryScopeAuthority::Read => &[GROCERY_READ_SCOPE][..],
        GroceryScopeAuthority::ReadWrite => &[GROCERY_READ_SCOPE, GROCERY_WRITE_SCOPE][..],
    };
    if required
        .iter()
        .any(|scope| !supported_scopes.iter().any(|candidate| candidate == scope))
    {
        return Err("the authorization server does not publish the required Grocery scopes");
    }

    let mut negotiated = Vec::new();
    for supported in supported_scopes {
        let preserve = current_scopes.iter().any(|scope| scope == supported)
            && !matches!(supported.as_str(), GROCERY_READ_SCOPE | GROCERY_WRITE_SCOPE);
        let require = required.iter().any(|scope| *scope == supported);
        if (preserve || require) && !negotiated.iter().any(|scope| scope == supported) {
            negotiated.push(supported.clone());
        }
    }
    if negotiated.is_empty() {
        return Err("authorization scope negotiation produced an empty grant");
    }
    Ok(negotiated)
}

/// The identity methods the hosted authorization page may offer. Authentication
/// remains browser-owned; the native client only validates advertised launch
/// capability and never handles verification codes itself.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityMethod {
    Sms,
    Email,
    Apple,
    Google,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationStatus {
    Available,
    Disabled,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileStatus {
    Ready,
    Missing,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SelfRegistrationCapability {
    pub status: RegistrationStatus,
    pub regions: Vec<String>,
    pub identity_methods: Vec<IdentityMethod>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuthorizationCapability {
    pub loopback_pkce: bool,
    pub device_code: bool,
    pub identity_methods: Vec<IdentityMethod>,
}

/// Versioned public response from `GET /v1/auth/capabilities`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuthCapabilities {
    pub schema_version: u16,
    pub self_registration: SelfRegistrationCapability,
    pub authorization: AuthorizationCapability,
    pub profile_readiness: bool,
    #[serde(default)]
    pub application_capabilities: std::collections::BTreeMap<String, String>,
}

impl AuthCapabilities {
    /// Registration launch requires the production US surface, the device flow,
    /// profile readiness, and both promised launch identity methods.
    pub fn validate_native_registration_launch(&self) -> Result<(), &'static str> {
        if self.schema_version != 1 {
            return Err("unsupported authentication capability schema");
        }
        if self.self_registration.status != RegistrationStatus::Available {
            return Err("self registration is not available");
        }
        if self.self_registration.regions.as_slice() != ["US"] {
            return Err("self registration must advertise the US launch region");
        }
        if !self.authorization.device_code || !self.profile_readiness {
            return Err("native registration dependencies are unavailable");
        }
        for method in [IdentityMethod::Sms, IdentityMethod::Email] {
            if !self.self_registration.identity_methods.contains(&method)
                || !self.authorization.identity_methods.contains(&method)
            {
                return Err("native registration requires both SMS and email identity methods");
            }
        }
        if self
            .self_registration
            .identity_methods
            .iter()
            .any(|method| !self.authorization.identity_methods.contains(method))
        {
            return Err("registration identity methods exceed authorization capability");
        }
        Ok(())
    }
}

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

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
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
        if value.len() > 256 || value.chars().any(char::is_control) {
            return Err("account ID is invalid");
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for AccountId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
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

/// Channel OAuth grant retained alongside the app session. The channel access
/// token is required for profile readiness and session re-exchange; keeping the
/// pair together prevents a successful registration from degrading after the
/// first app-session access token expires.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChannelCredentials {
    pub client_id: String,
    pub device_id: String,
    pub access_token: SensitiveString,
    pub refresh_token: SensitiveString,
    pub expires_at: OffsetDateTime,
    pub scope: String,
}

impl ChannelCredentials {
    pub fn from_unix_expiry(
        client_id: impl Into<String>,
        device_id: impl Into<String>,
        access_token: SensitiveString,
        refresh_token: SensitiveString,
        expires_at_unix: i64,
        scope: impl Into<String>,
    ) -> Result<Self, &'static str> {
        let client_id = client_id.into();
        let device_id = device_id.into();
        let scope = scope.into();
        if client_id.is_empty() || client_id.len() > 128 || client_id.chars().any(char::is_control)
        {
            return Err("OAuth client ID is invalid");
        }
        if device_id.len() < 3
            || device_id.len() > 255
            || device_id.trim() != device_id
            || device_id.chars().any(char::is_control)
        {
            return Err("device ID is invalid");
        }
        if access_token.expose_secret().is_empty() || refresh_token.expose_secret().is_empty() {
            return Err("channel credentials are incomplete");
        }
        if scope.is_empty() || scope.len() > 4_096 || scope.chars().any(char::is_control) {
            return Err("OAuth scope is invalid");
        }
        let expires_at = OffsetDateTime::from_unix_timestamp(expires_at_unix)
            .map_err(|_| "channel credential expiry is outside the supported range")?;
        Ok(Self {
            client_id,
            device_id,
            access_token,
            refresh_token,
            expires_at,
            scope,
        })
    }

    #[must_use]
    pub fn expires_at_unix(&self) -> i64 {
        self.expires_at.unix_timestamp()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthCredentialBundle {
    pub channel: ChannelCredentials,
    pub session: SessionCredentials,
}

impl AuthCredentialBundle {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.channel.device_id.is_empty() {
            return Err("authentication bundle device ID is missing");
        }
        Ok(())
    }
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

#[cfg(test)]
mod scope_tests {
    use super::*;

    fn values(scopes: &[&str]) -> Vec<String> {
        scopes.iter().map(|scope| (*scope).to_owned()).collect()
    }

    #[test]
    fn read_authority_never_infers_or_preserves_grocery_write() {
        let current = values(&["profile:read", "grocery:write"]);
        let supported = values(&[
            "profile:read",
            "health:read",
            GROCERY_READ_SCOPE,
            GROCERY_WRITE_SCOPE,
        ]);
        assert_eq!(
            negotiate_grocery_scopes(&current, &supported, GroceryScopeAuthority::Read).unwrap(),
            values(&["profile:read", GROCERY_READ_SCOPE])
        );
    }

    #[test]
    fn write_authority_requires_both_published_scopes_in_server_order() {
        let current = values(&["profile:read"]);
        let supported = values(&["profile:read", GROCERY_WRITE_SCOPE]);
        assert!(
            negotiate_grocery_scopes(&current, &supported, GroceryScopeAuthority::ReadWrite)
                .is_err()
        );
        let supported = values(&["profile:read", GROCERY_READ_SCOPE, GROCERY_WRITE_SCOPE]);
        assert_eq!(
            negotiate_grocery_scopes(&current, &supported, GroceryScopeAuthority::ReadWrite)
                .unwrap(),
            supported
        );
    }
}
