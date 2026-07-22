use std::future::Future;
use std::path::{Path, PathBuf};
#[cfg(not(windows))]
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(not(windows))]
use heyfood_application::{
    CommitOutcome, MutationProposal, OperationSnapshot, SerializedStateWriter,
};
use heyfood_application::{ConfigCommit, ConfigMutation, ConfigPort};
#[cfg(any(not(windows), feature = "native-credentials"))]
use heyfood_application::{CredentialCommit, CredentialPort};
#[cfg(any(not(windows), feature = "native-credentials"))]
use heyfood_core::{AccountId, CredentialVersion, SensitiveString, SessionCredentials};
#[cfg(any(not(windows), feature = "native-credentials"))]
use heyfood_core::{AuthCredentialBundle, ChannelCredentials};
use heyfood_core::{ClientConfig, CommitId, ConfigRevision, NetworkPolicy, ServiceUrl};
#[cfg(not(windows))]
use heyfood_core::{GenerationId, OperationId, SessionSnapshot};
#[cfg(not(windows))]
use heyfood_platform::AuthorizationSessionStore;
#[cfg(all(not(windows), feature = "native-credentials"))]
use heyfood_platform::KeyringCredentialStore;
#[cfg(any(not(windows), feature = "native-credentials"))]
use heyfood_platform::NativeAuthStore;
#[cfg(all(windows, feature = "native-credentials"))]
use heyfood_platform::WindowsCredentialStore;
use heyfood_platform::{AtomicFile, FileCredentialStore, NativeConfigStore};
#[cfg(any(not(windows), feature = "native-credentials"))]
use heyfood_platform::{AuthorizationReplacementJournal, AuthorizationReplacementPhase};

#[cfg(not(windows))]
type AuthorizationTestSessionStore = FileCredentialStore;
#[cfg(all(windows, feature = "native-credentials"))]
type AuthorizationTestSessionStore = WindowsCredentialStore;

fn block_on<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "heyfood-platform-{name}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn config(context: &str, revision: u64) -> ClientConfig {
    ClientConfig {
        active_context: context.into(),
        api_url: ServiceUrl::parse("http://127.0.0.1:8000", NetworkPolicy::DEVELOPMENT).unwrap(),
        auth_url: ServiceUrl::parse("http://localhost:8000", NetworkPolicy::DEVELOPMENT).unwrap(),
        revision: ConfigRevision::new(revision),
    }
}

#[cfg(any(not(windows), feature = "native-credentials"))]
fn credentials(version: u64) -> SessionCredentials {
    SessionCredentials::from_unix_expiry(
        AccountId::parse("account-fixture").unwrap(),
        SensitiveString::new(format!("access-{version}")),
        SensitiveString::new(format!("refresh-{version}")),
        CredentialVersion::new(version),
        4_102_444_800,
    )
    .unwrap()
}

#[cfg(any(not(windows), feature = "native-credentials"))]
fn auth_bundle() -> AuthCredentialBundle {
    AuthCredentialBundle {
        channel: ChannelCredentials::from_unix_expiry(
            "hf_cid_heyfood_cli",
            "heyfood-device-fixture",
            SensitiveString::new("channel-access"),
            SensitiveString::new("channel-refresh"),
            4_102_444_800,
            "account:link profile:read",
        )
        .unwrap(),
        session: credentials(1),
    }
}

#[test]
#[cfg(all(windows, feature = "native-credentials"))]
fn complete_auth_bundle_uses_windows_credential_manager_and_refuses_overwrite() {
    let root = TempRoot::new("windows-auth-bundle");
    let store = NativeAuthStore::open(&root.0).unwrap();
    let expected = auth_bundle();
    store.initialize(&expected).unwrap();
    assert_eq!(store.load().unwrap(), Some(expected));
    assert_eq!(
        store.initialize(&auth_bundle()).unwrap_err().code,
        "auth_exists"
    );
    let mut replacement = auth_bundle();
    replacement.channel.access_token = SensitiveString::new("channel-access-rotated");
    store.replace(&replacement).unwrap();
    assert_eq!(store.load().unwrap(), Some(replacement));
    assert!(!root.0.join("auth.native").exists());
    store.delete().unwrap();
    assert!(store.load().unwrap().is_none());
}

