use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(windows)]
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use fs2::FileExt;
use heyfood_application::{
    BoxFuture, ConfigCommit, ConfigMutation, ConfigPort, CredentialCommit, CredentialPort,
    PortError,
};
use heyfood_core::{
    AccountId, AuthCredentialBundle, ChannelCredentials, ClientConfig, CommitId, ConfigRevision,
    CredentialVersion, NetworkPolicy, SensitiveString, ServiceUrl, SessionCredentials,
};

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[cfg(windows)]
static WINDOWS_CURRENT_USER_SID: OnceLock<String> = OnceLock::new();
#[cfg(windows)]
static WINDOWS_HARDENED_PATHS: OnceLock<Mutex<std::collections::HashSet<(PathBuf, bool)>>> =
    OnceLock::new();
const LOCK_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(1);
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(10);
#[cfg(any(windows, test))]
const CREDENTIAL_WRITE_VERIFY_TIMEOUT: Duration = Duration::from_secs(1);
#[cfg(any(windows, test))]
const CREDENTIAL_WRITE_VERIFY_INTERVAL: Duration = Duration::from_millis(10);
const MAX_CONFIG_APPLIED_COMMITS: usize = 128;
const MAX_CONVERSATION_POINTER_BYTES: usize = 4 * 1_024;
const MAX_LOCAL_RECORD_KIND_BYTES: usize = 64;
const MAX_LOCAL_RECORD_BYTES: usize = 1024 * 1024;
const MAX_LOCAL_RECORDS: usize = 1_024;

/// Same-directory, exclusive staging followed by a flushed atomic replace.
pub struct AtomicFile;

impl AtomicFile {
    pub fn replace(path: &Path, bytes: &[u8]) -> Result<(), PortError> {
        let parent = path.parent().ok_or_else(|| {
            PortError::new("atomic_path", "atomic target must have a parent directory")
        })?;
        create_private_dir(parent)?;
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| PortError::new("atomic_path", "atomic target name is invalid"))?;
        let (staging_path, mut staging) = (0..32)
            .find_map(|_| {
                let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
                let candidate =
                    parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), sequence));
                match OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&candidate)
                {
                    Ok(file) => Some(Ok((candidate, file))),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                    Err(error) => Some(Err(error)),
                }
            })
            .transpose()
            .map_err(|error| PortError::new("atomic_stage", error.to_string()))?
            .ok_or_else(|| PortError::new("atomic_stage", "could not allocate staging file"))?;

        let mut replaced = false;
        if let Err(error) = (|| -> std::io::Result<()> {
            make_private_staging_file(&staging_path)?;
            staging.write_all(bytes)?;
            staging.flush()?;
            staging.sync_all()?;
            fs::rename(&staging_path, path)?;
            replaced = true;
            #[cfg(windows)]
            remember_windows_owner_acl(path, false)?;
            sync_directory(parent)?;
            Ok(())
        })() {
            let _ = fs::remove_file(&staging_path);
            return Err(if replaced {
                PortError::uncertain("atomic_replace", error.to_string())
            } else {
                PortError::new("atomic_replace", error.to_string())
            });
        }
        Ok(())
    }
}

pub(crate) struct FileLock {
    file: File,
}

impl FileLock {
    pub(crate) fn acquire(path: &Path, exclusive: bool) -> Result<Self, PortError> {
        let parent = path
            .parent()
            .ok_or_else(|| PortError::new("lock_path", "lock file must have a parent"))?;
        create_private_dir(parent)?;
        #[cfg(windows)]
        let existed = path.exists();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|error| PortError::new("lock_open", error.to_string()))?;
        #[cfg(windows)]
        if !existed {
            forget_windows_owner_acl(path, false)
                .map_err(|error| PortError::new("lock_permissions", error.to_string()))?;
        }
        make_private_file(path)
            .map_err(|error| PortError::new("lock_permissions", error.to_string()))?;
        let started = Instant::now();
        loop {
            let result = if exclusive {
                FileExt::try_lock_exclusive(&file)
            } else {
                FileExt::try_lock_shared(&file)
            };
            match result {
                Ok(()) => break,
                Err(error) if lock_is_contended(&error) => {
                    if started.elapsed() >= LOCK_ACQUIRE_TIMEOUT {
                        return Err(PortError::new(
                            "lock_timeout",
                            "state lock acquisition exceeded its deadline",
                        ));
                    }
                    thread::sleep(LOCK_RETRY_INTERVAL);
                }
                Err(error) => {
                    return Err(PortError::new("lock_acquire", error.to_string()));
                }
            }
        }
        Ok(Self { file })
    }

    pub(crate) async fn acquire_async(path: &Path, exclusive: bool) -> Result<Self, PortError> {
        let path = path.to_owned();
        tokio::task::spawn_blocking(move || Self::acquire(&path, exclusive))
            .await
            .map_err(|_| PortError::new("lock_task", "state lock worker did not complete"))?
    }
}

fn lock_is_contended(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        // LockFileEx reports ERROR_LOCK_VIOLATION when another process owns
        // the requested byte range. Rust does not currently classify this as
        // WouldBlock, but it is the Windows equivalent for this retry loop.
        if error.raw_os_error() == Some(33) {
            return true;
        }
    }
    false
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[derive(Clone, Debug)]
pub struct NativeConfigStore {
    state_path: PathBuf,
    lock_path: PathBuf,
    records_path: PathBuf,
    reconciliation_path: PathBuf,
    policy: NetworkPolicy,
    expected_account: Option<AccountId>,
}

impl NativeConfigStore {
    pub fn open(
        root: impl AsRef<Path>,
        initial: ClientConfig,
        policy: NetworkPolicy,
    ) -> Result<Self, PortError> {
        let root = root.as_ref();
        create_private_dir(root)?;
        Self::open_with_binding(root, initial, policy, None)
    }

    pub fn open_account_bound(
        root: impl AsRef<Path>,
        account_id: AccountId,
        initial: ClientConfig,
        policy: NetworkPolicy,
    ) -> Result<Self, PortError> {
        Self::open_with_binding(root, initial, policy, Some(account_id))
    }

    fn open_with_binding(
        root: impl AsRef<Path>,
        initial: ClientConfig,
        policy: NetworkPolicy,
        expected_account: Option<AccountId>,
    ) -> Result<Self, PortError> {
        initial
            .validate()
            .map_err(|error| PortError::new("config_validation", error))?;
        let root = root.as_ref();
        create_private_dir(root)?;
        let store = Self {
            state_path: root.join("config.native"),
            lock_path: root.join("config.lock"),
            records_path: root.join("records"),
            reconciliation_path: root.join("config.reconciliation"),
            policy,
            expected_account,
        };
        let _lock = FileLock::acquire(&store.lock_path, true)?;
        if !store.state_path.exists() {
            AtomicFile::replace(
                &store.state_path,
                &ConfigState::new(initial, store.expected_account.clone()).encode(),
            )?;
        } else {
            let mut state = store.read_unlocked()?;
            match (&state.account_id, &store.expected_account) {
                (Some(actual), Some(expected)) if actual != expected => {
                    return Err(PortError::new(
                        "config_account_conflict",
                        "native state belongs to a different account",
                    ));
                }
                (None, Some(expected)) => {
                    // An unbound document contains no credential and is
                    // safely claimed exactly once after verified login. Any
                    // account-scoped pointer/idempotency state is discarded,
                    // matching the Python oracle's fail-closed binding rule.
                    if store.records_path.exists()
                        && store
                            .records_path
                            .read_dir()
                            .map_err(|error| PortError::new("config_records", error.to_string()))?
                            .next()
                            .is_some()
                    {
                        return Err(PortError::new(
                            "config_account_unbound_state",
                            "unbound durable records require explicit reconciliation",
                        ));
                    }
                    state.account_id = Some(expected.clone());
                    state.conversation = None;
                    state.applied.clear();
                    AtomicFile::replace(&store.state_path, &state.encode())?;
                }
                _ => {
                    // Re-encode legacy schemas into the current version even
                    // when it remains intentionally unbound.
                    if state.schema_version < heyfood_core::CURRENT_CONFIG_SCHEMA.get() {
                        AtomicFile::replace(&store.state_path, &state.encode())?;
                    }
                }
            }
        }
        Ok(store)
    }

    fn read_unlocked(&self) -> Result<ConfigState, PortError> {
        let bytes = read_limited(&self.state_path, 1024 * 1024)?;
        let state = ConfigState::decode(&bytes, self.policy)?;
        if let (Some(actual), Some(expected)) = (&state.account_id, &self.expected_account)
            && actual != expected
        {
            return Err(PortError::new(
                "config_account_conflict",
                "native state belongs to a different account",
            ));
        }
        Ok(state)
    }

    pub fn reconciliation_required(&self) -> Result<bool, PortError> {
        let _lock = FileLock::acquire(&self.lock_path, false)?;
        Ok(self.reconciliation_path.exists())
    }
}

impl ConfigPort for NativeConfigStore {
    fn load(&self) -> BoxFuture<'_, Result<ClientConfig, PortError>> {
        Box::pin(async move {
            let _lock = FileLock::acquire_async(&self.lock_path, false).await?;
            Ok(self.read_unlocked()?.config)
        })
    }

    fn commit(&self, commit: ConfigCommit) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            let _lock = FileLock::acquire_async(&self.lock_path, true).await?;
            let mut state = self.read_unlocked()?;
            if state.applied.contains(&commit.commit_id) {
                return clear_reconciliation_marker(&self.reconciliation_path, commit.commit_id);
            }
            let mut local_record_applied = false;
            match commit.mutation {
                ConfigMutation::Replace(config) => {
                    config
                        .validate()
                        .map_err(|error| PortError::new("config_validation", error))?;
                    if config.revision <= state.config.revision {
                        return Err(PortError::new(
                            "config_revision_conflict",
                            "replacement config revision must advance",
                        ));
                    }
                    state.config = config;
                }
                ConfigMutation::ConversationPointer(pointer) => {
                    validate_conversation_pointer(pointer.as_deref())?;
                    state.conversation = pointer;
                }
                ConfigMutation::LocalFirstRecord { kind, payload } => {
                    heyfood_core::validate_identifier(&kind, MAX_LOCAL_RECORD_KIND_BYTES).map_err(
                        |_| PortError::new("config_record_kind", "record kind is invalid"),
                    )?;
                    if payload.is_empty() || payload.len() > MAX_LOCAL_RECORD_BYTES {
                        return Err(PortError::new(
                            "config_record_size",
                            "record payload must contain 1 byte through 1 MiB",
                        ));
                    }
                    create_private_dir(&self.records_path)?;
                    let name = format!(
                        "{}-{}.record",
                        hex_encode(kind.as_bytes()),
                        commit.commit_id.as_uuid()
                    );
                    let path = self.records_path.join(name);
                    if !path.exists()
                        && count_directory_entries(&self.records_path)? >= MAX_LOCAL_RECORDS
                    {
                        return Err(PortError::new(
                            "config_record_capacity",
                            "durable record capacity is exhausted and requires reconciliation",
                        ));
                    }
                    AtomicFile::replace(&path, &payload)?;
                    local_record_applied = true;
                }
            }
            state.remember_commit(commit.commit_id);
            if let Err(error) = AtomicFile::replace(&self.state_path, &state.encode()) {
                return Err(if local_record_applied && !error.outcome_uncertain {
                    PortError::uncertain(error.code, error.message)
                } else {
                    error
                });
            }
            clear_reconciliation_marker(&self.reconciliation_path, commit.commit_id)
        })
    }

    fn mark_reconciliation_required(
        &self,
        commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            let _lock = FileLock::acquire_async(&self.lock_path, true).await?;
            AtomicFile::replace(
                &self.reconciliation_path,
                format!("{}\n", commit_id.as_uuid()).as_bytes(),
            )
        })
    }

    fn clear_reconciliation_required(
        &self,
        commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            let _lock = FileLock::acquire_async(&self.lock_path, true).await?;
            clear_reconciliation_marker(&self.reconciliation_path, commit_id)
        })
    }
}

struct ConfigState {
    schema_version: u16,
    account_id: Option<AccountId>,
    config: ClientConfig,
    conversation: Option<String>,
    applied: Vec<CommitId>,
}

impl ConfigState {
    fn new(config: ClientConfig, account_id: Option<AccountId>) -> Self {
        Self {
            schema_version: heyfood_core::CURRENT_CONFIG_SCHEMA.get(),
            account_id,
            config,
            conversation: None,
            applied: Vec::new(),
        }
    }

    fn remember_commit(&mut self, commit_id: CommitId) {
        if self.applied.contains(&commit_id) {
            return;
        }
        if self.applied.len() == MAX_CONFIG_APPLIED_COMMITS {
            self.applied.remove(0);
        }
        self.applied.push(commit_id);
    }

    fn encode(&self) -> Vec<u8> {
        let applied = self
            .applied
            .iter()
            .map(|value| value.as_uuid().to_string())
            .collect::<Vec<_>>();
        let applied = applied.join(",");
        format!(
            "schema={}\naccount={}\nactive={}\napi={}\nauth={}\nrevision={}\nconversation={}\napplied={}\n",
            heyfood_core::CURRENT_CONFIG_SCHEMA.get(),
            self.account_id
                .as_ref()
                .map_or_else(String::new, |value| hex_encode(value.as_str().as_bytes())),
            hex_encode(self.config.active_context.as_bytes()),
            hex_encode(self.config.api_url.as_url().as_str().as_bytes()),
            hex_encode(self.config.auth_url.as_url().as_str().as_bytes()),
            self.config.revision.get(),
            self.conversation
                .as_deref()
                .map_or_else(String::new, |value| hex_encode(value.as_bytes())),
            applied,
        )
        .into_bytes()
    }

    fn decode(bytes: &[u8], policy: NetworkPolicy) -> Result<Self, PortError> {
        let fields = fields(bytes)?;
        let schema_version = required(&fields, "schema")?
            .parse::<u16>()
            .map_err(|_| PortError::new("config_schema", "invalid native config schema"))?;
        if !matches!(schema_version, 1..=3) {
            return Err(PortError::new(
                "config_schema",
                "unsupported native config schema",
            ));
        }
        let account_id = if schema_version == 1 {
            None
        } else {
            match required(&fields, "account")? {
                "" => None,
                value => Some(
                    AccountId::parse(hex_string(value)?)
                        .map_err(|error| PortError::new("config_account", error))?,
                ),
            }
        };
        let active_context = hex_string(required(&fields, "active")?)?;
        let api_url = ServiceUrl::parse(&hex_string(required(&fields, "api")?)?, policy)
            .map_err(|error| PortError::new("config_url", error.to_string()))?;
        let auth_url = ServiceUrl::parse(&hex_string(required(&fields, "auth")?)?, policy)
            .map_err(|error| PortError::new("config_url", error.to_string()))?;
        let revision = required(&fields, "revision")?
            .parse::<u64>()
            .map_err(|_| PortError::new("config_revision", "invalid config revision"))?;
        let conversation = match required(&fields, "conversation")? {
            "" => None,
            value => Some(hex_string(value)?),
        };
        validate_conversation_pointer(conversation.as_deref())?;
        let mut applied = parse_commit_set(required(&fields, "applied")?)?;
        if schema_version == 3 && applied.len() > MAX_CONFIG_APPLIED_COMMITS {
            return Err(PortError::new(
                "config_commit_capacity",
                "native config contains too many durable commit IDs",
            ));
        }
        if applied.len() > MAX_CONFIG_APPLIED_COMMITS {
            applied.drain(..applied.len() - MAX_CONFIG_APPLIED_COMMITS);
        }
        let config = ClientConfig {
            active_context,
            api_url,
            auth_url,
            revision: ConfigRevision::new(revision),
        };
        config
            .validate()
            .map_err(|error| PortError::new("config_validation", error))?;
        Ok(Self {
            schema_version,
            account_id,
            config,
            conversation,
            applied,
        })
    }
}

#[derive(Clone, Debug)]
pub struct FileCredentialStore {
    state_path: PathBuf,
    authorization_stage_path: PathBuf,
    lock_path: PathBuf,
    reconciliation_path: PathBuf,
}

/// Atomic owner-only persistence for the complete native authorization result.
/// This is separate from the rotating app-session store so a session refresh
/// cannot accidentally erase the channel grant required for re-exchange.
#[derive(Clone, Debug)]
pub struct NativeAuthStore {
    #[cfg(not(windows))]
    state_path: PathBuf,
    #[cfg(not(windows))]
    replacement_path: PathBuf,
    lock_path: PathBuf,
    reconciliation_path: PathBuf,
    #[cfg(all(windows, feature = "native-credentials"))]
    target: String,
    #[cfg(all(windows, feature = "native-credentials"))]
    replacement_target: String,
    #[cfg(all(windows, feature = "native-credentials"))]
    replacement_previous_target: String,
    #[cfg(all(windows, feature = "native-credentials"))]
    replacement_pending_target: String,
}

/// Durable phase of an explicit staged authority replacement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthorizationReplacementPhase {
    /// The client transaction ID is durable, but the server result has not yet
    /// been observed and staged locally.
    Preparing,
    /// Both complete bundles and the server recovery capability are durable;
    /// the old bundle remains active.
    Prepared,
    /// The promote request may have reached the server. Only authoritative
    /// status recovery may decide whether to activate or roll back locally.
    PromotionDispatched,
    /// Abort intent is durable. Recovery may retry abort, but must never
    /// promote even when the server still reports `staged`.
    AbortDispatched,
}

