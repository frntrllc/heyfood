//! Immutable compatibility floor for native household state.
//!
//! The floor is global to one physical native data root. It is written only
//! after a caller-supplied secure-store capability probe succeeds and must be
//! verified before any account-scoped migration guard, key, or vault is
//! created.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::future::Future;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use cap_fs_ext::DirExt as _;
#[cfg(unix)]
use cap_std::ambient_authority;
#[cfg(unix)]
use cap_std::fs::Dir as CapDir;
use fs2::FileExt as _;
use heyfood_application::PortError;
use heyfood_core::to_canonical_bytes_v1;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::household_vault::household_native_root_instance_digest_v1;
use crate::persistence::AtomicFile;
#[cfg(any(not(unix), test))]
use crate::persistence::OwnerOnlyPath;

pub const NATIVE_STATE_FLOOR_REVISION_V1: u64 = 1;
pub const NATIVE_STATE_FLOOR_SCHEMA_VERSION_V1: u64 = 1;
pub const MINIMUM_COMPATIBLE_NATIVE_STATE_VERSION_V1: u64 = 2;
pub const MAX_NATIVE_STATE_FLOOR_BYTES: usize = 4 * 1024;

pub const NATIVE_STATE_CAPABILITIES_V1: [&str; 4] = [
    "household-account-slot-v1",
    "household-lifecycle-lock-v1",
    "household-migration-guard-v1",
    "household-teardown-journal-v1",
];

const FLOOR_DIRECTORY: &str = "compatibility";
const FLOOR_FILE: &str = "native-state-floor.v1.json";
const FLOOR_LOCK: &str = "native-state-floor.lock";
const FLOOR_LOCK_TIMEOUT: Duration = Duration::from_secs(1);
const FLOOR_LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(10);
#[cfg(unix)]
const FLOOR_ARTIFACT_READY_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeStateFloorV1 {
    floor_revision: u64,
    minimum_compatible_native_state_version: u64,
    native_root_instance_digest: String,
    required_binary_capabilities: Vec<String>,
    schema_version: u64,
}

impl NativeStateFloorV1 {
    fn expected(native_root_instance_digest: [u8; 32]) -> Self {
        Self {
            floor_revision: NATIVE_STATE_FLOOR_REVISION_V1,
            minimum_compatible_native_state_version: MINIMUM_COMPATIBLE_NATIVE_STATE_VERSION_V1,
            native_root_instance_digest: lower_hex(&native_root_instance_digest),
            required_binary_capabilities: NATIVE_STATE_CAPABILITIES_V1
                .into_iter()
                .map(str::to_owned)
                .collect(),
            schema_version: NATIVE_STATE_FLOOR_SCHEMA_VERSION_V1,
        }
    }

    fn validate_for_root(&self, native_root_instance_digest: [u8; 32]) -> Result<(), PortError> {
        if self != &Self::expected(native_root_instance_digest) {
            return Err(floor_invalid());
        }
        Ok(())
    }

    fn canonical_bytes(&self) -> Result<Vec<u8>, PortError> {
        to_canonical_bytes_v1(self).map_err(|_| {
            PortError::new(
                "native_state_floor_invalid",
                "native state floor is invalid",
            )
        })
    }

    #[must_use]
    pub const fn floor_revision(&self) -> u64 {
        self.floor_revision
    }

    #[must_use]
    pub const fn minimum_compatible_native_state_version(&self) -> u64 {
        self.minimum_compatible_native_state_version
    }

    #[must_use]
    pub fn native_root_instance_digest(&self) -> &str {
        &self.native_root_instance_digest
    }

    #[must_use]
    pub fn required_binary_capabilities(&self) -> &[String] {
        &self.required_binary_capabilities
    }
}

impl fmt::Debug for NativeStateFloorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeStateFloorV1")
            .field("floor_revision", &self.floor_revision)
            .field(
                "minimum_compatible_native_state_version",
                &self.minimum_compatible_native_state_version,
            )
            .field("schema_version", &self.schema_version)
            .finish_non_exhaustive()
    }
}

/// Store for the immutable global native-state compatibility floor.
#[derive(Clone)]
pub struct NativeStateFloorStore {
    native_root: PathBuf,
    native_root_instance_digest: [u8; 32],
}