#[test]
#[cfg(not(windows))]
fn complete_auth_bundle_is_atomic_owner_only_and_refuses_overwrite() {
    let root = TempRoot::new("auth-bundle");
    let store = NativeAuthStore::open(&root.0).unwrap();
    let expected = auth_bundle();
    store.initialize(&expected).unwrap();
    assert_eq!(store.load().unwrap(), Some(expected));
    assert_eq!(
        store.initialize(&auth_bundle()).unwrap_err().code,
        "auth_exists"
    );
    let mut replacement = auth_bundle();
    replacement.channel.access_token = SensitiveString::new("channel-access-rotated");
    store.replace(&replacement).unwrap();
    assert_eq!(store.load().unwrap(), Some(replacement));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(root.0.join("auth.native"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
#[cfg(not(windows))]
fn account_bound_initialization_commits_and_verifies_both_stores() {
    let root = TempRoot::new("account-bound-initialize");
    let auth_store = NativeAuthStore::open(&root.0).unwrap();
    let session_store = FileCredentialStore::open(&root.0).unwrap();
    let expected = auth_bundle();

    auth_store
        .initialize_account_bound(&expected, &session_store)
        .unwrap();

    assert_eq!(
        auth_store.load_account_bound(&session_store).unwrap(),
        Some(expected)
    );
    assert!(!root.0.join("auth.reconciliation").exists());
}

#[test]
#[cfg(not(windows))]
fn account_bound_load_blocks_cross_account_state_with_a_durable_marker() {
    let root = TempRoot::new("account-bound-conflict");
    let auth_store = NativeAuthStore::open(&root.0).unwrap();
    let session_store = FileCredentialStore::open(&root.0).unwrap();
    auth_store.initialize(&auth_bundle()).unwrap();
    let other_session = SessionCredentials::from_unix_expiry(
        AccountId::parse("different-account").unwrap(),
        SensitiveString::new("other-access"),
        SensitiveString::new("other-refresh"),
        CredentialVersion::new(1),
        4_102_444_800,
    )
    .unwrap();
    session_store.initialize(&other_session).unwrap();

    let error = auth_store.load_account_bound(&session_store).unwrap_err();
    assert_eq!(error.code, "authorization_account_conflict");
    assert!(error.outcome_uncertain);
    assert_eq!(
        std::fs::read(root.0.join("auth.reconciliation")).unwrap(),
        b"account_binding_conflict\n"
    );
    assert_eq!(
        auth_store
            .load_account_bound(&session_store)
            .unwrap_err()
            .code,
        "auth_reconciliation_required"
    );
}

#[test]
#[cfg(not(windows))]
fn account_bound_load_recovers_a_legacy_missing_session_before_use() {
    let root = TempRoot::new("account-bound-legacy");
    let auth_store = NativeAuthStore::open(&root.0).unwrap();
    let session_store = FileCredentialStore::open(&root.0).unwrap();
    let expected = auth_bundle();
    auth_store.initialize(&expected).unwrap();

    assert_eq!(
        auth_store.load_account_bound(&session_store).unwrap(),
        Some(expected.clone())
    );
    assert_eq!(
        block_on(session_store.load()).unwrap(),
        Some(expected.session)
    );
    assert!(!root.0.join("auth.reconciliation").exists());
}

#[test]
#[cfg(not(windows))]
fn channel_refresh_transaction_is_single_flight_and_reconciliation_is_durable() {
    let root = TempRoot::new("auth-refresh-transaction");
    let store = Arc::new(NativeAuthStore::open(&root.0).unwrap());
    store.initialize(&auth_bundle()).unwrap();

    let refresh = store.begin_refresh().unwrap();
    assert_eq!(refresh.load().unwrap(), Some(auth_bundle()));
    let contender = Arc::clone(&store);
    let blocked = std::thread::spawn(move || match contender.begin_refresh() {
        Ok(_) => panic!("concurrent refresh unexpectedly acquired the auth lock"),
        Err(error) => error,
    })
    .join()
    .unwrap();
    assert_eq!(blocked.code, "lock_timeout");

    refresh.mark_reconciliation_required().unwrap();
    drop(refresh);
    let unresolved = store.load().unwrap_err();
    assert_eq!(unresolved.code, "auth_reconciliation_required");
    assert!(unresolved.outcome_uncertain);

    let refresh = store.begin_refresh().unwrap();
    let mut replacement = auth_bundle();
    replacement.channel.refresh_token = SensitiveString::new("channel-refresh-rotated");
    refresh.replace(&replacement).unwrap();
    drop(refresh);
    assert_eq!(store.load().unwrap(), Some(replacement));
    assert!(!root.0.join("auth.reconciliation").exists());
}

#[test]
#[cfg(any(not(windows), feature = "native-credentials"))]
fn staged_reauthorization_keeps_old_active_then_replaces_both_stores_after_promotion() {
    let root = TempRoot::new("authorization-replacement");
    let auth_store = NativeAuthStore::open(&root.0).unwrap();
    let session_store = AuthorizationTestSessionStore::open(&root.0).unwrap();
    #[cfg(windows)]
    {
        let _ = auth_store.delete();
        let _ = session_store.delete();
    }
    let current = auth_bundle();
    auth_store.initialize(&current).unwrap();
    session_store.initialize(&current.session).unwrap();

    let mut replacement = current.clone();
    replacement.channel.access_token = SensitiveString::new("expanded-channel-access");
    replacement.channel.refresh_token = SensitiveString::new("expanded-channel-refresh");
    replacement.channel.scope =
        "account:link profile:read health:read integrations:manage grocery:read grocery:write"
            .into();
    replacement.session = credentials(2);

    let client_transaction_id = "client-transaction-combined".to_owned();
    auth_store
        .begin_authorization_replacement(client_transaction_id.clone(), &session_store)
        .unwrap();
    let preparing = auth_store
        .record_provisional_authorization(
            &client_transaction_id,
            "authorization-transaction-combined".to_owned(),
            SensitiveString::new("provisional-access-token-combined"),
        )
        .unwrap();
    let prepared = AuthorizationReplacementJournal {
        phase: AuthorizationReplacementPhase::Prepared,
        client_transaction_id: client_transaction_id.clone(),
        stage_id: Some("stage-transaction-combined".to_owned()),
        authorization_transaction_id: Some("authorization-transaction-combined".to_owned()),
        provisional_access_token: None,
        recovery_token: Some(SensitiveString::new("recovery-token-combined")),
        bundle_digest: Some("a".repeat(64)),
        previous: preparing.previous,
        replacement: Some(replacement.clone()),
    };
    auth_store
        .stage_authorization_replacement(prepared, &session_store)
        .unwrap();
    assert!(auth_store.load().is_err(), "ordinary auth load must block");
    assert_eq!(
        block_on(session_store.load()).unwrap(),
        Some(current.session.clone()),
        "pending session must not become active before promotion"
    );
    auth_store
        .mark_authorization_promotion_dispatched(&client_transaction_id, &session_store)
        .unwrap();
    auth_store
        .finalize_promoted_authorization(&client_transaction_id, &session_store)
        .unwrap();
    assert_eq!(auth_store.load().unwrap(), Some(replacement.clone()));
    assert_eq!(
        block_on(session_store.load()).unwrap(),
        Some(replacement.session)
    );
    assert!(!root.0.join("auth.reconciliation").exists());
    #[cfg(windows)]
    {
        auth_store.delete().unwrap();
        session_store.delete().unwrap();
    }
}

#[test]
#[cfg(not(windows))]
fn partial_pending_session_write_replays_exact_prepared_journal() {
    struct FailingSessionStore;
    impl AuthorizationSessionStore for FailingSessionStore {
        fn load_authorized_session(
            &self,
        ) -> Result<Option<SessionCredentials>, heyfood_application::PortError> {
            Ok(Some(credentials(1)))
        }

        fn replace_authorized_session(
            &self,
            _credentials: &SessionCredentials,
        ) -> Result<(), heyfood_application::PortError> {
            unreachable!()
        }

        fn stage_authorized_session(
            &self,
            _client_transaction_id: &str,
            _previous: &SessionCredentials,
            _replacement: &SessionCredentials,
        ) -> Result<(), heyfood_application::PortError> {
            Err(heyfood_application::PortError::new(
                "fixture_write_failure",
                "controlled failure",
            ))
        }

        fn verify_staged_authorized_session(
            &self,
            _client_transaction_id: &str,
            _previous: &SessionCredentials,
            _replacement: &SessionCredentials,
        ) -> Result<(), heyfood_application::PortError> {
            Err(heyfood_application::PortError::new(
                "fixture_write_failure",
                "controlled failure",
            ))
        }

        fn clear_staged_authorized_session(
            &self,
            _client_transaction_id: &str,
            _expected_replacement: &SessionCredentials,
        ) -> Result<(), heyfood_application::PortError> {
            Ok(())
        }
    }

    let root = TempRoot::new("authorization-partial-write");
    let auth_store = NativeAuthStore::open(&root.0).unwrap();
    let session_store = FileCredentialStore::open(&root.0).unwrap();
    let current = auth_bundle();
    auth_store.initialize(&current).unwrap();
    session_store.initialize(&current.session).unwrap();
    let mut replacement = current.clone();
    replacement.channel.scope =
        "account:link profile:read health:read integrations:manage grocery:read grocery:write"
            .into();
    replacement.session = credentials(2);

    let client_transaction_id = "client-transaction-partial".to_owned();
    auth_store
        .begin_authorization_replacement(client_transaction_id.clone(), &session_store)
        .unwrap();
    let preparing = auth_store
        .record_provisional_authorization(
            &client_transaction_id,
            "authorization-transaction-partial".to_owned(),
            SensitiveString::new("provisional-access-token-partial"),
        )
        .unwrap();
    let prepared = AuthorizationReplacementJournal {
        phase: AuthorizationReplacementPhase::Prepared,
        client_transaction_id: client_transaction_id.clone(),
        stage_id: Some("stage-transaction-partial".to_owned()),
        authorization_transaction_id: preparing.authorization_transaction_id,
        provisional_access_token: None,
        recovery_token: Some(SensitiveString::new("recovery-token-partial")),
        bundle_digest: Some("b".repeat(64)),
        previous: preparing.previous,
        replacement: Some(replacement),
    };
    let error = auth_store
        .stage_authorization_replacement(prepared.clone(), &FailingSessionStore)
        .unwrap_err();
    assert_eq!(error.code, "fixture_write_failure");
    assert_eq!(
        auth_store.pending_authorization_replacement().unwrap(),
        Some(prepared.clone()),
        "complete prepared journal must survive second-store failure"
    );
    auth_store
        .stage_authorization_replacement(prepared, &session_store)
        .unwrap();
}

#[test]
#[cfg(not(windows))]
fn aborted_stage_preserves_authoritative_session_rotations_instead_of_restoring_stale_mirror() {
    let root = TempRoot::new("authorization-abort-session-race");
    let auth_store = NativeAuthStore::open(&root.0).unwrap();
    let session_store = FileCredentialStore::open(&root.0).unwrap();
    let current = auth_bundle();
    auth_store.initialize(&current).unwrap();
    session_store.initialize(&current.session).unwrap();

    block_on(session_store.commit(CredentialCommit {
        commit_id: CommitId::new(),
        expected_version: CredentialVersion::new(1),
        credentials: credentials(2),
    }))
    .unwrap();
    let client_transaction_id = "client-transaction-session-race".to_owned();
    let preparing = auth_store
        .begin_authorization_replacement(client_transaction_id.clone(), &session_store)
        .unwrap();
    assert_eq!(preparing.previous.session, credentials(2));
    assert_ne!(preparing.previous.session, current.session);
    let preparing = auth_store
        .record_provisional_authorization(
            &client_transaction_id,
            "authorization-transaction-session-race".to_owned(),
            SensitiveString::new("provisional-access-token-session-race"),
        )
        .unwrap();
    let mut replacement = preparing.previous.clone();
    replacement.channel.scope =
        "account:link profile:read health:read integrations:manage grocery:read grocery:write"
            .into();
    replacement.session = credentials(9);
    let prepared = AuthorizationReplacementJournal {
        phase: AuthorizationReplacementPhase::Prepared,
        client_transaction_id: client_transaction_id.clone(),
        stage_id: Some("stage-transaction-session-race".to_owned()),
        authorization_transaction_id: preparing.authorization_transaction_id,
        provisional_access_token: None,
        recovery_token: Some(SensitiveString::new("recovery-token-session-race")),
        bundle_digest: Some("c".repeat(64)),
        previous: preparing.previous,
        replacement: Some(replacement),
    };
    auth_store
        .stage_authorization_replacement(prepared.clone(), &session_store)
        .unwrap();
    auth_store
        .mark_authorization_abort_dispatched(prepared)
        .unwrap();

    // A refresh dispatched before the journal fence may legitimately finish
    // afterward. Aborted cleanup must retain it, never restore version 2.
    block_on(session_store.commit(CredentialCommit {
        commit_id: CommitId::new(),
        expected_version: CredentialVersion::new(2),
        credentials: credentials(3),
    }))
    .unwrap();
    auth_store
        .finalize_unpromoted_authorization(&client_transaction_id, &session_store)
        .unwrap();
    assert_eq!(
        block_on(session_store.load()).unwrap(),
        Some(credentials(3))
    );
    assert_eq!(auth_store.load().unwrap().unwrap().channel, current.channel);
}

#[test]
#[cfg(not(windows))]
fn promoted_activation_replays_after_split_auth_and_session_write() {
    struct FailActivation<'a>(&'a FileCredentialStore);
    impl AuthorizationSessionStore for FailActivation<'_> {
        fn load_authorized_session(
            &self,
        ) -> Result<Option<SessionCredentials>, heyfood_application::PortError> {
            self.0.load_authorized_session()
        }
        fn replace_authorized_session(
            &self,
            _credentials: &SessionCredentials,
        ) -> Result<(), heyfood_application::PortError> {
            Err(heyfood_application::PortError::new(
                "fixture_activation_failure",
                "controlled failure",
            ))
        }
        fn stage_authorized_session(
            &self,
            client_transaction_id: &str,
            previous: &SessionCredentials,
            replacement: &SessionCredentials,
        ) -> Result<(), heyfood_application::PortError> {
            self.0
                .stage_authorized_session(client_transaction_id, previous, replacement)
        }
        fn verify_staged_authorized_session(
            &self,
            client_transaction_id: &str,
            previous: &SessionCredentials,
            replacement: &SessionCredentials,
        ) -> Result<(), heyfood_application::PortError> {
            self.0
                .verify_staged_authorized_session(client_transaction_id, previous, replacement)
        }
        fn clear_staged_authorized_session(
            &self,
            client_transaction_id: &str,
            expected_replacement: &SessionCredentials,
        ) -> Result<(), heyfood_application::PortError> {
            self.0
                .clear_staged_authorized_session(client_transaction_id, expected_replacement)
        }
    }

    let root = TempRoot::new("authorization-promoted-split");
    let auth_store = NativeAuthStore::open(&root.0).unwrap();
    let session_store = FileCredentialStore::open(&root.0).unwrap();
    let current = auth_bundle();
    auth_store.initialize(&current).unwrap();
    session_store.initialize(&current.session).unwrap();
    let client_transaction_id = "client-transaction-promoted-split".to_owned();
    auth_store
        .begin_authorization_replacement(client_transaction_id.clone(), &session_store)
        .unwrap();
    let preparing = auth_store
        .record_provisional_authorization(
            &client_transaction_id,
            "authorization-transaction-promoted-split".to_owned(),
            SensitiveString::new("provisional-access-promoted-split"),
        )
        .unwrap();
    let mut replacement = preparing.previous.clone();
    replacement.channel.access_token = SensitiveString::new("promoted-channel-access");
    replacement.channel.scope =
        "account:link profile:read health:read integrations:manage grocery:read grocery:write"
            .into();
    replacement.session = credentials(8);
    let prepared = AuthorizationReplacementJournal {
        phase: AuthorizationReplacementPhase::Prepared,
        client_transaction_id: client_transaction_id.clone(),
        stage_id: Some("stage-transaction-promoted-split".to_owned()),
        authorization_transaction_id: preparing.authorization_transaction_id,
        provisional_access_token: None,
        recovery_token: Some(SensitiveString::new("recovery-token-promoted-split")),
        bundle_digest: Some("d".repeat(64)),
        previous: preparing.previous,
        replacement: Some(replacement.clone()),
    };
    auth_store
        .stage_authorization_replacement(prepared, &session_store)
        .unwrap();
    auth_store
        .mark_authorization_promotion_dispatched(&client_transaction_id, &session_store)
        .unwrap();
    let error = auth_store
        .finalize_promoted_authorization(&client_transaction_id, &FailActivation(&session_store))
        .unwrap_err();
    assert_eq!(error.code, "authorization_session_replace");
    assert!(auth_store.load().is_err());
    auth_store
        .finalize_promoted_authorization(&client_transaction_id, &session_store)
        .unwrap();
    assert_eq!(auth_store.load().unwrap(), Some(replacement.clone()));
    assert_eq!(
        block_on(session_store.load()).unwrap(),
        Some(replacement.session)
    );
}

#[test]
#[cfg(not(windows))]
fn credential_rotation_is_versioned_idempotent_and_owner_only() {
    let root = TempRoot::new("credentials");
    let store = FileCredentialStore::open(&root.0).unwrap();
    store.initialize(&credentials(1)).unwrap();
    let commit_id = CommitId::new();
    let commit = CredentialCommit {
        commit_id,
        expected_version: CredentialVersion::new(1),
        credentials: credentials(2),
    };
    block_on(store.commit(commit.clone())).unwrap();
    block_on(store.commit(commit)).unwrap();
    let loaded = block_on(store.load()).unwrap().unwrap();
    assert_eq!(loaded.version, CredentialVersion::new(2));
    assert_eq!(loaded.refresh_token.expose_secret(), "refresh-2");

    let stale = CredentialCommit {
        commit_id: CommitId::new(),
        expected_version: CredentialVersion::new(1),
        credentials: credentials(3),
    };
    assert_eq!(
        block_on(store.commit(stale)).unwrap_err().code,
        "credential_version_conflict"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(root.0.join("credentials.native"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(&root.0).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}

#[test]
#[cfg(not(windows))]
fn reconciliation_marker_is_durable_and_cleared_by_verified_rotation() {
    let root = TempRoot::new("reconciliation");
    let store = FileCredentialStore::open(&root.0).unwrap();
    store.initialize(&credentials(1)).unwrap();
    let commit_id = CommitId::new();
    block_on(store.mark_reconciliation_required(commit_id)).unwrap();
    assert!(store.reconciliation_required().unwrap());
    block_on(store.commit(CredentialCommit {
        commit_id,
        expected_version: CredentialVersion::new(1),
        credentials: credentials(2),
    }))
    .unwrap();
    assert!(!store.reconciliation_required().unwrap());
}

#[test]
#[cfg(not(windows))]
fn new_login_fails_closed_until_stale_reconciliation_is_explicitly_cleared() {
    let root = TempRoot::new("reconciliation-new-login");
    let store = FileCredentialStore::open(&root.0).unwrap();
    let commit_id = CommitId::new();
    block_on(store.mark_reconciliation_required(commit_id)).unwrap();
    assert!(store.reconciliation_required().unwrap());
    assert_eq!(
        store.initialize(&credentials(1)).unwrap_err().code,
        "credential_reconciliation_required"
    );
    block_on(store.clear_reconciliation_required(commit_id)).unwrap();
    store.initialize(&credentials(1)).unwrap();
    assert!(!store.reconciliation_required().unwrap());
}

#[test]
#[cfg(not(windows))]
fn verified_logout_allows_an_explicit_fallback_account_switch() {
    let root = TempRoot::new("fallback-account-switch");
    let store = FileCredentialStore::open(&root.0).unwrap();
    store.initialize(&credentials(1)).unwrap();
    let commit_id = CommitId::new();
    block_on(store.mark_reconciliation_required(commit_id)).unwrap();
    assert_eq!(
        store.delete().unwrap_err().code,
        "credential_reconciliation_required"
    );
    block_on(store.clear_reconciliation_required(commit_id)).unwrap();
    store.delete().unwrap();
    assert!(!store.reconciliation_required().unwrap());
    assert!(block_on(store.load()).unwrap().is_none());

    let replacement = SessionCredentials::from_unix_expiry(
        AccountId::parse("different-account").unwrap(),
        SensitiveString::new("replacement-access"),
        SensitiveString::new("replacement-refresh"),
        CredentialVersion::new(1),
        4_102_444_800,
    )
    .unwrap();
    store.initialize(&replacement).unwrap();
    assert_eq!(
        block_on(store.load()).unwrap().unwrap().account_id,
        replacement.account_id
    );
}

#[test]
#[cfg(not(windows))]
fn unrelated_rotation_preserves_another_commits_reconciliation_marker() {
    let root = TempRoot::new("reconciliation-unrelated");
    let store = FileCredentialStore::open(&root.0).unwrap();
    store.initialize(&credentials(1)).unwrap();
    block_on(store.mark_reconciliation_required(CommitId::new())).unwrap();

    block_on(store.commit(CredentialCommit {
        commit_id: CommitId::new(),
        expected_version: CredentialVersion::new(1),
        credentials: credentials(2),
    }))
    .unwrap();

    assert!(store.reconciliation_required().unwrap());
}

#[test]
#[cfg(not(windows))]
fn reconciliation_clear_is_idempotent_and_commit_specific() {
    let root = TempRoot::new("reconciliation-specific-clear");
    let store = FileCredentialStore::open(&root.0).unwrap();
    let marked = CommitId::new();
    block_on(store.mark_reconciliation_required(marked)).unwrap();

    block_on(store.clear_reconciliation_required(CommitId::new())).unwrap();
    assert!(store.reconciliation_required().unwrap());
    block_on(store.clear_reconciliation_required(marked)).unwrap();
    block_on(store.clear_reconciliation_required(marked)).unwrap();

    assert!(!store.reconciliation_required().unwrap());
}

#[test]
#[cfg(not(windows))]
fn idempotent_replay_clears_a_stale_reconciliation_marker() {
    let root = TempRoot::new("reconciliation-replay");
    let store = FileCredentialStore::open(&root.0).unwrap();
    store.initialize(&credentials(1)).unwrap();
    let commit = CredentialCommit {
        commit_id: CommitId::new(),
        expected_version: CredentialVersion::new(1),
        credentials: credentials(2),
    };
    block_on(store.commit(commit.clone())).unwrap();
    block_on(store.mark_reconciliation_required(commit.commit_id)).unwrap();
    assert!(store.reconciliation_required().unwrap());

    block_on(store.commit(commit)).unwrap();

    assert!(!store.reconciliation_required().unwrap());
}

#[test]
#[cfg(not(windows))]
fn state_writer_replays_credential_commit_after_process_restart() {
    let root = TempRoot::new("writer-restart-replay");
    let credential_store = Arc::new(FileCredentialStore::open(&root.0).unwrap());
    credential_store.initialize(&credentials(1)).unwrap();
    let config_store = Arc::new(
        NativeConfigStore::open(&root.0, config("fixture", 1), NetworkPolicy::DEVELOPMENT).unwrap(),
    );
    let operation = OperationSnapshot {
        operation_id: OperationId::new(),
        generation: GenerationId::INITIAL,
        config: config("fixture", 1),
        session: SessionSnapshot {
            credentials: credentials(1),
            reconciliation_required: false,
        },
    };
    let proposal =
        MutationProposal::credential_rotation(&operation, CommitId::new(), credentials(2));
    let first_writer = SerializedStateWriter::new(
        credential_store.clone(),
        config_store.clone(),
        GenerationId::INITIAL,
        Some(&credentials(1)),
    );
    assert_eq!(
        block_on(first_writer.commit(proposal.clone())).unwrap(),
        CommitOutcome::Applied
    );

    let persisted = block_on(credential_store.load()).unwrap().unwrap();
    let restarted_writer = SerializedStateWriter::new(
        credential_store.clone(),
        config_store,
        GenerationId::INITIAL,
        Some(&persisted),
    );
    assert_eq!(
        block_on(restarted_writer.commit(proposal)).unwrap(),
        CommitOutcome::Applied
    );
    assert_eq!(
        block_on(credential_store.load()).unwrap().unwrap().version,
        CredentialVersion::new(2)
    );
}

#[test]
fn async_config_lock_wait_is_off_executor_and_bounded() {
    use fs2::FileExt;
    use std::fs::OpenOptions;

    let root = TempRoot::new("bounded-lock");
    let store =
        NativeConfigStore::open(&root.0, config("fixture", 1), NetworkPolicy::DEVELOPMENT).unwrap();
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.0.join("config.lock"))
        .unwrap();
    lock.lock_exclusive().unwrap();

    let started = Instant::now();
    let error = block_on(store.load()).unwrap_err();
    assert_eq!(error.code, "lock_timeout");
    assert!(started.elapsed() < Duration::from_secs(2));
    lock.unlock().unwrap();
}

#[test]
#[cfg(windows)]
fn reversible_file_credentials_fail_closed_on_windows() {
    let root = TempRoot::new("windows-file-credentials");
    let error = FileCredentialStore::open(&root.0).unwrap_err();
    assert_eq!(error.code, "credential_file_unsupported");
    assert!(!root.0.join("credentials.native").exists());
}

#[test]
#[cfg(all(windows, feature = "native-credentials"))]
fn windows_credential_manager_rotates_and_reconciles_without_token_files() {
    struct Cleanup<'a>(&'a WindowsCredentialStore);

    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            let _ = self.0.delete();
        }
    }

    let root = TempRoot::new("windows-credential-manager");
    let store = WindowsCredentialStore::open(&root.0).unwrap();
    store.delete().unwrap();
    let _cleanup = Cleanup(&store);
    block_on(store.mark_reconciliation_required(CommitId::new())).unwrap();
    assert_eq!(
        store.initialize(&credentials(1)).unwrap_err().code,
        "credential_reconciliation_required"
    );
    assert_eq!(
        store.delete().unwrap_err().code,
        "credential_reconciliation_required"
    );
    std::fs::remove_file(root.0.join("credentials.reconciliation")).unwrap();
    store.initialize(&credentials(1)).unwrap();
    assert!(!store.reconciliation_required().unwrap());
    block_on(store.mark_reconciliation_required(CommitId::new())).unwrap();
    assert_eq!(
        store.delete().unwrap_err().code,
        "credential_reconciliation_required"
    );
    std::fs::remove_file(root.0.join("credentials.reconciliation")).unwrap();
    assert!(!store.reconciliation_required().unwrap());
    let commit = CredentialCommit {
        commit_id: CommitId::new(),
        expected_version: CredentialVersion::new(1),
        credentials: credentials(2),
    };
    block_on(store.commit(commit.clone())).unwrap();
    block_on(store.mark_reconciliation_required(commit.commit_id)).unwrap();
    block_on(store.commit(commit)).unwrap();
    for version in 3..=96 {
        let rotation = CredentialCommit {
            commit_id: CommitId::new(),
            expected_version: CredentialVersion::new(version - 1),
            credentials: credentials(version),
        };
        block_on(store.commit(rotation.clone())).unwrap();
        block_on(store.commit(rotation)).unwrap();
    }

    let reopened = WindowsCredentialStore::open(&root.0).unwrap();
    let loaded = block_on(reopened.load()).unwrap().unwrap();
    assert_eq!(loaded.version, CredentialVersion::new(96));
    assert_eq!(loaded.refresh_token.expose_secret(), "refresh-96");
    assert!(!store.reconciliation_required().unwrap());
    assert!(!root.0.join("credentials.native").exists());
}

