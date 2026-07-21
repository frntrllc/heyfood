//! H1/H2 provider-neutral application ports from the reviewed backend freeze.

use heyfood_core::{
    BrowserUrl, HealthConnectionStatus, HealthFreshness, HealthProvider, HealthTrend, OperationId,
    SensitiveString, SessionCredentials,
};
use tokio_util::sync::CancellationToken;

use crate::{BoxFuture, PortError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HealthContext {
    pub freshness: HealthFreshness,
    pub trends: Vec<HealthTrend>,
    /// Goals are sensitive health context and redact from diagnostics.
    pub goals: Vec<SensitiveString>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HealthConnection {
    pub provider: HealthProvider,
    pub status: HealthConnectionStatus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HealthAuthorization {
    pub provider: HealthProvider,
    pub browser_url: BrowserUrl,
    /// Opaque server completion handle; never a provider OAuth credential.
    pub completion_handle: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HealthManagementOutcome {
    Accepted,
    Completed(HealthConnectionStatus),
    OutcomeUncertain,
}

pub trait HealthPort: Send + Sync {
    fn read_context(
        &self,
        credentials: SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<HealthContext, PortError>>;

    fn list_connections(
        &self,
        credentials: SessionCredentials,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<Vec<HealthConnection>, PortError>>;

    fn authorize(
        &self,
        credentials: SessionCredentials,
        provider: HealthProvider,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<HealthAuthorization, PortError>>;

    fn sync(
        &self,
        credentials: SessionCredentials,
        provider: HealthProvider,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<HealthManagementOutcome, PortError>>;

    fn disconnect(
        &self,
        credentials: SessionCredentials,
        provider: HealthProvider,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<HealthManagementOutcome, PortError>>;
}