/// Owner-only write-ahead record for a staged server authority replacement.
///
/// The previous and replacement bundles deliberately coexist until terminal
/// server status is known. This makes both promotion and abort/expiry
/// recoverable after a crash without reconstructing credentials.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizationReplacementJournal {
    pub phase: AuthorizationReplacementPhase,
    pub client_transaction_id: String,
    pub stage_id: Option<String>,
    pub authorization_transaction_id: Option<String>,
    /// Present only between the device-token response and idempotent prepare.
    /// It is persisted so a lost prepare response can replay the exact request
    /// after a process crash without issuing a second authority bundle.
    pub provisional_access_token: Option<SensitiveString>,
    pub recovery_token: Option<SensitiveString>,
    pub bundle_digest: Option<String>,
    pub previous: AuthCredentialBundle,
    pub replacement: Option<AuthCredentialBundle>,
}

impl AuthorizationReplacementJournal {
    fn validate(&self) -> Result<(), PortError> {
        validate_transaction_id("client transaction", &self.client_transaction_id)?;
        self.previous
            .validate()
            .map_err(|error| PortError::new("auth_validation", error))?;
        match self.phase {
            AuthorizationReplacementPhase::Preparing => {
                if self.stage_id.is_some()
                    || self.recovery_token.is_some()
                    || self.bundle_digest.is_some()
                    || self.replacement.is_some()
                {
                    return Err(PortError::new(
                        "authorization_journal",
                        "preparing authorization journal contains staged result data",
                    ));
                }
                if self.provisional_access_token.is_some()
                    != self.authorization_transaction_id.is_some()
                {
                    return Err(PortError::new(
                        "authorization_journal",
                        "provisional credential binding is incomplete",
                    ));
                }
                if let Some(token) = &self.provisional_access_token
                    && (token.expose_secret().len() < 20
                        || token.expose_secret().len() > 1_024
                        || token.expose_secret().chars().any(char::is_control))
                {
                    return Err(PortError::new(
                        "authorization_journal",
                        "provisional credential is invalid",
                    ));
                }
                if let Some(transaction_id) = &self.authorization_transaction_id {
                    validate_transaction_id("authorization transaction", transaction_id)?;
                }
            }
            AuthorizationReplacementPhase::Prepared
            | AuthorizationReplacementPhase::PromotionDispatched
            | AuthorizationReplacementPhase::AbortDispatched => {
                validate_transaction_id(
                    "stage",
                    self.stage_id.as_deref().ok_or_else(|| {
                        PortError::new("authorization_journal", "stage ID is missing")
                    })?,
                )?;
                validate_transaction_id(
                    "authorization transaction",
                    self.authorization_transaction_id
                        .as_deref()
                        .ok_or_else(|| {
                            PortError::new(
                                "authorization_journal",
                                "authorization transaction ID is missing",
                            )
                        })?,
                )?;
                let recovery_token = self.recovery_token.as_ref().ok_or_else(|| {
                    PortError::new("authorization_journal", "recovery capability is missing")
                })?;
                if self.provisional_access_token.is_some() {
                    return Err(PortError::new(
                        "authorization_journal",
                        "staged journal retained a provisional credential",
                    ));
                }
                if recovery_token.expose_secret().len() < 20
                    || recovery_token.expose_secret().len() > 1_024
                    || recovery_token.expose_secret().chars().any(char::is_control)
                {
                    return Err(PortError::new(
                        "authorization_journal",
                        "recovery capability is invalid",
                    ));
                }
                let digest = self.bundle_digest.as_deref().ok_or_else(|| {
                    PortError::new("authorization_journal", "bundle digest is missing")
                })?;
                if digest.len() != 64
                    || !digest
                        .as_bytes()
                        .iter()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
                {
                    return Err(PortError::new(
                        "authorization_journal",
                        "bundle digest is invalid",
                    ));
                }
                let replacement = self.replacement.as_ref().ok_or_else(|| {
                    PortError::new("authorization_journal", "replacement bundle is missing")
                })?;
                replacement
                    .validate()
                    .map_err(|error| PortError::new("auth_validation", error))?;
                if self.phase != AuthorizationReplacementPhase::AbortDispatched
                    && (replacement.session.account_id != self.previous.session.account_id
                        || replacement.channel.device_id != self.previous.channel.device_id
                        || replacement.channel.client_id != self.previous.channel.client_id)
                {
                    return Err(PortError::new(
                        "authorization_binding",
                        "staged authorization changed its account, device, or client binding",
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Exclusive channel-refresh transaction. The lock is held from the second
/// read through the remote rotation and its durable commit, preventing two
/// CLI processes from consuming the same rotating refresh grant.
pub struct NativeAuthRefreshGuard<'a> {
    store: &'a NativeAuthStore,
    _lock: FileLock,
}

/// Session-store half of an explicit authorization replacement. Implementors
/// must either leave the previous credential intact or return an uncertain
/// error; the auth-store transaction marker blocks use after either outcome.
pub trait AuthorizationSessionStore {
    /// Initialize a previously empty authoritative session store. Callers must
    /// hold a durable cross-store write-ahead marker until both stores verify.
    fn initialize_authorized_session(
        &self,
        _credentials: &SessionCredentials,
    ) -> Result<(), PortError> {
        Err(PortError::new(
            "credential_initialization_unsupported",
            "this session store does not support account-bound initialization",
        ))
    }

    /// Load the authoritative active session under the store's own lock. The
    /// session embedded in the auth bundle may lag normal app-session rotation
    /// and must never be used as rollback authority.
    fn load_authorized_session(&self) -> Result<Option<SessionCredentials>, PortError>;

    fn replace_authorized_session(&self, credentials: &SessionCredentials)
    -> Result<(), PortError>;

    /// Persist a pending session without making it visible to ordinary loads.
    fn stage_authorized_session(
        &self,
        client_transaction_id: &str,
        previous: &SessionCredentials,
        replacement: &SessionCredentials,
    ) -> Result<(), PortError>;

    /// Verify that the exact pending session is durable.
    fn verify_staged_authorized_session(
        &self,
        client_transaction_id: &str,
        previous: &SessionCredentials,
        replacement: &SessionCredentials,
    ) -> Result<(), PortError>;

    /// Remove pending session state after terminal reconciliation.
    fn clear_staged_authorized_session(
        &self,
        client_transaction_id: &str,
        expected_replacement: &SessionCredentials,
    ) -> Result<(), PortError>;
}

#[cfg(any(not(windows), feature = "native-credentials"))]
impl NativeAuthRefreshGuard<'_> {
    pub fn load(&self) -> Result<Option<AuthCredentialBundle>, PortError> {
        self.store.ensure_reconciled_unlocked()?;
        self.store.load_unlocked()
    }

    pub fn replace(&self, bundle: &AuthCredentialBundle) -> Result<(), PortError> {
        self.store.replace_unlocked(bundle)?;
        clear_any_reconciliation_marker(&self.store.reconciliation_path)
    }

    pub fn mark_reconciliation_required(&self) -> Result<(), PortError> {
        AtomicFile::replace(
            &self.store.reconciliation_path,
            b"channel_refresh_outcome_uncertain\n",
        )
        .map_err(|error| PortError::uncertain("auth_reconciliation_write", error.to_string()))
    }

    /// Clear a write-ahead marker only after an observed response proves the
    /// rotating grant was not accepted, or after the replacement is durable.
    pub fn clear_reconciliation_required(&self) -> Result<(), PortError> {
        clear_any_reconciliation_marker(&self.store.reconciliation_path)
    }
}

impl NativeAuthStore {
    /// Load authorization and rotating-session state as one account-bound
    /// composition. A recoverable initialization window is completed under a
    /// durable write-ahead marker; every other split or cross-account state is
    /// left blocked for explicit reconciliation before any request can run.
    #[cfg(any(not(windows), feature = "native-credentials"))]
    pub fn load_account_bound(
        &self,
        session_store: &impl AuthorizationSessionStore,
    ) -> Result<Option<AuthCredentialBundle>, PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        if self.load_authorization_journal_unlocked()?.is_some() {
            return Err(PortError::uncertain(
                "auth_reconciliation_required",
                "authorization replacement must be reconciled before loading account state",
            ));
        }
        let initialization_pending = if self.reconciliation_path.exists() {
            read_limited(&self.reconciliation_path, 1024)? == b"account_binding_pending\n"
        } else {
            false
        };
        if self.reconciliation_path.exists() && !initialization_pending {
            return Err(PortError::uncertain(
                "auth_reconciliation_required",
                "authorization credentials have an unresolved outcome",
            ));
        }

        let auth = self.load_unlocked()?;
        let session = session_store.load_authorized_session()?;
        match (auth, session) {
            (None, None) => {
                if initialization_pending {
                    return Err(PortError::uncertain(
                        "auth_initialization_incomplete",
                        "account initialization was interrupted before authorization became durable",
                    ));
                }
                Ok(None)
            }
            (Some(auth), Some(session)) if auth.session.account_id == session.account_id => {
                if initialization_pending {
                    if auth.session != session {
                        AtomicFile::replace(
                            &self.reconciliation_path,
                            b"account_binding_conflict\n",
                        )?;
                        return Err(PortError::uncertain(
                            "authorization_session_version_conflict",
                            "account initialization observed a different active session",
                        ));
                    }
                    clear_any_reconciliation_marker(&self.reconciliation_path)?;
                }
                Ok(Some(AuthCredentialBundle {
                    channel: auth.channel,
                    session,
                }))
            }
            (Some(auth), None) if initialization_pending => {
                session_store.initialize_authorized_session(&auth.session)?;
                let session = session_store.load_authorized_session()?.ok_or_else(|| {
                    PortError::uncertain(
                        "auth_account_binding_verify",
                        "session initialization was not durably observable",
                    )
                })?;
                if session != auth.session {
                    AtomicFile::replace(&self.reconciliation_path, b"account_binding_conflict\n")?;
                    return Err(PortError::uncertain(
                        "authorization_session_version_conflict",
                        "initialized session did not match the pending authorization transaction",
                    ));
                }
                clear_any_reconciliation_marker(&self.reconciliation_path)?;
                Ok(Some(AuthCredentialBundle {
                    channel: auth.channel,
                    session,
                }))
            }
            (Some(_), None) => {
                AtomicFile::replace(
                    &self.reconciliation_path,
                    b"account_binding_missing_session\n",
                )
                .map_err(|error| {
                    PortError::uncertain("auth_reconciliation_write", error.to_string())
                })?;
                Err(PortError::uncertain(
                    "credentials_missing",
                    "the authoritative rotating session is missing; reauthorization is required and the stale auth-bundle mirror was not restored",
                ))
            }
            _ => {
                AtomicFile::replace(&self.reconciliation_path, b"account_binding_conflict\n")
                    .map_err(|error| {
                        PortError::uncertain("auth_reconciliation_write", error.to_string())
                    })?;
                Err(PortError::uncertain(
                    "authorization_account_conflict",
                    "authorization and active session are missing or belong to different accounts",
                ))
            }
        }
    }

    /// Initialize both credential stores under one durable cross-store
    /// transaction. Any interrupted write leaves the auth reconciliation
    /// marker in place, so no command can consume half-initialized authority.
    #[cfg(any(not(windows), feature = "native-credentials"))]
    pub fn initialize_account_bound(
        &self,
        bundle: &AuthCredentialBundle,
        session_store: &impl AuthorizationSessionStore,
    ) -> Result<(), PortError> {
        bundle
            .validate()
            .map_err(|error| PortError::new("auth_validation", error))?;
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        self.ensure_reconciled_unlocked()?;
        if self.load_unlocked()?.is_some() || session_store.load_authorized_session()?.is_some() {
            return Err(PortError::new(
                "auth_exists",
                "an account is already connected in one or both credential stores",
            ));
        }
        AtomicFile::replace(&self.reconciliation_path, b"account_binding_pending\n").map_err(
            |error| PortError::uncertain("auth_reconciliation_write", error.to_string()),
        )?;
        // Authorization is written first. While the marker exists it is not
        // visible to ordinary loads, but it is exact recovery authority for a
        // session-store write interrupted during this one initialization.
        self.write_initial_unlocked(bundle)?;
        session_store.initialize_authorized_session(&bundle.session)?;
        let verified_session = session_store.load_authorized_session()?.ok_or_else(|| {
            PortError::uncertain(
                "auth_account_binding_verify",
                "initialized session was not durably observable",
            )
        })?;
        if verified_session != bundle.session {
            return Err(PortError::uncertain(
                "auth_account_binding_verify",
                "initialized session did not match the authorized session",
            ));
        }
        if self.load_unlocked()?.as_ref() != Some(bundle) {
            return Err(PortError::uncertain(
                "auth_account_binding_verify",
                "initialized authorization was not durably observable",
            ));
        }
        clear_any_reconciliation_marker(&self.reconciliation_path)
    }

    #[cfg(not(windows))]
    fn write_initial_unlocked(&self, bundle: &AuthCredentialBundle) -> Result<(), PortError> {
        if self.state_path.exists() {
            return Err(PortError::new(
                "auth_exists",
                "authorization state already exists",
            ));
        }
        AtomicFile::replace(&self.state_path, &encode_auth_bundle(bundle))
    }

    #[cfg(all(windows, feature = "native-credentials"))]
    fn write_initial_unlocked(&self, bundle: &AuthCredentialBundle) -> Result<(), PortError> {
        if self.read_windows_unlocked()?.is_some() {
            return Err(PortError::new(
                "auth_exists",
                "authorization state already exists",
            ));
        }
        let document = encode_auth_bundle(bundle);
        if document.len() > WINDOWS_CREDENTIAL_BLOB_MAX_BYTES {
            return Err(PortError::new(
                "credential_manager_size",
                "authorization document exceeds the Windows Credential Manager limit",
            ));
        }
        self.windows_entry()?
            .set_secret(&document)
            .map_err(|error| keyring_error("credential_manager_write", error))
    }

    #[cfg(not(windows))]
    pub fn open(root: impl AsRef<Path>) -> Result<Self, PortError> {
        let root = root.as_ref();
        create_private_dir(root)?;
        Ok(Self {
            state_path: root.join("auth.native"),
            replacement_path: root.join("auth.authorization-replacement"),
            lock_path: root.join("auth.lock"),
            reconciliation_path: root.join("auth.reconciliation"),
        })
    }

    #[cfg(all(windows, feature = "native-credentials"))]
    pub fn open(root: impl AsRef<Path>) -> Result<Self, PortError> {
        use std::os::windows::ffi::OsStrExt;

        let root = root.as_ref();
        create_private_dir(root)?;
        let mut identity = Vec::new();
        for unit in root.as_os_str().encode_wide() {
            identity.extend_from_slice(&unit.to_le_bytes());
        }
        Ok(Self {
            lock_path: root.join("auth.lock"),
            reconciliation_path: root.join("auth.reconciliation"),
            target: format!(
                "{WINDOWS_CREDENTIAL_SERVICE}:auth:{}",
                hex_encode(&identity)
            ),
            replacement_target: format!(
                "{WINDOWS_CREDENTIAL_SERVICE}:auth-replacement:{}",
                hex_encode(&identity)
            ),
            replacement_previous_target: format!(
                "{WINDOWS_CREDENTIAL_SERVICE}:auth-replacement-previous:{}",
                hex_encode(&identity)
            ),
            replacement_pending_target: format!(
                "{WINDOWS_CREDENTIAL_SERVICE}:auth-replacement-pending:{}",
                hex_encode(&identity)
            ),
        })
    }

    /// Start a serialized refresh transaction. Callers must acquire this only
    /// after a cheap initial load, then reload through the guard before making
    /// a consuming refresh request.
    #[cfg(any(not(windows), feature = "native-credentials"))]
    pub fn begin_refresh(&self) -> Result<NativeAuthRefreshGuard<'_>, PortError> {
        Ok(NativeAuthRefreshGuard {
            store: self,
            _lock: FileLock::acquire(&self.lock_path, true)?,
        })
    }

    #[cfg(any(not(windows), feature = "native-credentials"))]
    fn ensure_reconciled_unlocked(&self) -> Result<(), PortError> {
        if self.reconciliation_path.exists()
            || self.load_authorization_journal_unlocked()?.is_some()
        {
            return Err(PortError::uncertain(
                "auth_reconciliation_required",
                "authorization credentials have an unresolved refresh or replacement outcome; stop and reconcile native account state before retrying",
            ));
        }
        Ok(())
    }

    /// Begin an explicit authority replacement before any staged server
    /// prepare request can be dispatched. The authoritative current session
    /// is loaded from its own store; the possibly stale session mirror in the
    /// auth bundle is never used as rollback authority.
    #[cfg(any(not(windows), feature = "native-credentials"))]
    pub fn begin_authorization_replacement(
        &self,
        client_transaction_id: String,
        session_store: &impl AuthorizationSessionStore,
    ) -> Result<AuthorizationReplacementJournal, PortError> {
        validate_transaction_id("client transaction", &client_transaction_id)?;
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        self.ensure_reconciled_unlocked()?;
        let current_auth = self
            .load_unlocked()?
            .ok_or_else(|| PortError::new("auth_missing", "authorization state is missing"))?;
        let current_session = session_store
            .load_authorized_session()?
            .ok_or_else(|| PortError::new("credentials_missing", "credentials are missing"))?;
        if current_auth.session.account_id != current_session.account_id {
            return Err(PortError::new(
                "authorization_account_conflict",
                "authorization and active session belong to different accounts",
            ));
        }
        let journal = AuthorizationReplacementJournal {
            phase: AuthorizationReplacementPhase::Preparing,
            client_transaction_id,
            stage_id: None,
            authorization_transaction_id: None,
            provisional_access_token: None,
            recovery_token: None,
            bundle_digest: None,
            previous: AuthCredentialBundle {
                channel: current_auth.channel,
                session: current_session,
            },
            replacement: None,
        };
        journal.validate()?;
        self.write_authorization_journal_unlocked(&journal)?;
        AtomicFile::replace(
            &self.reconciliation_path,
            b"authorization_replacement_pending\n",
        )
        .map_err(|error| PortError::uncertain("auth_reconciliation_write", error.to_string()))?;
        Ok(journal)
    }

    #[cfg(any(not(windows), feature = "native-credentials"))]
    pub fn record_provisional_authorization(
        &self,
        client_transaction_id: &str,
        authorization_transaction_id: String,
        provisional_access_token: SensitiveString,
    ) -> Result<AuthorizationReplacementJournal, PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        let mut journal = self.load_authorization_journal_unlocked()?.ok_or_else(|| {
            PortError::uncertain(
                "authorization_journal_missing",
                "authorization replacement marker exists without its journal",
            )
        })?;
        if journal.phase != AuthorizationReplacementPhase::Preparing
            || journal.client_transaction_id != client_transaction_id
        {
            return Err(PortError::new(
                "authorization_transaction_conflict",
                "a different authorization replacement is already pending",
            ));
        }
        journal.authorization_transaction_id = Some(authorization_transaction_id);
        journal.provisional_access_token = Some(provisional_access_token);
        journal.validate()?;
        self.write_authorization_journal_unlocked(&journal)?;
        Ok(journal)
    }

    #[cfg(any(not(windows), feature = "native-credentials"))]
    pub fn stage_authorization_replacement(
        &self,
        staged: AuthorizationReplacementJournal,
        session_store: &impl AuthorizationSessionStore,
    ) -> Result<(), PortError> {
        staged.validate()?;
        if staged.phase != AuthorizationReplacementPhase::Prepared {
            return Err(PortError::new(
                "authorization_journal_phase",
                "only a prepared replacement can be staged",
            ));
        }
        let replacement = staged.replacement.as_ref().ok_or_else(|| {
            PortError::new("authorization_journal", "replacement bundle is missing")
        })?;
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        let current = self.load_authorization_journal_unlocked()?.ok_or_else(|| {
            PortError::uncertain(
                "authorization_journal_missing",
                "authorization replacement marker exists without its journal",
            )
        })?;
        let resumable_preparing = current.phase == AuthorizationReplacementPhase::Preparing
            && current.client_transaction_id == staged.client_transaction_id
            && current.previous == staged.previous
            && current.authorization_transaction_id == staged.authorization_transaction_id;
        let resumable_prepared = current == staged;
        if !resumable_preparing && !resumable_prepared {
            return Err(PortError::new(
                "authorization_transaction_conflict",
                "staged replacement does not match its durable prepare transaction",
            ));
        }
        let active_session = session_store
            .load_authorized_session()?
            .ok_or_else(|| PortError::new("credentials_missing", "credentials are missing"))?;
        if active_session != staged.previous.session {
            return Err(PortError::new(
                "authorization_session_version_conflict",
                "session changed while authorization replacement was prepared",
            ));
        }
        // The complete owner-only journal is durable before the second store
        // is touched. Active credentials remain unchanged throughout.
        self.write_authorization_journal_unlocked(&staged)?;
        session_store.stage_authorized_session(
            &staged.client_transaction_id,
            &staged.previous.session,
            &replacement.session,
        )?;
        session_store.verify_staged_authorized_session(
            &staged.client_transaction_id,
            &staged.previous.session,
            &replacement.session,
        )?;
        let verified = self.load_authorization_journal_unlocked()?.ok_or_else(|| {
            PortError::uncertain(
                "authorization_journal_missing",
                "staged authorization journal disappeared before promotion",
            )
        })?;
        if verified != staged {
            return Err(PortError::uncertain(
                "authorization_journal_verify",
                "staged authorization journal changed before promotion",
            ));
        }
        Ok(())
    }

    #[cfg(any(not(windows), feature = "native-credentials"))]
    pub fn mark_authorization_promotion_dispatched(
        &self,
        client_transaction_id: &str,
        session_store: &impl AuthorizationSessionStore,
    ) -> Result<AuthorizationReplacementJournal, PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        let mut journal = self.load_authorization_journal_unlocked()?.ok_or_else(|| {
            PortError::uncertain(
                "authorization_journal_missing",
                "staged authorization journal is missing",
            )
        })?;
        if journal.client_transaction_id != client_transaction_id
            || !matches!(
                journal.phase,
                AuthorizationReplacementPhase::Prepared
                    | AuthorizationReplacementPhase::PromotionDispatched
            )
        {
            return Err(PortError::new(
                "authorization_transaction_conflict",
                "authorization promotion does not match the staged transaction",
            ));
        }
        let replacement = journal.replacement.as_ref().ok_or_else(|| {
            PortError::new("authorization_journal", "replacement bundle is missing")
        })?;
        session_store.verify_staged_authorized_session(
            &journal.client_transaction_id,
            &journal.previous.session,
            &replacement.session,
        )?;
        journal.phase = AuthorizationReplacementPhase::PromotionDispatched;
        journal.validate()?;
        self.write_authorization_journal_unlocked(&journal)?;
        Ok(journal)
    }

    #[cfg(any(not(windows), feature = "native-credentials"))]
    pub fn mark_authorization_abort_dispatched(
        &self,
        mut abort_journal: AuthorizationReplacementJournal,
    ) -> Result<AuthorizationReplacementJournal, PortError> {
        abort_journal.phase = AuthorizationReplacementPhase::AbortDispatched;
        abort_journal.validate()?;
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        let current = self.load_authorization_journal_unlocked()?.ok_or_else(|| {
            PortError::uncertain(
                "authorization_journal_missing",
                "authorization abort journal is missing",
            )
        })?;
        let matches_preparing = current.phase == AuthorizationReplacementPhase::Preparing
            && current.client_transaction_id == abort_journal.client_transaction_id
            && current.previous == abort_journal.previous
            && current.authorization_transaction_id == abort_journal.authorization_transaction_id;
        let matches_prepared = current.phase == AuthorizationReplacementPhase::Prepared
            && current.client_transaction_id == abort_journal.client_transaction_id
            && current.previous == abort_journal.previous
            && current.stage_id == abort_journal.stage_id
            && current.authorization_transaction_id == abort_journal.authorization_transaction_id
            && current.bundle_digest == abort_journal.bundle_digest
            && current.replacement == abort_journal.replacement;
        if current != abort_journal && !matches_preparing && !matches_prepared {
            return Err(PortError::new(
                "authorization_transaction_conflict",
                "authorization abort does not match its durable transaction",
            ));
        }
        self.write_authorization_journal_unlocked(&abort_journal)?;
        Ok(abort_journal)
    }

    #[cfg(any(not(windows), feature = "native-credentials"))]
    pub fn pending_authorization_replacement(
        &self,
    ) -> Result<Option<AuthorizationReplacementJournal>, PortError> {
        let _lock = FileLock::acquire(&self.lock_path, false)?;
        self.load_authorization_journal_unlocked()
    }

    /// Forward-complete a server-promoted replacement. Re-entry after any
    /// split local write is idempotent because the journal retains both exact
    /// bundles until both active stores and pending cleanup are verified.
    #[cfg(any(not(windows), feature = "native-credentials"))]
    pub fn finalize_promoted_authorization(
        &self,
        client_transaction_id: &str,
        session_store: &impl AuthorizationSessionStore,
    ) -> Result<AuthCredentialBundle, PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        let journal = self.load_authorization_journal_unlocked()?.ok_or_else(|| {
            PortError::uncertain(
                "authorization_journal_missing",
                "promoted authorization journal is missing",
            )
        })?;
        if journal.client_transaction_id != client_transaction_id
            || journal.phase != AuthorizationReplacementPhase::PromotionDispatched
        {
            return Err(PortError::new(
                "authorization_transaction_conflict",
                "promoted authorization does not match the durable transaction",
            ));
        }
        let replacement = journal.replacement.as_ref().ok_or_else(|| {
            PortError::new("authorization_journal", "replacement bundle is missing")
        })?;
        let active_auth = self
            .load_unlocked()?
            .ok_or_else(|| PortError::new("auth_missing", "authorization state is missing"))?;
        if active_auth.channel != journal.previous.channel
            && active_auth.channel != replacement.channel
        {
            return Err(PortError::uncertain(
                "authorization_version_conflict",
                "active channel changed outside the promoted transaction",
            ));
        }
        let active_session = session_store
            .load_authorized_session()?
            .ok_or_else(|| PortError::new("credentials_missing", "credentials are missing"))?;
        if active_session.account_id != journal.previous.session.account_id {
            return Err(PortError::uncertain(
                "authorization_session_version_conflict",
                "active session changed accounts outside the promoted transaction",
            ));
        }
        self.replace_unlocked(replacement).map_err(|error| {
            PortError::uncertain("authorization_bundle_replace", error.to_string())
        })?;
        session_store
            .replace_authorized_session(&replacement.session)
            .map_err(|error| {
                PortError::uncertain("authorization_session_replace", error.to_string())
            })?;
        let verified_auth = self.load_unlocked()?.ok_or_else(|| {
            PortError::uncertain("authorization_bundle_verify", "active bundle disappeared")
        })?;
        let verified_session = session_store.load_authorized_session()?.ok_or_else(|| {
            PortError::uncertain("authorization_session_verify", "active session disappeared")
        })?;
        if verified_auth != *replacement || verified_session != replacement.session {
            return Err(PortError::uncertain(
                "authorization_activation_verify",
                "promoted credentials were not durably activated in both stores",
            ));
        }
        session_store.clear_staged_authorized_session(
            &journal.client_transaction_id,
            &replacement.session,
        )?;
        write_authorization_cleanup_marker(
            &self.reconciliation_path,
            "promoted",
            &journal.client_transaction_id,
        )?;
        self.delete_authorization_journal_unlocked()?;
        clear_any_reconciliation_marker(&self.reconciliation_path)?;
        Ok(replacement.clone())
    }