impl NativeStateFloorStore {
    pub fn open(
        native_root: impl Into<PathBuf>,
        native_root_instance_digest: [u8; 32],
    ) -> Result<Self, PortError> {
        let native_root = native_root.into();
        let actual_digest = household_native_root_instance_digest_v1(&native_root)?;
        if actual_digest != native_root_instance_digest {
            return Err(PortError::new(
                "native_state_floor_root_mismatch",
                "native state floor root identity does not match",
            ));
        }
        Ok(Self {
            native_root,
            native_root_instance_digest,
        })
    }

    /// Run a bounded, nonmutating secure-store probe and only then create or
    /// verify the immutable floor.
    pub async fn ensure_after_secure_store_probe<Probe, ProbeFuture>(
        &self,
        cancellation: CancellationToken,
        probe: Probe,
    ) -> Result<NativeStateFloorV1, PortError>
    where
        Probe: FnOnce(CancellationToken) -> ProbeFuture,
        ProbeFuture: Future<Output = Result<(), PortError>>,
    {
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }
        probe(cancellation.child_token()).await?;
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }

        let store = self.clone();
        tokio::task::spawn_blocking(move || store.ensure_blocking(&cancellation))
            .await
            .map_err(|_| {
                PortError::new(
                    "native_state_floor_task",
                    "native state floor worker did not complete",
                )
            })?
    }

    /// Read and verify an existing floor without creating any artifact.
    pub async fn load(
        &self,
        cancellation: CancellationToken,
    ) -> Result<Option<NativeStateFloorV1>, PortError> {
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.load_blocking(&cancellation))
            .await
            .map_err(|_| {
                PortError::new(
                    "native_state_floor_task",
                    "native state floor worker did not complete",
                )
            })?
    }

    #[must_use]
    pub fn floor_path(&self) -> PathBuf {
        self.native_root.join(FLOOR_DIRECTORY).join(FLOOR_FILE)
    }

    #[must_use]
    pub fn lock_path(&self) -> PathBuf {
        self.native_root.join(FLOOR_DIRECTORY).join(FLOOR_LOCK)
    }

    fn ensure_blocking(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<NativeStateFloorV1, PortError> {
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }
        self.verify_root_identity()?;
        let directory = self.native_root.join(FLOOR_DIRECTORY);
        ensure_floor_directory(&directory, &self.native_root)?;
        let lock = NativeStateFloorLock::acquire(&self.lock_path(), &self.native_root)?;
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }
        self.verify_root_identity()?;
        lock.validate_directory(&directory, &self.native_root)?;

        let expected = NativeStateFloorV1::expected(self.native_root_instance_digest);
        let expected_bytes = expected.canonical_bytes()?;
        if expected_bytes.len() > MAX_NATIVE_STATE_FLOOR_BYTES {
            return Err(floor_invalid());
        }

        if let Some(current) = read_floor_bytes(&self.floor_path(), &self.native_root)? {
            let decoded = decode_floor(&current, self.native_root_instance_digest)?;
            if current != expected_bytes {
                return Err(floor_invalid());
            }
            return Ok(decoded);
        }

        if cancellation.is_cancelled() {
            return Err(cancelled());
        }
        lock.validate_directory(&directory, &self.native_root)?;
        AtomicFile::replace(&self.floor_path(), &expected_bytes)?;
        lock.validate_directory(&directory, &self.native_root)?;
        let current =
            read_floor_bytes(&self.floor_path(), &self.native_root)?.ok_or_else(|| {
                PortError::uncertain(
                    "native_state_floor_publish",
                    "native state floor publication is uncertain",
                )
            })?;
        let decoded = decode_floor(&current, self.native_root_instance_digest)?;
        if current != expected_bytes {
            return Err(PortError::uncertain(
                "native_state_floor_publish",
                "native state floor publication is uncertain",
            ));
        }
        Ok(decoded)
    }

    fn load_blocking(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<Option<NativeStateFloorV1>, PortError> {
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }
        self.verify_root_identity()?;
        let Some(bytes) = read_floor_bytes(&self.floor_path(), &self.native_root)? else {
            return Ok(None);
        };
        decode_floor(&bytes, self.native_root_instance_digest).map(Some)
    }

    fn verify_root_identity(&self) -> Result<(), PortError> {
        if household_native_root_instance_digest_v1(&self.native_root)?
            != self.native_root_instance_digest
        {
            return Err(PortError::new(
                "native_state_floor_root_mismatch",
                "native state floor root identity changed",
            ));
        }
        Ok(())
    }
}

