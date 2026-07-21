use std::sync::{Arc, Mutex};

use heyfood_application::{
    AcceptedTurn, BoxFuture, ClockPort, CredentialCommit, CredentialPort, EnsureSession,
    EnsureSessionError, EnsureSessionOutcome, PortError, ServicePort,
};
use heyfood_core::{
    AccountId, CredentialVersion, RefreshOutcome, RefreshRequest, RefreshResult, SensitiveString,
    SessionCredentials, SessionSnapshot,
};
use tokio_util::sync::CancellationToken;

fn credentials(version: u64, expires_at: i64) -> SessionCredentials {
    SessionCredentials::from_unix_expiry(
        AccountId::parse("ensure-session-account").unwrap(),
        SensitiveString::new(format!("access-{version}")),
        SensitiveString::new(format!("refresh-{version}")),
        CredentialVersion::new(version),
        expires_at,
    )
    .unwrap()
}

struct Clock(i64);

impl ClockPort for Clock {
    fn unix_timestamp(&self) -> i64 {
        self.0
    }
}

#[derive(Clone, Copy)]
enum ServiceBehavior {
    Refreshed { cancel_before_return: bool },
    Uncertain,
}

struct Service {
    calls: Mutex<usize>,
    behavior: ServiceBehavior,
}

impl ServicePort for Service {
    fn refresh_session(
        &self,
        request: RefreshRequest,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<RefreshOutcome, PortError>> {
        *self.calls.lock().unwrap() += 1;
        Box::pin(async move {
            match self.behavior {
                ServiceBehavior::Refreshed {
                    cancel_before_return,
                } => {
                    if cancel_before_return {
                        cancellation.cancel();
                    }
                    let rotated = credentials(request.current_version.next().get(), 5_000);
                    Ok(RefreshOutcome::Refreshed(
                        RefreshResult::validated(&request, rotated).unwrap(),
                    ))
                }
                ServiceBehavior::Uncertain => Err(PortError::uncertain(
                    "refresh_transport",
                    "response was not observed",
                )),
            }
        })
    }

    fn open_turn(
        &self,
        _request: heyfood_application::TurnRequest,
        _credentials: SessionCredentials,
        _operation_id: heyfood_core::OperationId,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<AcceptedTurn, PortError>> {
        Box::pin(async { Err(PortError::new("unused", "unused")) })
    }
}

#[derive(Default)]
struct CredentialStore {
    commits: Mutex<Vec<CredentialCommit>>,
    markers: Mutex<usize>,
    fail_commit: bool,
}

impl CredentialPort for CredentialStore {
    fn load(&self) -> BoxFuture<'_, Result<Option<SessionCredentials>, PortError>> {
        Box::pin(async { Ok(None) })
    }

    fn commit(&self, commit: CredentialCommit) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            if self.fail_commit {
                return Err(PortError::new("credential_write", "write failed"));
            }
            self.commits.lock().unwrap().push(commit);
            Ok(())
        })
    }

    fn mark_reconciliation_required(
        &self,
        _commit_id: heyfood_core::CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            *self.markers.lock().unwrap() += 1;
            Ok(())
        })
    }

    fn clear_reconciliation_required(
        &self,
        _commit_id: heyfood_core::CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn current_session_performs_no_network_or_write() {
    let service = Arc::new(Service {
        calls: Mutex::new(0),
        behavior: ServiceBehavior::Refreshed {
            cancel_before_return: false,
        },
    });
    let store = Arc::new(CredentialStore::default());
    let ensure = EnsureSession::new(service.clone(), store.clone(), Arc::new(Clock(100)));
    let outcome = ensure
        .execute(
            SessionSnapshot {
                credentials: credentials(1, 101),
                reconciliation_required: false,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(matches!(outcome, EnsureSessionOutcome::Current(_)));
    assert_eq!(*service.calls.lock().unwrap(), 0);
    assert!(store.commits.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancellation_after_refresh_acceptance_cannot_skip_rotation_commit() {
    let service = Arc::new(Service {
        calls: Mutex::new(0),
        behavior: ServiceBehavior::Refreshed {
            cancel_before_return: true,
        },
    });
    let store = Arc::new(CredentialStore::default());
    let ensure = EnsureSession::new(service, store.clone(), Arc::new(Clock(100)));
    let cancellation = CancellationToken::new();
    let outcome = ensure
        .execute(
            SessionSnapshot {
                credentials: credentials(1, 99),
                reconciliation_required: false,
            },
            cancellation.clone(),
        )
        .await
        .unwrap();
    assert!(matches!(outcome, EnsureSessionOutcome::Refreshed(_)));
    assert_eq!(store.commits.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn uncertain_refresh_marks_reconciliation_and_returns_no_credentials() {
    let service = Arc::new(Service {
        calls: Mutex::new(0),
        behavior: ServiceBehavior::Uncertain,
    });
    let store = Arc::new(CredentialStore::default());
    let ensure = EnsureSession::new(service, store.clone(), Arc::new(Clock(100)));
    let error = ensure
        .execute(
            SessionSnapshot {
                credentials: credentials(1, 99),
                reconciliation_required: false,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        EnsureSessionError::ServiceReconciliationRequired(_)
    ));
    assert_eq!(*store.markers.lock().unwrap(), 1);
    assert!(store.commits.lock().unwrap().is_empty());
}

#[tokio::test]
async fn accepted_rotation_write_failure_marks_reconciliation() {
    let service = Arc::new(Service {
        calls: Mutex::new(0),
        behavior: ServiceBehavior::Refreshed {
            cancel_before_return: false,
        },
    });
    let store = Arc::new(CredentialStore {
        fail_commit: true,
        ..Default::default()
    });
    let ensure = EnsureSession::new(service, store.clone(), Arc::new(Clock(100)));
    let error = ensure
        .execute(
            SessionSnapshot {
                credentials: credentials(1, 99),
                reconciliation_required: false,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        EnsureSessionError::CredentialReconciliationRequired(_)
    ));
    assert_eq!(*store.markers.lock().unwrap(), 1);
}