#[test]
#[cfg(all(not(windows), feature = "native-credentials"))]
fn native_keyring_rotates_and_reconciles_without_token_files() {
    struct Cleanup<'a>(&'a KeyringCredentialStore);

    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            let _ = self.0.delete();
        }
    }

    let root = TempRoot::new("native-keyring");
    let store = KeyringCredentialStore::open(&root.0).unwrap();
    let _ = store.delete();
    let _cleanup = Cleanup(&store);
    block_on(store.mark_reconciliation_required(CommitId::new())).unwrap();
    store.initialize(&credentials(1)).unwrap();
    assert!(!store.reconciliation_required().unwrap());
    block_on(store.mark_reconciliation_required(CommitId::new())).unwrap();
    store.delete().unwrap();
    assert!(!store.reconciliation_required().unwrap());
    store.initialize(&credentials(1)).unwrap();
    let staged = credentials(2);
    store
        .stage_authorized_session("native-keyring-stage", &credentials(1), &staged)
        .unwrap();
    store
        .verify_staged_authorized_session("native-keyring-stage", &credentials(1), &staged)
        .unwrap();
    assert!(store.reconciliation_required().unwrap());
    store
        .clear_staged_authorized_session("native-keyring-stage", &staged)
        .unwrap();
    assert_eq!(block_on(store.load()).unwrap(), Some(credentials(1)));
    let commit = CredentialCommit {
        commit_id: CommitId::new(),
        expected_version: CredentialVersion::new(1),
        credentials: credentials(2),
    };
    block_on(store.commit(commit.clone())).unwrap();
    block_on(store.mark_reconciliation_required(commit.commit_id)).unwrap();
    block_on(store.commit(commit)).unwrap();

    let loaded = block_on(store.load()).unwrap().unwrap();
    assert_eq!(loaded.version, CredentialVersion::new(2));
    assert!(!store.reconciliation_required().unwrap());
    assert!(!root.0.join("credentials.native").exists());
}

