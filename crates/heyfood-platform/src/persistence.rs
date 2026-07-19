use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use fs2::FileExt;
use heyfood_application::{
    BoxFuture, ConfigCommit, ConfigMutation, ConfigPort, CredentialCommit, CredentialPort,
    PortError,
};
use heyfood_core::{
    AccountId, ClientConfig, CommitId, ConfigRevision, CredentialVersion, NetworkPolicy,
    SensitiveString, ServiceUrl, SessionCredentials,
};

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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

        if let Err(error) = (|| -> std::io::Result<()> {
            make_private_file(&staging_path)?;
            staging.write_all(bytes)?;
            staging.flush()?;
            staging.sync_all()?;
            fs::rename(&staging_path, path)?;
            make_private_file(path)?;
            sync_directory(parent)?;
            Ok(())
        })() {
            let _ = fs::remove_file(&staging_path);
            return Err(PortError::new("atomic_replace", error.to_string()));
        }
        Ok(())
    }
}

struct FileLock {
    file: File,
}

impl FileLock {
    fn acquire(path: &Path, exclusive: bool) -> Result<Self, PortError> {
        let parent = path
            .parent()
            .ok_or_else(|| PortError::new("lock_path", "lock file must have a parent"))?;
        create_private_dir(parent)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|error| PortError::new("lock_open", error.to_string()))?;
        make_private_file(path)
            .map_err(|error| PortError::new("lock_permissions", error.to_string()))?;
        if exclusive {
            FileExt::lock_exclusive(&file)
        } else {
            FileExt::lock_shared(&file)
        }
        .map_err(|error| PortError::new("lock_acquire", error.to_string()))?;
        Ok(Self { file })
    }
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
    policy: NetworkPolicy,
}

impl NativeConfigStore {
    pub fn open(
        root: impl AsRef<Path>,
        initial: ClientConfig,
        policy: NetworkPolicy,
    ) -> Result<Self, PortError> {
        let root = root.as_ref();
        create_private_dir(root)?;
        let store = Self {
            state_path: root.join("config.native"),
            lock_path: root.join("config.lock"),
            records_path: root.join("records"),
            policy,
        };
        let _lock = FileLock::acquire(&store.lock_path, true)?;
        if !store.state_path.exists() {
            AtomicFile::replace(&store.state_path, &ConfigState::new(initial).encode())?;
        }
        Ok(store)
    }

    fn read_unlocked(&self) -> Result<ConfigState, PortError> {
        let bytes = read_limited(&self.state_path, 1024 * 1024)?;
        ConfigState::decode(&bytes, self.policy)
    }
}

impl ConfigPort for NativeConfigStore {
    fn load(&self) -> BoxFuture<'_, Result<ClientConfig, PortError>> {
        Box::pin(async move {
            let _lock = FileLock::acquire(&self.lock_path, false)?;
            Ok(self.read_unlocked()?.config)
        })
    }

    fn commit(&self, commit: ConfigCommit) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            let _lock = FileLock::acquire(&self.lock_path, true)?;
            let mut state = self.read_unlocked()?;
            if state.applied.contains(&commit.commit_id) {
                return Ok(());
            }
            match commit.mutation {
                ConfigMutation::Replace(config) => state.config = config,
                ConfigMutation::ConversationPointer(pointer) => state.conversation = pointer,
                ConfigMutation::LocalFirstRecord { kind, payload } => {
                    create_private_dir(&self.records_path)?;
                    let name = format!(
                        "{}-{}.record",
                        hex_encode(kind.as_bytes()),
                        commit.commit_id.as_uuid()
                    );
                    AtomicFile::replace(&self.records_path.join(name), &payload)?;
                }
            }
            state.applied.insert(commit.commit_id);
            AtomicFile::replace(&self.state_path, &state.encode())
        })
    }
}

struct ConfigState {
    config: ClientConfig,
    conversation: Option<String>,
    applied: HashSet<CommitId>,
}

impl ConfigState {
    fn new(config: ClientConfig) -> Self {
        Self {
            config,
            conversation: None,
            applied: HashSet::new(),
        }
    }

    fn encode(&self) -> Vec<u8> {
        let mut applied = self
            .applied
            .iter()
            .map(|value| value.as_uuid().to_string())
            .collect::<Vec<_>>();
        applied.sort_unstable();
        let applied = applied.join(",");
        format!(
            "schema=1\nactive={}\napi={}\nauth={}\nrevision={}\nconversation={}\napplied={}\n",
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
        if required(&fields, "schema")? != "1" {
            return Err(PortError::new(
                "config_schema",
                "unsupported native config schema",
            ));
        }
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
        let applied = parse_commit_set(required(&fields, "applied")?)?;
        Ok(Self {
            config: ClientConfig {
                active_context,
                api_url,
                auth_url,
                revision: ConfigRevision::new(revision),
            },
            conversation,
            applied,
        })
    }
}

#[derive(Clone, Debug)]
pub struct FileCredentialStore {
    state_path: PathBuf,
    lock_path: PathBuf,
    reconciliation_path: PathBuf,
}

impl FileCredentialStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, PortError> {
        let root = root.as_ref();
        create_private_dir(root)?;
        Ok(Self {
            state_path: root.join("credentials.native"),
            lock_path: root.join("credentials.lock"),
            reconciliation_path: root.join("credentials.reconciliation"),
        })
    }

