use std::sync::Arc;
use std::time::Duration;

use heyfood_application::{
    BoxFuture, ConfigCommit, ConfigPort, CredentialCommit, CredentialPort, GroceryCacheKey,
    GroceryItemReferenceCache, OperationSupervisor, PortError, SerializedStateWriter,
    SupervisorError,
};
use heyfood_core::{
    AccountId, ClientConfig, ConfigRevision, ContextFingerprint, CredentialVersion,
    FrozenGroceryPreconditions, GenerationId, GroceryEntityId, GroceryListVersion, NetworkPolicy,
    SensitiveString, ServiceUrl, SessionCredentials, SessionSnapshot,
};

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
