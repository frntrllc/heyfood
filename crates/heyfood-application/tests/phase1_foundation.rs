use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use heyfood_application::{
    BoxFuture, CommitError, ConfigCommit, ConfigPort, CredentialCommit, CredentialPort,
    GroceryCacheKey, GroceryItemReferenceCache, MutationProposal, OperationSnapshot,
    OperationSupervisor, PortError, SerializedStateWriter, SupervisorError,
};
use heyfood_core::{
    AccountId, AgentEvent, ClientConfig, CommitId, ConfigRevision, ContextFingerprint,
    CredentialVersion, FrozenGroceryPreconditions, GenerationId, GroceryEntityId,
    GroceryListVersion, NetworkPolicy, OperationId, SensitiveString, ServiceUrl,
    SessionCredentials, SessionSnapshot,
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

fn credentials(account: &str) -> SessionCredentials {
    SessionCredentials::from_unix_expiry(
        AccountId::parse(account).unwrap(),
        SensitiveString::new("access-private"),
        SensitiveString::new("refresh-private"),
        CredentialVersion::new(1),
        4_102_444_800,
    )
    .unwrap()
}

fn config() -> ClientConfig {
    ClientConfig {
        active_context: "production".into(),
        api_url: ServiceUrl::parse("https://api.hello.food", NetworkPolicy::HTTPS_ONLY).unwrap(),
        auth_url: ServiceUrl::parse(
            "https://auth.hello.food/authorize",
            NetworkPolicy::HTTPS_ONLY,
        )
        .unwrap(),
        revision: ConfigRevision::new(1),
    }
}

struct MemoryCredentials(SessionCredentials);

impl CredentialPort for MemoryCredentials {
    fn load(&self) -> BoxFuture<'_, Result<Option<SessionCredentials>, PortError>> {
        Box::pin(async { Ok(Some(self.0.clone())) })
    }

    fn commit(&self, _commit: CredentialCommit) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async { Ok(()) })
    }

    fn mark_reconciliation_required(
        &self,
        _commit_id: heyfood_core::CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async { Ok(()) })
    }

    fn clear_reconciliation_required(
        &self,
        _commit_id: heyfood_core::CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async { Ok(()) })
    }
}

struct MemoryConfig(ClientConfig);

impl ConfigPort for MemoryConfig {
    fn load(&self) -> BoxFuture<'_, Result<ClientConfig, PortError>> {
        Box::pin(async { Ok(self.0.clone()) })
    }

    fn commit(&self, _commit: ConfigCommit) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async { Ok(()) })
    }

    fn mark_reconciliation_required(
        &self,
        _commit_id: heyfood_core::CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async { Ok(()) })
    }

    fn clear_reconciliation_required(
        &self,
        _commit_id: heyfood_core::CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Default)]
struct RepairConfig {
    fail: Mutex<Option<PortError>>,
    markers: Mutex<Vec<CommitId>>,
    clears: Mutex<Vec<CommitId>>,
}

impl ConfigPort for RepairConfig {
    fn load(&self) -> BoxFuture<'_, Result<ClientConfig, PortError>> {
        Box::pin(async { Ok(config()) })
    }

    fn commit(&self, _commit: ConfigCommit) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            if let Some(error) = self.fail.lock().unwrap().take() {
                return Err(error);
            }
            Ok(())
        })
    }

    fn mark_reconciliation_required(
        &self,
        commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            self.markers.lock().unwrap().push(commit_id);
            Ok(())
        })
    }

    fn clear_reconciliation_required(
        &self,
        commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            self.clears.lock().unwrap().push(commit_id);
            Ok(())
        })
    }
}

struct BlockingCredentials {
    current: SessionCredentials,
    entered: Notify,
    release: Notify,
}

impl CredentialPort for BlockingCredentials {
    fn load(&self) -> BoxFuture<'_, Result<Option<SessionCredentials>, PortError>> {
        Box::pin(async { Ok(Some(self.current.clone())) })
    }

    fn commit(&self, _commit: CredentialCommit) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(())
        })
    }

    fn mark_reconciliation_required(
        &self,
        _commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async { Ok(()) })
    }

    fn clear_reconciliation_required(
        &self,
        _commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Default)]