    pub fn initialize(&self, credentials: &SessionCredentials) -> Result<(), PortError> {
        let _lock = FileLock::acquire(&self.lock_path, true)?;
        if self.state_path.exists() {
            return Err(PortError::new(
                "credentials_exist",
                "credentials have already been initialized",
            ));
        }
        AtomicFile::replace(
            &self.state_path,
            &CredentialState::new(credentials.clone()).encode(),
        )
    }

    pub fn reconciliation_required(&self) -> Result<bool, PortError> {
        let _lock = FileLock::acquire(&self.lock_path, false)?;
        Ok(self.reconciliation_path.exists())
    }

    fn read_unlocked(&self) -> Result<Option<CredentialState>, PortError> {
        if !self.state_path.exists() {
            return Ok(None);
        }
        CredentialState::decode(&read_limited(&self.state_path, 1024 * 1024)?).map(Some)
    }
}

impl CredentialPort for FileCredentialStore {
    fn load(&self) -> BoxFuture<'_, Result<Option<SessionCredentials>, PortError>> {
        Box::pin(async move {
            let _lock = FileLock::acquire(&self.lock_path, false)?;
            Ok(self.read_unlocked()?.map(|state| state.credentials))
        })
    }

    fn commit(&self, commit: CredentialCommit) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            let _lock = FileLock::acquire(&self.lock_path, true)?;
            let mut state = self.read_unlocked()?.ok_or_else(|| {
                PortError::new("credentials_missing", "credentials are not initialized")
            })?;
            if state.applied.contains(&commit.commit_id) {
                return Ok(());
            }
            if state.credentials.version != commit.expected_version {
                return Err(PortError::new(
                    "credential_version_conflict",
                    "stored credential version does not match the expected version",
                ));
            }
            if state.credentials.account_id != commit.credentials.account_id {
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
            state.credentials = commit.credentials;
            state.applied.insert(commit.commit_id);
            AtomicFile::replace(&self.state_path, &state.encode())?;
            if self.reconciliation_path.exists() {
                fs::remove_file(&self.reconciliation_path)
                    .map_err(|error| PortError::new("reconciliation_clear", error.to_string()))?;
                if let Some(parent) = self.reconciliation_path.parent() {
                    sync_directory(parent).map_err(|error| {
                        PortError::new("reconciliation_clear", error.to_string())
                    })?;
                }
            }
            Ok(())
        })
    }

    fn mark_reconciliation_required(
        &self,
        commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            let _lock = FileLock::acquire(&self.lock_path, true)?;
            AtomicFile::replace(
                &self.reconciliation_path,
                format!("{}\n", commit_id.as_uuid()).as_bytes(),
            )
        })
    }
}

struct CredentialState {
    credentials: SessionCredentials,
    applied: HashSet<CommitId>,
}

impl CredentialState {
    fn new(credentials: SessionCredentials) -> Self {
        Self {
            credentials,
            applied: HashSet::new(),
        }
    }

    fn encode(&self) -> Vec<u8> {
        let mut applied = self
            .applied
            .iter()
            .map(|value| value.as_uuid().to_string())
            .collect::<Vec<_>>();
        applied.sort_unstable();
        let applied = applied.join(",");
        format!(
            "schema=1\naccount={}\naccess={}\nrefresh={}\nversion={}\nexpires={}\napplied={}\n",
            hex_encode(self.credentials.account_id.as_str().as_bytes()),
            hex_encode(self.credentials.access_token.expose_secret().as_bytes()),
            hex_encode(self.credentials.refresh_token.expose_secret().as_bytes()),
            self.credentials.version.get(),
            self.credentials.expires_at_unix(),
            applied,
        )
        .into_bytes()
    }

    fn decode(bytes: &[u8]) -> Result<Self, PortError> {
        let fields = fields(bytes)?;
        if required(&fields, "schema")? != "1" {
            return Err(PortError::new(
                "credential_schema",
                "unsupported credential schema",
            ));
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
        Ok(Self {
            credentials,
            applied: parse_commit_set(required(&fields, "applied")?)?,
        })
    }
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

fn parse_commit_set(value: &str) -> Result<HashSet<CommitId>, PortError> {
    if value.is_empty() {
        return Ok(HashSet::new());
    }
    value
        .split(',')
        .map(|value| {
            value
                .parse()
                .map(CommitId::from_uuid)
                .map_err(|_| PortError::new("native_commit", "invalid durable commit ID"))
        })
        .collect()
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
    if !value.len().is_multiple_of(2) {
        return Err(PortError::new("native_hex", "invalid hex-encoded field"));
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().chunks_exact(2) {
        let high = hex_digit(pair[0])?;
        let low = hex_digit(pair[1])?;
        bytes.push((high << 4) | low);
    }
    String::from_utf8(bytes)
        .map_err(|_| PortError::new("native_utf8", "native field is not valid UTF-8"))
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

fn create_private_dir(path: &Path) -> Result<(), PortError> {
    fs::create_dir_all(path)
        .map_err(|error| PortError::new("native_directory", error.to_string()))?;
    make_private_dir(path).map_err(|error| PortError::new("native_permissions", error.to_string()))
}

#[cfg(unix)]
fn make_private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn make_private_dir(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn make_private_file(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn make_private_file(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}
