//! Renderer-neutral capability discovery.

use heyfood_core::GroceryCapability;
use tokio_util::sync::CancellationToken;

use crate::{BoxFuture, PortError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistrationAvailability {
    Available,
    Disabled,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilitySnapshot {
    pub schema_version: u16,
    pub registration: RegistrationAvailability,
    pub profile_readiness: bool,
    pub loopback_pkce: bool,
    pub device_code: bool,
    pub grocery: GroceryCapability,
}

pub trait CapabilityPort: Send + Sync {
    fn discover(
        &self,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<CapabilitySnapshot, PortError>>;
}

pub struct DiscoverCapabilities<'a> {
    port: &'a dyn CapabilityPort,
}

impl<'a> DiscoverCapabilities<'a> {
    #[must_use]
    pub const fn new(port: &'a dyn CapabilityPort) -> Self {
        Self { port }
    }

    pub async fn execute(
        &self,
        cancellation: CancellationToken,
    ) -> Result<CapabilitySnapshot, PortError> {
        if cancellation.is_cancelled() {
            return Err(PortError::new(
                "capability_cancelled_before_dispatch",
                "Capability discovery was cancelled before dispatch",
            ));
        }
        self.port.discover(cancellation).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    struct FakeCapabilityPort {
        called: AtomicBool,
        snapshot: CapabilitySnapshot,
    }

    impl CapabilityPort for FakeCapabilityPort {
        fn discover(
            &self,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<CapabilitySnapshot, PortError>> {
            self.called.store(true, Ordering::SeqCst);
            let snapshot = self.snapshot.clone();
            Box::pin(async move { Ok(snapshot) })
        }
    }

    fn snapshot() -> CapabilitySnapshot {
        CapabilitySnapshot {
            schema_version: 1,
            registration: RegistrationAvailability::Available,
            profile_readiness: true,
            loopback_pkce: true,
            device_code: false,
            grocery: GroceryCapability::V1,
        }
    }

    #[tokio::test]
    async fn controller_forwards_to_the_port() {
        let port = FakeCapabilityPort {
            called: AtomicBool::new(false),
            snapshot: snapshot(),
        };

        let discovered = DiscoverCapabilities::new(&port)
            .execute(CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(discovered, snapshot());
        assert!(port.called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn controller_does_not_dispatch_after_pre_cancellation() {
        let port = FakeCapabilityPort {
            called: AtomicBool::new(false),
            snapshot: snapshot(),
        };
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let error = DiscoverCapabilities::new(&port)
            .execute(cancellation)
            .await
            .unwrap_err();

        assert_eq!(error.code, "capability_cancelled_before_dispatch");
        assert!(!port.called.load(Ordering::SeqCst));
    }
}