    /// Finalize an authoritative aborted/expired stage. Active stores are
    /// verified, never overwritten from a possibly stale mirror.
    #[cfg(any(not(windows), feature = "native-credentials"))]
    pub fn finalize_unpromoted_authorization(
        &self,
        client_transaction_id: &str,
        session_store: &impl AuthorizationSessionStore,
    ) -> Result<AuthCredentialBundle, PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        let journal = self.load_authorization_journal_unlocked()?.ok_or_else(|| {
            PortError::uncertain(
                "authorization_journal_missing",
                "authorization replacement journal is missing",
            )
        })?;
        if journal.client_transaction_id != client_transaction_id {
            return Err(PortError::new(
                "authorization_transaction_conflict",
                "terminal authorization does not match the durable transaction",
            ));
        }
        let active_auth = self
            .load_unlocked()?
            .ok_or_else(|| PortError::new("auth_missing", "authorization state is missing"))?;
        let active_session = session_store
            .load_authorized_session()?
            .ok_or_else(|| PortError::new("credentials_missing", "credentials are missing"))?;
        if active_auth.channel.client_id != journal.previous.channel.client_id
            || active_auth.channel.device_id != journal.previous.channel.device_id
            || active_auth.channel.scope != journal.previous.channel.scope
            || active_auth.session.account_id != journal.previous.session.account_id
            || active_session.account_id != journal.previous.session.account_id
        {
            return Err(PortError::uncertain(
                "authorization_abort_local_conflict",
                "active credentials changed account or channel while the server stage was unpromoted",
            ));
        }
        if let Some(replacement) = &journal.replacement {
            session_store.clear_staged_authorized_session(
                &journal.client_transaction_id,
                &replacement.session,
            )?;
            write_authorization_cleanup_marker(
                &self.reconciliation_path,
                "unpromoted",
                &journal.client_transaction_id,
            )?;
        } else {
            write_authorization_cleanup_marker(
                &self.reconciliation_path,
                "unpromoted",
                &journal.client_transaction_id,
            )?;
        }
        self.delete_authorization_journal_unlocked()?;
        clear_any_reconciliation_marker(&self.reconciliation_path)?;
        Ok(AuthCredentialBundle {
            channel: active_auth.channel,
            session: active_session,
        })
    }

    /// Complete the final marker-only cleanup window. Data-bearing journal and
    /// pending-session entries are always removed before metadata and before
    /// this marker is cleared, so marker-only recovery contains no secrets.
    #[cfg(any(not(windows), feature = "native-credentials"))]
    pub fn finish_authorization_terminal_cleanup(&self) -> Result<bool, PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        if !self.reconciliation_path.exists() {
            return Ok(false);
        }
        let marker = read_limited(&self.reconciliation_path, 512)?;
        let Some((_terminal, _client_transaction_id)) =
            parse_authorization_cleanup_marker(&marker)?
        else {
            return Ok(false);
        };
        // The validated terminal marker is the cleanup commit and is written
        // only after active-state verification. Do not attempt to decode a
        // partially deleted split journal: idempotently remove data entries,
        // then metadata, then the marker.
        self.delete_authorization_journal_unlocked()?;
        clear_any_reconciliation_marker(&self.reconciliation_path)?;
        Ok(true)
    }

    #[cfg(all(windows, not(feature = "native-credentials")))]
    pub fn open(_root: impl AsRef<Path>) -> Result<Self, PortError> {
        Err(PortError::new(
            "auth_file_unsupported",
            "native-credentials is required for authorization storage on Windows",
        ))
    }

    #[cfg(not(windows))]
    fn write_authorization_journal_unlocked(
        &self,
        journal: &AuthorizationReplacementJournal,
    ) -> Result<(), PortError> {
        journal.validate()?;
        AtomicFile::replace(
            &self.replacement_path,
            &encode_authorization_journal(journal),
        )
        .map_err(|error| PortError::uncertain("authorization_journal_write", error.to_string()))
    }

    #[cfg(not(windows))]
    fn load_authorization_journal_unlocked(
        &self,
    ) -> Result<Option<AuthorizationReplacementJournal>, PortError> {
        if !self.replacement_path.exists() {
            return Ok(None);
        }
        decode_authorization_journal(&read_limited(&self.replacement_path, 4 * 1024 * 1024)?)
            .map(Some)
    }

    #[cfg(not(windows))]
    fn delete_authorization_journal_unlocked(&self) -> Result<(), PortError> {
        remove_private_state_file(&self.replacement_path)
    }

    #[cfg(not(windows))]
    pub fn initialize(&self, bundle: &AuthCredentialBundle) -> Result<(), PortError> {
        bundle
            .validate()
            .map_err(|error| PortError::new("auth_validation", error))?;
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        self.ensure_reconciled_unlocked()?;
        if self.state_path.exists() {
            return Err(PortError::new(
                "auth_exists",
                "an account is already connected; log out before registering another account",
            ));
        }
        AtomicFile::replace(&self.state_path, &encode_auth_bundle(bundle))?;
        clear_any_reconciliation_marker(&self.reconciliation_path)
    }

    #[cfg(not(windows))]
    pub fn load(&self) -> Result<Option<AuthCredentialBundle>, PortError> {
        let _lock = FileLock::acquire(&self.lock_path, false)?;
        self.ensure_reconciled_unlocked()?;
        self.load_unlocked()
    }

    #[cfg(not(windows))]
    pub fn replace(&self, bundle: &AuthCredentialBundle) -> Result<(), PortError> {
        bundle
            .validate()
            .map_err(|error| PortError::new("auth_validation", error))?;
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        self.replace_unlocked(bundle)?;
        clear_any_reconciliation_marker(&self.reconciliation_path)
    }

    #[cfg(not(windows))]
    fn load_unlocked(&self) -> Result<Option<AuthCredentialBundle>, PortError> {
        if !self.state_path.exists() {
            return Ok(None);
        }
        decode_auth_bundle(&read_limited(&self.state_path, 1024 * 1024)?).map(Some)
    }

    #[cfg(not(windows))]
    fn replace_unlocked(&self, bundle: &AuthCredentialBundle) -> Result<(), PortError> {
        bundle
            .validate()
            .map_err(|error| PortError::new("auth_validation", error))?;
        if !self.state_path.exists() {
            return Err(PortError::new(
                "auth_missing",
                "authorization state is missing",
            ));
        }
        AtomicFile::replace(&self.state_path, &encode_auth_bundle(bundle))
    }

    #[cfg(all(windows, feature = "native-credentials"))]
    pub fn initialize(&self, bundle: &AuthCredentialBundle) -> Result<(), PortError> {
        bundle
            .validate()
            .map_err(|error| PortError::new("auth_validation", error))?;
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        self.ensure_reconciled_unlocked()?;
        if self.read_windows_unlocked()?.is_some() {
            return Err(PortError::new(
                "auth_exists",
                "an account is already connected; log out before registering another account",
            ));
        }
        let document = encode_auth_bundle(bundle);
        if document.len() > WINDOWS_CREDENTIAL_BLOB_MAX_BYTES {
            return Err(PortError::new(
                "credential_manager_size",
                "authorization document exceeds the Windows Credential Manager limit",
            ));
        }
        self.windows_entry()?
            .set_secret(&document)
            .map_err(|error| keyring_error("credential_manager_write", error))?;
        clear_any_reconciliation_marker(&self.reconciliation_path)
    }

    #[cfg(all(windows, feature = "native-credentials"))]
    pub fn load(&self) -> Result<Option<AuthCredentialBundle>, PortError> {
        let _lock = FileLock::acquire(&self.lock_path, false)?;
        self.ensure_reconciled_unlocked()?;
        self.load_unlocked()
    }

    #[cfg(all(windows, feature = "native-credentials"))]
    pub fn replace(&self, bundle: &AuthCredentialBundle) -> Result<(), PortError> {
        bundle
            .validate()
            .map_err(|error| PortError::new("auth_validation", error))?;
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        self.replace_unlocked(bundle)?;
        clear_any_reconciliation_marker(&self.reconciliation_path)
    }

    #[cfg(all(windows, feature = "native-credentials"))]
    fn load_unlocked(&self) -> Result<Option<AuthCredentialBundle>, PortError> {
        self.read_windows_unlocked()
    }

    #[cfg(all(windows, feature = "native-credentials"))]
    fn replace_unlocked(&self, bundle: &AuthCredentialBundle) -> Result<(), PortError> {
        bundle
            .validate()
            .map_err(|error| PortError::new("auth_validation", error))?;
        if self.read_windows_unlocked()?.is_none() {
            return Err(PortError::new(
                "auth_missing",
                "authorization state is missing",
            ));
        }
        let document = encode_auth_bundle(bundle);
        if document.len() > WINDOWS_CREDENTIAL_BLOB_MAX_BYTES {
            return Err(PortError::new(
                "credential_manager_size",
                "authorization document exceeds the Windows Credential Manager limit",
            ));
        }
        self.windows_entry()?
            .set_secret(&document)
            .map_err(|error| keyring_error("credential_manager_write", error))
    }

    #[cfg(all(windows, feature = "native-credentials"))]
    pub fn delete(&self) -> Result<(), PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        self.ensure_reconciled_unlocked()?;
        match self.windows_entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {
                clear_any_reconciliation_marker(&self.reconciliation_path)
            }
            Err(error) => Err(keyring_error("credential_manager_delete", error)),
        }
    }

    #[cfg(all(windows, feature = "native-credentials"))]
    fn windows_entry(&self) -> Result<keyring::Entry, PortError> {
        keyring::Entry::new_with_target(&self.target, WINDOWS_CREDENTIAL_SERVICE, "authorization")
            .map_err(|error| keyring_error("credential_manager_entry", error))
    }

    #[cfg(all(windows, feature = "native-credentials"))]
    fn replacement_entry(&self, target: &str, username: &str) -> Result<keyring::Entry, PortError> {
        keyring::Entry::new_with_target(target, WINDOWS_CREDENTIAL_SERVICE, username)
            .map_err(|error| keyring_error("authorization_journal_entry", error))
    }

    #[cfg(all(windows, feature = "native-credentials"))]
    fn write_authorization_journal_unlocked(
        &self,
        journal: &AuthorizationReplacementJournal,
    ) -> Result<(), PortError> {
        journal.validate()?;
        let previous = encode_bound_auth_bundle(&journal.client_transaction_id, &journal.previous);
        let pending = journal
            .replacement
            .as_ref()
            .map(|bundle| encode_bound_auth_bundle(&journal.client_transaction_id, bundle));
        let metadata = encode_authorization_journal_metadata(journal);
        for (document, label) in [
            (&previous, "previous authorization bundle"),
            (&metadata, "authorization replacement metadata"),
        ] {
            if document.len() > WINDOWS_CREDENTIAL_BLOB_MAX_BYTES {
                return Err(PortError::new(
                    "credential_manager_size",
                    format!("{label} exceeds the Windows Credential Manager limit"),
                ));
            }
        }
        if pending
            .as_ref()
            .is_some_and(|document| document.len() > WINDOWS_CREDENTIAL_BLOB_MAX_BYTES)
        {
            return Err(PortError::new(
                "credential_manager_size",
                "pending authorization bundle exceeds the Windows Credential Manager limit",
            ));
        }
        self.replacement_entry(
            &self.replacement_previous_target,
            "authorization-replacement-previous",
        )?
        .set_secret(&previous)
        .map_err(|error| keyring_error("authorization_journal_previous_write", error))?;
        if let Some(pending) = pending {
            self.replacement_entry(
                &self.replacement_pending_target,
                "authorization-replacement-pending",
            )?
            .set_secret(&pending)
            .map_err(|error| keyring_error("authorization_journal_pending_write", error))?;
        }
        // Metadata is the commit record and is written last.
        self.replacement_entry(&self.replacement_target, "authorization-replacement")?
            .set_secret(&metadata)
            .map_err(|error| keyring_error("authorization_journal_write", error))
    }

    #[cfg(all(windows, feature = "native-credentials"))]
    fn load_authorization_journal_unlocked(
        &self,
    ) -> Result<Option<AuthorizationReplacementJournal>, PortError> {
        let metadata = match self
            .replacement_entry(&self.replacement_target, "authorization-replacement")?
            .get_secret()
        {
            Ok(document) => decode_authorization_journal_metadata(&document)?,
            Err(keyring::Error::NoEntry) => {
                let previous_exists = keyring_secret_exists(&self.replacement_entry(
                    &self.replacement_previous_target,
                    "authorization-replacement-previous",
                )?)?;
                let pending_exists = keyring_secret_exists(&self.replacement_entry(
                    &self.replacement_pending_target,
                    "authorization-replacement-pending",
                )?)?;
                if previous_exists || pending_exists {
                    return Err(PortError::uncertain(
                        "authorization_journal_incomplete",
                        "authorization journal data exists without its generation metadata",
                    ));
                }
                return Ok(None);
            }
            Err(error) => return Err(keyring_error("authorization_journal_read", error)),
        };
        let (previous_transaction_id, previous) = self
            .replacement_entry(
                &self.replacement_previous_target,
                "authorization-replacement-previous",
            )?
            .get_secret()
            .map_err(|error| keyring_error("authorization_journal_previous_read", error))
            .and_then(|document| decode_bound_auth_bundle(&document))?;
        if previous_transaction_id != metadata.client_transaction_id {
            return Err(PortError::uncertain(
                "authorization_journal_generation",
                "authorization journal previous bundle belongs to another generation",
            ));
        }
        let replacement = if metadata.phase == AuthorizationReplacementPhase::Preparing {
            // Metadata is the commit record. A pending blob written before a
            // crashed Prepared metadata update is intentionally invisible.
            None
        } else {
            match self
                .replacement_entry(
                    &self.replacement_pending_target,
                    "authorization-replacement-pending",
                )?
                .get_secret()
            {
                Ok(document) => {
                    let (transaction_id, bundle) = decode_bound_auth_bundle(&document)?;
                    if transaction_id != metadata.client_transaction_id {
                        return Err(PortError::uncertain(
                            "authorization_journal_generation",
                            "authorization journal pending bundle belongs to another generation",
                        ));
                    }
                    Some(bundle)
                }
                Err(keyring::Error::NoEntry) => {
                    return Err(PortError::uncertain(
                        "authorization_journal_incomplete",
                        "prepared authorization journal is missing its pending bundle",
                    ));
                }
                Err(error) => {
                    return Err(keyring_error("authorization_journal_pending_read", error));
                }
            }
        };
        let journal = AuthorizationReplacementJournal {
            phase: metadata.phase,
            client_transaction_id: metadata.client_transaction_id,
            stage_id: metadata.stage_id,
            authorization_transaction_id: metadata.authorization_transaction_id,
            provisional_access_token: metadata.provisional_access_token,
            recovery_token: metadata.recovery_token,
            bundle_digest: metadata.bundle_digest,
            previous,
            replacement,
        };
        journal.validate()?;
        Ok(Some(journal))
    }

    #[cfg(all(windows, feature = "native-credentials"))]
    fn delete_authorization_journal_unlocked(&self) -> Result<(), PortError> {
        for (target, username) in [
            (
                &self.replacement_pending_target,
                "authorization-replacement-pending",
            ),
            (
                &self.replacement_previous_target,
                "authorization-replacement-previous",
            ),
            (&self.replacement_target, "authorization-replacement"),
        ] {
            delete_keyring_entry(
                &self.replacement_entry(target, username)?,
                "authorization_journal_delete",
            )?;
        }
        Ok(())
    }

    #[cfg(all(windows, feature = "native-credentials"))]
    fn read_windows_unlocked(&self) -> Result<Option<AuthCredentialBundle>, PortError> {
        match self.windows_entry()?.get_secret() {
            Ok(document) => decode_auth_bundle(&document).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(keyring_error("credential_manager_read", error)),
        }
    }

    #[cfg(all(windows, not(feature = "native-credentials")))]
    pub fn initialize(&self, _bundle: &AuthCredentialBundle) -> Result<(), PortError> {
        Err(PortError::new(
            "auth_file_unsupported",
            "native-credentials is required for authorization storage on Windows",
        ))
    }

    #[cfg(all(windows, not(feature = "native-credentials")))]
    pub fn load(&self) -> Result<Option<AuthCredentialBundle>, PortError> {
        Err(PortError::new(
            "auth_file_unsupported",
            "native-credentials is required for authorization storage on Windows",
        ))
    }

    #[cfg(all(windows, not(feature = "native-credentials")))]
    pub fn replace(&self, _bundle: &AuthCredentialBundle) -> Result<(), PortError> {
        Err(PortError::new(
            "auth_file_unsupported",
            "native-credentials is required for authorization storage on Windows",
        ))
    }
}