#[cfg(unix)]
fn ensure_floor_directory(directory: &Path, native_root: &Path) -> Result<(), PortError> {
    match fs::symlink_metadata(directory) {
        Ok(metadata) => {
            return validate_floor_directory_after_concurrent_create(
                directory,
                &metadata,
                native_root,
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(floor_unavailable()),
    }

    match create_new_floor_directory(directory, native_root) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(directory).map_err(|_| floor_unavailable())?;
            return validate_floor_directory_after_concurrent_create(
                directory,
                &metadata,
                native_root,
            );
        }
        Err(_) => return Err(floor_unavailable()),
    }

    let metadata = fs::symlink_metadata(directory).map_err(|_| floor_unavailable())?;
    validate_owner_only_directory(&metadata, native_root)
}

#[cfg(not(unix))]
fn ensure_floor_directory(directory: &Path, native_root: &Path) -> Result<(), PortError> {
    match fs::symlink_metadata(directory) {
        Ok(metadata) => validate_owner_only_directory(&metadata, native_root),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            OwnerOnlyPath::directory(directory)?;
            let metadata = fs::symlink_metadata(directory).map_err(|_| floor_unavailable())?;
            validate_owner_only_directory(&metadata, native_root)
        }
        Err(_) => Err(floor_unavailable()),
    }
}