struct AtomicConfigGate {
    entered: Notify,
    applied: Notify,
    release: Notify,
    apply_before_release: bool,
    commits: Mutex<Vec<CommitId>>,
}

impl ConfigPort for AtomicConfigGate {
    fn load(&self) -> BoxFuture<'_, Result<ClientConfig, PortError>> {
        Box::pin(async { Ok(config()) })
    }

    fn commit(&self, commit: ConfigCommit) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            self.entered.notify_one();
            if self.apply_before_release {
                self.commits.lock().unwrap().push(commit.commit_id);
                self.applied.notify_one();
                self.release.notified().await;
            } else {
                self.release.notified().await;
                self.commits.lock().unwrap().push(commit.commit_id);
                self.applied.notify_one();
            }
            Ok(())
        })
    }

    fn mark_reconciliation_required(
        &self,
        _commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async { Ok(()) })
    }

    fn clear_reconciliation_required(
        &self,
        _commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async { Ok(()) })
    }
}

fn operation_snapshot() -> OperationSnapshot {
    OperationSnapshot {
        operation_id: OperationId::new(),
        generation: GenerationId::INITIAL,
        config: config(),
        session: SessionSnapshot {
            credentials: credentials("account-a"),
            reconciliation_required: false,
        },
    }
}

fn supervisor() -> Arc<OperationSupervisor> {
    let credentials = credentials("account-a");
    let writer = Arc::new(SerializedStateWriter::new(
        Arc::new(MemoryCredentials(credentials.clone())),
        Arc::new(MemoryConfig(config())),
        GenerationId::INITIAL,
        Some(&credentials),
    ));
    Arc::new(OperationSupervisor::new(writer))
}