impl FileCredentialStore {
    #[cfg(not(windows))]
    pub fn open(root: impl AsRef<Path>) -> Result<Self, PortError> {
        let root = root.as_ref();
        create_private_dir(root)?;
        Ok(Self {
            state_path: root.join("credentials.native"),
            authorization_stage_path: root.join("credentials.authorization-stage"),
            lock_path: root.join("credentials.lock"),
            reconciliation_path: root.join("credentials.reconciliation"),
        })
    }

    /// Reversible credential files are not permitted on Windows because the
    /// standard library cannot enforce a private owner-only ACL. Use
    /// [`WindowsCredentialStore`] with the `native-credentials` feature.
    #[cfg(windows)]
    pub fn open(_root: impl AsRef<Path>) -> Result<Self, PortError> {
        Err(PortError::new(
            "credential_file_unsupported",
            "file credential storage is disabled on Windows; enable native-credentials",
        ))
    }

    pub fn initialize(&self, credentials: &SessionCredentials) -> Result<(), PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        if self.reconciliation_path.exists() || self.authorization_stage_path.exists() {
            return Err(PortError::uncertain(
                "credential_reconciliation_required",
                "session state has an unresolved rotation or authorization replacement",
            ));
        }
        if self.state_path.exists() {
            return Err(PortError::new(
                "credentials_exist",
                "credentials have already been initialized",
            ));
        }
        AtomicFile::replace(
            &self.state_path,
            &CredentialState::new(credentials.clone()).encode(),
        )?;
        clear_any_reconciliation_marker(&self.reconciliation_path)
    }

    pub fn reconciliation_required(&self) -> Result<bool, PortError> {
        let _lock = FileLock::acquire(&self.lock_path, false)?;
        Ok(self.reconciliation_path.exists() || self.authorization_stage_path.exists())
    }

    /// Verified logout for the explicit owner-only fallback. The credential
    /// document and any stale reconciliation marker are removed under one
    /// process lock before another account can initialize this store.
    pub fn delete(&self) -> Result<(), PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        if self.reconciliation_path.exists() || self.authorization_stage_path.exists() {
            return Err(PortError::uncertain(
                "credential_reconciliation_required",
                "session state has an unresolved rotation or authorization replacement",
            ));
        }
        if self.state_path.exists() {
            fs::remove_file(&self.state_path)
                .map_err(|error| PortError::new("credential_file_delete", error.to_string()))?;
            if let Some(parent) = self.state_path.parent() {
                sync_directory(parent).map_err(|error| {
                    PortError::uncertain("credential_file_delete", error.to_string())
                })?;
            }
        }
        remove_private_state_file(&self.authorization_stage_path)?;
        clear_any_reconciliation_marker(&self.reconciliation_path)
    }

    fn read_unlocked(&self) -> Result<Option<CredentialState>, PortError> {
        if !self.state_path.exists() {
            return Ok(None);
        }
        CredentialState::decode(&read_limited(&self.state_path, 1024 * 1024)?).map(Some)
    }
}

impl AuthorizationSessionStore for FileCredentialStore {
    fn initialize_authorized_session(
        &self,
        credentials: &SessionCredentials,
    ) -> Result<(), PortError> {
        self.initialize(credentials)
    }

    fn load_authorized_session(&self) -> Result<Option<SessionCredentials>, PortError> {
        let _lock = FileLock::acquire(&self.lock_path, false)?;
        if self.reconciliation_path.exists() {
            return Err(PortError::uncertain(
                "credential_reconciliation_required",
                "session rotation has an unresolved outcome",
            ));
        }
        Ok(self.read_unlocked()?.map(|state| state.credentials))
    }

    fn replace_authorized_session(
        &self,
        credentials: &SessionCredentials,
    ) -> Result<(), PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        let current = self.read_unlocked()?.ok_or_else(|| {
            PortError::new("credentials_missing", "credentials are not initialized")
        })?;
        if current.credentials.account_id != credentials.account_id {
            return Err(PortError::new(
                "credential_account_conflict",
                "authorization replacement cannot change accounts",
            ));
        }
        AtomicFile::replace(
            &self.state_path,
            &CredentialState::new(credentials.clone()).encode(),
        )
    }

    fn stage_authorized_session(
        &self,
        client_transaction_id: &str,
        previous: &SessionCredentials,
        replacement: &SessionCredentials,
    ) -> Result<(), PortError> {
        validate_transaction_id("client transaction", client_transaction_id)?;
        if previous.account_id != replacement.account_id {
            return Err(PortError::new(
                "credential_account_conflict",
                "staged authorization cannot change accounts",
            ));
        }
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        let current = self.read_unlocked()?.ok_or_else(|| {
            PortError::new("credentials_missing", "credentials are not initialized")
        })?;
        if current.credentials != *previous {
            return Err(PortError::new(
                "authorization_session_version_conflict",
                "session changed while staged authorization was prepared",
            ));
        }
        let stage = AuthorizationSessionStage {
            client_transaction_id: client_transaction_id.to_owned(),
            previous: previous.clone(),
            replacement: replacement.clone(),
        };
        AtomicFile::replace(&self.authorization_stage_path, &stage.encode())
    }

    fn verify_staged_authorized_session(
        &self,
        client_transaction_id: &str,
        previous: &SessionCredentials,
        replacement: &SessionCredentials,
    ) -> Result<(), PortError> {
        let _lock = FileLock::acquire(&self.lock_path, false)?;
        let stage = AuthorizationSessionStage::decode(&read_limited(
            &self.authorization_stage_path,
            1024 * 1024,
        )?)?;
        stage.verify(client_transaction_id, previous, replacement)
    }

    fn clear_staged_authorized_session(
        &self,
        client_transaction_id: &str,
        expected_replacement: &SessionCredentials,
    ) -> Result<(), PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        if !self.authorization_stage_path.exists() {
            return Ok(());
        }
        let stage = AuthorizationSessionStage::decode(&read_limited(
            &self.authorization_stage_path,
            1024 * 1024,
        )?)?;
        if stage.client_transaction_id != client_transaction_id
            || stage.replacement != *expected_replacement
        {
            return Err(PortError::new(
                "authorization_session_stage_conflict",
                "stale cleanup cannot remove a different staged session",
            ));
        }
        remove_private_state_file(&self.authorization_stage_path)
    }
}

impl CredentialPort for FileCredentialStore {
    fn load(&self) -> BoxFuture<'_, Result<Option<SessionCredentials>, PortError>> {
        Box::pin(async move {
            let _lock = FileLock::acquire_async(&self.lock_path, false).await?;
            Ok(self.read_unlocked()?.map(|state| state.credentials))
        })
    }

    fn commit(&self, commit: CredentialCommit) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            let _lock = FileLock::acquire_async(&self.lock_path, true).await?;
            let mut state = self.read_unlocked()?.ok_or_else(|| {
                PortError::new("credentials_missing", "credentials are not initialized")
            })?;
            if credential_commit_is_already_reflected(&state.credentials, &commit)? {
                clear_reconciliation_marker(&self.reconciliation_path, commit.commit_id)?;
                return Ok(());
            }
            validate_credential_commit(&state.credentials, &commit)?;
            state.credentials = commit.credentials;
            AtomicFile::replace(&self.state_path, &state.encode())?;
            clear_reconciliation_marker(&self.reconciliation_path, commit.commit_id)
        })
    }

    fn mark_reconciliation_required(
        &self,
        commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            let _lock = FileLock::acquire_async(&self.lock_path, true).await?;
            AtomicFile::replace(
                &self.reconciliation_path,
                format!("{}\n", commit_id.as_uuid()).as_bytes(),
            )
        })
    }

    fn clear_reconciliation_required(
        &self,
        commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            let _lock = FileLock::acquire_async(&self.lock_path, true).await?;
            clear_reconciliation_marker(&self.reconciliation_path, commit_id)
        })
    }
}

#[cfg(feature = "native-credentials")]
const WINDOWS_CREDENTIAL_SERVICE: &str = "ai.frntr.heyfood";
#[cfg(feature = "native-credentials")]
const WINDOWS_CREDENTIAL_BLOB_MAX_BYTES: usize = 2_560;

