//! Cancellation-safe session refresh shared by Phase 2 one-shot commands.

use std::fmt;
use std::sync::Arc;

use heyfood_core::{CommitId, RefreshOutcome, RefreshRequest, SessionCredentials, SessionSnapshot};
use tokio_util::sync::CancellationToken;

use crate::{ClockPort, CredentialCommit, CredentialPort, PortError, ServicePort};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnsureSessionOutcome {
    Current(SessionCredentials),
    Refreshed(SessionCredentials),
    CancelledBeforeDispatch,
}

#[derive(Clone, Eq, PartialEq)]
pub enum EnsureSessionError {
    ReconciliationRequired,
    Service(PortError),
    ServiceReconciliationRequired(PortError),
    CredentialReconciliationRequired(PortError),
    ReconciliationMarkerWrite {
        operation: PortError,
        marker: PortError,
    },
}

impl fmt::Debug for EnsureSessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReconciliationRequired => formatter.write_str("ReconciliationRequired"),
            Self::Service(error) => formatter.debug_tuple("Service").field(error).finish(),
            Self::ServiceReconciliationRequired(error) => formatter
                .debug_tuple("ServiceReconciliationRequired")
                .field(error)
                .finish(),
            Self::CredentialReconciliationRequired(error) => formatter
                .debug_tuple("CredentialReconciliationRequired")
                .field(error)
                .finish(),
            Self::ReconciliationMarkerWrite { operation, marker } => formatter
                .debug_struct("ReconciliationMarkerWrite")
                .field("operation", operation)
                .field("marker", marker)
                .finish(),
        }
    }
}

impl fmt::Display for EnsureSessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReconciliationRequired => formatter
                .write_str("credentials have an unresolved outcome; reconciliation is required"),
            Self::Service(error) => write!(formatter, "session refresh failed: {error}"),
            Self::ServiceReconciliationRequired(error) => write!(
                formatter,
                "session refresh outcome is uncertain and requires reconciliation: {error}"
            ),
            Self::CredentialReconciliationRequired(error) => write!(
                formatter,
                "session refresh succeeded but credential persistence requires reconciliation: {error}"
            ),
            Self::ReconciliationMarkerWrite { operation, marker } => write!(
                formatter,
                "session outcome requires reconciliation ({operation}) and its marker could not be persisted: {marker}"
            ),
        }
    }
}

impl std::error::Error for EnsureSessionError {}

pub struct EnsureSession {
    service: Arc<dyn ServicePort>,
    credentials: Arc<dyn CredentialPort>,
    clock: Arc<dyn ClockPort>,
}

impl EnsureSession {
    #[must_use]
    pub fn new(
        service: Arc<dyn ServicePort>,
        credentials: Arc<dyn CredentialPort>,
        clock: Arc<dyn ClockPort>,
    ) -> Self {
        Self {
            service,
            credentials,
            clock,
        }
    }

    pub async fn execute(
        &self,
        snapshot: SessionSnapshot,
        cancellation: CancellationToken,
    ) -> Result<EnsureSessionOutcome, EnsureSessionError> {
        if snapshot.reconciliation_required {
            return Err(EnsureSessionError::ReconciliationRequired);
        }
        if snapshot.credentials.expires_at_unix() > self.clock.unix_timestamp() {
            return Ok(EnsureSessionOutcome::Current(snapshot.credentials));
        }
        if cancellation.is_cancelled() {
            return Ok(EnsureSessionOutcome::CancelledBeforeDispatch);
        }

        let commit_id = CommitId::new();
        let request = RefreshRequest::from(&snapshot.credentials);
        let refreshed = match self
            .service
            .refresh_session(request, cancellation.child_token())
            .await
        {
            Ok(RefreshOutcome::Refreshed(result)) => result.into_rotated(),
            Ok(RefreshOutcome::CancelledBeforeDispatch) => {
                return Ok(EnsureSessionOutcome::CancelledBeforeDispatch);
            }
            Err(error) if error.outcome_uncertain => {
                self.mark_reconciliation(commit_id, error.clone()).await?;
                return Err(EnsureSessionError::ServiceReconciliationRequired(error));
            }
            Err(error) => return Err(EnsureSessionError::Service(error)),
        };

        // A successful refresh response is server-accepted durable state.
        // Cancellation cannot interrupt or undo the bounded credential commit.
        let commit = CredentialCommit {
            commit_id,
            expected_version: snapshot.credentials.version,
            credentials: refreshed.clone(),
        };
        if let Err(error) = self.credentials.commit(commit).await {
            self.mark_reconciliation(commit_id, error.clone()).await?;
            return Err(EnsureSessionError::CredentialReconciliationRequired(error));
        }
        self.credentials
            .clear_reconciliation_required(commit_id)
            .await
            .map_err(EnsureSessionError::CredentialReconciliationRequired)?;
        Ok(EnsureSessionOutcome::Refreshed(refreshed))
    }

    async fn mark_reconciliation(
        &self,
        commit_id: CommitId,
        operation: PortError,
    ) -> Result<(), EnsureSessionError> {
        self.credentials
            .mark_reconciliation_required(commit_id)
            .await
            .map_err(|marker| EnsureSessionError::ReconciliationMarkerWrite { operation, marker })
    }
}
