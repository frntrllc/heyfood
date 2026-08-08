//! Renderer-neutral diet catalog reads.

use heyfood_core::{DietCapability, DietCatalog, DietDetail, OperationId, SessionCredentials};
use tokio_util::sync::CancellationToken;

use crate::{BoxFuture, CapabilitySnapshot, PortError};

pub trait DietPort: Send + Sync {
    fn list(
        &self,
        capabilities: CapabilitySnapshot,
        credentials: SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<DietCatalog, PortError>>;

    fn detail(
        &self,
        capabilities: CapabilitySnapshot,
        credentials: SessionCredentials,
        operation_id: OperationId,
        diet_id: String,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<DietDetail, PortError>>;
}

pub struct ReadDietCatalog<'a> {
    port: &'a dyn DietPort,
}

impl<'a> ReadDietCatalog<'a> {
    #[must_use]
    pub const fn new(port: &'a dyn DietPort) -> Self {
        Self { port }
    }

    pub async fn execute(
        &self,
        capabilities: CapabilitySnapshot,
        credentials: SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<DietCatalog, PortError> {
        ensure_diet_v1(&capabilities)?;
        ensure_not_cancelled(&cancellation, "diet_list_cancelled_before_dispatch")?;
        self.port
            .list(capabilities, credentials, operation_id, cancellation)
            .await
    }
}

pub struct ReadDietDetail<'a> {
    port: &'a dyn DietPort,
}

impl<'a> ReadDietDetail<'a> {
    #[must_use]
    pub const fn new(port: &'a dyn DietPort) -> Self {
        Self { port }
    }

    pub async fn execute(
        &self,
        capabilities: CapabilitySnapshot,
        credentials: SessionCredentials,
        operation_id: OperationId,
        diet_id: String,
        cancellation: CancellationToken,
    ) -> Result<DietDetail, PortError> {
        ensure_diet_v1(&capabilities)?;
        ensure_not_cancelled(&cancellation, "diet_detail_cancelled_before_dispatch")?;
        self.port
            .detail(
                capabilities,
                credentials,
                operation_id,
                diet_id,
                cancellation,
            )
            .await
    }
}

fn ensure_diet_v1(capabilities: &CapabilitySnapshot) -> Result<(), PortError> {
    match &capabilities.diet {
        DietCapability::V1 => Ok(()),
        DietCapability::Unavailable => Err(PortError::new(
            "diet_capability_unavailable",
            "Diet guidance is not advertised by this deployment",
        )),
        DietCapability::UnsupportedVersion(_) => Err(PortError::new(
            "diet_capability_unsupported",
            "Diet guidance advertises an unsupported contract version",
        )),
    }
}

fn ensure_not_cancelled(
    cancellation: &CancellationToken,
    code: &'static str,
) -> Result<(), PortError> {
    if cancellation.is_cancelled() {
        Err(PortError::new(
            code,
            "Diet guidance read was cancelled before dispatch",
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use heyfood_core::{AccountId, CredentialVersion, GroceryCapability, SensitiveString};

    use crate::RegistrationAvailability;

    use super::*;

    struct RejectingPort(AtomicUsize);

    impl DietPort for RejectingPort {
        fn list(
            &self,
            _capabilities: CapabilitySnapshot,
            _credentials: SessionCredentials,
            _operation_id: OperationId,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<DietCatalog, PortError>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { unreachable!() })
        }

        fn detail(
            &self,
            _capabilities: CapabilitySnapshot,
            _credentials: SessionCredentials,
            _operation_id: OperationId,
            _diet_id: String,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<DietDetail, PortError>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { unreachable!() })
        }
    }

    fn capabilities(diet: DietCapability) -> CapabilitySnapshot {
        CapabilitySnapshot {
            schema_version: 1,
            registration: RegistrationAvailability::Disabled,
            profile_readiness: true,
            loopback_pkce: true,
            device_code: true,
            grocery: GroceryCapability::Unavailable,
            diet,
        }
    }

    fn credentials() -> SessionCredentials {
        SessionCredentials::from_unix_expiry(
            AccountId::parse("diet-test").unwrap(),
            SensitiveString::new("access"),
            SensitiveString::new("refresh"),
            CredentialVersion::new(1),
            4_102_444_800,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn reads_fail_closed_before_port_dispatch() {
        let port = RejectingPort(AtomicUsize::new(0));
        let error = ReadDietCatalog::new(&port)
            .execute(
                capabilities(DietCapability::Unavailable),
                credentials(),
                OperationId::new(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "diet_capability_unavailable");

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let error = ReadDietDetail::new(&port)
            .execute(
                capabilities(DietCapability::V1),
                credentials(),
                OperationId::new(),
                "keto".into(),
                cancellation,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "diet_detail_cancelled_before_dispatch");
        assert_eq!(port.0.load(Ordering::SeqCst), 0);
    }
}