/// Windows Credential Manager-backed session storage.
///
/// The complete credential document is stored as one Generic Credential so a
/// file containing reversible access or refresh tokens is never created.
#[cfg(all(windows, feature = "native-credentials"))]
#[derive(Clone, Debug)]
pub struct WindowsCredentialStore {
    target: String,
    authorization_stage_target: String,
    lock_path: PathBuf,
    reconciliation_path: PathBuf,
}

#[cfg(all(windows, feature = "native-credentials"))]
impl WindowsCredentialStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, PortError> {
        use std::os::windows::ffi::OsStrExt;

        let root = root.as_ref();
        create_private_dir(root)?;
        let mut identity = Vec::new();
        for unit in root.as_os_str().encode_wide() {
            identity.extend_from_slice(&unit.to_le_bytes());
        }
        let identity = hex_encode(&identity);
        Ok(Self {
            target: format!("{WINDOWS_CREDENTIAL_SERVICE}:{identity}"),
            authorization_stage_target: format!(
                "{WINDOWS_CREDENTIAL_SERVICE}:session-authorization-stage:{identity}"
            ),
            lock_path: root.join("credentials.lock"),
            reconciliation_path: root.join("credentials.reconciliation"),
        })
    }

    pub fn initialize(&self, credentials: &SessionCredentials) -> Result<(), PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        if self.reconciliation_path.exists() || self.read_authorization_stage_unlocked()?.is_some()
        {
            return Err(PortError::uncertain(
                "credential_reconciliation_required",
                "session state has an unresolved rotation or authorization replacement",
            ));
        }
        if self.read_unlocked()?.is_some() {
            return Err(PortError::new(
                "credentials_exist",
                "credentials have already been initialized",
            ));
        }
        self.write_unlocked(&CredentialState::new(credentials.clone()))?;
        clear_any_reconciliation_marker(&self.reconciliation_path)
    }

    pub fn reconciliation_required(&self) -> Result<bool, PortError> {
        let _lock = FileLock::acquire(&self.lock_path, false)?;
        Ok(
            self.reconciliation_path.exists()
                || self.read_authorization_stage_unlocked()?.is_some(),
        )
    }

    /// Delete the native credential. This is also the logout/test-cleanup seam.
    pub fn delete(&self) -> Result<(), PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        if self.reconciliation_path.exists() || self.read_authorization_stage_unlocked()?.is_some()
        {
            return Err(PortError::uncertain(
                "credential_reconciliation_required",
                "session state has an unresolved rotation or authorization replacement",
            ));
        }
        delete_keyring_entry(&self.entry()?, "credential_manager_delete")?;
        clear_any_reconciliation_marker(&self.reconciliation_path)
    }

    fn entry(&self) -> Result<keyring::Entry, PortError> {
        keyring::Entry::new_with_target(&self.target, WINDOWS_CREDENTIAL_SERVICE, "session")
            .map_err(|error| keyring_error("credential_manager_entry", error))
    }

    fn authorization_stage_entry(&self) -> Result<keyring::Entry, PortError> {
        keyring::Entry::new_with_target(
            &self.authorization_stage_target,
            WINDOWS_CREDENTIAL_SERVICE,
            "authorization-session-stage",
        )
        .map_err(|error| keyring_error("credential_manager_stage_entry", error))
    }

    fn read_authorization_stage_unlocked(
        &self,
    ) -> Result<Option<AuthorizationSessionStage>, PortError> {
        match self.authorization_stage_entry()?.get_secret() {
            Ok(document) => AuthorizationSessionStage::decode(&document).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(keyring_error("credential_manager_stage_read", error)),
        }
    }

    fn write_authorization_stage_unlocked(
        &self,
        stage: &AuthorizationSessionStage,
    ) -> Result<(), PortError> {
        let document = stage.encode();
        if document.len() > WINDOWS_CREDENTIAL_BLOB_MAX_BYTES {
            return Err(PortError::new(
                "credential_manager_size",
                "staged session exceeds the Windows Credential Manager limit",
            ));
        }
        self.authorization_stage_entry()?
            .set_secret(&document)
            .map_err(|error| keyring_error("credential_manager_stage_write", error))
    }

    fn read_unlocked(&self) -> Result<Option<CredentialState>, PortError> {
        match self.entry()?.get_secret() {
            Ok(document) => CredentialState::decode(&document).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(keyring_error("credential_manager_read", error)),
        }
    }

    fn write_unlocked(&self, state: &CredentialState) -> Result<(), PortError> {
        let document = state.encode();
        if document.len() > WINDOWS_CREDENTIAL_BLOB_MAX_BYTES {
            return Err(PortError::new(
                "credential_manager_size",
                "credential document exceeds the Windows Credential Manager limit",
            ));
        }
        self.entry()?
            .set_secret(&document)
            .map_err(|error| keyring_error("credential_manager_write", error))?;
        verify_credential_write_visibility(
            state,
            CREDENTIAL_WRITE_VERIFY_TIMEOUT,
            || self.read_unlocked(),
            thread::sleep,
        )
    }

    pub(crate) fn broker_load(&self) -> Result<Option<SessionCredentials>, PortError> {
        let _lock = FileLock::acquire(&self.lock_path, false)?;
        Ok(self.read_unlocked()?.map(|state| state.credentials))
    }

    pub(crate) fn broker_commit(&self, commit: CredentialCommit) -> Result<(), PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        let mut state = self.read_unlocked()?.ok_or_else(|| {
            PortError::new("credentials_missing", "credentials are not initialized")
        })?;
        if credential_commit_is_already_reflected(&state.credentials, &commit)? {
            return clear_reconciliation_marker(&self.reconciliation_path, commit.commit_id);
        }
        validate_credential_commit(&state.credentials, &commit)?;
        state.credentials = commit.credentials;
        self.write_unlocked(&state)?;
        clear_reconciliation_marker(&self.reconciliation_path, commit.commit_id)
    }

    pub(crate) fn broker_mark(&self, commit_id: CommitId) -> Result<(), PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        AtomicFile::replace(
            &self.reconciliation_path,
            format!("{}\n", commit_id.as_uuid()).as_bytes(),
        )
    }

    pub(crate) fn broker_clear(&self, commit_id: CommitId) -> Result<(), PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        clear_reconciliation_marker(&self.reconciliation_path, commit_id)
    }
}

#[cfg(all(windows, feature = "native-credentials"))]
impl AuthorizationSessionStore for WindowsCredentialStore {
    fn initialize_authorized_session(
        &self,
        credentials: &SessionCredentials,
    ) -> Result<(), PortError> {
        self.initialize(credentials)
    }

    fn load_authorized_session(&self) -> Result<Option<SessionCredentials>, PortError> {
        let _lock = FileLock::acquire(&self.lock_path, false)?;
        if self.reconciliation_path.exists() {
            return Err(PortError::uncertain(
                "credential_reconciliation_required",
                "session rotation has an unresolved outcome",
            ));
        }
        Ok(self.read_unlocked()?.map(|state| state.credentials))
    }

    fn replace_authorized_session(
        &self,
        credentials: &SessionCredentials,
    ) -> Result<(), PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        let current = self.read_unlocked()?.ok_or_else(|| {
            PortError::new("credentials_missing", "credentials are not initialized")
        })?;
        if current.credentials.account_id != credentials.account_id {
            return Err(PortError::new(
                "credential_account_conflict",
                "authorization replacement cannot change accounts",
            ));
        }
        self.write_unlocked(&CredentialState::new(credentials.clone()))
    }

    fn stage_authorized_session(
        &self,
        client_transaction_id: &str,
        previous: &SessionCredentials,
        replacement: &SessionCredentials,
    ) -> Result<(), PortError> {
        validate_transaction_id("client transaction", client_transaction_id)?;
        if previous.account_id != replacement.account_id {
            return Err(PortError::new(
                "credential_account_conflict",
                "staged authorization cannot change accounts",
            ));
        }
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        let current = self.read_unlocked()?.ok_or_else(|| {
            PortError::new("credentials_missing", "credentials are not initialized")
        })?;
        if current.credentials != *previous {
            return Err(PortError::new(
                "authorization_session_version_conflict",
                "session changed while staged authorization was prepared",
            ));
        }
        self.write_authorization_stage_unlocked(&AuthorizationSessionStage {
            client_transaction_id: client_transaction_id.to_owned(),
            previous: previous.clone(),
            replacement: replacement.clone(),
        })
    }

    fn verify_staged_authorized_session(
        &self,
        client_transaction_id: &str,
        previous: &SessionCredentials,
        replacement: &SessionCredentials,
    ) -> Result<(), PortError> {
        let _lock = FileLock::acquire(&self.lock_path, false)?;
        self.read_authorization_stage_unlocked()?
            .ok_or_else(|| {
                PortError::new(
                    "authorization_session_stage_missing",
                    "staged session is missing",
                )
            })?
            .verify(client_transaction_id, previous, replacement)
    }

    fn clear_staged_authorized_session(
        &self,
        client_transaction_id: &str,
        expected_replacement: &SessionCredentials,
    ) -> Result<(), PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        let Some(stage) = self.read_authorization_stage_unlocked()? else {
            return Ok(());
        };
        if stage.client_transaction_id != client_transaction_id
            || stage.replacement != *expected_replacement
        {
            return Err(PortError::new(
                "authorization_session_stage_conflict",
                "stale cleanup cannot remove a different staged session",
            ));
        }
        delete_keyring_entry(
            &self.authorization_stage_entry()?,
            "credential_manager_stage_delete",
        )
    }
}

#[cfg(all(windows, feature = "native-credentials"))]
impl CredentialPort for WindowsCredentialStore {
    fn load(&self) -> BoxFuture<'_, Result<Option<SessionCredentials>, PortError>> {
        Box::pin(async move {
            let _lock = FileLock::acquire_async(&self.lock_path, false).await?;
            Ok(self.read_unlocked()?.map(|state| state.credentials))
        })
    }

    fn commit(&self, commit: CredentialCommit) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            let _lock = FileLock::acquire_async(&self.lock_path, true).await?;
            let mut state = self.read_unlocked()?.ok_or_else(|| {
                PortError::new("credentials_missing", "credentials are not initialized")
            })?;
            if credential_commit_is_already_reflected(&state.credentials, &commit)? {
                clear_reconciliation_marker(&self.reconciliation_path, commit.commit_id)?;
                return Ok(());
            }
            validate_credential_commit(&state.credentials, &commit)?;
            state.credentials = commit.credentials;
            self.write_unlocked(&state)?;
            clear_reconciliation_marker(&self.reconciliation_path, commit.commit_id)
        })
    }

    fn mark_reconciliation_required(
        &self,
        commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            let _lock = FileLock::acquire_async(&self.lock_path, true).await?;
            AtomicFile::replace(
                &self.reconciliation_path,
                format!("{}\n", commit_id.as_uuid()).as_bytes(),
            )
        })
    }

    fn clear_reconciliation_required(
        &self,
        commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            let _lock = FileLock::acquire_async(&self.lock_path, true).await?;
            clear_reconciliation_marker(&self.reconciliation_path, commit_id)
        })
    }
}

/// macOS Keychain / Linux Secret Service-backed session storage. The standard
/// file store remains an explicit owner-only fallback for headless systems;
/// selection is made by the composition root and is never silent downgrade.
#[cfg(all(not(windows), feature = "native-credentials"))]
#[derive(Clone, Debug)]
pub struct KeyringCredentialStore {
    target: String,
    authorization_stage_target: String,
    lock_path: PathBuf,
    reconciliation_path: PathBuf,
}

#[cfg(all(not(windows), feature = "native-credentials"))]
impl KeyringCredentialStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, PortError> {
        let root = root.as_ref();
        create_private_dir(root)?;
        let identity = hex_encode(root.to_string_lossy().as_bytes());
        Ok(Self {
            target: format!("{WINDOWS_CREDENTIAL_SERVICE}:{identity}"),
            authorization_stage_target: format!(
                "{WINDOWS_CREDENTIAL_SERVICE}:session-authorization-stage:{identity}"
            ),
            lock_path: root.join("credentials.lock"),
            reconciliation_path: root.join("credentials.reconciliation"),
        })
    }

    pub fn initialize(&self, credentials: &SessionCredentials) -> Result<(), PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        if self.read_authorization_stage_unlocked()?.is_some() {
            return Err(PortError::uncertain(
                "credential_reconciliation_required",
                "session state has an unresolved authorization replacement",
            ));
        }
        if self.read_unlocked()?.is_some() {
            return Err(PortError::new(
                "credentials_exist",
                "credentials have already been initialized",
            ));
        }
        self.write_unlocked(&CredentialState::new(credentials.clone()))?;
        clear_any_reconciliation_marker(&self.reconciliation_path)
    }

    pub fn reconciliation_required(&self) -> Result<bool, PortError> {
        let _lock = FileLock::acquire(&self.lock_path, false)?;
        Ok(
            self.reconciliation_path.exists()
                || self.read_authorization_stage_unlocked()?.is_some(),
        )
    }

    pub fn delete(&self) -> Result<(), PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        if self.read_authorization_stage_unlocked()?.is_some() {
            return Err(PortError::uncertain(
                "credential_reconciliation_required",
                "session state has an unresolved authorization replacement",
            ));
        }
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {
                clear_any_reconciliation_marker(&self.reconciliation_path)
            }
            Err(error) => Err(keyring_error("native_keyring_delete", error)),
        }
    }

    fn entry(&self) -> Result<keyring::Entry, PortError> {
        keyring::Entry::new(&self.target, "session")
            .map_err(|error| keyring_error("native_keyring_entry", error))
    }

    fn authorization_stage_entry(&self) -> Result<keyring::Entry, PortError> {
        keyring::Entry::new(
            &self.authorization_stage_target,
            "authorization-session-stage",
        )
        .map_err(|error| keyring_error("native_keyring_stage_entry", error))
    }

    fn read_authorization_stage_unlocked(
        &self,
    ) -> Result<Option<AuthorizationSessionStage>, PortError> {
        match self.authorization_stage_entry()?.get_secret() {
            Ok(document) => AuthorizationSessionStage::decode(&document).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(keyring_error("native_keyring_stage_read", error)),
        }
    }

    fn write_authorization_stage_unlocked(
        &self,
        stage: &AuthorizationSessionStage,
    ) -> Result<(), PortError> {
        let document = stage.encode();
        if document.len() > WINDOWS_CREDENTIAL_BLOB_MAX_BYTES {
            return Err(PortError::new(
                "native_keyring_size",
                "staged session exceeds the native keyring limit",
            ));
        }
        self.authorization_stage_entry()?
            .set_secret(&document)
            .map_err(|error| keyring_error("native_keyring_stage_write", error))
    }

    fn read_unlocked(&self) -> Result<Option<CredentialState>, PortError> {
        match self.entry()?.get_secret() {
            Ok(document) => CredentialState::decode(&document).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(keyring_error("native_keyring_read", error)),
        }
    }

    fn write_unlocked(&self, state: &CredentialState) -> Result<(), PortError> {
        let document = state.encode();
        if document.len() > WINDOWS_CREDENTIAL_BLOB_MAX_BYTES {
            return Err(PortError::new(
                "native_keyring_size",
                "credential document exceeds the native keyring limit",
            ));
        }
        self.entry()?
            .set_secret(&document)
            .map_err(|error| keyring_error("native_keyring_write", error))
    }

    pub(crate) fn broker_load(&self) -> Result<Option<SessionCredentials>, PortError> {
        let _lock = FileLock::acquire(&self.lock_path, false)?;
        Ok(self.read_unlocked()?.map(|state| state.credentials))
    }

    pub(crate) fn broker_commit(&self, commit: CredentialCommit) -> Result<(), PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        let mut state = self.read_unlocked()?.ok_or_else(|| {
            PortError::new("credentials_missing", "credentials are not initialized")
        })?;
        if credential_commit_is_already_reflected(&state.credentials, &commit)? {
            return clear_reconciliation_marker(&self.reconciliation_path, commit.commit_id);
        }
        validate_credential_commit(&state.credentials, &commit)?;
        state.credentials = commit.credentials;
        self.write_unlocked(&state)?;
        clear_reconciliation_marker(&self.reconciliation_path, commit.commit_id)
    }

    pub(crate) fn broker_mark(&self, commit_id: CommitId) -> Result<(), PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        AtomicFile::replace(
            &self.reconciliation_path,
            format!("{}\n", commit_id.as_uuid()).as_bytes(),
        )
    }

    pub(crate) fn broker_clear(&self, commit_id: CommitId) -> Result<(), PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        clear_reconciliation_marker(&self.reconciliation_path, commit_id)
    }
}

#[cfg(all(not(windows), feature = "native-credentials"))]
impl AuthorizationSessionStore for KeyringCredentialStore {
    fn initialize_authorized_session(
        &self,
        credentials: &SessionCredentials,
    ) -> Result<(), PortError> {
        self.initialize(credentials)
    }

    fn load_authorized_session(&self) -> Result<Option<SessionCredentials>, PortError> {
        let _lock = FileLock::acquire(&self.lock_path, false)?;
        if self.reconciliation_path.exists() {
            return Err(PortError::uncertain(
                "credential_reconciliation_required",
                "session rotation has an unresolved outcome",
            ));
        }
        Ok(self.read_unlocked()?.map(|state| state.credentials))
    }