#[cfg(unix)]
fn create_new_floor_directory(directory: &Path, native_root: &Path) -> std::io::Result<()> {
    use cap_std::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    if directory.parent() != Some(native_root)
        || directory.file_name() != Some(FLOOR_DIRECTORY.as_ref())
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "native state floor directory must be one direct child",
        ));
    }

    let parent = CapDir::open_ambient_dir(native_root, ambient_authority())?;
    let parent_path_metadata = fs::symlink_metadata(native_root)?;
    let parent_open_metadata = parent.dir_metadata()?;
    if parent_path_metadata.file_type().is_symlink()
        || !parent_path_metadata.is_dir()
        || parent_path_metadata.dev() != parent_open_metadata.dev()
        || parent_path_metadata.ino() != parent_open_metadata.ino()
    {
        return Err(std::io::Error::other(
            "native state floor parent identity changed",
        ));
    }

    let mut builder = cap_std::fs::DirBuilder::new();
    builder.mode(0o700);
    parent.create_dir_with(FLOOR_DIRECTORY, &builder)?;
    let created = parent.symlink_metadata(FLOOR_DIRECTORY)?;
    if created.file_type().is_symlink()
        || !created.is_dir()
        || created.uid() != parent_open_metadata.uid()
    {
        return Err(std::io::Error::other(
            "native state floor directory identity changed",
        ));
    }

    let opened = parent.open_dir_nofollow(FLOOR_DIRECTORY)?;
    let opened_metadata = opened.dir_metadata()?;
    if created.dev() != opened_metadata.dev() || created.ino() != opened_metadata.ino() {
        return Err(std::io::Error::other(
            "native state floor directory identity changed",
        ));
    }

    // rustix rejects chmodat(AT_SYMLINK_NOFOLLOW) on Linux because the
    // fchmodat syscall has no flags argument. Apply the exact mode relative to
    // the already no-follow-opened, identity-verified directory instead.
    opened.set_permissions(
        Path::new("."),
        cap_std::fs::Permissions::from_std(fs::Permissions::from_mode(0o700)),
    )?;
    let finalized_metadata = opened.dir_metadata()?;
    let path_metadata = fs::symlink_metadata(directory)?;
    if path_metadata.file_type().is_symlink()
        || !path_metadata.is_dir()
        || created.dev() != finalized_metadata.dev()
        || created.ino() != finalized_metadata.ino()
        || created.dev() != path_metadata.dev()
        || created.ino() != path_metadata.ino()
        || finalized_metadata.permissions().mode() & 0o777 != 0o700
        || path_metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(std::io::Error::other(
            "native state floor directory identity changed",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn validate_floor_directory_after_concurrent_create(
    directory: &Path,
    initial: &fs::Metadata,
    native_root: &Path,
) -> Result<(), PortError> {
    if validate_owner_only_directory(initial, native_root).is_ok() {
        return Ok(());
    }
    if !is_floor_directory_creation_in_progress(initial, native_root)? {
        return Err(floor_invalid());
    }

    use std::os::unix::fs::MetadataExt as _;

    let expected_device = initial.dev();
    let expected_inode = initial.ino();
    let started = Instant::now();
    loop {
        if started.elapsed() >= FLOOR_ARTIFACT_READY_TIMEOUT {
            return Err(floor_invalid());
        }
        thread::sleep(FLOOR_LOCK_RETRY_INTERVAL);
        let current = fs::symlink_metadata(directory).map_err(|_| floor_unavailable())?;
        if current.dev() != expected_device || current.ino() != expected_inode {
            return Err(floor_unavailable());
        }
        if validate_owner_only_directory(&current, native_root).is_ok() {
            return Ok(());
        }
        if !is_floor_directory_creation_in_progress(&current, native_root)? {
            return Err(floor_invalid());
        }
    }
}

#[cfg(unix)]
fn is_floor_directory_creation_in_progress(
    metadata: &fs::Metadata,
    native_root: &Path,
) -> Result<bool, PortError> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let root = fs::metadata(native_root).map_err(|_| floor_unavailable())?;
    let mode = metadata.permissions().mode() & 0o777;
    Ok(!metadata.file_type().is_symlink()
        && metadata.is_dir()
        && metadata.uid() == root.uid()
        && mode & 0o077 == 0
        && mode != 0o700)
}

#[cfg(unix)]
fn validate_floor_lock_after_concurrent_create(
    path: &Path,
    initial: &fs::Metadata,
    native_root: &Path,
) -> Result<(), PortError> {
    if validate_owner_only_lock_file(initial, native_root).is_ok() {
        return Ok(());
    }
    if !is_floor_lock_creation_in_progress(initial, native_root)? {
        return Err(floor_invalid());
    }

    use std::os::unix::fs::MetadataExt as _;

    let expected_device = initial.dev();
    let expected_inode = initial.ino();
    let started = Instant::now();
    loop {
        if started.elapsed() >= FLOOR_ARTIFACT_READY_TIMEOUT {
            return Err(floor_invalid());
        }
        thread::sleep(FLOOR_LOCK_RETRY_INTERVAL);
        let current = fs::symlink_metadata(path).map_err(|_| floor_unavailable())?;
        if current.dev() != expected_device || current.ino() != expected_inode {
            return Err(floor_unavailable());
        }
        if validate_owner_only_lock_file(&current, native_root).is_ok() {
            return Ok(());
        }
        if !is_floor_lock_creation_in_progress(&current, native_root)? {
            return Err(floor_invalid());
        }
    }
}

#[cfg(not(unix))]
fn validate_floor_lock_after_concurrent_create(
    _path: &Path,
    initial: &fs::Metadata,
    native_root: &Path,
) -> Result<(), PortError> {
    validate_owner_only_lock_file(initial, native_root)
}

#[cfg(unix)]
fn is_floor_lock_creation_in_progress(
    metadata: &fs::Metadata,
    native_root: &Path,
) -> Result<bool, PortError> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let root = fs::metadata(native_root).map_err(|_| floor_unavailable())?;
    let mode = metadata.permissions().mode() & 0o777;
    Ok(!metadata.file_type().is_symlink()
        && metadata.is_file()
        && metadata.uid() == root.uid()
        && mode & 0o177 == 0
        && mode != 0o600)
}

struct NativeStateFloorLock {
    file: fs::File,
    directory: fs::Metadata,
}

