use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use heyfood_application::{
    ConfigCommit, ConfigMutation, ConfigPort, CredentialCommit, CredentialPort,
};
use heyfood_core::{
    AccountId, ClientConfig, CommitId, ConfigRevision, CredentialVersion, NetworkPolicy,
    SensitiveString, ServiceUrl, SessionCredentials,
};
use heyfood_platform::{AtomicFile, FileCredentialStore, NativeConfigStore};

struct ThreadWake(thread::Thread);

impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn block_on<T>(future: impl Future<Output = T>) -> T {
    let waker = Waker::from(Arc::new(ThreadWake(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    loop {
        match Future::poll(Pin::as_mut(&mut future), &mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => thread::park_timeout(Duration::from_millis(10)),
        }
    }
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
fn reconciliation_marker_is_durable_and_cleared_by_verified_rotation() {
    let root = TempRoot::new("reconciliation");
    let store = FileCredentialStore::open(&root.0).unwrap();
    store.initialize(&credentials(1)).unwrap();
    block_on(store.mark_reconciliation_required(CommitId::new())).unwrap();
    assert!(store.reconciliation_required().unwrap());
    block_on(store.commit(CredentialCommit {
        commit_id: CommitId::new(),
        expected_version: CredentialVersion::new(1),
        credentials: credentials(2),
    }))
    .unwrap();
    assert!(!store.reconciliation_required().unwrap());
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