    fn replace_authorized_session(
        &self,
        credentials: &SessionCredentials,
    ) -> Result<(), PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        let current = self.read_unlocked()?.ok_or_else(|| {
            PortError::new("credentials_missing", "credentials are not initialized")
        })?;
        if current.credentials.account_id != credentials.account_id {
            return Err(PortError::new(
                "credential_account_conflict",
                "authorization replacement cannot change accounts",
            ));
        }
        self.write_unlocked(&CredentialState::new(credentials.clone()))
    }

    fn stage_authorized_session(
        &self,
        client_transaction_id: &str,
        previous: &SessionCredentials,
        replacement: &SessionCredentials,
    ) -> Result<(), PortError> {
        validate_transaction_id("client transaction", client_transaction_id)?;
        if previous.account_id != replacement.account_id {
            return Err(PortError::new(
                "credential_account_conflict",
                "staged authorization cannot change accounts",
            ));
        }
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        let current = self.read_unlocked()?.ok_or_else(|| {
            PortError::new("credentials_missing", "credentials are not initialized")
        })?;
        if current.credentials != *previous {
            return Err(PortError::new(
                "authorization_session_version_conflict",
                "session changed while staged authorization was prepared",
            ));
        }
        self.write_authorization_stage_unlocked(&AuthorizationSessionStage {
            client_transaction_id: client_transaction_id.to_owned(),
            previous: previous.clone(),
            replacement: replacement.clone(),
        })
    }

    fn verify_staged_authorized_session(
        &self,
        client_transaction_id: &str,
        previous: &SessionCredentials,
        replacement: &SessionCredentials,
    ) -> Result<(), PortError> {
        let _lock = FileLock::acquire(&self.lock_path, false)?;
        self.read_authorization_stage_unlocked()?
            .ok_or_else(|| {
                PortError::new(
                    "authorization_session_stage_missing",
                    "staged session is missing",
                )
            })?
            .verify(client_transaction_id, previous, replacement)
    }

    fn clear_staged_authorized_session(
        &self,
        client_transaction_id: &str,
        expected_replacement: &SessionCredentials,
    ) -> Result<(), PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        let Some(stage) = self.read_authorization_stage_unlocked()? else {
            return Ok(());
        };
        if stage.client_transaction_id != client_transaction_id
            || stage.replacement != *expected_replacement
        {
            return Err(PortError::new(
                "authorization_session_stage_conflict",
                "stale cleanup cannot remove a different staged session",
            ));
        }
        delete_keyring_entry(
            &self.authorization_stage_entry()?,
            "native_keyring_stage_delete",
        )
    }
}

#[cfg(all(not(windows), feature = "native-credentials"))]
impl CredentialPort for KeyringCredentialStore {
    fn load(&self) -> BoxFuture<'_, Result<Option<SessionCredentials>, PortError>> {
        Box::pin(async move {
            let _lock = FileLock::acquire_async(&self.lock_path, false).await?;
            Ok(self.read_unlocked()?.map(|state| state.credentials))
        })
    }

    fn commit(&self, commit: CredentialCommit) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            let _lock = FileLock::acquire_async(&self.lock_path, true).await?;
            let mut state = self.read_unlocked()?.ok_or_else(|| {
                PortError::new("credentials_missing", "credentials are not initialized")
            })?;
            if credential_commit_is_already_reflected(&state.credentials, &commit)? {
                clear_reconciliation_marker(&self.reconciliation_path, commit.commit_id)?;
                return Ok(());
            }
            validate_credential_commit(&state.credentials, &commit)?;
            state.credentials = commit.credentials;
            self.write_unlocked(&state)?;
            clear_reconciliation_marker(&self.reconciliation_path, commit.commit_id)
        })
    }

    fn mark_reconciliation_required(
        &self,
        commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            let _lock = FileLock::acquire_async(&self.lock_path, true).await?;
            AtomicFile::replace(
                &self.reconciliation_path,
                format!("{}\n", commit_id.as_uuid()).as_bytes(),
            )
        })
    }

    fn clear_reconciliation_required(
        &self,
        commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            let _lock = FileLock::acquire_async(&self.lock_path, true).await?;
            clear_reconciliation_marker(&self.reconciliation_path, commit_id)
        })
    }
}

fn validate_credential_commit(
    current: &SessionCredentials,
    commit: &CredentialCommit,
) -> Result<(), PortError> {
    if current.version != commit.expected_version {
        return Err(PortError::new(
            "credential_version_conflict",
            "stored credential version does not match the expected version",
        ));
    }
    if current.account_id != commit.credentials.account_id {
        return Err(PortError::new(
            "credential_account_conflict",
            "credential rotation cannot change accounts",
        ));
    }
    if commit.credentials.version <= commit.expected_version {
        return Err(PortError::new(
            "credential_version_conflict",
            "credential rotation must advance the version",
        ));
    }
    Ok(())
}

fn credential_commit_is_already_reflected(
    current: &SessionCredentials,
    commit: &CredentialCommit,
) -> Result<bool, PortError> {
    if current.account_id != commit.credentials.account_id {
        return Err(PortError::new(
            "credential_account_conflict",
            "credential rotation cannot change accounts",
        ));
    }
    if commit.credentials.version <= commit.expected_version {
        return Err(PortError::new(
            "credential_version_conflict",
            "credential rotation must advance the version",
        ));
    }
    if current == &commit.credentials {
        return Ok(true);
    }
    if current.version > commit.credentials.version {
        // A later accepted rotation safely supersedes a replay of this older
        // durable proposal. Never roll credentials backward.
        return Ok(true);
    }
    if current.version == commit.credentials.version {
        return Err(PortError::new(
            "credential_version_conflict",
            "stored credential version has different token material",
        ));
    }
    Ok(false)
}

fn clear_reconciliation_marker(path: &Path, commit_id: CommitId) -> Result<(), PortError> {
    if !path.exists() {
        return Ok(());
    }
    let marker = read_limited(path, 128)
        .map_err(|error| PortError::uncertain("reconciliation_read", error.to_string()))?;
    let expected = format!("{}\n", commit_id.as_uuid());
    if marker != expected.as_bytes() {
        return Ok(());
    }
    fs::remove_file(path)
        .map_err(|error| PortError::uncertain("reconciliation_clear", error.to_string()))?;
    if let Some(parent) = path.parent() {
        sync_directory(parent)
            .map_err(|error| PortError::uncertain("reconciliation_clear", error.to_string()))?;
    }
    Ok(())
}

fn clear_any_reconciliation_marker(path: &Path) -> Result<(), PortError> {
    if !path.exists() {
        return Ok(());
    }
    fs::remove_file(path)
        .map_err(|error| PortError::uncertain("reconciliation_clear", error.to_string()))?;
    if let Some(parent) = path.parent() {
        sync_directory(parent)
            .map_err(|error| PortError::uncertain("reconciliation_clear", error.to_string()))?;
    }
    Ok(())
}

fn write_authorization_cleanup_marker(
    path: &Path,
    terminal: &str,
    client_transaction_id: &str,
) -> Result<(), PortError> {
    if !matches!(terminal, "promoted" | "unpromoted") {
        return Err(PortError::new(
            "authorization_cleanup_marker",
            "invalid terminal cleanup state",
        ));
    }
    validate_transaction_id("client transaction", client_transaction_id)?;
    AtomicFile::replace(
        path,
        format!(
            "authorization_replacement_cleanup:{terminal}:{}\n",
            hex_encode(client_transaction_id.as_bytes())
        )
        .as_bytes(),
    )
    .map_err(|error| PortError::uncertain("authorization_cleanup_marker", error.to_string()))
}

fn parse_authorization_cleanup_marker(
    marker: &[u8],
) -> Result<Option<(&'static str, String)>, PortError> {
    let marker = std::str::from_utf8(marker).map_err(|_| {
        PortError::uncertain(
            "authorization_cleanup_marker",
            "authorization cleanup marker is invalid",
        )
    })?;
    let Some(value) = marker
        .strip_prefix("authorization_replacement_cleanup:")
        .and_then(|value| value.strip_suffix('\n'))
    else {
        return Ok(None);
    };
    let (terminal, transaction) = value.split_once(':').ok_or_else(|| {
        PortError::uncertain(
            "authorization_cleanup_marker",
            "authorization cleanup marker is incomplete",
        )
    })?;
    let terminal = match terminal {
        "promoted" => "promoted",
        "unpromoted" => "unpromoted",
        _ => {
            return Err(PortError::uncertain(
                "authorization_cleanup_marker",
                "authorization cleanup marker has an unknown terminal state",
            ));
        }
    };
    let transaction = hex_string(transaction)?;
    validate_transaction_id("client transaction", &transaction)?;
    Ok(Some((terminal, transaction)))
}

#[cfg(feature = "native-credentials")]
fn keyring_error(code: &'static str, error: keyring::Error) -> PortError {
    PortError::new(code, error.to_string())
}

#[cfg(feature = "native-credentials")]
fn delete_keyring_entry(entry: &keyring::Entry, code: &'static str) -> Result<(), PortError> {
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(keyring_error(code, error)),
    }
}

#[cfg(all(windows, feature = "native-credentials"))]
fn keyring_secret_exists(entry: &keyring::Entry) -> Result<bool, PortError> {
    match entry.get_secret() {
        Ok(_) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(error) => Err(keyring_error("credential_manager_read", error)),
    }
}

pub(crate) struct AuthorizationSessionStage {
    pub(crate) client_transaction_id: String,
    pub(crate) previous: SessionCredentials,
    pub(crate) replacement: SessionCredentials,
}