impl NativeStateFloorLock {
    fn acquire(path: &Path, native_root: &Path) -> Result<Self, PortError> {
        let parent = path.parent().ok_or_else(floor_unavailable)?;
        let parent_metadata = fs::symlink_metadata(parent).map_err(|_| floor_unavailable())?;
        validate_owner_only_directory(&parent_metadata, native_root)?;

        let file = loop {
            match fs::symlink_metadata(path) {
                Ok(metadata) => {
                    validate_floor_lock_after_concurrent_create(path, &metadata, native_root)?;
                    break OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(path)
                        .map_err(|_| floor_unavailable())?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    match open_new_floor_lock(path) {
                        Ok(file) => break file,
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                        Err(_) => return Err(floor_unavailable()),
                    }
                }
                Err(_) => return Err(floor_unavailable()),
            }
        };

        let opened = file.metadata().map_err(|_| floor_unavailable())?;
        let path_metadata = fs::symlink_metadata(path).map_err(|_| floor_unavailable())?;
        validate_owner_only_lock_file(&opened, native_root)?;
        validate_owner_only_lock_file(&path_metadata, native_root)?;
        if !same_file(&opened, &path_metadata) {
            return Err(floor_unavailable());
        }

        let started = Instant::now();
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if started.elapsed() >= FLOOR_LOCK_TIMEOUT {
                        return Err(PortError::new(
                            "native_state_floor_lock_timeout",
                            "native state floor lock acquisition timed out",
                        ));
                    }
                    thread::sleep(FLOOR_LOCK_RETRY_INTERVAL);
                }
                Err(_) => return Err(floor_unavailable()),
            }
        }

        let after = fs::symlink_metadata(path).map_err(|_| floor_unavailable())?;
        validate_owner_only_lock_file(&after, native_root)?;
        if !same_file(&opened, &after) {
            return Err(floor_unavailable());
        }
        let current_parent = fs::symlink_metadata(parent).map_err(|_| floor_unavailable())?;
        validate_owner_only_directory(&current_parent, native_root)?;
        if !same_directory(&parent_metadata, &current_parent) {
            return Err(floor_unavailable());
        }
        Ok(Self {
            file,
            directory: current_parent,
        })
    }

    fn validate_directory(&self, path: &Path, native_root: &Path) -> Result<(), PortError> {
        let current = fs::symlink_metadata(path).map_err(|_| floor_unavailable())?;
        validate_owner_only_directory(&current, native_root)?;
        if !same_directory(&self.directory, &current) {
            return Err(floor_unavailable());
        }
        Ok(())
    }
}

impl Drop for NativeStateFloorLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

#[cfg(unix)]
fn open_new_floor_lock(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

#[cfg(not(unix))]
fn open_new_floor_lock(path: &Path) -> std::io::Result<fs::File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)?;
    if let Err(error) = OwnerOnlyPath::file(path) {
        let _ = fs::remove_file(path);
        return Err(std::io::Error::other(error.message));
    }
    Ok(file)
}

impl fmt::Debug for NativeStateFloorStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeStateFloorStore")
            .field(
                "native_root_instance_digest",
                &lower_hex(&self.native_root_instance_digest),
            )
            .finish_non_exhaustive()
    }
}

fn decode_floor(
    bytes: &[u8],
    native_root_instance_digest: [u8; 32],
) -> Result<NativeStateFloorV1, PortError> {
    if bytes.is_empty() || bytes.len() > MAX_NATIVE_STATE_FLOOR_BYTES {
        return Err(floor_invalid());
    }
    let floor: NativeStateFloorV1 = serde_json::from_slice(bytes).map_err(|_| floor_invalid())?;
    floor.validate_for_root(native_root_instance_digest)?;
    if floor.canonical_bytes()? != bytes {
        return Err(floor_invalid());
    }
    Ok(floor)
}

fn read_floor_bytes(path: &Path, native_root: &Path) -> Result<Option<Vec<u8>>, PortError> {
    let parent = path.parent().ok_or_else(floor_unavailable)?;
    match fs::symlink_metadata(parent) {
        Ok(metadata) => validate_owner_only_directory(&metadata, native_root)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(floor_unavailable()),
    }
    let before = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(floor_unavailable()),
    };
    if before.file_type().is_symlink()
        || !before.is_file()
        || before.len() > MAX_NATIVE_STATE_FLOOR_BYTES as u64
    {
        return Err(floor_invalid());
    }
    validate_owner_only(path, &before, native_root)?;

    let mut file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|_| floor_unavailable())?;
    let opened = file.metadata().map_err(|_| floor_unavailable())?;
    if !same_file(&before, &opened) {
        return Err(floor_unavailable());
    }

    let mut bytes = Vec::with_capacity(before.len() as usize);
    file.by_ref()
        .take((MAX_NATIVE_STATE_FLOOR_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| floor_unavailable())?;
    if bytes.len() > MAX_NATIVE_STATE_FLOOR_BYTES {
        return Err(floor_invalid());
    }
    let after = fs::symlink_metadata(path).map_err(|_| floor_unavailable())?;
    if !same_file(&before, &after) || !same_file(&opened, &after) {
        return Err(floor_unavailable());
    }
    validate_owner_only(path, &after, native_root)?;
    Ok(Some(bytes))
}