#[tokio::test]
async fn supervisor_enforces_single_flight_and_advances_only_after_join() {
    let supervisor = supervisor();
    let lease = supervisor
        .begin(
            config(),
            SessionSnapshot {
                credentials: credentials("account-a"),
                reconciliation_required: false,
            },
        )
        .await
        .unwrap();
    let operation_id = lease.snapshot.operation_id;
    let cancellation = lease.cancellation.clone();
    assert_eq!(
        supervisor
            .begin(
                config(),
                SessionSnapshot {
                    credentials: credentials("account-a"),
                    reconciliation_required: false,
                },
            )
            .await
            .unwrap_err(),
        SupervisorError::WorkflowActive
    );

    let worker = tokio::spawn(async move {
        cancellation.cancelled().await;
        lease.finish();
    });
    supervisor
        .cancel_and_join(operation_id, Duration::from_secs(1))
        .await
        .unwrap();
    worker.await.unwrap();
    assert!(!supervisor.has_active_workflow().await);

    let next = supervisor
        .begin(
            config(),
            SessionSnapshot {
                credentials: credentials("account-a"),
                reconciliation_required: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(next.snapshot.generation, GenerationId::INITIAL.next());
    let next_id = next.snapshot.operation_id;
    next.finish();
    supervisor
        .join(next_id, Duration::from_secs(1))
        .await
        .unwrap();
}

#[tokio::test]
async fn supervisor_timeout_keeps_the_workflow_closed_to_replacement() {
    let supervisor = supervisor();
    let lease = supervisor
        .begin(
            config(),
            SessionSnapshot {
                credentials: credentials("account-a"),
                reconciliation_required: false,
            },
        )
        .await
        .unwrap();
    let operation_id = lease.snapshot.operation_id;
    assert_eq!(
        supervisor
            .cancel_and_join(operation_id, Duration::from_millis(1))
            .await
            .unwrap_err(),
        SupervisorError::JoinTimeout
    );
    assert!(supervisor.has_active_workflow().await);
    lease.finish();
    supervisor
        .join(operation_id, Duration::from_secs(1))
        .await
        .unwrap();
}

fn cache_key(account: &str, version: u64) -> GroceryCacheKey {
    let preconditions = FrozenGroceryPreconditions {
        list_id: GroceryEntityId::parse("00000000-0000-4000-8000-000000000001").unwrap(),
        list_version: GroceryListVersion::new(version).unwrap(),
        context_fingerprint: ContextFingerprint::parse("0123456789abcdef").unwrap(),
    };
    GroceryCacheKey::new(
        "https://api.hello.food",
        "production",
        AccountId::parse(account).unwrap(),
        &preconditions,
    )
    .unwrap()
}

#[test]
fn grocery_item_cache_is_exact_account_version_bound_and_expires() {
    let item = GroceryEntityId::parse("00000000-0000-4000-8000-000000000002").unwrap();
    let key = cache_key("account-a", 4);
    let mut cache = GroceryItemReferenceCache::default();
    cache.replace(key.clone(), 1_000, [(1, item)]);
    assert_eq!(cache.resolve(&key, 1_001, 1), Some(item));

    cache.replace(key.clone(), 2_000, [(1, item)]);
    assert_eq!(cache.resolve(&cache_key("account-b", 4), 2_001, 1), None);

    cache.replace(key.clone(), 3_000, [(1, item)]);
    assert_eq!(cache.resolve(&cache_key("account-a", 5), 3_001, 1), None);

    cache.replace(key.clone(), 4_000, [(1, item)]);
    assert_eq!(
        cache.resolve(&key, 4_000 + GroceryItemReferenceCache::LIFETIME_SECONDS, 1),
        None
    );
}

#[tokio::test]
async fn account_bound_proposal_fails_closed_when_writer_is_unbound() {
    let writer = SerializedStateWriter::new(
        Arc::new(MemoryCredentials(credentials("account-a"))),
        Arc::new(MemoryConfig(config())),
        GenerationId::INITIAL,
        None,
    );
    let error = writer
        .commit(MutationProposal::presentation(
            &operation_snapshot(),
            AgentEvent::Partial {
                text: "private".into(),
            },
        ))
        .await
        .unwrap_err();
    assert_eq!(
        error,
        CommitError::InvalidProposal("state writer is not bound to the proposal account")
    );
}

#[tokio::test]
async fn accepted_config_failure_persists_exact_reconciliation_marker() {
    let snapshot = operation_snapshot();
    let config_port = Arc::new(RepairConfig::default());
    *config_port.fail.lock().unwrap() = Some(PortError::new(
        "config_write",
        "accepted config could not be stored",
    ));
    let writer = SerializedStateWriter::new(
        Arc::new(MemoryCredentials(snapshot.session.credentials.clone())),
        config_port.clone(),
        GenerationId::INITIAL,
        Some(&snapshot.session.credentials),
    );
    let commit_id = CommitId::new();
    let mut replacement = config();
    replacement.revision = ConfigRevision::new(2);
    assert!(matches!(
        writer
            .commit(MutationProposal::accepted_config(
                &snapshot,
                commit_id,
                replacement,
            ))
            .await,
        Err(CommitError::ReconciliationRequired(_))
    ));
    assert_eq!(*config_port.markers.lock().unwrap(), vec![commit_id]);
}

#[tokio::test]
async fn uncertain_local_first_failure_persists_repair_marker() {
    let snapshot = operation_snapshot();
    let config_port = Arc::new(RepairConfig::default());
    *config_port.fail.lock().unwrap() = Some(PortError::uncertain(
        "atomic_replace",
        "record may have been durably renamed",
    ));
    let writer = SerializedStateWriter::new(
        Arc::new(MemoryCredentials(snapshot.session.credentials.clone())),
        config_port.clone(),
        GenerationId::INITIAL,
        Some(&snapshot.session.credentials),
    );
    let commit_id = CommitId::new();
    assert!(matches!(
        writer
            .commit(MutationProposal::local_first(
                &snapshot,
                commit_id,
                "repair",
                vec![1],
            ))
            .await,
        Err(CommitError::ReconciliationRequired(_))
    ));
    assert_eq!(*config_port.markers.lock().unwrap(), vec![commit_id]);
}

#[tokio::test]
async fn cancellation_while_accepted_config_is_queued_behind_writer_does_not_drop_it() {
    let snapshot = operation_snapshot();
    let credentials_port = Arc::new(BlockingCredentials {
        current: snapshot.session.credentials.clone(),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let writer = Arc::new(SerializedStateWriter::new(
        credentials_port.clone(),
        Arc::new(MemoryConfig(config())),
        GenerationId::INITIAL,
        Some(&snapshot.session.credentials),
    ));
    let first = {
        let writer = writer.clone();
        let proposal = MutationProposal::credential_rotation(
            &snapshot,
            CommitId::new(),
            SessionCredentials::from_unix_expiry(
                snapshot.session.credentials.account_id.clone(),
                SensitiveString::new("rotated-access"),
                SensitiveString::new("rotated-refresh"),
                CredentialVersion::new(2),
                4_102_444_800,
            )
            .unwrap(),
        );
        tokio::spawn(async move { writer.commit(proposal).await })
    };
    credentials_port.entered.notified().await;

    let cancellation = CancellationToken::new();
    let mut replacement = config();
    replacement.revision = ConfigRevision::new(2);
    let second = {
        let writer = writer.clone();
        let proposal = MutationProposal::accepted_config(&snapshot, CommitId::new(), replacement);
        tokio::spawn(async move { writer.commit(proposal).await })
    };
    cancellation.cancel();
    tokio::task::yield_now().await;
    assert!(!second.is_finished());

    credentials_port.release.notify_one();
    assert_eq!(
        first.await.unwrap().unwrap(),
        heyfood_application::CommitOutcome::Applied
    );
    assert_eq!(
        second.await.unwrap().unwrap(),
        heyfood_application::CommitOutcome::Applied
    );
}

#[tokio::test]
async fn cancellation_during_atomic_adapter_replacement_waits_for_durable_result() {
    let snapshot = operation_snapshot();
    let config_port = Arc::new(AtomicConfigGate {
        apply_before_release: false,
        ..AtomicConfigGate::default()
    });
    let writer = Arc::new(SerializedStateWriter::new(
        Arc::new(MemoryCredentials(snapshot.session.credentials.clone())),
        config_port.clone(),
        GenerationId::INITIAL,
        Some(&snapshot.session.credentials),
    ));
    let commit_id = CommitId::new();
    let commit = {
        let writer = writer.clone();
        let proposal = MutationProposal::local_first(&snapshot, commit_id, "repair", vec![1, 2, 3]);
        tokio::spawn(async move { writer.commit(proposal).await })
    };
    config_port.entered.notified().await;
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert!(!commit.is_finished());
    config_port.release.notify_one();
    assert_eq!(
        commit.await.unwrap().unwrap(),
        heyfood_application::CommitOutcome::Applied
    );
    assert_eq!(*config_port.commits.lock().unwrap(), vec![commit_id]);
}

#[tokio::test]
async fn cancellation_immediately_after_adapter_commit_cannot_undo_durable_state() {
    let snapshot = operation_snapshot();
    let config_port = Arc::new(AtomicConfigGate {
        apply_before_release: true,
        ..AtomicConfigGate::default()
    });
    let writer = Arc::new(SerializedStateWriter::new(
        Arc::new(MemoryCredentials(snapshot.session.credentials.clone())),
        config_port.clone(),
        GenerationId::INITIAL,
        Some(&snapshot.session.credentials),
    ));
    let commit_id = CommitId::new();
    let commit = {
        let writer = writer.clone();
        let proposal = MutationProposal::local_first(&snapshot, commit_id, "repair", vec![1, 2, 3]);
        tokio::spawn(async move { writer.commit(proposal).await })
    };
    config_port.applied.notified().await;
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert_eq!(*config_port.commits.lock().unwrap(), vec![commit_id]);
    assert!(!commit.is_finished());
    config_port.release.notify_one();
    assert_eq!(
        commit.await.unwrap().unwrap(),
        heyfood_application::CommitOutcome::Applied
    );
}