impl AuthorizationSessionStage {
    pub(crate) fn encode(&self) -> Vec<u8> {
        format!(
            "schema=1\nclient_transaction={}\nprevious={}\nreplacement={}\n",
            hex_encode(self.client_transaction_id.as_bytes()),
            hex_encode(&CredentialState::new(self.previous.clone()).encode()),
            hex_encode(&CredentialState::new(self.replacement.clone()).encode()),
        )
        .into_bytes()
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, PortError> {
        let values = fields(bytes)?;
        if required(&values, "schema")? != "1" {
            return Err(PortError::new(
                "authorization_session_stage_schema",
                "unsupported staged session schema",
            ));
        }
        let client_transaction_id = hex_string(required(&values, "client_transaction")?)?;
        validate_transaction_id("client transaction", &client_transaction_id)?;
        let previous =
            CredentialState::decode(&hex_decode(required(&values, "previous")?)?)?.credentials;
        let replacement =
            CredentialState::decode(&hex_decode(required(&values, "replacement")?)?)?.credentials;
        if previous.account_id != replacement.account_id {
            return Err(PortError::new(
                "authorization_session_stage_account",
                "staged session changed accounts",
            ));
        }
        Ok(Self {
            client_transaction_id,
            previous,
            replacement,
        })
    }

    fn verify(
        &self,
        client_transaction_id: &str,
        previous: &SessionCredentials,
        replacement: &SessionCredentials,
    ) -> Result<(), PortError> {
        if self.client_transaction_id != client_transaction_id
            || self.previous != *previous
            || self.replacement != *replacement
        {
            return Err(PortError::new(
                "authorization_session_stage_conflict",
                "staged session does not match the authorization journal",
            ));
        }
        Ok(())
    }
}

pub(crate) struct CredentialState {
    pub(crate) credentials: SessionCredentials,
}

impl CredentialState {
    pub(crate) fn new(credentials: SessionCredentials) -> Self {
        Self { credentials }
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        format!(
            "schema=2\naccount={}\naccess={}\nrefresh={}\nversion={}\nexpires={}\n",
            hex_encode(self.credentials.account_id.as_str().as_bytes()),
            hex_encode(self.credentials.access_token.expose_secret().as_bytes()),
            hex_encode(self.credentials.refresh_token.expose_secret().as_bytes()),
            self.credentials.version.get(),
            self.credentials.expires_at_unix(),
        )
        .into_bytes()
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, PortError> {
        let fields = fields(bytes)?;
        let schema = required(&fields, "schema")?;
        if schema != "1" && schema != "2" {
            return Err(PortError::new(
                "credential_schema",
                "unsupported credential schema",
            ));
        }
        if schema == "1" {
            // Schema 1 retained an unbounded commit-ID set. Validate it while
            // migrating, then discard it: credential version monotonicity is
            // the bounded durable idempotency key in schema 2.
            let _ = parse_commit_set(required(&fields, "applied")?)?;
        }
        let account_id = AccountId::parse(hex_string(required(&fields, "account")?)?)
            .map_err(|error| PortError::new("credential_account", error))?;
        let access_token = SensitiveString::new(hex_string(required(&fields, "access")?)?);
        let refresh_token = SensitiveString::new(hex_string(required(&fields, "refresh")?)?);
        let version = required(&fields, "version")?
            .parse::<u64>()
            .map(CredentialVersion::new)
            .map_err(|_| PortError::new("credential_version", "invalid credential version"))?;
        let expires = required(&fields, "expires")?
            .parse::<i64>()
            .map_err(|_| PortError::new("credential_expiry", "invalid credential expiry"))?;
        let credentials = SessionCredentials::from_unix_expiry(
            account_id,
            access_token,
            refresh_token,
            version,
            expires,
        )
        .map_err(|error| PortError::new("credential_expiry", error))?;
        Ok(Self { credentials })
    }
}

#[cfg(any(windows, test))]
fn verify_credential_write_visibility(
    expected: &CredentialState,
    timeout: Duration,
    mut read: impl FnMut() -> Result<Option<CredentialState>, PortError>,
    mut wait: impl FnMut(Duration),
) -> Result<(), PortError> {
    let started = Instant::now();
    loop {
        match read() {
            Ok(Some(actual)) if actual.credentials == expected.credentials => return Ok(()),
            Ok(Some(actual))
                if actual.credentials.account_id == expected.credentials.account_id
                    && actual.credentials.version < expected.credentials.version => {}
            Ok(None) => {}
            Ok(Some(_)) | Err(_) => return Err(credential_write_verify_error()),
        }
        if started.elapsed() >= timeout {
            return Err(credential_write_verify_error());
        }
        wait(CREDENTIAL_WRITE_VERIFY_INTERVAL);
    }
}

#[cfg(any(windows, test))]
fn credential_write_verify_error() -> PortError {
    PortError::uncertain(
        "credential_manager_write_verify",
        "Windows Credential Manager did not return the exact credential state after a successful write",
    )
}

fn encode_auth_bundle(bundle: &AuthCredentialBundle) -> Vec<u8> {
    format!(
        concat!(
            "schema=1\n",
            "client={}\n",
            "device={}\n",
            "channel_access={}\n",
            "channel_refresh={}\n",
            "channel_expires={}\n",
            "channel_scope={}\n",
            "account={}\n",
            "session_access={}\n",
            "session_refresh={}\n",
            "session_version={}\n",
            "session_expires={}\n"
        ),
        hex_encode(bundle.channel.client_id.as_bytes()),
        hex_encode(bundle.channel.device_id.as_bytes()),
        hex_encode(bundle.channel.access_token.expose_secret().as_bytes()),
        hex_encode(bundle.channel.refresh_token.expose_secret().as_bytes()),
        bundle.channel.expires_at_unix(),
        hex_encode(bundle.channel.scope.as_bytes()),
        hex_encode(bundle.session.account_id.as_str().as_bytes()),
        hex_encode(bundle.session.access_token.expose_secret().as_bytes()),
        hex_encode(bundle.session.refresh_token.expose_secret().as_bytes()),
        bundle.session.version.get(),
        bundle.session.expires_at_unix(),
    )
    .into_bytes()
}

fn decode_auth_bundle(bytes: &[u8]) -> Result<AuthCredentialBundle, PortError> {
    let values = fields(bytes)?;
    if required(&values, "schema")? != "1" {
        return Err(PortError::new(
            "auth_schema",
            "unsupported authorization schema",
        ));
    }
    let channel_expires = required(&values, "channel_expires")?
        .parse::<i64>()
        .map_err(|_| PortError::new("auth_expiry", "invalid channel credential expiry"))?;
    let channel = ChannelCredentials::from_unix_expiry(
        hex_string(required(&values, "client")?)?,
        hex_string(required(&values, "device")?)?,
        SensitiveString::new(hex_string(required(&values, "channel_access")?)?),
        SensitiveString::new(hex_string(required(&values, "channel_refresh")?)?),
        channel_expires,
        hex_string(required(&values, "channel_scope")?)?,
    )
    .map_err(|error| PortError::new("auth_channel", error))?;
    let account = AccountId::parse(hex_string(required(&values, "account")?)?)
        .map_err(|error| PortError::new("auth_account", error))?;
    let version = required(&values, "session_version")?
        .parse::<u64>()
        .map(CredentialVersion::new)
        .map_err(|_| PortError::new("auth_version", "invalid session credential version"))?;
    let session_expires = required(&values, "session_expires")?
        .parse::<i64>()
        .map_err(|_| PortError::new("auth_expiry", "invalid session credential expiry"))?;
    let session = SessionCredentials::from_unix_expiry(
        account,
        SensitiveString::new(hex_string(required(&values, "session_access")?)?),
        SensitiveString::new(hex_string(required(&values, "session_refresh")?)?),
        version,
        session_expires,
    )
    .map_err(|error| PortError::new("auth_session", error))?;
    let bundle = AuthCredentialBundle { channel, session };
    bundle
        .validate()
        .map_err(|error| PortError::new("auth_validation", error))?;
    Ok(bundle)
}

#[cfg(all(windows, feature = "native-credentials"))]
fn encode_bound_auth_bundle(client_transaction_id: &str, bundle: &AuthCredentialBundle) -> Vec<u8> {
    format!(
        "schema=1\nclient_transaction={}\nbundle={}\n",
        hex_encode(client_transaction_id.as_bytes()),
        hex_encode(&encode_auth_bundle(bundle)),
    )
    .into_bytes()
}

#[cfg(all(windows, feature = "native-credentials"))]
fn decode_bound_auth_bundle(bytes: &[u8]) -> Result<(String, AuthCredentialBundle), PortError> {
    let values = fields(bytes)?;
    if required(&values, "schema")? != "1" {
        return Err(PortError::new(
            "authorization_journal_schema",
            "unsupported bound authorization bundle schema",
        ));
    }
    let client_transaction_id = hex_string(required(&values, "client_transaction")?)?;
    validate_transaction_id("client transaction", &client_transaction_id)?;
    let bundle = decode_auth_bundle(&hex_decode(required(&values, "bundle")?)?)?;
    Ok((client_transaction_id, bundle))
}

#[cfg(not(windows))]
fn encode_authorization_journal(journal: &AuthorizationReplacementJournal) -> Vec<u8> {
    let phase = match journal.phase {
        AuthorizationReplacementPhase::Preparing => "preparing",
        AuthorizationReplacementPhase::Prepared => "prepared",
        AuthorizationReplacementPhase::PromotionDispatched => "promotion_dispatched",
        AuthorizationReplacementPhase::AbortDispatched => "abort_dispatched",
    };
    format!(
        concat!(
            "schema=1\n",
            "phase={}\n",
            "client_transaction={}\n",
            "stage={}\n",
            "authorization_transaction={}\n",
            "provisional_access={}\n",
            "recovery={}\n",
            "digest={}\n",
            "previous={}\n",
            "replacement={}\n"
        ),
        phase,
        hex_encode(journal.client_transaction_id.as_bytes()),
        hex_encode(journal.stage_id.as_deref().unwrap_or_default().as_bytes()),
        hex_encode(
            journal
                .authorization_transaction_id
                .as_deref()
                .unwrap_or_default()
                .as_bytes()
        ),
        hex_encode(
            journal
                .provisional_access_token
                .as_ref()
                .map(SensitiveString::expose_secret)
                .unwrap_or_default()
                .as_bytes()
        ),
        hex_encode(
            journal
                .recovery_token
                .as_ref()
                .map(SensitiveString::expose_secret)
                .unwrap_or_default()
                .as_bytes()
        ),
        journal.bundle_digest.as_deref().unwrap_or_default(),
        hex_encode(&encode_auth_bundle(&journal.previous)),
        journal
            .replacement
            .as_ref()
            .map(encode_auth_bundle)
            .map_or_else(String::new, |bundle| hex_encode(&bundle)),
    )
    .into_bytes()
}

#[cfg(not(windows))]
fn decode_authorization_journal(
    bytes: &[u8],
) -> Result<AuthorizationReplacementJournal, PortError> {
    let values = fields(bytes)?;
    if required(&values, "schema")? != "1" {
        return Err(PortError::new(
            "authorization_journal_schema",
            "unsupported authorization journal schema",
        ));
    }
    let phase = match required(&values, "phase")? {
        "preparing" => AuthorizationReplacementPhase::Preparing,
        "prepared" => AuthorizationReplacementPhase::Prepared,
        "promotion_dispatched" => AuthorizationReplacementPhase::PromotionDispatched,
        "abort_dispatched" => AuthorizationReplacementPhase::AbortDispatched,
        _ => {
            return Err(PortError::new(
                "authorization_journal_phase",
                "unknown authorization journal phase",
            ));
        }
    };
    let optional_hex = |name| -> Result<Option<String>, PortError> {
        let value = hex_string(required(&values, name)?)?;
        Ok((!value.is_empty()).then_some(value))
    };
    let replacement = required(&values, "replacement")?;
    let journal = AuthorizationReplacementJournal {
        phase,
        client_transaction_id: hex_string(required(&values, "client_transaction")?)?,
        stage_id: optional_hex("stage")?,
        authorization_transaction_id: optional_hex("authorization_transaction")?,
        provisional_access_token: optional_hex("provisional_access")?.map(SensitiveString::new),
        recovery_token: optional_hex("recovery")?.map(SensitiveString::new),
        bundle_digest: {
            let value = required(&values, "digest")?;
            (!value.is_empty()).then(|| value.to_owned())
        },
        previous: decode_auth_bundle(&hex_decode(required(&values, "previous")?)?)?,
        replacement: if replacement.is_empty() {
            None
        } else {
            Some(decode_auth_bundle(&hex_decode(replacement)?)?)
        },
    };
    journal.validate()?;
    Ok(journal)
}

#[cfg(all(windows, feature = "native-credentials"))]
struct AuthorizationJournalMetadata {
    phase: AuthorizationReplacementPhase,
    client_transaction_id: String,
    stage_id: Option<String>,
    authorization_transaction_id: Option<String>,
    provisional_access_token: Option<SensitiveString>,
    recovery_token: Option<SensitiveString>,
    bundle_digest: Option<String>,
}

#[cfg(all(windows, feature = "native-credentials"))]
fn encode_authorization_journal_metadata(journal: &AuthorizationReplacementJournal) -> Vec<u8> {
    let phase = match journal.phase {
        AuthorizationReplacementPhase::Preparing => "preparing",
        AuthorizationReplacementPhase::Prepared => "prepared",
        AuthorizationReplacementPhase::PromotionDispatched => "promotion_dispatched",
        AuthorizationReplacementPhase::AbortDispatched => "abort_dispatched",
    };
    format!(
        concat!(
            "schema=1\nphase={}\nclient_transaction={}\nstage={}\n",
            "authorization_transaction={}\nprovisional_access={}\nrecovery={}\ndigest={}\n"
        ),
        phase,
        hex_encode(journal.client_transaction_id.as_bytes()),
        hex_encode(journal.stage_id.as_deref().unwrap_or_default().as_bytes()),
        hex_encode(
            journal
                .authorization_transaction_id
                .as_deref()
                .unwrap_or_default()
                .as_bytes()
        ),
        hex_encode(
            journal
                .provisional_access_token
                .as_ref()
                .map(SensitiveString::expose_secret)
                .unwrap_or_default()
                .as_bytes()
        ),
        hex_encode(
            journal
                .recovery_token
                .as_ref()
                .map(SensitiveString::expose_secret)
                .unwrap_or_default()
                .as_bytes()
        ),
        journal.bundle_digest.as_deref().unwrap_or_default(),
    )
    .into_bytes()
}

#[cfg(all(windows, feature = "native-credentials"))]
fn decode_authorization_journal_metadata(
    bytes: &[u8],
) -> Result<AuthorizationJournalMetadata, PortError> {
    let values = fields(bytes)?;
    if required(&values, "schema")? != "1" {
        return Err(PortError::new(
            "authorization_journal_schema",
            "unsupported authorization journal schema",
        ));
    }
    let phase = match required(&values, "phase")? {
        "preparing" => AuthorizationReplacementPhase::Preparing,
        "prepared" => AuthorizationReplacementPhase::Prepared,
        "promotion_dispatched" => AuthorizationReplacementPhase::PromotionDispatched,
        "abort_dispatched" => AuthorizationReplacementPhase::AbortDispatched,
        _ => {
            return Err(PortError::new(
                "authorization_journal_phase",
                "unknown authorization journal phase",
            ));
        }
    };
    let optional_hex = |name| -> Result<Option<String>, PortError> {
        let value = hex_string(required(&values, name)?)?;
        Ok((!value.is_empty()).then_some(value))
    };
    Ok(AuthorizationJournalMetadata {
        phase,
        client_transaction_id: hex_string(required(&values, "client_transaction")?)?,
        stage_id: optional_hex("stage")?,
        authorization_transaction_id: optional_hex("authorization_transaction")?,
        provisional_access_token: optional_hex("provisional_access")?.map(SensitiveString::new),
        recovery_token: optional_hex("recovery")?.map(SensitiveString::new),
        bundle_digest: {
            let value = required(&values, "digest")?;
            (!value.is_empty()).then(|| value.to_owned())
        },
    })
}

fn fields(bytes: &[u8]) -> Result<std::collections::BTreeMap<&str, &str>, PortError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| PortError::new("native_format", "native state is not UTF-8"))?;
    text.lines()
        .map(|line| {
            line.split_once('=').ok_or_else(|| {
                PortError::new("native_format", "native state contains an invalid line")
            })
        })
        .collect()
}

fn required<'a>(
    fields: &'a std::collections::BTreeMap<&str, &str>,
    name: &str,
) -> Result<&'a str, PortError> {
    fields
        .get(name)
        .copied()
        .ok_or_else(|| PortError::new("native_format", format!("native state is missing {name}")))
}

fn parse_commit_set(value: &str) -> Result<Vec<CommitId>, PortError> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let commits = value
        .split(',')
        .map(|value| {
            value
                .parse()
                .map(CommitId::from_uuid)
                .map_err(|_| PortError::new("native_commit", "invalid durable commit ID"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut unique = Vec::with_capacity(commits.len());
    for commit in commits {
        if !unique.contains(&commit) {
            unique.push(commit);
        }
    }
    Ok(unique)
}

fn validate_conversation_pointer(pointer: Option<&str>) -> Result<(), PortError> {
    let Some(pointer) = pointer else {
        return Ok(());
    };
    if pointer.is_empty()
        || pointer.len() > MAX_CONVERSATION_POINTER_BYTES
        || pointer.chars().any(char::is_control)
    {
        return Err(PortError::new(
            "config_conversation",
            "conversation pointer is invalid",
        ));
    }
    Ok(())
}

fn count_directory_entries(path: &Path) -> Result<usize, PortError> {
    path.read_dir()
        .map_err(|error| PortError::new("config_records", error.to_string()))?
        .take(MAX_LOCAL_RECORDS + 1)
        .try_fold(0usize, |count, entry| {
            entry
                .map(|_| count + 1)
                .map_err(|error| PortError::new("config_records", error.to_string()))
        })
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn hex_string(value: &str) -> Result<String, PortError> {
    String::from_utf8(hex_decode(value)?)
        .map_err(|_| PortError::new("native_utf8", "native field is not valid UTF-8"))
}

fn hex_decode(value: &str) -> Result<Vec<u8>, PortError> {
    if !value.len().is_multiple_of(2) {
        return Err(PortError::new("native_hex", "invalid hex-encoded field"));
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().chunks_exact(2) {
        let high = hex_digit(pair[0])?;
        let low = hex_digit(pair[1])?;
        bytes.push((high << 4) | low);
    }
    Ok(bytes)
}

fn validate_transaction_id(label: &str, value: &str) -> Result<(), PortError> {
    if value.len() < 16
        || value.len() > 128
        || !value.is_ascii()
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(PortError::new(
            "authorization_transaction",
            format!("{label} identifier is invalid"),
        ));
    }
    Ok(())
}

fn remove_private_state_file(path: &Path) -> Result<(), PortError> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(PortError::uncertain(
                "authorization_cleanup",
                error.to_string(),
            ));
        }
    }
    if let Some(parent) = path.parent() {
        sync_directory(parent)
            .map_err(|error| PortError::uncertain("authorization_cleanup", error.to_string()))?;
    }
    Ok(())
}

fn hex_digit(value: u8) -> Result<u8, PortError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(PortError::new("native_hex", "invalid hex-encoded field")),
    }
}

fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>, PortError> {
    let file =
        File::open(path).map_err(|error| PortError::new("native_read", error.to_string()))?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| PortError::new("native_read", error.to_string()))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
        return Err(PortError::new(
            "native_size",
            "native state exceeds its size limit",
        ));
    }
    Ok(bytes)
}

pub(crate) fn create_private_dir(path: &Path) -> Result<(), PortError> {
    #[cfg(windows)]
    let existed = path.is_dir();
    fs::create_dir_all(path)
        .map_err(|error| PortError::new("native_directory", error.to_string()))?;
    #[cfg(windows)]
    if !existed {
        forget_windows_owner_acl(path, true)
            .map_err(|error| PortError::new("native_permissions", error.to_string()))?;
    }
    make_private_dir(path).map_err(|error| PortError::new("native_permissions", error.to_string()))
}

#[cfg(unix)]
fn make_private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(windows)]
fn make_private_dir(path: &Path) -> std::io::Result<()> {
    ensure_windows_owner_acl(path, true)
}

#[cfg(not(any(unix, windows)))]
fn make_private_dir(_path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "private directory permissions are not implemented for this platform",
    ))
}

#[cfg(unix)]
fn make_private_file(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(windows)]
fn make_private_file(path: &Path) -> std::io::Result<()> {
    ensure_windows_owner_acl(path, false)
}

#[cfg(not(any(unix, windows)))]
fn make_private_file(_path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "private file permissions are not implemented for this platform",
    ))
}

#[cfg(unix)]
fn make_private_staging_file(path: &Path) -> std::io::Result<()> {
    make_private_file(path)
}

#[cfg(windows)]
fn make_private_staging_file(path: &Path) -> std::io::Result<()> {
    // `AtomicFile` created this path with create-new semantics inside a
    // directory whose only inheritable ACE belongs to the current owner.
    // Converting that inherited ACE to an explicit owner grant is therefore
    // safe: an arbitrary pre-existing explicit ACE cannot exist on this file.
    let sid = windows_current_user_sid()?;
    let grant = format!("*{sid}:F");
    let owner = format!("*{sid}");
    let output = Command::new("icacls")
        .arg(path)
        .arg("/setowner")
        .arg(owner)
        .output()?;
    windows_acl_command_result("set-owner", path, &sid, output)?;

    let output = Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r"])
        .arg(grant)
        .output()?;
    windows_acl_command_result("install", path, &sid, output)
}

#[cfg(windows)]
fn windows_acl_command_result(
    operation: &'static str,
    path: &Path,
    sid: &str,
    output: Output,
) -> std::io::Result<()> {
    if output.status.success() {
        return Ok(());
    }
    let path = path.to_string_lossy();
    let detail = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .replace(path.as_ref(), "[PATH]")
    .replace(sid, "[SID]");
    let detail: String = heyfood_core::terminal_safe_text(&detail)
        .chars()
        .take(512)
        .collect();
    Err(std::io::Error::other(format!(
        "Windows fresh-file ACL {operation} failed with status {}: {detail}",
        output.status
    )))
}

#[cfg(not(any(unix, windows)))]
fn make_private_staging_file(_path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "private file permissions are not implemented for this platform",
    ))
}

#[cfg(windows)]
fn apply_windows_owner_acl(path: &Path, directory: bool) -> std::io::Result<()> {
    let sid = windows_current_user_sid()?;
    run_windows_acl_script(
        "install-and-verify",
        WINDOWS_INSTALL_OWNER_ONLY_ACL,
        path,
        &sid,
        directory,
    )
}

#[cfg(windows)]
fn ensure_windows_owner_acl(path: &Path, directory: bool) -> std::io::Result<()> {
    let hardened =
        WINDOWS_HARDENED_PATHS.get_or_init(|| Mutex::new(std::collections::HashSet::new()));
    let mut hardened = hardened
        .lock()
        .map_err(|_| std::io::Error::other("Windows ACL state lock is poisoned"))?;
    let key = (path.to_path_buf(), directory);
    if hardened.contains(&key) {
        return Ok(());
    }
    apply_windows_owner_acl(path, directory)?;
    hardened.insert(key);
    Ok(())
}

#[cfg(windows)]
fn forget_windows_owner_acl(path: &Path, directory: bool) -> std::io::Result<()> {
    WINDOWS_HARDENED_PATHS
        .get_or_init(|| Mutex::new(std::collections::HashSet::new()))
        .lock()
        .map_err(|_| std::io::Error::other("Windows ACL state lock is poisoned"))?
        .remove(&(path.to_path_buf(), directory));
    Ok(())
}

#[cfg(windows)]
fn remember_windows_owner_acl(path: &Path, directory: bool) -> std::io::Result<()> {
    WINDOWS_HARDENED_PATHS
        .get_or_init(|| Mutex::new(std::collections::HashSet::new()))
        .lock()
        .map_err(|_| std::io::Error::other("Windows ACL state lock is poisoned"))?
        .insert((path.to_path_buf(), directory));
    Ok(())
}

#[cfg(windows)]
fn run_windows_acl_script(
    operation: &'static str,
    script: &str,
    path: &Path,
    sid: &str,
    directory: bool,
) -> std::io::Result<()> {
    let output = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .env("HEYFOOD_ACL_TARGET", path)
        .env("HEYFOOD_ACL_OWNER_SID", sid)
        .env(
            "HEYFOOD_ACL_TARGET_KIND",
            if directory { "directory" } else { "file" },
        )
        .stdout(Stdio::null())
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        let path = path.to_string_lossy();
        let detail = String::from_utf8_lossy(&output.stderr)
            .replace(path.as_ref(), "[PATH]")
            .replace(sid, "[SID]");
        let detail: String = heyfood_core::terminal_safe_text(&detail)
            .chars()
            .take(512)
            .collect();
        Err(std::io::Error::other(format!(
            "Windows owner-only ACL {operation} failed with status {}: {detail}",
            output.status
        )))
    }
}