#[cfg(unix)]
fn validate_owner_only_directory(
    metadata: &fs::Metadata,
    native_root: &Path,
) -> Result<(), PortError> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let root = fs::metadata(native_root).map_err(|_| floor_unavailable())?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != root.uid()
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(floor_invalid());
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_owner_only_directory(
    metadata: &fs::Metadata,
    _native_root: &Path,
) -> Result<(), PortError> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(floor_invalid());
    }
    Ok(())
}

#[cfg(unix)]
fn validate_owner_only(
    _path: &Path,
    metadata: &fs::Metadata,
    native_root: &Path,
) -> Result<(), PortError> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let root = fs::metadata(native_root).map_err(|_| floor_unavailable())?;
    if metadata.uid() != root.uid() || metadata.permissions().mode() & 0o777 != 0o600 {
        return Err(floor_invalid());
    }
    Ok(())
}

#[cfg(unix)]
fn validate_owner_only_lock_file(
    metadata: &fs::Metadata,
    native_root: &Path,
) -> Result<(), PortError> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let root = fs::metadata(native_root).map_err(|_| floor_unavailable())?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != root.uid()
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        return Err(floor_invalid());
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_owner_only_lock_file(
    metadata: &fs::Metadata,
    _native_root: &Path,
) -> Result<(), PortError> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(floor_invalid());
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_owner_only(
    _path: &Path,
    _metadata: &fs::Metadata,
    _native_root: &Path,
) -> Result<(), PortError> {
    Ok(())
}

#[cfg(unix)]
fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
}

#[cfg(unix)]
fn same_directory(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_directory(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.is_dir() && right.is_dir()
}

#[cfg(not(unix))]
fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.len() == right.len()
        && left
            .modified()
            .ok()
            .zip(right.modified().ok())
            .is_some_and(|(left, right)| left == right)
}