#[test]
fn interrupted_staging_file_cannot_replace_last_complete_config() {
    let root = TempRoot::new("interrupted");
    let target = root.0.join("state");
    AtomicFile::replace(&target, b"complete-old").unwrap();
    std::fs::write(root.0.join(".state.interrupted.tmp"), b"partial-new").unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"complete-old");
    AtomicFile::replace(&target, b"complete-new").unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"complete-new");
}

#[test]
fn config_and_local_first_record_commits_survive_reopen() {
    let root = TempRoot::new("config");
    let store =
        NativeConfigStore::open(&root.0, config("initial", 1), NetworkPolicy::DEVELOPMENT).unwrap();
    block_on(store.commit(ConfigCommit {
        commit_id: CommitId::new(),
        mutation: ConfigMutation::Replace(config("updated", 2)),
    }))
    .unwrap();
    block_on(store.commit(ConfigCommit {
        commit_id: CommitId::new(),
        mutation: ConfigMutation::ConversationPointer(Some("conversation-1".into())),
    }))
    .unwrap();
    block_on(store.commit(ConfigCommit {
        commit_id: CommitId::new(),
        mutation: ConfigMutation::LocalFirstRecord {
            kind: "fixture".into(),
            payload: b"complete-record".to_vec(),
        },
    }))
    .unwrap();

    let reopened =
        NativeConfigStore::open(&root.0, config("ignored", 99), NetworkPolicy::DEVELOPMENT)
            .unwrap();
    let loaded = block_on(reopened.load()).unwrap();
    assert_eq!(loaded.active_context, "updated");
    assert_eq!(loaded.revision, ConfigRevision::new(2));
    let record = std::fs::read_dir(root.0.join("records"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(std::fs::read(record).unwrap(), b"complete-record");
}

#[test]
fn config_replacement_validates_and_cannot_roll_revision_backward() {
    let root = TempRoot::new("config-validation");
    let store =
        NativeConfigStore::open(&root.0, config("initial", 4), NetworkPolicy::DEVELOPMENT).unwrap();

    let invalid = ClientConfig {
        active_context: String::new(),
        ..config("ignored", 5)
    };
    assert_eq!(
        block_on(store.commit(ConfigCommit {
            commit_id: CommitId::new(),
            mutation: ConfigMutation::Replace(invalid),
        }))
        .unwrap_err()
        .code,
        "config_validation"
    );
    assert_eq!(
        block_on(store.commit(ConfigCommit {
            commit_id: CommitId::new(),
            mutation: ConfigMutation::Replace(config("rollback", 4)),
        }))
        .unwrap_err()
        .code,
        "config_revision_conflict"
    );
    assert_eq!(block_on(store.load()).unwrap().active_context, "initial");
}

#[test]
fn config_inputs_and_replay_window_are_bounded() {
    let root = TempRoot::new("config-bounds");
    let store =
        NativeConfigStore::open(&root.0, config("initial", 1), NetworkPolicy::DEVELOPMENT).unwrap();

    assert_eq!(
        block_on(store.commit(ConfigCommit {
            commit_id: CommitId::new(),
            mutation: ConfigMutation::ConversationPointer(Some("x".repeat(4 * 1024 + 1))),
        }))
        .unwrap_err()
        .code,
        "config_conversation"
    );
    assert_eq!(
        block_on(store.commit(ConfigCommit {
            commit_id: CommitId::new(),
            mutation: ConfigMutation::LocalFirstRecord {
                kind: "invalid kind".into(),
                payload: vec![1],
            },
        }))
        .unwrap_err()
        .code,
        "config_record_kind"
    );
    assert_eq!(
        block_on(store.commit(ConfigCommit {
            commit_id: CommitId::new(),
            mutation: ConfigMutation::LocalFirstRecord {
                kind: "bounded".into(),
                payload: vec![0; 1024 * 1024 + 1],
            },
        }))
        .unwrap_err()
        .code,
        "config_record_size"
    );

    for index in 0..150 {
        block_on(store.commit(ConfigCommit {
            commit_id: CommitId::new(),
            mutation: ConfigMutation::ConversationPointer(Some(format!("conversation-{index}"))),
        }))
        .unwrap();
    }
    let document = std::fs::read_to_string(root.0.join("config.native")).unwrap();
    let applied = document
        .lines()
        .find_map(|line| line.strip_prefix("applied="))
        .unwrap();
    assert_eq!(applied.split(',').count(), 128);
    assert!(document.len() < 8 * 1024);
}

#[test]
fn config_reconciliation_marker_is_exact_commit_and_durable() {
    let root = TempRoot::new("config-reconciliation");
    let store =
        NativeConfigStore::open(&root.0, config("initial", 1), NetworkPolicy::DEVELOPMENT).unwrap();
    let commit_id = CommitId::new();
    block_on(store.mark_reconciliation_required(commit_id)).unwrap();
    assert!(store.reconciliation_required().unwrap());
    block_on(store.clear_reconciliation_required(CommitId::new())).unwrap();
    assert!(store.reconciliation_required().unwrap());
    block_on(store.clear_reconciliation_required(commit_id)).unwrap();
    assert!(!store.reconciliation_required().unwrap());
}

#[test]
fn cross_process_commits_are_locked_and_leave_a_complete_document() {
    const CHILD_ENV: &str = "HEYFOOD_PLATFORM_LOCK_CHILD";
    if let Ok(root) = std::env::var(CHILD_ENV) {
        let store = NativeConfigStore::open(
            Path::new(&root),
            config("child-initial", 1),
            NetworkPolicy::DEVELOPMENT,
        )
        .unwrap();
        block_on(store.commit(ConfigCommit {
            commit_id: CommitId::new(),
            mutation: ConfigMutation::ConversationPointer(Some(format!(
                "child-{}",
                std::process::id()
            ))),
        }))
        .unwrap();
        return;
    }

    let root = TempRoot::new("process-lock");
    NativeConfigStore::open(&root.0, config("shared", 1), NetworkPolicy::DEVELOPMENT).unwrap();
    let executable = std::env::current_exe().unwrap();
    let children = (0..4)
        .map(|_| {
            std::process::Command::new(&executable)
                .args([
                    "--exact",
                    "cross_process_commits_are_locked_and_leave_a_complete_document",
                    "--nocapture",
                ])
                .env(CHILD_ENV, &root.0)
                .spawn()
                .unwrap()
        })
        .collect::<Vec<_>>();
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }
    let reopened =
        NativeConfigStore::open(&root.0, config("ignored", 2), NetworkPolicy::DEVELOPMENT).unwrap();
    assert_eq!(block_on(reopened.load()).unwrap().active_context, "shared");
}