#[cfg(windows)]
const WINDOWS_INSTALL_OWNER_ONLY_ACL: &str = r#"
$ErrorActionPreference = 'Stop'
$target = $env:HEYFOOD_ACL_TARGET
$kind = $env:HEYFOOD_ACL_TARGET_KIND
$owner = [System.Security.Principal.SecurityIdentifier]::new($env:HEYFOOD_ACL_OWNER_SID)
if ($kind -eq 'directory') {
    if (-not [System.IO.Directory]::Exists($target)) { throw 'ACL directory target does not exist' }
    $security = [System.Security.AccessControl.DirectorySecurity]::new()
    $inheritance = [System.Security.AccessControl.InheritanceFlags]::ContainerInherit -bor [System.Security.AccessControl.InheritanceFlags]::ObjectInherit
} elseif ($kind -eq 'file') {
    if (-not [System.IO.File]::Exists($target)) { throw 'ACL file target does not exist' }
    $security = [System.Security.AccessControl.FileSecurity]::new()
    $inheritance = [System.Security.AccessControl.InheritanceFlags]::None
} else {
    throw 'ACL target kind is invalid'
}
$security.SetOwner($owner)
$security.SetAccessRuleProtection($true, $false)
$rule = [System.Security.AccessControl.FileSystemAccessRule]::new(
    $owner,
    [System.Security.AccessControl.FileSystemRights]::FullControl,
    $inheritance,
    [System.Security.AccessControl.PropagationFlags]::None,
    [System.Security.AccessControl.AccessControlType]::Allow
)
$security.SetAccessRule($rule)
if ($kind -eq 'directory') {
    [System.IO.Directory]::SetAccessControl($target, $security)
    $actual = [System.IO.Directory]::GetAccessControl($target)
} else {
    [System.IO.File]::SetAccessControl($target, $security)
    $actual = [System.IO.File]::GetAccessControl($target)
}
if (-not $actual.AreAccessRulesProtected) { throw 'DACL is not protected after installation' }
$ownerSid = $actual.GetOwner([System.Security.Principal.SecurityIdentifier]).Value
if ($ownerSid -ne $owner.Value) { throw 'ACL owner differs from the current user after installation' }
$rules = @($actual.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier]))
if ($rules.Count -ne 1) { throw 'DACL contains a foreign or duplicate ACE after installation' }
$actualRule = $rules[0]
if ($actualRule.IdentityReference.Value -ne $owner.Value) { throw 'DACL contains a foreign principal after installation' }
if ($actualRule.AccessControlType -ne [System.Security.AccessControl.AccessControlType]::Allow) { throw 'owner ACE is not allow after installation' }
if ($actualRule.IsInherited) { throw 'owner ACE is inherited after installation' }
if ($actualRule.FileSystemRights -ne [System.Security.AccessControl.FileSystemRights]::FullControl) { throw 'owner ACE is not full control after installation' }
if ($actualRule.InheritanceFlags -ne $inheritance) { throw 'owner ACE inheritance flags are invalid after installation' }
if ($actualRule.PropagationFlags -ne [System.Security.AccessControl.PropagationFlags]::None) { throw 'owner ACE propagation flags are invalid after installation' }
"#;

#[cfg(all(windows, test))]
const WINDOWS_VERIFY_OWNER_ONLY_ACL: &str = r#"
$ErrorActionPreference = 'Stop'
$target = $env:HEYFOOD_ACL_TARGET
$kind = $env:HEYFOOD_ACL_TARGET_KIND
$expectedSid = $env:HEYFOOD_ACL_OWNER_SID
if ($kind -eq 'directory') {
    if (-not [System.IO.Directory]::Exists($target)) { throw 'ACL directory target does not exist' }
    $security = [System.IO.Directory]::GetAccessControl($target)
} elseif ($kind -eq 'file') {
    if (-not [System.IO.File]::Exists($target)) { throw 'ACL file target does not exist' }
    $security = [System.IO.File]::GetAccessControl($target)
} else {
    throw 'ACL target kind is invalid'
}
if (-not $security.AreAccessRulesProtected) { throw 'DACL is not protected' }
$ownerSid = $security.GetOwner([System.Security.Principal.SecurityIdentifier]).Value
if ($ownerSid -ne $expectedSid) { throw 'ACL owner differs from the current user' }
$rules = @($security.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier]))
if ($rules.Count -ne 1) { throw 'DACL contains a foreign or duplicate ACE' }
$rule = $rules[0]
if ($rule.IdentityReference.Value -ne $expectedSid) { throw 'DACL contains a foreign principal' }
if ($rule.AccessControlType -ne [System.Security.AccessControl.AccessControlType]::Allow) { throw 'owner ACE is not allow' }
if ($rule.IsInherited) { throw 'owner ACE is inherited' }
if ($rule.FileSystemRights -ne [System.Security.AccessControl.FileSystemRights]::FullControl) { throw 'owner ACE is not full control' }
$expectedInheritance = if ($kind -eq 'directory') {
    [System.Security.AccessControl.InheritanceFlags]::ContainerInherit -bor [System.Security.AccessControl.InheritanceFlags]::ObjectInherit
} else {
    [System.Security.AccessControl.InheritanceFlags]::None
}
if ($rule.InheritanceFlags -ne $expectedInheritance) { throw 'owner ACE inheritance flags are invalid' }
if ($rule.PropagationFlags -ne [System.Security.AccessControl.PropagationFlags]::None) { throw 'owner ACE propagation flags are invalid' }
"#;

#[cfg(windows)]
fn windows_current_user_sid() -> std::io::Result<String> {
    if let Some(sid) = WINDOWS_CURRENT_USER_SID.get() {
        return Ok(sid.clone());
    }
    let output = Command::new("whoami")
        .args(["/user", "/fo", "csv", "/nh"])
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(
            "whoami could not resolve the current Windows SID",
        ));
    }
    let start = output
        .stdout
        .windows(b"S-1-".len())
        .position(|window| window == b"S-1-")
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "whoami did not return a Windows SID",
            )
        })?;
    let sid = output.stdout[start..]
        .iter()
        .copied()
        .take_while(|byte| byte.is_ascii_digit() || *byte == b'-' || *byte == b'S')
        .collect::<Vec<_>>();
    let sid = String::from_utf8(sid).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "whoami returned an invalid Windows SID",
        )
    })?;
    if sid.len() <= "S-1-".len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "whoami returned an invalid Windows SID",
        ));
    }
    let _ = WINDOWS_CURRENT_USER_SID.set(sid.clone());
    Ok(sid)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod credential_write_verification_tests {
    use std::collections::VecDeque;

    use super::*;

    fn state(account: &str, version: u64, token_suffix: &str) -> CredentialState {
        CredentialState::new(
            SessionCredentials::from_unix_expiry(
                AccountId::parse(account).unwrap(),
                SensitiveString::new(format!("access-{token_suffix}")),
                SensitiveString::new(format!("refresh-{token_suffix}")),
                CredentialVersion::new(version),
                4_102_444_800,
            )
            .unwrap(),
        )
    }

    fn assert_uncertain_verify_error(error: PortError) {
        assert_eq!(error.code, "credential_manager_write_verify");
        assert!(error.outcome_uncertain);
    }

    #[test]
    fn delayed_credential_visibility_retries_only_missing_and_older_state() {
        let expected = state("account-one", 3, "expected");
        let mut observations = VecDeque::from([
            None,
            Some(state("account-one", 2, "older")),
            Some(state("account-one", 3, "expected")),
        ]);
        let mut waits = 0;

        verify_credential_write_visibility(
            &expected,
            CREDENTIAL_WRITE_VERIFY_TIMEOUT,
            || Ok(observations.pop_front().unwrap()),
            |_| waits += 1,
        )
        .unwrap();

        assert_eq!(waits, 2);
        assert!(observations.is_empty());
    }

    #[test]
    fn delayed_credential_visibility_times_out_as_uncertain_without_rewriting() {
        let expected = state("account-one", 3, "expected");
        let mut reads = 0;
        let error = verify_credential_write_visibility(
            &expected,
            Duration::ZERO,
            || {
                reads += 1;
                Ok(Some(state("account-one", 2, "older")))
            },
            |_| panic!("a zero-duration deadline must not wait"),
        )
        .unwrap_err();

        assert_eq!(reads, 1);
        assert_uncertain_verify_error(error);
    }

    #[test]
    fn delayed_credential_visibility_preserves_true_conflicts_and_read_errors() {
        let expected = state("account-one", 3, "expected");
        for conflict in [
            state("account-one", 4, "newer"),
            state("account-one", 3, "different"),
            state("account-two", 2, "other-account"),
        ] {
            let mut observations =
                VecDeque::from([Some(state("account-one", 2, "older")), Some(conflict)]);
            let error = verify_credential_write_visibility(
                &expected,
                CREDENTIAL_WRITE_VERIFY_TIMEOUT,
                || Ok(observations.pop_front().unwrap()),
                |_| {},
            )
            .unwrap_err();
            assert_eq!(observations.len(), 0);
            assert_uncertain_verify_error(error);
        }

        let error = verify_credential_write_visibility(
            &expected,
            CREDENTIAL_WRITE_VERIFY_TIMEOUT,
            || Err(PortError::new("credential_schema", "malformed fixture")),
            |_| panic!("read errors must fail immediately"),
        )
        .unwrap_err();
        assert_uncertain_verify_error(error);
    }
}

#[cfg(all(test, windows))]
mod windows_acl_tests {
    use super::*;

    struct Cleanup(PathBuf);

    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn add_explicit_grant(path: &Path, sid: &str, directory: bool) {
        let grant = if directory {
            format!("*{sid}:(OI)(CI)F")
        } else {
            format!("*{sid}:F")
        };
        let status = Command::new("icacls")
            .arg(path)
            .arg("/grant")
            .arg(grant)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("seed explicit Windows ACE");
        assert!(status.success(), "icacls must seed the broad ACE");
    }

    #[test]
    fn owner_only_acl_replaces_explicit_everyone_and_users_aces() {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "heyfood-owner-acl-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create ACL fixture directory");
        let _cleanup = Cleanup(root.clone());
        let file = root.join("private.state");
        fs::write(&file, b"fixture").expect("create ACL fixture file");

        for sid in ["S-1-1-0", "S-1-5-32-545"] {
            add_explicit_grant(&root, sid, true);
            add_explicit_grant(&file, sid, false);
        }

        let owner = windows_current_user_sid().expect("resolve current Windows SID");
        assert!(
            run_windows_acl_script("verify", WINDOWS_VERIFY_OWNER_ONLY_ACL, &root, &owner, true)
                .is_err(),
            "broad directory ACEs must violate the owner-only contract"
        );
        assert!(
            run_windows_acl_script(
                "verify",
                WINDOWS_VERIFY_OWNER_ONLY_ACL,
                &file,
                &owner,
                false
            )
            .is_err(),
            "broad file ACEs must violate the owner-only contract"
        );

        apply_windows_owner_acl(&root, true).expect("replace directory DACL");
        apply_windows_owner_acl(&file, false).expect("replace file DACL");

        run_windows_acl_script("verify", WINDOWS_VERIFY_OWNER_ONLY_ACL, &root, &owner, true)
            .expect("directory DACL is protected and owner-only");
        run_windows_acl_script(
            "verify",
            WINDOWS_VERIFY_OWNER_ONLY_ACL,
            &file,
            &owner,
            false,
        )
        .expect("file DACL is protected and owner-only");

        let replacement = root.join("replacement.state");
        fs::write(&replacement, b"legacy").expect("create replacement fixture file");
        for sid in ["S-1-1-0", "S-1-5-32-545"] {
            add_explicit_grant(&replacement, sid, false);
        }
        AtomicFile::replace(&replacement, b"private replacement").unwrap_or_else(|error| {
            panic!(
                "atomically replace broadly accessible file: {}: {}",
                error.code, error.message
            )
        });
        run_windows_acl_script(
            "verify",
            WINDOWS_VERIFY_OWNER_ONLY_ACL,
            &replacement,
            &owner,
            false,
        )
        .expect("atomic replacement installs a protected owner-only DACL");
    }

    #[cfg(feature = "native-credentials")]
    fn windows_bundle(suffix: &str, version: u64) -> AuthCredentialBundle {
        AuthCredentialBundle {
            channel: ChannelCredentials::from_unix_expiry(
                "hf_cid_heyfood_cli",
                "windows-staged-device",
                SensitiveString::new(format!("channel-access-{suffix}")),
                SensitiveString::new(format!("channel-refresh-{suffix}")),
                4_102_444_800,
                "account:link profile:read",
            )
            .unwrap(),
            session: SessionCredentials::from_unix_expiry(
                AccountId::parse("windows-staged-account").unwrap(),
                SensitiveString::new(format!("session-access-{suffix}")),
                SensitiveString::new(format!("session-refresh-{suffix}")),
                CredentialVersion::new(version),
                4_102_444_800,
            )
            .unwrap(),
        }
    }

    #[test]
    #[cfg(feature = "native-credentials")]
    fn windows_preparing_metadata_ignores_uncommitted_pending_and_rejects_mixed_generation() {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "heyfood-windows-journal-generation-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let _cleanup = Cleanup(root.clone());
        let auth = NativeAuthStore::open(&root).unwrap();
        let session = WindowsCredentialStore::open(&root).unwrap();
        let previous = windows_bundle("previous", 1);
        let pending = windows_bundle("pending", 2);
        auth.initialize(&previous).unwrap();
        session.initialize(&previous.session).unwrap();
        let client_transaction_id = "windows-client-transaction-generation".to_owned();
        let preparing = auth
            .begin_authorization_replacement(client_transaction_id.clone(), &session)
            .unwrap();

        auth.replacement_entry(
            &auth.replacement_pending_target,
            "authorization-replacement-pending",
        )
        .unwrap()
        .set_secret(&encode_bound_auth_bundle(
            "different-client-transaction-generation",
            &pending,
        ))
        .unwrap();
        assert_eq!(
            auth.pending_authorization_replacement().unwrap(),
            Some(preparing.clone()),
            "Preparing metadata is the commit record and must ignore orphan pending data"
        );

        let mut prepared = preparing;
        prepared.phase = AuthorizationReplacementPhase::Prepared;
        prepared.stage_id = Some("windows-stage-transaction-generation".into());
        prepared.authorization_transaction_id =
            Some("windows-authorization-transaction-generation".into());
        prepared.recovery_token = Some(SensitiveString::new("windows-recovery-token-generation"));
        prepared.bundle_digest = Some("e".repeat(64));
        prepared.replacement = Some(pending);
        auth.replacement_entry(&auth.replacement_target, "authorization-replacement")
            .unwrap()
            .set_secret(&encode_authorization_journal_metadata(&prepared))
            .unwrap();
        assert_eq!(
            auth.pending_authorization_replacement().unwrap_err().code,
            "authorization_journal_generation"
        );
        auth.delete_authorization_journal_unlocked().unwrap();
        clear_any_reconciliation_marker(&auth.reconciliation_path).unwrap();
        auth.delete().unwrap();
        session.delete().unwrap();
    }

    #[test]
    #[cfg(feature = "native-credentials")]
    fn windows_terminal_cleanup_converges_after_each_split_delete_boundary() {
        for delete_pending_first in [false, true] {
            let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "heyfood-windows-journal-delete-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&root).unwrap();
            let _cleanup = Cleanup(root.clone());
            let auth = NativeAuthStore::open(&root).unwrap();
            let session = WindowsCredentialStore::open(&root).unwrap();
            let previous = windows_bundle("delete-previous", 1);
            let pending = windows_bundle("delete-pending", 2);
            auth.initialize(&previous).unwrap();
            session.initialize(&previous.session).unwrap();
            let client_transaction_id = format!("windows-client-transaction-delete-{sequence}");
            auth.begin_authorization_replacement(client_transaction_id.clone(), &session)
                .unwrap();
            let preparing = auth
                .record_provisional_authorization(
                    &client_transaction_id,
                    format!("windows-authorization-transaction-delete-{sequence}"),
                    SensitiveString::new("windows-provisional-access-delete"),
                )
                .unwrap();
            let prepared = AuthorizationReplacementJournal {
                phase: AuthorizationReplacementPhase::Prepared,
                client_transaction_id: client_transaction_id.clone(),
                stage_id: Some(format!("windows-stage-transaction-delete-{sequence}")),
                authorization_transaction_id: preparing.authorization_transaction_id,
                provisional_access_token: None,
                recovery_token: Some(SensitiveString::new("windows-recovery-token-delete")),
                bundle_digest: Some("f".repeat(64)),
                previous: preparing.previous,
                replacement: Some(pending.clone()),
            };
            auth.stage_authorization_replacement(prepared.clone(), &session)
                .unwrap();
            auth.mark_authorization_abort_dispatched(prepared).unwrap();
            write_authorization_cleanup_marker(
                &auth.reconciliation_path,
                "unpromoted",
                &client_transaction_id,
            )
            .unwrap();
            session
                .clear_staged_authorized_session(&client_transaction_id, &pending.session)
                .unwrap();
            if delete_pending_first {
                delete_keyring_entry(
                    &auth
                        .replacement_entry(
                            &auth.replacement_pending_target,
                            "authorization-replacement-pending",
                        )
                        .unwrap(),
                    "fixture-delete",
                )
                .unwrap();
            }
            assert!(auth.finish_authorization_terminal_cleanup().unwrap());
            assert!(!auth.reconciliation_path.exists());
            assert!(auth.pending_authorization_replacement().unwrap().is_none());
            assert_eq!(auth.load().unwrap().unwrap().channel, previous.channel);
            auth.delete().unwrap();
            session.delete().unwrap();
        }
    }
}