fn lower_hex(bytes: &[u8; 32]) -> String {
    let mut output = String::with_capacity(64);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn floor_invalid() -> PortError {
    PortError::new(
        "native_state_floor_invalid",
        "native state compatibility floor is invalid",
    )
}

fn floor_unavailable() -> PortError {
    PortError::new(
        "native_state_floor_unavailable",
        "native state compatibility floor is unavailable",
    )
}

fn cancelled() -> PortError {
    PortError::new(
        "native_state_floor_cancelled",
        "native state compatibility floor operation was cancelled",
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            #[cfg(unix)]
            let base = PathBuf::from("/tmp");
            #[cfg(not(unix))]
            let base = std::env::temp_dir();
            let path = base.join(format!(
                "heyfood-native-state-floor-{}-{nonce}-{sequence}",
                std::process::id(),
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn private_root() -> (TestDirectory, [u8; 32]) {
        let temporary = TestDirectory::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let digest = household_native_root_instance_digest_v1(temporary.path()).unwrap();
        (temporary, digest)
    }

    #[tokio::test]
    async fn successful_probe_precedes_exact_immutable_floor() {
        let (temporary, digest) = private_root();
        let store = NativeStateFloorStore::open(temporary.path(), digest).unwrap();
        let probes = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&probes);

        let floor = store
            .ensure_after_secure_store_probe(CancellationToken::new(), move |_| async move {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await
            .unwrap();

        assert_eq!(probes.load(Ordering::SeqCst), 1);
        assert_eq!(
            floor.minimum_compatible_native_state_version(),
            MINIMUM_COMPATIBLE_NATIVE_STATE_VERSION_V1
        );
        let bytes = fs::read(store.floor_path()).unwrap();
        assert_eq!(bytes, floor.canonical_bytes().unwrap());
        assert!(!bytes.ends_with(b"\n"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(store.floor_path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(store.floor_path().parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn fresh_lock_is_exact_under_restrictive_umask() {
        const CHILD: &str = "HEYFOOD_TEST_NATIVE_FLOOR_UMASK_CHILD";
        if std::env::var_os(CHILD).as_deref() == Some(std::ffi::OsStr::new("1")) {
            let (temporary, digest) = private_root();
            let store = NativeStateFloorStore::open(temporary.path(), digest).unwrap();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let first = store.clone();
            let second = store.clone();
            let (left, right) = runtime.block_on(async move {
                tokio::join!(
                    first.ensure_after_secure_store_probe(CancellationToken::new(), |_| async {
                        Ok(())
                    },),
                    second.ensure_after_secure_store_probe(CancellationToken::new(), |_| async {
                        Ok(())
                    },)
                )
            });
            assert_eq!(left.unwrap(), right.unwrap());
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(store.lock_path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            return;
        }

        let executable = std::env::current_exe().unwrap();
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg("umask 0277; exec \"$1\" --exact native_state_floor::tests::fresh_lock_is_exact_under_restrictive_umask --nocapture")
            .arg("sh")
            .arg(executable)
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[tokio::test]
    async fn failed_probe_creates_no_floor_artifact() {
        let (temporary, digest) = private_root();
        let store = NativeStateFloorStore::open(temporary.path(), digest).unwrap();

        let error = store
            .ensure_after_secure_store_probe(CancellationToken::new(), |_| async {
                Err(PortError::new(
                    "household_secure_store_unavailable",
                    "secure store is unavailable",
                ))
            })
            .await
            .unwrap_err();

        assert_eq!(error.code, "household_secure_store_unavailable");
        assert!(!store.floor_path().exists());
        assert!(!store.lock_path().exists());
    }

    #[tokio::test]
    async fn existing_mismatch_is_never_overwritten() {
        let (temporary, digest) = private_root();
        let store = NativeStateFloorStore::open(temporary.path(), digest).unwrap();
        OwnerOnlyPath::directory(store.floor_path().parent().unwrap()).unwrap();
        let mismatched = br#"{"floor_revision":2}"#;
        AtomicFile::replace(&store.floor_path(), mismatched).unwrap();

        let error = store
            .ensure_after_secure_store_probe(CancellationToken::new(), |_| async { Ok(()) })
            .await
            .unwrap_err();

        assert_eq!(error.code, "native_state_floor_invalid");
        assert_eq!(fs::read(store.floor_path()).unwrap(), mismatched);
    }

    #[tokio::test]
    async fn unknown_duplicate_and_unsorted_capabilities_fail_closed() {
        let (temporary, digest) = private_root();
        let store = NativeStateFloorStore::open(temporary.path(), digest).unwrap();
        OwnerOnlyPath::directory(store.floor_path().parent().unwrap()).unwrap();
        let mut value = serde_json::to_value(NativeStateFloorV1::expected(digest)).unwrap();
        value["required_binary_capabilities"] = serde_json::json!([
            "household-lifecycle-lock-v1",
            "household-account-slot-v1",
            "household-migration-guard-v1",
            "household-teardown-journal-v1"
        ]);
        let bytes = to_canonical_bytes_v1(&value).unwrap();
        AtomicFile::replace(&store.floor_path(), &bytes).unwrap();

        let error = store.load(CancellationToken::new()).await.unwrap_err();
        assert_eq!(error.code, "native_state_floor_invalid");
    }

    #[tokio::test]
    async fn concurrent_first_enable_converges_on_one_exact_floor() {
        let (temporary, digest) = private_root();
        let store = NativeStateFloorStore::open(temporary.path(), digest).unwrap();
        let first = store.clone();
        let second = store.clone();

        let (left, right) = tokio::join!(
            first.ensure_after_secure_store_probe(CancellationToken::new(), |_| async { Ok(()) }),
            second.ensure_after_secure_store_probe(CancellationToken::new(), |_| async { Ok(()) })
        );

        assert_eq!(left.unwrap(), right.unwrap());
        let bytes = fs::read(store.floor_path()).unwrap();
        assert_eq!(
            bytes,
            NativeStateFloorV1::expected(digest)
                .canonical_bytes()
                .unwrap()
        );
    }

    #[tokio::test]
    async fn pre_cancelled_operation_runs_no_probe_and_creates_nothing() {
        let (temporary, digest) = private_root();
        let store = NativeStateFloorStore::open(temporary.path(), digest).unwrap();
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let probes = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&probes);

        let error = store
            .ensure_after_secure_store_probe(cancellation, move |_| async move {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await
            .unwrap_err();

        assert_eq!(error.code, "native_state_floor_cancelled");
        assert_eq!(probes.load(Ordering::SeqCst), 0);
        assert!(!store.floor_path().exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn permissive_compatibility_directory_fails_closed_without_repair() {
        use std::os::unix::fs::PermissionsExt as _;

        let (temporary, digest) = private_root();
        let store = NativeStateFloorStore::open(temporary.path(), digest).unwrap();
        let floor_path = store.floor_path();
        let compatibility = floor_path.parent().unwrap();
        fs::create_dir(compatibility).unwrap();
        fs::set_permissions(compatibility, fs::Permissions::from_mode(0o755)).unwrap();

        let error = store
            .ensure_after_secure_store_probe(CancellationToken::new(), |_| async { Ok(()) })
            .await
            .unwrap_err();

        assert_eq!(error.code, "native_state_floor_invalid");
        assert_eq!(
            fs::metadata(compatibility).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert!(!store.floor_path().exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unsafe_existing_lock_is_rejected_without_chmod_or_following() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let (temporary, digest) = private_root();
        let store = NativeStateFloorStore::open(temporary.path(), digest).unwrap();
        OwnerOnlyPath::directory(store.floor_path().parent().unwrap()).unwrap();
        fs::write(store.lock_path(), b"retained").unwrap();
        fs::set_permissions(store.lock_path(), fs::Permissions::from_mode(0o644)).unwrap();

        let error = store
            .ensure_after_secure_store_probe(CancellationToken::new(), |_| async { Ok(()) })
            .await
            .unwrap_err();
        assert_eq!(error.code, "native_state_floor_invalid");
        assert_eq!(
            fs::metadata(store.lock_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o644
        );

        fs::remove_file(store.lock_path()).unwrap();
        let outside = temporary.path().join("outside-lock");
        fs::write(&outside, b"outside").unwrap();
        symlink(&outside, store.lock_path()).unwrap();
        let error = store
            .ensure_after_secure_store_probe(CancellationToken::new(), |_| async { Ok(()) })
            .await
            .unwrap_err();
        assert_eq!(error.code, "native_state_floor_invalid");
        assert_eq!(fs::read(outside).unwrap(), b"outside");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn socket_lock_path_is_rejected_without_blocking() {
        use std::os::unix::net::UnixListener;

        let (temporary, digest) = private_root();
        let store = NativeStateFloorStore::open(temporary.path(), digest).unwrap();
        OwnerOnlyPath::directory(store.floor_path().parent().unwrap()).unwrap();
        let _listener = UnixListener::bind(store.lock_path()).unwrap();

        let error = store
            .ensure_after_secure_store_probe(CancellationToken::new(), |_| async { Ok(()) })
            .await
            .unwrap_err();

        assert_eq!(error.code, "native_state_floor_invalid");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn floor_requires_exact_file_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        let (temporary, digest) = private_root();
        let store = NativeStateFloorStore::open(temporary.path(), digest).unwrap();
        let floor = store
            .ensure_after_secure_store_probe(CancellationToken::new(), |_| async { Ok(()) })
            .await
            .unwrap();
        assert_eq!(floor.floor_revision(), 1);
        fs::set_permissions(store.floor_path(), fs::Permissions::from_mode(0o400)).unwrap();

        let error = store.load(CancellationToken::new()).await.unwrap_err();
        assert_eq!(error.code, "native_state_floor_invalid");
        assert_eq!(
            fs::metadata(store.floor_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o400
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn redirected_compatibility_directory_is_never_followed() {
        use std::os::unix::fs::symlink;

        let (temporary, digest) = private_root();
        let store = NativeStateFloorStore::open(temporary.path(), digest).unwrap();
        let outside = temporary.path().join("outside-directory");
        fs::create_dir(&outside).unwrap();
        let floor_path = store.floor_path();
        let compatibility = floor_path.parent().unwrap();
        symlink(&outside, compatibility).unwrap();

        let error = store
            .ensure_after_secure_store_probe(CancellationToken::new(), |_| async { Ok(()) })
            .await
            .unwrap_err();

        assert_eq!(error.code, "native_state_floor_invalid");
        assert!(fs::read_dir(&outside).unwrap().next().is_none());
        assert!(
            fs::symlink_metadata(compatibility)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_floor_is_rejected_without_following_or_replacing_it() {
        use std::os::unix::fs::symlink;

        let (temporary, digest) = private_root();
        let store = NativeStateFloorStore::open(temporary.path(), digest).unwrap();
        OwnerOnlyPath::directory(store.floor_path().parent().unwrap()).unwrap();
        let outside = temporary.path().join("outside");
        fs::write(&outside, b"outside").unwrap();
        symlink(&outside, store.floor_path()).unwrap();

        let error = store
            .ensure_after_secure_store_probe(CancellationToken::new(), |_| async { Ok(()) })
            .await
            .unwrap_err();

        assert_eq!(error.code, "native_state_floor_invalid");
        assert_eq!(fs::read(&outside).unwrap(), b"outside");
        assert!(
            fs::symlink_metadata(store.floor_path())
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
