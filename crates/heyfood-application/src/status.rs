//! Typed status discovery shared by terminal and future agent presentations.

use heyfood_core::{OperationId, SessionCredentials};
use tokio_util::sync::CancellationToken;

use crate::{BoxFuture, CapabilityPort, CapabilitySnapshot, PortError, RegistrationAvailability};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileReadinessStatus {
    NotAuthorized,
    AuthorizedConsentGranted,
    AuthorizedConsentNotGranted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OptionalCapabilityStatus {
    NotAdvertised,
    AuthorizationRequired,
    Authorized,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VoiceReadinessStatus {
    AuthorizationRequiredCaptureAvailable,
    AuthorizationRequiredCaptureUnavailable,
    AuthorizedCaptureAvailable,
    AuthorizedCaptureUnavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatusSnapshot {
    pub service_reachable: bool,
    pub registration: RegistrationAvailability,
    pub profile: ProfileReadinessStatus,
    pub grocery: OptionalCapabilityStatus,
    pub diet: OptionalCapabilityStatus,
    pub menu_watch: OptionalCapabilityStatus,
    pub voice: VoiceReadinessStatus,
}

pub trait StatusPort: CapabilityPort {
    fn profile_consent_granted(
        &self,
        credentials: SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<bool, PortError>>;
}

pub struct ReadStatus<'a> {
    port: &'a dyn StatusPort,
}

impl<'a> ReadStatus<'a> {
    #[must_use]
    pub const fn new(port: &'a dyn StatusPort) -> Self {
        Self { port }
    }

    pub async fn execute(
        &self,
        credentials: SessionCredentials,
        authorization_scope: &str,
        native_voice_available: bool,
        cancellation: CancellationToken,
    ) -> Result<StatusSnapshot, PortError> {
        if cancellation.is_cancelled() {
            return Err(PortError::new(
                "status_cancelled_before_dispatch",
                "Status discovery was cancelled before dispatch",
            ));
        }
        let capabilities = self.port.discover(cancellation.child_token()).await?;
        let profile_authorized = has_scope(authorization_scope, "profile:read");
        let profile = if profile_authorized {
            if self
                .port
                .profile_consent_granted(
                    credentials,
                    OperationId::new(),
                    cancellation.child_token(),
                )
                .await?
            {
                ProfileReadinessStatus::AuthorizedConsentGranted
            } else {
                ProfileReadinessStatus::AuthorizedConsentNotGranted
            }
        } else {
            ProfileReadinessStatus::NotAuthorized
        };

        Ok(compose_status(
            capabilities,
            profile,
            authorization_scope,
            native_voice_available,
        ))
    }
}

fn compose_status(
    capabilities: CapabilitySnapshot,
    profile: ProfileReadinessStatus,
    authorization_scope: &str,
    native_voice_available: bool,
) -> StatusSnapshot {
    let grocery = if !capabilities.grocery.is_usable() {
        OptionalCapabilityStatus::NotAdvertised
    } else if has_scope(authorization_scope, "grocery:read") {
        OptionalCapabilityStatus::Authorized
    } else {
        OptionalCapabilityStatus::AuthorizationRequired
    };
    let diet = if !capabilities.diet.is_usable() {
        OptionalCapabilityStatus::NotAdvertised
    } else if has_scope(authorization_scope, "knowledge:read") {
        OptionalCapabilityStatus::Authorized
    } else {
        OptionalCapabilityStatus::AuthorizationRequired
    };
    let menu_watch = if has_scope(authorization_scope, "menu:watch") {
        OptionalCapabilityStatus::Authorized
    } else {
        OptionalCapabilityStatus::AuthorizationRequired
    };
    let voice = match (
        has_scope(authorization_scope, "audio:transcribe"),
        native_voice_available,
    ) {
        (true, true) => VoiceReadinessStatus::AuthorizedCaptureAvailable,
        (true, false) => VoiceReadinessStatus::AuthorizedCaptureUnavailable,
        (false, true) => VoiceReadinessStatus::AuthorizationRequiredCaptureAvailable,
        (false, false) => VoiceReadinessStatus::AuthorizationRequiredCaptureUnavailable,
    };
    StatusSnapshot {
        service_reachable: true,
        registration: capabilities.registration,
        profile,
        grocery,
        diet,
        menu_watch,
        voice,
    }
}

fn has_scope(granted: &str, required: &str) -> bool {
    granted
        .split_ascii_whitespace()
        .any(|scope| scope == required)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use heyfood_core::{AccountId, CredentialVersion, GroceryCapability, SensitiveString};

    use super::*;

    struct FakeStatusPort {
        capabilities: CapabilitySnapshot,
        consent: bool,
        discovery_reads: AtomicUsize,
        consent_reads: AtomicUsize,
    }

    impl CapabilityPort for FakeStatusPort {
        fn discover(
            &self,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<CapabilitySnapshot, PortError>> {
            self.discovery_reads.fetch_add(1, Ordering::SeqCst);
            let capabilities = self.capabilities.clone();
            Box::pin(async move { Ok(capabilities) })
        }
    }

    impl StatusPort for FakeStatusPort {
        fn profile_consent_granted(
            &self,
            _credentials: SessionCredentials,
            _operation_id: OperationId,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<bool, PortError>> {
            self.consent_reads.fetch_add(1, Ordering::SeqCst);
            let consent = self.consent;
            Box::pin(async move { Ok(consent) })
        }
    }

    fn credentials() -> SessionCredentials {
        SessionCredentials::from_unix_expiry(
            AccountId::parse("status-test").unwrap(),
            SensitiveString::new("access"),
            SensitiveString::new("refresh"),
            CredentialVersion::new(1),
            4_102_444_800,
        )
        .unwrap()
    }

    fn port(consent: bool) -> FakeStatusPort {
        FakeStatusPort {
            capabilities: CapabilitySnapshot {
                schema_version: 1,
                registration: RegistrationAvailability::Available,
                profile_readiness: true,
                loopback_pkce: true,
                device_code: true,
                grocery: GroceryCapability::V1,
                diet: heyfood_core::DietCapability::V1,
            },
            consent,
            discovery_reads: AtomicUsize::new(0),
            consent_reads: AtomicUsize::new(0),
        }
    }

    #[tokio::test]
    async fn status_composes_capabilities_scopes_consent_and_local_voice() {
        let port = port(true);
        let snapshot = ReadStatus::new(&port)
            .execute(
                credentials(),
                "profile:read grocery:read knowledge:read menu:watch audio:transcribe",
                false,
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(
            snapshot.profile,
            ProfileReadinessStatus::AuthorizedConsentGranted
        );
        assert_eq!(snapshot.grocery, OptionalCapabilityStatus::Authorized);
        assert_eq!(snapshot.diet, OptionalCapabilityStatus::Authorized);
        assert_eq!(snapshot.menu_watch, OptionalCapabilityStatus::Authorized);
        assert_eq!(
            snapshot.voice,
            VoiceReadinessStatus::AuthorizedCaptureUnavailable
        );
        assert_eq!(port.consent_reads.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn missing_profile_scope_never_reads_profile_consent() {
        let port = port(true);
        let snapshot = ReadStatus::new(&port)
            .execute(credentials(), "", true, CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(snapshot.profile, ProfileReadinessStatus::NotAuthorized);
        assert_eq!(
            snapshot.grocery,
            OptionalCapabilityStatus::AuthorizationRequired
        );
        assert_eq!(
            snapshot.diet,
            OptionalCapabilityStatus::AuthorizationRequired
        );
        assert_eq!(
            snapshot.voice,
            VoiceReadinessStatus::AuthorizationRequiredCaptureAvailable
        );
        assert_eq!(port.consent_reads.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn pre_dispatch_cancellation_opens_no_status_port() {
        let port = port(true);
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let error = ReadStatus::new(&port)
            .execute(credentials(), "profile:read", true, cancellation)
            .await
            .unwrap_err();

        assert_eq!(error.code, "status_cancelled_before_dispatch");
        assert_eq!(port.discovery_reads.load(Ordering::SeqCst), 0);
        assert_eq!(port.consent_reads.load(Ordering::SeqCst), 0);
    }
}
