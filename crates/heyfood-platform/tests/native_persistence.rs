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
use heyfood_core::{ClientConfig, CommitId, ConfigRevision, NetworkPolicy, ServiceUrl};
#[cfg(not(windows))]
use heyfood_core::{GenerationId, OperationId, SessionSnapshot};
#[cfg(all(windows, feature = "native-credentials"))]
use heyfood_platform::WindowsCredentialStore;
use heyfood_platform::{AtomicFile, FileCredentialStore, NativeConfigStore};

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
    store.initialize(&credentials(1)).unwrap();
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
