//! Account-bound encrypted household vault.
//!
//! The authenticated journal is the only load authority. The three physical
//! generation slots provide two retained generations without overwriting
//! either journaled generation before a commit point.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use chacha20poly1305::aead::{Aead as _, Generate as _, Payload};
use chacha20poly1305::{KeyInit as _, XChaCha20Poly1305, XNonce};
use fs2::FileExt as _;
use heyfood_application::PortError;
use heyfood_core::{AccountId, decode_canonical_household_state_v1};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::credential_broker::{
    HouseholdKeyBundle, HouseholdKeyBundlePhase, HouseholdKeyMaterial, HouseholdKeyStore,
    HouseholdMigrationGuardDocument, HouseholdMigrationGuardStateV1, HouseholdMigrationGuardStore,
    HouseholdMigrationInitializationPhaseV1, HouseholdMigrationRepairFailureCategoryV1,
    HouseholdSecureStore, KeyId, MigrationGuardExpectation,
};
use crate::{AtomicFile, NativePaths};

pub const VAULT_ENVELOPE_HEADER_BYTES: usize = 84;
pub const MAX_HOUSEHOLD_VAULT_PLAINTEXT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_HOUSEHOLD_VAULT_CIPHERTEXT_BYTES: usize = MAX_HOUSEHOLD_VAULT_PLAINTEXT_BYTES + 16;

const ENVELOPE_MAGIC: &[u8; 8] = b"HFVAULT1";
const ENVELOPE_VERSION: u16 = 1;
const STATE_SCHEMA_VERSION: u16 = 1;
const CANONICAL_BYTES_VERSION: u16 = 1;
const JOURNAL_SLOT: u8 = u8::MAX;
const JOURNAL_SCHEMA_VERSION: u16 = 1;
const AAD_LABEL: &[u8] = b"heyfood.household.vault.aad.v1";
const HKDF_SALT: &[u8] = b"heyfood.household.vault.hkdf.salt.v1";
const GENERATION_HKDF_INFO: &[u8] = b"heyfood.household.vault.subkey.generation.v1";
const JOURNAL_HKDF_INFO: &[u8] = b"heyfood.household.vault.subkey.journal.v1";
const LOCK_TIMEOUT: Duration = Duration::from_secs(30);
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeRootPlatformV1 {
    Macos,
    Linux,
}

impl NativeRootPlatformV1 {
    const fn label(self) -> &'static [u8] {
        match self {
            Self::Macos => b"macos",
            Self::Linux => b"linux",
        }
    }

    #[cfg(target_os = "macos")]
    const fn current() -> Result<Self, PortError> {
        Ok(Self::Macos)
    }

    #[cfg(target_os = "linux")]
    const fn current() -> Result<Self, PortError> {
        Ok(Self::Linux)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    fn current() -> Result<Self, PortError> {
        Err(PortError::new(
            "household_secure_store_unavailable",
            "native household root identity is unavailable on this platform",
        ))
    }
}

pub fn household_native_root_instance_digest_v1(native_root: &Path) -> Result<[u8; 32], PortError> {
    if !native_root.is_absolute() {
        return Err(PortError::new(
            "household_native_root",
            "native household root must be absolute",
        ));
    }
    let metadata = std::fs::symlink_metadata(native_root).map_err(|_| {
        PortError::new(
            "household_native_root",
            "native household root is unavailable",
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(PortError::new(
            "household_native_root",
            "native household root must be a physical directory",
        ));
    }
    let physical = std::fs::canonicalize(native_root).map_err(|_| {
        PortError::new(
            "household_native_root",
            "native household root is unavailable",
        )
    })?;
    let physical_metadata = std::fs::symlink_metadata(&physical).map_err(|_| {
        PortError::new(
            "household_native_root",
            "native household root is unavailable",
        )
    })?;
    if physical_metadata.file_type().is_symlink() || !physical_metadata.is_dir() {
        return Err(PortError::new(
            "household_native_root",
            "native household root must be a physical directory",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

        if metadata.dev() != physical_metadata.dev()
            || metadata.ino() != physical_metadata.ino()
            || metadata.uid() != physical_metadata.uid()
            || physical_metadata.permissions().mode() & 0o077 != 0
        {
            return Err(PortError::new(
                "household_native_root",
                "native household root changed or is not owner-only",
            ));
        }
        domain_hash_v1(
            b"heyfood.household.native-root-instance.v1",
            &[
                NativeRootPlatformV1::current()?.label(),
                physical.as_os_str().as_bytes(),
            ],
        )
    }
    #[cfg(not(unix))]
    {
        let _ = (physical, physical_metadata);
        Err(PortError::new(
            "household_secure_store_unavailable",
            "native household root identity is unavailable on this platform",
        ))
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct HouseholdAccountSlotV1 {
    account_digest: [u8; 32],
    native_root_instance_digest: [u8; 32],
    account_locator_digest: [u8; 32],
    directory_name: String,
}

impl HouseholdAccountSlotV1 {
    #[cfg(feature = "native-credentials")]
    pub(crate) fn from_components(
        account_digest: [u8; 32],
        native_root_instance_digest: [u8; 32],
        account_locator_digest: [u8; 32],
        directory_name: String,
    ) -> Result<Self, PortError> {
        let slot = Self {
            account_digest,
            native_root_instance_digest,
            account_locator_digest,
            directory_name,
        };
        slot.validate()?;
        Ok(slot)
    }

    pub fn from_root_bytes(
        account: &AccountId,
        platform: NativeRootPlatformV1,
        native_root_absolute_physical_path_bytes: &[u8],
    ) -> Result<Self, PortError> {
        if native_root_absolute_physical_path_bytes.is_empty()
            || native_root_absolute_physical_path_bytes.last() == Some(&b'/')
            || native_root_absolute_physical_path_bytes.first() != Some(&b'/')
        {
            return Err(PortError::new(
                "household_native_root",
                "native household root bytes are not an absolute canonical path",
            ));
        }
        let account_digest = domain_hash_v1(
            b"heyfood.household.account-digest.v1",
            &[account.as_str().as_bytes()],
        )?;
        let native_root_instance_digest = domain_hash_v1(
            b"heyfood.household.native-root-instance.v1",
            &[platform.label(), native_root_absolute_physical_path_bytes],
        )?;
        let account_locator_digest = domain_hash_v1(
            b"heyfood.household.account-locator.v1",
            &[&native_root_instance_digest, &account_digest],
        )?;
        let directory_name = lower_hex(&account_digest);
        let slot = Self {
            account_digest,
            native_root_instance_digest,
            account_locator_digest,
            directory_name,
        };
        slot.validate()?;
        Ok(slot)
    }

    fn validate(&self) -> Result<(), PortError> {
        if self.directory_name.len() != 64
            || self
                .directory_name
                .as_bytes()
                .iter()
                .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(byte))
            || self.directory_name != lower_hex(&self.account_digest)
        {
            return Err(PortError::new(
                "household_account_slot",
                "household account slot is invalid",
            ));
        }
        let expected_locator = domain_hash_v1(
            b"heyfood.household.account-locator.v1",
            &[&self.native_root_instance_digest, &self.account_digest],
        )?;
        if expected_locator != self.account_locator_digest {
            return Err(PortError::new(
                "household_account_slot",
                "household account locator digest is invalid",
            ));
        }
        Ok(())
    }

    #[must_use]
    pub const fn account_digest(&self) -> [u8; 32] {
        self.account_digest
    }

    #[must_use]
    pub const fn native_root_instance_digest(&self) -> [u8; 32] {
        self.native_root_instance_digest
    }

    #[must_use]
    pub const fn account_locator_digest(&self) -> [u8; 32] {
        self.account_locator_digest
    }

    #[must_use]
    pub fn directory_name(&self) -> &str {
        &self.directory_name
    }
}

impl fmt::Debug for HouseholdAccountSlotV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdAccountSlotV1")
            .field("account_digest", &lower_hex(&self.account_digest))
            .field(
                "native_root_instance_digest",
                &lower_hex(&self.native_root_instance_digest),
            )
            .field(
                "account_locator_digest",
                &lower_hex(&self.account_locator_digest),
            )
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum VaultArtifactKindV1 {
    Generation = 0,
    Journal = 1,
}

impl VaultArtifactKindV1 {
    fn from_byte(value: u8) -> Result<Self, PortError> {
        match value {
            0 => Ok(Self::Generation),
            1 => Ok(Self::Journal),
            _ => Err(vault_format_error()),
        }
    }

    const fn hkdf_info(self) -> &'static [u8] {
        match self {
            Self::Generation => GENERATION_HKDF_INFO,
            Self::Journal => JOURNAL_HKDF_INFO,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VaultEnvelopeHeaderV1 {
    pub artifact_kind: VaultArtifactKindV1,
    pub slot: u8,
    pub key_id: KeyId,
    pub revision: u64,
    pub commit_id: Uuid,
    pub nonce: [u8; 24],
    pub ciphertext_length: u32,
}

impl VaultEnvelopeHeaderV1 {
    pub fn generation(
        slot: u8,
        key_id: KeyId,
        state_revision: u64,
        commit_id: Uuid,
        nonce: [u8; 24],
        ciphertext_length: u32,
    ) -> Result<Self, PortError> {
        let header = Self {
            artifact_kind: VaultArtifactKindV1::Generation,
            slot,
            key_id,
            revision: state_revision,
            commit_id,
            nonce,
            ciphertext_length,
        };
        header.validate()?;
        Ok(header)
    }

    pub fn journal(
        key_id: KeyId,
        journal_revision: u64,
        current_commit_id: Uuid,
        nonce: [u8; 24],
        ciphertext_length: u32,
    ) -> Result<Self, PortError> {
        let header = Self {
            artifact_kind: VaultArtifactKindV1::Journal,
            slot: JOURNAL_SLOT,
            key_id,
            revision: journal_revision,
            commit_id: current_commit_id,
            nonce,
            ciphertext_length,
        };
        header.validate()?;
        Ok(header)
    }

    fn validate(&self) -> Result<(), PortError> {
        let slot_valid = match self.artifact_kind {
            VaultArtifactKindV1::Generation => self.slot <= 2,
            VaultArtifactKindV1::Journal => self.slot == JOURNAL_SLOT,
        };
        let ciphertext_length =
            usize::try_from(self.ciphertext_length).map_err(|_| vault_format_error())?;
        if !slot_valid
            || self.revision == 0
            || self.key_id.as_uuid().is_nil()
            || self.commit_id.is_nil()
            || !(16..=MAX_HOUSEHOLD_VAULT_CIPHERTEXT_BYTES).contains(&ciphertext_length)
        {
            return Err(vault_format_error());
        }
        Ok(())
    }

    #[must_use]
    pub fn encode(self) -> [u8; VAULT_ENVELOPE_HEADER_BYTES] {
        let mut bytes = [0_u8; VAULT_ENVELOPE_HEADER_BYTES];
        bytes[0..8].copy_from_slice(ENVELOPE_MAGIC);
        bytes[8..10].copy_from_slice(&ENVELOPE_VERSION.to_be_bytes());
        bytes[10] = self.artifact_kind as u8;
        bytes[11] = self.slot;
        bytes[12..14].copy_from_slice(&STATE_SCHEMA_VERSION.to_be_bytes());
        bytes[14..16].copy_from_slice(&CANONICAL_BYTES_VERSION.to_be_bytes());
        bytes[16..32].copy_from_slice(self.key_id.as_uuid().as_bytes());
        bytes[32..40].copy_from_slice(&self.revision.to_be_bytes());
        bytes[40..56].copy_from_slice(self.commit_id.as_bytes());
        bytes[56..80].copy_from_slice(&self.nonce);
        bytes[80..84].copy_from_slice(&self.ciphertext_length.to_be_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, PortError> {
        if bytes.len() != VAULT_ENVELOPE_HEADER_BYTES
            || &bytes[0..8] != ENVELOPE_MAGIC
            || u16::from_be_bytes(bytes[8..10].try_into().map_err(|_| vault_format_error())?)
                != ENVELOPE_VERSION
            || u16::from_be_bytes(bytes[12..14].try_into().map_err(|_| vault_format_error())?)
                != STATE_SCHEMA_VERSION
            || u16::from_be_bytes(bytes[14..16].try_into().map_err(|_| vault_format_error())?)
                != CANONICAL_BYTES_VERSION
        {
            return Err(vault_format_error());
        }
        let artifact_kind = VaultArtifactKindV1::from_byte(bytes[10])?;
        let key_id = KeyId::from_uuid(Uuid::from_bytes(
            bytes[16..32].try_into().map_err(|_| vault_format_error())?,
        ));
        let revision =
            u64::from_be_bytes(bytes[32..40].try_into().map_err(|_| vault_format_error())?);
        let commit_id =
            Uuid::from_bytes(bytes[40..56].try_into().map_err(|_| vault_format_error())?);
        let nonce = bytes[56..80].try_into().map_err(|_| vault_format_error())?;
        let ciphertext_length =
            u32::from_be_bytes(bytes[80..84].try_into().map_err(|_| vault_format_error())?);
        let header = Self {
            artifact_kind,
            slot: bytes[11],
            key_id,
            revision,
            commit_id,
            nonce,
            ciphertext_length,
        };
        header.validate()?;
        Ok(header)
    }
}

pub fn household_vault_aad_v1(
    account: &AccountId,
    slot: &HouseholdAccountSlotV1,
    header: VaultEnvelopeHeaderV1,
) -> Result<Vec<u8>, PortError> {
    slot.validate()?;
    let account_bytes = account.as_str().as_bytes();
    let account_length = u32::try_from(account_bytes.len()).map_err(|_| {
        PortError::new(
            "household_vault_aad",
            "household account binding is too large",
        )
    })?;
    let mut aad = Vec::with_capacity(
        AAD_LABEL.len() + 4 + account_bytes.len() + 32 + 32 + VAULT_ENVELOPE_HEADER_BYTES,
    );
    aad.extend_from_slice(AAD_LABEL);
    aad.extend_from_slice(&account_length.to_be_bytes());
    aad.extend_from_slice(account_bytes);
    aad.extend_from_slice(&slot.account_digest);
    aad.extend_from_slice(&slot.native_root_instance_digest);
    aad.extend_from_slice(&header.encode());
    Ok(aad)
}

#[derive(Clone, Eq, PartialEq)]
pub struct HouseholdVaultWrite {
    pub state_revision: u64,
    pub commit_id: Uuid,
    pub canonical_state: Zeroizing<Vec<u8>>,
}

impl HouseholdVaultWrite {
    pub fn new(
        state_revision: u64,
        commit_id: Uuid,
        canonical_state: Vec<u8>,
    ) -> Result<Self, PortError> {
        let canonical_state = Zeroizing::new(canonical_state);
        if state_revision == 0
            || commit_id.is_nil()
            || canonical_state.is_empty()
            || canonical_state.len() > MAX_HOUSEHOLD_VAULT_PLAINTEXT_BYTES
        {
            return Err(PortError::new(
                "household_vault_state",
                "canonical household state is invalid",
            ));
        }
        let state = decode_canonical_household_state_v1(&canonical_state).map_err(|_| {
            PortError::new(
                "household_vault_state",
                "canonical household state is invalid",
            )
        })?;
        if state.revision.get() != state_revision {
            return Err(PortError::new(
                "household_vault_state",
                "canonical household state revision does not match the write",
            ));
        }
        Ok(Self {
            state_revision,
            commit_id,
            canonical_state,
        })
    }

    #[must_use]
    pub fn plaintext_sha256(&self) -> [u8; 32] {
        sha256(&self.canonical_state)
    }
}

impl fmt::Debug for HouseholdVaultWrite {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdVaultWrite")
            .field("state_revision", &self.state_revision)
            .field("commit_id", &self.commit_id)
            .field("plaintext_sha256", &lower_hex(&self.plaintext_sha256()))
            .field("canonical_byte_length", &self.canonical_state.len())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HouseholdVaultHealthV1 {
    Healthy,
    PreviousRepairedFromAuthoritativeCurrent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HouseholdVaultInitializationAbortV1 {
    VerifiedAlreadyAbsent,
    DeletedAndVerified { artifact_count: u8 },
}

/// Closed, content-free artifact evidence used only by native-household
/// startup while both the account lifecycle and vault leases are retained.
///
/// The startup classifier never receives paths, ciphertext, key material, or
/// plaintext through this seam. Every present initialization artifact has
/// already authenticated against the exact guard/key transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HouseholdVaultStartupArtifactsV1 {
    Absent,
    MatchingUncommitted,
    MatchingCommitted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InitializationAbortArtifactResultV1 {
    CleanupRequired { artifact_count: u8 },
    VerifiedAlreadyAbsent,
}

struct AuthenticatedInitializationArtifactsV1 {
    generations: [Option<DecryptedArtifact>; 3],
    journal: Option<VaultJournalV1>,
}

/// An exclusive, account-bound lease for account lifecycle reads and for
/// entering the narrower vault critical section.
pub struct HouseholdLifecycleLease {
    account_slot: HouseholdAccountSlotV1,
    lock: Arc<LockedFile>,
    lock_path: PathBuf,
    #[cfg(unix)]
    owner_uid: u32,
}

impl HouseholdLifecycleLease {
    #[must_use]
    pub fn account_slot(&self) -> &HouseholdAccountSlotV1 {
        &self.account_slot
    }

    pub(crate) fn validate_for(
        &self,
        account_slot: &HouseholdAccountSlotV1,
    ) -> Result<(), PortError> {
        if self.account_slot != *account_slot {
            return Err(PortError::new(
                "household_lifecycle_lease_mismatch",
                "household lifecycle lease does not match the requested account slot",
            ));
        }
        #[cfg(unix)]
        validate_held_lock(&self.lock, &self.lock_path, self.owner_uid)?;
        #[cfg(not(unix))]
        validate_held_lock(&self.lock, &self.lock_path)?;
        Ok(())
    }
}

impl fmt::Debug for HouseholdLifecycleLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdLifecycleLease")
            .field("account_slot", &self.account_slot)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HouseholdVaultLeaseModeV1 {
    CreateIfMissing,
    RequireExisting,
}

/// A non-cloneable lease retaining both `account-lifecycle.lock` and
/// `vault.lock`. All vault mutations and secure-store CAS operations require
/// this type so the copy-on-write files, key bundle, and migration guard share
/// one critical section.
pub struct HouseholdVaultLease {
    lifecycle_lease: HouseholdLifecycleLease,
    vault_lock: Arc<LockedFile>,
    vault_lock_path: PathBuf,
    #[cfg(unix)]
    owner_uid: u32,
    operation_gate: Arc<tokio::sync::Mutex<()>>,
}

impl HouseholdVaultLease {
    #[must_use]
    pub fn account_slot(&self) -> &HouseholdAccountSlotV1 {
        self.lifecycle_lease.account_slot()
    }

    #[must_use]
    pub fn lifecycle_lease(&self) -> &HouseholdLifecycleLease {
        &self.lifecycle_lease
    }

    pub async fn release_vault(
        self,
        cancellation: CancellationToken,
    ) -> Result<HouseholdLifecycleLease, PortError> {
        let _operation = self.acquire_operation(&cancellation).await?;
        self.validate_for(self.account_slot()).map_err(|_| {
            PortError::uncertain(
                "household_vault_release",
                "household vault lease release requires reconciliation",
            )
        })?;
        let Self {
            lifecycle_lease,
            vault_lock,
            vault_lock_path: _,
            #[cfg(unix)]
                owner_uid: _,
            operation_gate: _,
        } = self;
        drop(vault_lock);
        Ok(lifecycle_lease)
    }

    pub(crate) fn validate_for(
        &self,
        account_slot: &HouseholdAccountSlotV1,
    ) -> Result<(), PortError> {
        self.lifecycle_lease.validate_for(account_slot)?;
        #[cfg(unix)]
        validate_held_lock(&self.vault_lock, &self.vault_lock_path, self.owner_uid)?;
        #[cfg(not(unix))]
        validate_held_lock(&self.vault_lock, &self.vault_lock_path)?;
        Ok(())
    }

    pub(crate) async fn acquire_operation(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<HouseholdVaultLeaseOperationGuard, PortError> {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        let lifecycle_lock = Arc::clone(&self.lifecycle_lease.lock);
        let vault_lock = Arc::clone(&self.vault_lock);
        let operation_gate = Arc::clone(&self.operation_gate);
        let gate = tokio::select! {
            _ = cancellation.cancelled() => return Err(cancelled_error()),
            result = tokio::time::timeout(LOCK_TIMEOUT, operation_gate.lock_owned()) => {
                result.map_err(|_| {
                    PortError::new(
                        "household_vault_lock_timeout",
                        "household vault operation gate acquisition timed out",
                    )
                })?
            }
        };
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        Ok(HouseholdVaultLeaseOperationGuard {
            _gate: gate,
            _lifecycle_lock: lifecycle_lock,
            _vault_lock: vault_lock,
        })
    }
}

pub(crate) struct HouseholdVaultLeaseOperationGuard {
    _gate: tokio::sync::OwnedMutexGuard<()>,
    _lifecycle_lock: Arc<LockedFile>,
    _vault_lock: Arc<LockedFile>,
}

impl fmt::Debug for HouseholdVaultLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdVaultLease")
            .field("account_slot", self.account_slot())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct HouseholdVaultLoad {
    pub state_revision: u64,
    pub commit_id: Uuid,
    pub journal_revision: u64,
    pub canonical_state: Zeroizing<Vec<u8>>,
    pub health: HouseholdVaultHealthV1,
}

impl HouseholdVaultLoad {
    #[must_use]
    pub fn plaintext_sha256(&self) -> [u8; 32] {
        sha256(&self.canonical_state)
    }
}

impl fmt::Debug for HouseholdVaultLoad {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdVaultLoad")
            .field("state_revision", &self.state_revision)
            .field("commit_id", &self.commit_id)
            .field("journal_revision", &self.journal_revision)
            .field("plaintext_sha256", &lower_hex(&self.plaintext_sha256()))
            .field("canonical_byte_length", &self.canonical_state.len())
            .field("health", &self.health)
            .finish()
    }
}

#[derive(Clone)]
pub struct HouseholdVault {
    native_root: PathBuf,
    account_id: AccountId,
    account_slot: HouseholdAccountSlotV1,
    #[cfg(unix)]
    owner_uid: u32,
}

/// Slot-only lock and deletion authority used after account credentials have
/// already been removed. It cannot decrypt, initialize, load, or commit a
/// household because it deliberately carries no raw account ID or key.
#[derive(Clone)]
#[cfg(feature = "native-credentials")]
pub(crate) struct HouseholdTeardownVaultTargetV1 {
    native_root: PathBuf,
    account_slot: HouseholdAccountSlotV1,
    #[cfg(unix)]
    owner_uid: u32,
}

#[cfg(feature = "native-credentials")]
impl HouseholdTeardownVaultTargetV1 {
    pub(crate) fn open(
        native_root: &Path,
        account_slot: HouseholdAccountSlotV1,
    ) -> Result<Self, PortError> {
        if !native_root.is_absolute()
            || household_native_root_instance_digest_v1(native_root)?
                != account_slot.native_root_instance_digest()
        {
            return Err(PortError::new(
                "household_teardown_slot",
                "household teardown slot does not match the native root",
            ));
        }
        let native_root = std::fs::canonicalize(native_root).map_err(|_| {
            PortError::new(
                "household_teardown_slot",
                "household teardown native root is unavailable",
            )
        })?;
        let metadata = std::fs::symlink_metadata(&native_root).map_err(|_| {
            PortError::new(
                "household_teardown_slot",
                "household teardown native root is unavailable",
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(PortError::new(
                "household_teardown_slot",
                "household teardown native root is invalid",
            ));
        }
        #[cfg(unix)]
        let owner_uid = {
            use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(PortError::new(
                    "household_teardown_slot",
                    "household teardown native root is not owner-only",
                ));
            }
            metadata.uid()
        };
        Ok(Self {
            native_root,
            account_slot,
            #[cfg(unix)]
            owner_uid,
        })
    }

    #[must_use]
    pub(crate) fn account_slot(&self) -> &HouseholdAccountSlotV1 {
        &self.account_slot
    }

    pub(crate) async fn acquire_lifecycle_lease(
        &self,
        cancellation: CancellationToken,
    ) -> Result<HouseholdLifecycleLease, PortError> {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        let target = self.clone();
        tokio::task::spawn_blocking(move || target.acquire_lifecycle_lease_blocking(&cancellation))
            .await
            .map_err(|_| {
                PortError::new(
                    "household_teardown_lock_task",
                    "household teardown lock task did not complete",
                )
            })?
    }

    pub(crate) async fn acquire_vault_lease(
        &self,
        lifecycle_lease: HouseholdLifecycleLease,
        cancellation: CancellationToken,
    ) -> Result<HouseholdVaultLease, PortError> {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        lifecycle_lease.validate_for(&self.account_slot)?;
        let target = self.clone();
        tokio::task::spawn_blocking(move || {
            target.acquire_vault_lease_blocking(lifecycle_lease, &cancellation)
        })
        .await
        .map_err(|_| {
            PortError::new(
                "household_teardown_lock_task",
                "household teardown lock task did not complete",
            )
        })?
    }

    pub(crate) async fn ensure_artifacts_absent(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        snapshot_path: &Path,
        expected_snapshot_digest: Option<[u8; 32]>,
        cancellation: CancellationToken,
    ) -> Result<(), PortError> {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        vault_lease.validate_for(&self.account_slot)?;
        let operation = vault_lease.acquire_operation(&cancellation).await?;
        let target = self.clone();
        let snapshot_path = snapshot_path.to_owned();
        let result = tokio::task::spawn_blocking(move || {
            let _operation = operation;
            target.ensure_artifacts_absent_blocking(
                &snapshot_path,
                expected_snapshot_digest,
                &cancellation,
            )
        })
        .await
        .map_err(|_| {
            PortError::new(
                "household_teardown_artifact_task",
                "household teardown artifact task did not complete",
            )
        })?;
        vault_lease
            .validate_for(&self.account_slot)
            .map_err(|_| vault_lease_post_error())?;
        result
    }

    /// Re-read the exact plaintext import snapshot while the teardown's
    /// lifecycle/source/vault lock bundle is retained. Absence is valid
    /// because committed migration normally retires the snapshot before
    /// logout; if a snapshot remains, it must be bound to the guard digest
    /// before the globally resumable teardown journal is created.
    pub(crate) async fn verify_snapshot_evidence(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        snapshot_path: &Path,
        expected_snapshot_digest: Option<[u8; 32]>,
        cancellation: CancellationToken,
    ) -> Result<(), PortError> {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        vault_lease.validate_for(&self.account_slot)?;
        let operation = vault_lease.acquire_operation(&cancellation).await?;
        let target = self.clone();
        let snapshot_path = snapshot_path.to_owned();
        let result = tokio::task::spawn_blocking(move || {
            let _operation = operation;
            target.verify_snapshot_evidence_blocking(
                &snapshot_path,
                expected_snapshot_digest,
                &cancellation,
            )
        })
        .await
        .map_err(|_| {
            PortError::new(
                "household_teardown_snapshot_task",
                "legacy import snapshot inspection did not complete",
            )
        })?;
        vault_lease
            .validate_for(&self.account_slot)
            .map_err(|_| vault_lease_post_error())?;
        result
    }

    /// Validate the closed household directory and report only the number of
    /// canonical ciphertext artifacts. This purpose-limited seam is usable
    /// after the raw account ID and DEK have already been removed.
    pub(crate) async fn startup_artifact_count_for_teardown(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        cancellation: CancellationToken,
    ) -> Result<u8, PortError> {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        vault_lease.validate_for(&self.account_slot)?;
        let operation = vault_lease.acquire_operation(&cancellation).await?;
        let target = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _operation = operation;
            target.startup_artifact_count_for_teardown_blocking()
        })
        .await
        .map_err(|_| {
            PortError::new(
                "household_teardown_artifact_task",
                "household teardown artifact inspection did not complete",
            )
        })?;
        vault_lease
            .validate_for(&self.account_slot)
            .map_err(|_| vault_lease_post_error())?;
        result
    }

    fn acquire_lifecycle_lease_blocking(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<HouseholdLifecycleLease, PortError> {
        let accounts_directory = self.native_root.join("accounts");
        let account_directory = accounts_directory.join(self.account_slot.directory_name());
        ensure_private_directory(&accounts_directory)?;
        ensure_private_directory(&account_directory)?;
        let lifecycle_path = account_directory.join("account-lifecycle.lock");
        let lifecycle_lock = self.acquire_lock(&lifecycle_path, cancellation)?;
        Ok(HouseholdLifecycleLease {
            account_slot: self.account_slot.clone(),
            lock: Arc::new(lifecycle_lock),
            lock_path: lifecycle_path,
            #[cfg(unix)]
            owner_uid: self.owner_uid,
        })
    }

    fn acquire_vault_lease_blocking(
        &self,
        lifecycle_lease: HouseholdLifecycleLease,
        cancellation: &CancellationToken,
    ) -> Result<HouseholdVaultLease, PortError> {
        lifecycle_lease.validate_for(&self.account_slot)?;
        let account_directory = self
            .native_root
            .join("accounts")
            .join(self.account_slot.directory_name());
        let household_directory = account_directory.join("household");
        validate_private_directory(&account_directory)?;
        ensure_private_directory(&household_directory)?;
        let vault_lock_path = household_directory.join("vault.lock");
        let vault_lock = self.acquire_lock(&vault_lock_path, cancellation)?;
        Ok(HouseholdVaultLease {
            lifecycle_lease,
            vault_lock: Arc::new(vault_lock),
            vault_lock_path,
            #[cfg(unix)]
            owner_uid: self.owner_uid,
            operation_gate: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    fn ensure_artifacts_absent_blocking(
        &self,
        snapshot_path: &Path,
        expected_snapshot_digest: Option<[u8; 32]>,
        cancellation: &CancellationToken,
    ) -> Result<(), PortError> {
        let household_directory = self
            .native_root
            .join("accounts")
            .join(self.account_slot.directory_name())
            .join("household");
        validate_private_directory(&household_directory)?;
        for name in [
            "generation-0.hfv",
            "generation-1.hfv",
            "generation-2.hfv",
            "commit.hfj",
        ] {
            self.check_cancelled(cancellation)?;
            self.remove_regular_file_if_present(&household_directory.join(name))?;
        }
        sync_teardown_directory(&household_directory).map_err(|_| {
            PortError::uncertain(
                "household_teardown_artifact_sync",
                "household ciphertext deletion durability is uncertain",
            )
        })?;
        self.check_cancelled(cancellation)?;
        self.remove_snapshot_if_exact(snapshot_path, expected_snapshot_digest)?;
        for name in [
            "generation-0.hfv",
            "generation-1.hfv",
            "generation-2.hfv",
            "commit.hfj",
        ] {
            if path_present(&household_directory.join(name))? {
                return Err(PortError::uncertain(
                    "household_teardown_artifact_verify",
                    "household ciphertext deletion could not be verified",
                ));
            }
        }
        Ok(())
    }

    fn verify_snapshot_evidence_blocking(
        &self,
        snapshot_path: &Path,
        expected_digest: Option<[u8; 32]>,
        cancellation: &CancellationToken,
    ) -> Result<(), PortError> {
        self.check_cancelled(cancellation)?;
        if !snapshot_path.is_absolute() {
            return Err(PortError::new(
                "household_teardown_snapshot",
                "legacy import snapshot locator is invalid",
            ));
        }
        let metadata = match std::fs::symlink_metadata(snapshot_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => {
                return Err(PortError::new(
                    "household_teardown_snapshot",
                    "legacy import snapshot is unavailable",
                ));
            }
            Ok(metadata) => metadata,
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(PortError::new(
                "household_teardown_snapshot",
                "legacy import snapshot is not a regular file",
            ));
        }
        #[cfg(unix)]
        self.validate_owner_only_metadata(&metadata, false)?;
        let Some(expected_digest) = expected_digest else {
            return Err(PortError::new(
                "household_teardown_snapshot_mismatch",
                "an unbound legacy import snapshot cannot enter teardown",
            ));
        };
        let maximum = 4 * 1024 * 1024;
        let length = usize::try_from(metadata.len()).map_err(|_| {
            PortError::new(
                "household_teardown_snapshot",
                "legacy import snapshot exceeds its limit",
            )
        })?;
        if length == 0 || length > maximum {
            return Err(PortError::new(
                "household_teardown_snapshot",
                "legacy import snapshot exceeds its limit",
            ));
        }
        let file = File::open(snapshot_path).map_err(|_| {
            PortError::new(
                "household_teardown_snapshot",
                "legacy import snapshot is unavailable",
            )
        })?;
        let opened = file.metadata().map_err(|_| {
            PortError::new(
                "household_teardown_snapshot",
                "legacy import snapshot is unavailable",
            )
        })?;
        let mut bytes = Zeroizing::new(Vec::with_capacity(length));
        file.take((maximum + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| {
                PortError::new(
                    "household_teardown_snapshot",
                    "legacy import snapshot is unavailable",
                )
            })?;
        self.check_cancelled(cancellation)?;
        let after = std::fs::symlink_metadata(snapshot_path).map_err(|_| {
            PortError::new(
                "household_teardown_snapshot",
                "legacy import snapshot changed during inspection",
            )
        })?;
        if !same_file_metadata(&metadata, &opened)
            || !same_file_metadata(&opened, &after)
            || bytes.len() != length
            || sha256(&bytes) != expected_digest
        {
            return Err(PortError::new(
                "household_teardown_snapshot_mismatch",
                "legacy import snapshot changed before teardown",
            ));
        }
        Ok(())
    }

    fn startup_artifact_count_for_teardown_blocking(&self) -> Result<u8, PortError> {
        const EXPECTED_NAMES: [&str; 5] = [
            "commit.hfj",
            "generation-0.hfv",
            "generation-1.hfv",
            "generation-2.hfv",
            "vault.lock",
        ];
        let household_directory = self
            .native_root
            .join("accounts")
            .join(self.account_slot.directory_name())
            .join("household");
        validate_private_directory(&household_directory)?;
        for entry in std::fs::read_dir(&household_directory).map_err(|_| {
            PortError::new(
                "household_teardown_artifact",
                "household teardown artifact directory is unavailable",
            )
        })? {
            let entry = entry.map_err(|_| {
                PortError::new(
                    "household_teardown_artifact",
                    "household teardown artifact directory is unavailable",
                )
            })?;
            let name = entry.file_name();
            if !EXPECTED_NAMES
                .iter()
                .any(|expected| name == std::ffi::OsStr::new(expected))
            {
                return Err(PortError::new(
                    "household_teardown_artifact",
                    "household teardown artifact directory contains an unknown entry",
                ));
            }
        }
        let mut count = 0_u8;
        for name in [
            "generation-0.hfv",
            "generation-1.hfv",
            "generation-2.hfv",
            "commit.hfj",
        ] {
            let path = household_directory.join(name);
            match std::fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink() || !metadata.is_file() {
                        return Err(PortError::new(
                            "household_teardown_artifact",
                            "household teardown artifact is not a regular physical file",
                        ));
                    }
                    #[cfg(unix)]
                    self.validate_owner_only_metadata(&metadata, false)?;
                    count = count.checked_add(1).ok_or_else(vault_format_error)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {
                    return Err(PortError::new(
                        "household_teardown_artifact",
                        "household teardown artifact could not be inspected",
                    ));
                }
            }
        }
        Ok(count)
    }

    fn remove_snapshot_if_exact(
        &self,
        snapshot_path: &Path,
        expected_digest: Option<[u8; 32]>,
    ) -> Result<(), PortError> {
        if !snapshot_path.is_absolute() {
            return Err(PortError::new(
                "household_teardown_snapshot",
                "legacy import snapshot locator is invalid",
            ));
        }
        let metadata = match std::fs::symlink_metadata(snapshot_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => {
                return Err(PortError::new(
                    "household_teardown_snapshot",
                    "legacy import snapshot is unavailable",
                ));
            }
            Ok(metadata) => metadata,
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(PortError::new(
                "household_teardown_snapshot",
                "legacy import snapshot is not a regular file",
            ));
        }
        #[cfg(unix)]
        self.validate_owner_only_metadata(&metadata, false)?;
        let Some(expected_digest) = expected_digest else {
            return Err(PortError::new(
                "household_teardown_snapshot_mismatch",
                "an unbound legacy import snapshot cannot be deleted",
            ));
        };
        let maximum = 4 * 1024 * 1024;
        let length = usize::try_from(metadata.len()).map_err(|_| {
            PortError::new(
                "household_teardown_snapshot",
                "legacy import snapshot exceeds its limit",
            )
        })?;
        if length == 0 || length > maximum {
            return Err(PortError::new(
                "household_teardown_snapshot",
                "legacy import snapshot exceeds its limit",
            ));
        }
        let mut bytes = Zeroizing::new(Vec::with_capacity(length));
        File::open(snapshot_path)
            .and_then(|file| file.take((maximum + 1) as u64).read_to_end(&mut bytes))
            .map_err(|_| {
                PortError::new(
                    "household_teardown_snapshot",
                    "legacy import snapshot is unavailable",
                )
            })?;
        if bytes.len() != length || sha256(&bytes) != expected_digest {
            return Err(PortError::new(
                "household_teardown_snapshot_mismatch",
                "legacy import snapshot changed before deletion",
            ));
        }
        std::fs::remove_file(snapshot_path).map_err(|_| {
            PortError::uncertain(
                "household_teardown_snapshot_delete",
                "legacy import snapshot deletion is uncertain",
            )
        })?;
        let parent = snapshot_path.parent().ok_or_else(vault_format_error)?;
        sync_teardown_directory(parent).map_err(|_| {
            PortError::uncertain(
                "household_teardown_snapshot_sync",
                "legacy import snapshot deletion durability is uncertain",
            )
        })?;
        if path_present(snapshot_path)? {
            return Err(PortError::uncertain(
                "household_teardown_snapshot_verify",
                "legacy import snapshot deletion could not be verified",
            ));
        }
        Ok(())
    }

    fn remove_regular_file_if_present(&self, path: &Path) -> Result<(), PortError> {
        let metadata = match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => {
                return Err(PortError::new(
                    "household_teardown_artifact",
                    "household ciphertext artifact is unavailable",
                ));
            }
            Ok(metadata) => metadata,
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(PortError::new(
                "household_teardown_artifact",
                "household ciphertext artifact is not a regular file",
            ));
        }
        #[cfg(unix)]
        self.validate_owner_only_metadata(&metadata, false)?;
        std::fs::remove_file(path).map_err(|_| {
            PortError::uncertain(
                "household_teardown_artifact_delete",
                "household ciphertext deletion is uncertain",
            )
        })
    }

    fn acquire_lock(
        &self,
        path: &Path,
        cancellation: &CancellationToken,
    ) -> Result<LockedFile, PortError> {
        let file = open_private_lock(path)?;
        let metadata = file.metadata().map_err(|_| {
            PortError::new(
                "household_teardown_lock",
                "household teardown lock is unavailable",
            )
        })?;
        #[cfg(unix)]
        self.validate_owner_only_metadata(&metadata, false)?;
        let started = Instant::now();
        loop {
            self.check_cancelled(cancellation)?;
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(LockedFile(file)),
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && started.elapsed() < LOCK_TIMEOUT =>
                {
                    thread::sleep(LOCK_RETRY_INTERVAL);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    return Err(PortError::new(
                        "household_teardown_lock_timeout",
                        "household teardown lock acquisition timed out",
                    ));
                }
                Err(_) => {
                    return Err(PortError::new(
                        "household_teardown_lock",
                        "household teardown lock is unavailable",
                    ));
                }
            }
        }
    }

    fn check_cancelled(&self, cancellation: &CancellationToken) -> Result<(), PortError> {
        if cancellation.is_cancelled() {
            Err(cancelled_error())
        } else {
            Ok(())
        }
    }

    #[cfg(unix)]
    fn validate_owner_only_metadata(
        &self,
        metadata: &std::fs::Metadata,
        directory: bool,
    ) -> Result<(), PortError> {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        if (directory && !metadata.is_dir())
            || (!directory && !metadata.is_file())
            || metadata.uid() != self.owner_uid
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(PortError::new(
                "household_teardown_permissions",
                "household teardown path is not owner-only",
            ));
        }
        Ok(())
    }
}

impl HouseholdVault {
    pub fn from_native_paths(
        paths: &NativePaths,
        account_id: AccountId,
    ) -> Result<Self, PortError> {
        Self::open(paths.data_dir(), account_id)
    }

    pub fn open(native_root: &Path, account_id: AccountId) -> Result<Self, PortError> {
        if !native_root.is_absolute() {
            return Err(PortError::new(
                "household_native_root",
                "native household root must be absolute",
            ));
        }
        ensure_private_directory(native_root)?;
        let before = std::fs::symlink_metadata(native_root).map_err(|_| {
            PortError::new(
                "household_native_root",
                "native household root is unavailable",
            )
        })?;
        if before.file_type().is_symlink() || !before.is_dir() {
            return Err(PortError::new(
                "household_native_root",
                "native household root must be a physical directory",
            ));
        }
        let physical = std::fs::canonicalize(native_root).map_err(|_| {
            PortError::new(
                "household_native_root",
                "native household root is unavailable",
            )
        })?;
        let after = std::fs::symlink_metadata(&physical).map_err(|_| {
            PortError::new(
                "household_native_root",
                "native household root is unavailable",
            )
        })?;
        if after.file_type().is_symlink() || !after.is_dir() {
            return Err(PortError::new(
                "household_native_root",
                "native household root must be a physical directory",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

            if before.dev() != after.dev()
                || before.ino() != after.ino()
                || before.uid() != after.uid()
                || after.permissions().mode() & 0o077 != 0
            {
                return Err(PortError::new(
                    "household_native_root",
                    "native household root changed or is not owner-only",
                ));
            }
        }
        #[cfg(unix)]
        let root_bytes = {
            use std::os::unix::ffi::OsStrExt as _;
            physical.as_os_str().as_bytes()
        };
        #[cfg(not(unix))]
        let root_bytes: &[u8] = &[];
        let account_slot = HouseholdAccountSlotV1::from_root_bytes(
            &account_id,
            NativeRootPlatformV1::current()?,
            root_bytes,
        )?;
        #[cfg(unix)]
        let owner_uid = {
            use std::os::unix::fs::MetadataExt as _;
            after.uid()
        };
        Ok(Self {
            native_root: physical,
            account_id,
            account_slot,
            #[cfg(unix)]
            owner_uid,
        })
    }

    #[must_use]
    pub fn account_slot(&self) -> &HouseholdAccountSlotV1 {
        &self.account_slot
    }

    #[must_use]
    pub(crate) fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    #[must_use]
    pub fn account_directory(&self) -> PathBuf {
        self.native_root
            .join("accounts")
            .join(self.account_slot.directory_name())
    }

    #[must_use]
    pub fn household_directory(&self) -> PathBuf {
        self.account_directory().join("household")
    }

    #[must_use]
    pub(crate) fn native_root(&self) -> &Path {
        &self.native_root
    }

    pub async fn acquire_lifecycle_lease(
        &self,
        cancellation: CancellationToken,
    ) -> Result<HouseholdLifecycleLease, PortError> {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        let vault = self.clone();
        tokio::task::spawn_blocking(move || vault.acquire_lifecycle_lease_blocking(&cancellation))
            .await
            .map_err(|_| {
                PortError::new(
                    "household_lifecycle_task",
                    "household lifecycle lease task did not complete",
                )
            })?
    }

    pub async fn acquire_vault_lease(
        &self,
        lifecycle_lease: HouseholdLifecycleLease,
        mode: HouseholdVaultLeaseModeV1,
        cancellation: CancellationToken,
    ) -> Result<HouseholdVaultLease, PortError> {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        lifecycle_lease.validate_for(&self.account_slot)?;
        let vault = self.clone();
        tokio::task::spawn_blocking(move || {
            vault.acquire_vault_lease_blocking(lifecycle_lease, mode, &cancellation)
        })
        .await
        .map_err(|_| {
            PortError::new(
                "household_vault_lease_task",
                "household vault lease task did not complete",
            )
        })?
    }

    /// Acquire the vault lock only when the household directory already
    /// exists. Absence is observed while the retained lifecycle lock excludes
    /// a conforming concurrent creator, and does not create a lock-only
    /// household directory.
    pub(crate) async fn acquire_existing_vault_lease_if_present(
        &self,
        lifecycle_lease: HouseholdLifecycleLease,
        cancellation: CancellationToken,
    ) -> Result<Option<HouseholdVaultLease>, PortError> {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        lifecycle_lease.validate_for(&self.account_slot)?;
        let vault = self.clone();
        tokio::task::spawn_blocking(move || {
            vault.acquire_existing_vault_lease_if_present_blocking(lifecycle_lease, &cancellation)
        })
        .await
        .map_err(|_| {
            PortError::new(
                "household_vault_lease_task",
                "household vault lease task did not complete",
            )
        })?
    }

    /// Classify exact initialization artifacts without exposing their bytes.
    /// The caller supplies the immutable guard tuple; every present artifact
    /// must authenticate to that tuple or classification fails closed.
    pub(crate) async fn classify_startup_artifacts(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        key_bundle: Option<HouseholdKeyBundle>,
        expected_commit_id: Option<Uuid>,
        expected_state_digest: Option<[u8; 32]>,
        cancellation: CancellationToken,
    ) -> Result<HouseholdVaultStartupArtifactsV1, PortError> {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        vault_lease.validate_for(&self.account_slot)?;
        let operation = vault_lease.acquire_operation(&cancellation).await?;
        let vault = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _operation = operation;
            vault.classify_startup_artifacts_blocking(
                key_bundle.as_ref(),
                expected_commit_id,
                expected_state_digest,
            )
        })
        .await
        .map_err(|_| {
            PortError::new(
                "household_vault_evidence_task",
                "household vault evidence task did not complete",
            )
        })?;
        vault_lease
            .validate_for(&self.account_slot)
            .map_err(|_| vault_lease_post_error())?;
        result
    }

    /// Recover the exact canonical write already authenticated in an
    /// uncommitted initialization generation.
    ///
    /// This is a read-only crash-resume seam. It never selects a "newest"
    /// generation and never accepts a journal, generation 2, or generation 1
    /// without generation 0. The returned write remains crate-internal and is
    /// bound to the ready guard, initializing key, account/root, initial
    /// commit, state digest, effect fingerprint, and canonical initial ledger.
    pub(crate) async fn recover_uncommitted_initialization_write(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        key_bundle: HouseholdKeyBundle,
        guard: HouseholdMigrationGuardDocument,
        cancellation: CancellationToken,
    ) -> Result<HouseholdVaultWrite, PortError> {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        vault_lease.validate_for(&self.account_slot)?;
        guard.validate_for(&self.account_slot)?;
        if guard.state() != HouseholdMigrationGuardStateV1::Initializing
            || guard.initialization_phase()
                != Some(HouseholdMigrationInitializationPhaseV1::ReadyToInitialize)
        {
            return Err(initialization_resume_error());
        }
        key_bundle.validate_initial_for(&self.account_slot, &guard)?;
        let operation = vault_lease.acquire_operation(&cancellation).await?;
        let vault = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _operation = operation;
            vault.recover_uncommitted_initialization_write_blocking(
                &key_bundle,
                &guard,
                &cancellation,
            )
        })
        .await
        .map_err(|_| {
            PortError::new(
                "household_vault_initialization_resume_task",
                "household vault initialization resume task did not complete",
            )
        })?;
        vault_lease
            .validate_for(&self.account_slot)
            .map_err(|_| vault_lease_post_error())?;
        result
    }

    /// Validate the closed vault directory namespace and report only how many
    /// canonical encrypted artifact paths are present. This is used for
    /// guard states that require authoritative artifact absence and before a
    /// committed load; it never interprets ciphertext without a key.
    pub(crate) async fn startup_artifact_count(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        cancellation: CancellationToken,
    ) -> Result<u8, PortError> {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        vault_lease.validate_for(&self.account_slot)?;
        let operation = vault_lease.acquire_operation(&cancellation).await?;
        let vault = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _operation = operation;
            vault.validate_startup_directory_entries()?;
            vault.startup_artifact_presence_count()
        })
        .await
        .map_err(|_| {
            PortError::new(
                "household_vault_evidence_task",
                "household vault evidence task did not complete",
            )
        })?;
        vault_lease
            .validate_for(&self.account_slot)
            .map_err(|_| vault_lease_post_error())?;
        result
    }

    pub async fn initialize(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        key_bundle: HouseholdKeyBundle,
        state: HouseholdVaultWrite,
        cancellation: CancellationToken,
    ) -> Result<HouseholdVaultLoad, PortError> {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        vault_lease.validate_for(&self.account_slot)?;
        let operation = vault_lease.acquire_operation(&cancellation).await?;
        vault_lease.validate_for(&self.account_slot)?;
        let vault = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _operation = operation;
            vault.initialize_blocking(&key_bundle, &state, &cancellation, true)
        })
        .await
        .map_err(|_| {
            PortError::uncertain(
                "household_vault_task",
                "household vault task did not complete",
            )
        })?;
        vault_lease
            .validate_for(&self.account_slot)
            .map_err(|_| vault_lease_post_error())?;
        result
    }

    pub async fn load(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        key_bundle: HouseholdKeyBundle,
        cancellation: CancellationToken,
    ) -> Result<HouseholdVaultLoad, PortError> {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        vault_lease.validate_for(&self.account_slot)?;
        let operation = vault_lease.acquire_operation(&cancellation).await?;
        vault_lease.validate_for(&self.account_slot)?;
        let vault = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _operation = operation;
            vault.load_blocking(&key_bundle, &cancellation, true)
        })
        .await
        .map_err(|_| {
            PortError::uncertain(
                "household_vault_task",
                "household vault task did not complete",
            )
        })?;
        vault_lease
            .validate_for(&self.account_slot)
            .map_err(|_| vault_lease_post_error())?;
        result
    }

    pub async fn commit(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        key_bundle: HouseholdKeyBundle,
        expected_revision: u64,
        state: HouseholdVaultWrite,
        cancellation: CancellationToken,
    ) -> Result<HouseholdVaultLoad, PortError> {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        vault_lease.validate_for(&self.account_slot)?;
        let operation = vault_lease.acquire_operation(&cancellation).await?;
        vault_lease.validate_for(&self.account_slot)?;
        let vault = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _operation = operation;
            vault.commit_blocking(&key_bundle, expected_revision, &state, &cancellation, true)
        })
        .await
        .map_err(|_| {
            PortError::uncertain(
                "household_vault_task",
                "household vault task did not complete",
            )
        })?;
        vault_lease
            .validate_for(&self.account_slot)
            .map_err(|_| vault_lease_post_error())?;
        result
    }

    pub async fn rotate(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        rewriting_bundle: HouseholdKeyBundle,
        cancellation: CancellationToken,
    ) -> Result<HouseholdVaultLoad, PortError> {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        vault_lease.validate_for(&self.account_slot)?;
        let operation = vault_lease.acquire_operation(&cancellation).await?;
        vault_lease.validate_for(&self.account_slot)?;
        let vault = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _operation = operation;
            vault.rotate_blocking(&rewriting_bundle, &cancellation, true)
        })
        .await
        .map_err(|_| {
            PortError::uncertain(
                "household_vault_task",
                "household vault task did not complete",
            )
        })?;
        vault_lease
            .validate_for(&self.account_slot)
            .map_err(|_| vault_lease_post_error())?;
        result
    }

    /// Record a durable cleanup-pending guard before deleting any exact
    /// initialization artifact or key, then resume the cleanup to a verified
    /// `blocked_repair` guard.
    ///
    /// A crash after any step leaves `Aborting` as the authoritative guard.
    /// Once that guard is durable, cleanup uses only the guard, the exact
    /// initializing key bundle, and authenticated artifacts; the candidate
    /// state and migration source may be unavailable or changed after restart.
    /// Key initialization is independently bound to a `ReadyToInitialize`
    /// guard, so an absent key can never be reminted while cleanup is pending.
    pub async fn abort_invalid_initialization_to_blocked_repair(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        secure_store: &dyn HouseholdSecureStore,
        expected_initialization_id: Uuid,
        expected_state: Option<HouseholdVaultWrite>,
        failure: HouseholdMigrationRepairFailureCategoryV1,
        cancellation: CancellationToken,
    ) -> Result<HouseholdMigrationGuardDocument, PortError> {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        vault_lease.validate_for(&self.account_slot)?;
        let current = HouseholdMigrationGuardStore::load(
            secure_store,
            vault_lease.lifecycle_lease(),
            cancellation.clone(),
        )
        .await?
        .ok_or_else(|| {
            PortError::new(
                "household_migration_guard_missing",
                "household initialization cleanup requires its migration guard",
            )
        })?;
        current.validate_for(&self.account_slot)?;
        if current.initialization_id() != expected_initialization_id {
            return Err(PortError::new(
                "household_initialization_abort_mismatch",
                "household initialization cleanup does not match the requested transaction",
            ));
        }

        let aborting = match current.state() {
            HouseholdMigrationGuardStateV1::Initializing => {
                self.record_initialization_abort_intent(
                    vault_lease,
                    secure_store,
                    current,
                    expected_state.as_ref(),
                    failure,
                    cancellation.clone(),
                )
                .await?
            }
            HouseholdMigrationGuardStateV1::Aborting => {
                if current.repair_failure_category() != Some(failure) {
                    return Err(PortError::new(
                        "household_initialization_abort_mismatch",
                        "household initialization cleanup failure category changed",
                    ));
                }
                current
            }
            HouseholdMigrationGuardStateV1::BlockedRepair => {
                if current.repair_failure_category() != Some(failure) {
                    return Err(PortError::new(
                        "household_initialization_abort_mismatch",
                        "household initialization repair guard does not match the requested failure",
                    ));
                }
                return Ok(current);
            }
            _ => {
                return Err(PortError::new(
                    "household_initialization_abort_state",
                    "household initialization is not eligible for cleanup",
                ));
            }
        };

        self.resume_initialization_abort(vault_lease, secure_store, aborting, cancellation)
            .await
    }

    async fn record_initialization_abort_intent(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        secure_store: &dyn HouseholdSecureStore,
        initializing_guard: HouseholdMigrationGuardDocument,
        expected_state: Option<&HouseholdVaultWrite>,
        failure: HouseholdMigrationRepairFailureCategoryV1,
        cancellation: CancellationToken,
    ) -> Result<HouseholdMigrationGuardDocument, PortError> {
        initializing_guard.validate_for(&self.account_slot)?;
        let key_bundle = HouseholdKeyStore::load(
            secure_store,
            vault_lease.lifecycle_lease(),
            cancellation.clone(),
        )
        .await?;
        match initializing_guard.initialization_phase() {
            Some(HouseholdMigrationInitializationPhaseV1::ReservedSource) => {
                if expected_state.is_some() || key_bundle.is_some() {
                    return Err(PortError::new(
                        "household_initialization_abort_ambiguous",
                        "reserved household initialization cleanup has unexpected state",
                    ));
                }
                self.verify_initialization_artifacts_absent(vault_lease, cancellation.clone())
                    .await?;
            }
            Some(HouseholdMigrationInitializationPhaseV1::ReadyToInitialize) => {
                let state = expected_state.ok_or_else(|| {
                    PortError::new(
                        "household_initialization_abort_state",
                        "ready household initialization cleanup requires its exact candidate",
                    )
                })?;
                if initializing_guard.initial_commit_id() != state.commit_id
                    || initializing_guard.initial_state_digest() != Some(state.plaintext_sha256())
                {
                    return Err(PortError::new(
                        "household_initialization_abort_mismatch",
                        "household initialization candidate does not match its guard",
                    ));
                }
                let bundle = key_bundle.as_ref().ok_or_else(|| {
                    PortError::new(
                        "household_vault_initialization_resumable",
                        "ready household initialization without a key remains resumable",
                    )
                })?;
                bundle.validate_initial_for(&self.account_slot, &initializing_guard)?;
                let inspected = self
                    .inspect_invalid_initialization_artifacts(
                        vault_lease,
                        bundle.clone(),
                        state.clone(),
                        cancellation.clone(),
                    )
                    .await?;
                if !matches!(
                    inspected,
                    InitializationAbortArtifactResultV1::CleanupRequired { .. }
                ) {
                    return Err(PortError::new(
                        "household_vault_initialization_resumable",
                        "household vault initialization remains resumable",
                    ));
                }
            }
            None => return Err(initialization_abort_ambiguous()),
        }
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }

        let aborting = initializing_guard.begin_aborting(failure)?;
        self.compare_exchange_guard_reconciled(
            vault_lease,
            secure_store,
            &initializing_guard,
            &aborting,
            cancellation,
        )
        .await?;
        Ok(aborting)
    }

    async fn resume_initialization_abort(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        secure_store: &dyn HouseholdSecureStore,
        aborting_guard: HouseholdMigrationGuardDocument,
        cancellation: CancellationToken,
    ) -> Result<HouseholdMigrationGuardDocument, PortError> {
        aborting_guard.validate_for(&self.account_slot)?;
        if aborting_guard.state() != HouseholdMigrationGuardStateV1::Aborting {
            return Err(initialization_abort_ambiguous());
        }
        let key_bundle = HouseholdKeyStore::load(
            secure_store,
            vault_lease.lifecycle_lease(),
            cancellation.clone(),
        )
        .await?;
        match aborting_guard.initialization_phase() {
            Some(HouseholdMigrationInitializationPhaseV1::ReservedSource) => {
                if key_bundle.is_some() {
                    return Err(initialization_abort_ambiguous());
                }
                self.verify_initialization_artifacts_absent(vault_lease, cancellation.clone())
                    .await?;
            }
            Some(HouseholdMigrationInitializationPhaseV1::ReadyToInitialize) => {
                if let Some(bundle) = &key_bundle {
                    bundle.validate_for(&self.account_slot)?;
                    if bundle.phase != HouseholdKeyBundlePhase::Initializing
                        || bundle.initialization_id != Some(aborting_guard.initialization_id())
                        || bundle.initial_commit_id != Some(aborting_guard.initial_commit_id())
                        || bundle.initial_effect_fingerprint
                            != aborting_guard.initial_effect_fingerprint()
                        || bundle.initial_state_digest != aborting_guard.initial_state_digest()
                    {
                        return Err(initialization_abort_ambiguous());
                    }
                    self.delete_aborting_initialization_artifacts(
                        vault_lease,
                        bundle.clone(),
                        aborting_guard.clone(),
                        cancellation.clone(),
                    )
                    .await?;
                } else {
                    self.verify_initialization_artifacts_absent(vault_lease, cancellation.clone())
                        .await?;
                }
            }
            None => return Err(initialization_abort_ambiguous()),
        }

        if cancellation.is_cancelled() {
            return Err(initialization_abort_uncertain());
        }
        if let Some(bundle) = key_bundle {
            let abort_result = HouseholdKeyStore::abort_initialization_and_verify(
                secure_store,
                vault_lease,
                bundle.revision,
                aborting_guard.initialization_id(),
                aborting_guard.clone(),
                cancellation.clone(),
            )
            .await;
            if let Err(error) = abort_result {
                let observed = HouseholdKeyStore::load(
                    secure_store,
                    vault_lease.lifecycle_lease(),
                    CancellationToken::new(),
                )
                .await?;
                if observed.is_some() {
                    return Err(error);
                }
            }
        }
        let key_after = HouseholdKeyStore::load(
            secure_store,
            vault_lease.lifecycle_lease(),
            CancellationToken::new(),
        )
        .await?;
        if key_after.is_some() {
            return Err(initialization_abort_uncertain());
        }
        self.verify_initialization_artifacts_absent(vault_lease, CancellationToken::new())
            .await?;
        if cancellation.is_cancelled() {
            return Err(initialization_abort_uncertain());
        }

        let blocked = aborting_guard.blocked_repair_after_cleanup()?;
        self.compare_exchange_guard_reconciled(
            vault_lease,
            secure_store,
            &aborting_guard,
            &blocked,
            cancellation,
        )
        .await?;
        Ok(blocked)
    }

    async fn compare_exchange_guard_reconciled(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        secure_store: &dyn HouseholdSecureStore,
        current: &HouseholdMigrationGuardDocument,
        replacement: &HouseholdMigrationGuardDocument,
        cancellation: CancellationToken,
    ) -> Result<(), PortError> {
        let result = HouseholdMigrationGuardStore::compare_exchange(
            secure_store,
            vault_lease,
            MigrationGuardExpectation::Revision(current.guard_revision()),
            Some(replacement.clone()),
            cancellation,
        )
        .await;
        if let Err(error) = result {
            let observed = HouseholdMigrationGuardStore::load(
                secure_store,
                vault_lease.lifecycle_lease(),
                CancellationToken::new(),
            )
            .await?;
            if observed.as_ref() == Some(replacement) {
                return Ok(());
            }
            if observed.as_ref() == Some(current) {
                return Err(error);
            }
            return Err(PortError::uncertain(
                "household_migration_guard_cas",
                "household migration guard transition requires reconciliation",
            ));
        }
        let observed = HouseholdMigrationGuardStore::load(
            secure_store,
            vault_lease.lifecycle_lease(),
            CancellationToken::new(),
        )
        .await?;
        if observed.as_ref() != Some(replacement) {
            return Err(PortError::uncertain(
                "household_migration_guard_cas",
                "household migration guard transition could not be verified",
            ));
        }
        Ok(())
    }

    async fn verify_initialization_artifacts_absent(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        cancellation: CancellationToken,
    ) -> Result<(), PortError> {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        let operation = vault_lease.acquire_operation(&cancellation).await?;
        vault_lease.validate_for(&self.account_slot)?;
        let vault = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _operation = operation;
            vault.verify_initialization_artifacts_absent_blocking()
        })
        .await
        .map_err(|_| initialization_abort_uncertain())?;
        vault_lease
            .validate_for(&self.account_slot)
            .map_err(|_| vault_lease_post_error())?;
        result
    }

    async fn inspect_invalid_initialization_artifacts(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        initializing_bundle: HouseholdKeyBundle,
        expected_state: HouseholdVaultWrite,
        cancellation: CancellationToken,
    ) -> Result<InitializationAbortArtifactResultV1, PortError> {
        let operation = vault_lease.acquire_operation(&cancellation).await?;
        vault_lease.validate_for(&self.account_slot)?;
        let vault = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _operation = operation;
            vault.abort_invalid_initialization_blocking(
                &initializing_bundle,
                initializing_bundle
                    .initialization_id
                    .ok_or_else(initialization_abort_ambiguous)?,
                &expected_state,
                &cancellation,
                true,
            )
        })
        .await
        .map_err(|_| initialization_abort_uncertain())?;
        vault_lease
            .validate_for(&self.account_slot)
            .map_err(|_| vault_lease_post_error())?;
        result
    }

    async fn delete_aborting_initialization_artifacts(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        initializing_bundle: HouseholdKeyBundle,
        aborting_guard: HouseholdMigrationGuardDocument,
        cancellation: CancellationToken,
    ) -> Result<HouseholdVaultInitializationAbortV1, PortError> {
        let operation = vault_lease.acquire_operation(&cancellation).await?;
        vault_lease.validate_for(&self.account_slot)?;
        let vault = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _operation = operation;
            vault.abort_aborting_initialization_blocking(
                &initializing_bundle,
                &aborting_guard,
                &cancellation,
                true,
            )
        })
        .await
        .map_err(|_| initialization_abort_uncertain())?;
        vault_lease
            .validate_for(&self.account_slot)
            .map_err(|_| vault_lease_post_error())?;
        result
    }

    fn initialize_blocking(
        &self,
        key_bundle: &HouseholdKeyBundle,
        state: &HouseholdVaultWrite,
        cancellation: &CancellationToken,
        vault_lease_held: bool,
    ) -> Result<HouseholdVaultLoad, PortError> {
        key_bundle.validate_for(&self.account_slot)?;
        if key_bundle.phase != HouseholdKeyBundlePhase::Initializing
            || key_bundle.initial_commit_id != Some(state.commit_id)
            || key_bundle.initial_state_digest != Some(state.plaintext_sha256())
        {
            return Err(PortError::new(
                "household_vault_initialization",
                "household vault initialization tuple is invalid",
            ));
        }
        let _locks = self.acquire_locks(cancellation, true, vault_lease_held)?;
        self.check_cancelled(cancellation)?;
        if !self.artifact_is_absent(&self.generation_path(2)?)? {
            return Err(PortError::new(
                "household_vault_initialization",
                "household vault initialization has an unexpected staging artifact",
            ));
        }
        if !self.artifact_is_absent(&self.journal_path())? {
            let opened = self
                .open_authoritative_locked(key_bundle, cancellation, false)
                .map_err(|error| {
                    if error.code == "household_operation_cancelled" {
                        PortError::uncertain(
                            "household_vault_initialization_verify",
                            "committed household vault initialization requires reconciliation",
                        )
                    } else {
                        error
                    }
                })?;
            if opened.journal.journal_revision != 1
                || opened.journal.current.slot != 0
                || opened.journal.previous.slot != 1
                || opened.current.header.key_id != key_bundle.active_key_id
                || opened.previous.header.key_id != key_bundle.active_key_id
                || opened.current.header.revision != state.state_revision
                || opened.previous.header.revision != state.state_revision
                || opened.current.header.commit_id != state.commit_id
                || opened.previous.header.commit_id != state.commit_id
                || opened.current.plaintext.as_slice() != state.canonical_state.as_slice()
                || opened.previous.plaintext.as_slice() != state.canonical_state.as_slice()
            {
                return Err(PortError::new(
                    "household_vault_initialization",
                    "committed household vault does not match its initialization transaction",
                ));
            }
            return Ok(opened.into_load());
        }
        let current = self.resume_or_write_initial_generation(0, key_bundle, state)?;
        self.check_cancelled(cancellation)?;
        let previous = self.resume_or_write_initial_generation(1, key_bundle, state)?;
        self.check_cancelled(cancellation)?;
        let journal = VaultJournalV1::new(1, current.reference(), previous.reference())?;
        self.write_journal(key_bundle, &journal)?;
        let opened = self
            .open_authoritative_locked(key_bundle, cancellation, false)
            .map_err(|_| {
                PortError::uncertain(
                    "household_vault_initialization_verify",
                    "household vault initialization requires reconciliation",
                )
            })?;
        if opened.current.plaintext.as_slice() != state.canonical_state.as_slice() {
            return Err(PortError::uncertain(
                "household_vault_initialization_verify",
                "household vault initialization could not be verified",
            ));
        }
        Ok(opened.into_load())
    }

    fn resume_or_write_initial_generation(
        &self,
        slot: u8,
        key_bundle: &HouseholdKeyBundle,
        state: &HouseholdVaultWrite,
    ) -> Result<DecryptedArtifact, PortError> {
        let path = self.generation_path(slot)?;
        if self.artifact_is_absent(&path)? {
            return self.write_generation(slot, key_bundle, state);
        }
        let artifact = self.read_artifact(&path, key_bundle)?;
        if artifact.header.artifact_kind != VaultArtifactKindV1::Generation
            || artifact.header.slot != slot
            || artifact.header.revision != state.state_revision
            || artifact.header.commit_id != state.commit_id
            || artifact.header.key_id != key_bundle.active_key_id
            || artifact.plaintext.as_slice() != state.canonical_state.as_slice()
        {
            return Err(PortError::new(
                "household_vault_initialization",
                "household vault initialization artifact does not match its transaction",
            ));
        }
        Ok(artifact)
    }

    fn artifact_is_absent(&self, path: &Path) -> Result<bool, PortError> {
        match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                Err(PortError::new(
                    "household_vault_path",
                    "vault artifact must be a regular physical file",
                ))
            }
            Ok(_) => Ok(false),
            Err(_) => Err(PortError::new(
                "household_vault_path",
                "vault artifact could not be inspected",
            )),
        }
    }

    fn verify_initialization_artifacts_absent_blocking(&self) -> Result<(), PortError> {
        for path in [
            self.journal_path(),
            self.generation_path(0)?,
            self.generation_path(1)?,
            self.generation_path(2)?,
        ] {
            if !self
                .artifact_is_absent(&path)
                .map_err(|_| initialization_abort_ambiguous())?
            {
                return Err(initialization_abort_ambiguous());
            }
        }
        Ok(())
    }

    fn abort_invalid_initialization_blocking(
        &self,
        key_bundle: &HouseholdKeyBundle,
        expected_initialization_id: Uuid,
        expected_state: &HouseholdVaultWrite,
        cancellation: &CancellationToken,
        vault_lease_held: bool,
    ) -> Result<InitializationAbortArtifactResultV1, PortError> {
        key_bundle.validate_for(&self.account_slot)?;
        if key_bundle.phase != HouseholdKeyBundlePhase::Initializing
            || key_bundle.revision.get() != 1
            || key_bundle.initialization_id != Some(expected_initialization_id)
            || expected_state.state_revision != 1
            || key_bundle.initial_commit_id != Some(expected_state.commit_id)
            || key_bundle.initial_state_digest != Some(expected_state.plaintext_sha256())
        {
            return Err(PortError::new(
                "household_vault_initialization_abort",
                "household vault initialization abort tuple is invalid",
            ));
        }
        if !vault_lease_held {
            return Err(PortError::new(
                "household_vault_initialization_abort",
                "household vault initialization abort requires a vault lease",
            ));
        }
        let _locks = self
            .acquire_locks(cancellation, false, true)
            .map_err(|_| initialization_abort_ambiguous())?;
        self.check_cancelled(cancellation)?;
        let artifacts = self.authenticate_initialization_artifacts_locked(
            key_bundle,
            expected_state.commit_id,
            expected_state.plaintext_sha256(),
        )?;

        let present_slots: Vec<u8> = artifacts
            .generations
            .iter()
            .enumerate()
            .filter_map(|(slot, artifact)| artifact.as_ref().map(|_| slot as u8))
            .collect();
        if present_slots.is_empty() && artifacts.journal.is_none() {
            return Ok(InitializationAbortArtifactResultV1::VerifiedAlreadyAbsent);
        }
        if artifacts.journal.is_none()
            && (present_slots.as_slice() == [0] || present_slots.as_slice() == [0, 1])
        {
            return Err(PortError::new(
                "household_vault_initialization_resumable",
                "exact household vault initialization artifacts are resumable and must not be deleted",
            ));
        }
        let valid_committed = match (
            artifacts.generations[0].as_ref(),
            artifacts.generations[1].as_ref(),
            artifacts.generations[2].as_ref(),
            artifacts.journal.as_ref(),
        ) {
            (Some(current), Some(previous), None, Some(journal)) => {
                let expected = VaultJournalV1::new(1, current.reference(), previous.reference())
                    .map_err(|_| initialization_abort_ambiguous())?;
                journal == &expected
            }
            _ => false,
        };
        if valid_committed {
            return Err(PortError::new(
                "household_vault_initialization_committed",
                "exact committed household vault initialization must not be deleted",
            ));
        }

        self.check_cancelled(cancellation)?;
        let artifact_count =
            u8::try_from(present_slots.len() + usize::from(artifacts.journal.is_some()))
                .map_err(|_| vault_format_error())?;
        Ok(InitializationAbortArtifactResultV1::CleanupRequired { artifact_count })
    }

    fn abort_aborting_initialization_blocking(
        &self,
        key_bundle: &HouseholdKeyBundle,
        aborting_guard: &HouseholdMigrationGuardDocument,
        cancellation: &CancellationToken,
        vault_lease_held: bool,
    ) -> Result<HouseholdVaultInitializationAbortV1, PortError> {
        key_bundle.validate_for(&self.account_slot)?;
        aborting_guard.validate_for(&self.account_slot)?;
        if !vault_lease_held
            || aborting_guard.state() != HouseholdMigrationGuardStateV1::Aborting
            || aborting_guard.initialization_phase()
                != Some(HouseholdMigrationInitializationPhaseV1::ReadyToInitialize)
            || key_bundle.phase != HouseholdKeyBundlePhase::Initializing
            || key_bundle.revision.get() != 1
            || key_bundle.initialization_id != Some(aborting_guard.initialization_id())
            || key_bundle.initial_commit_id != Some(aborting_guard.initial_commit_id())
            || key_bundle.initial_effect_fingerprint != aborting_guard.initial_effect_fingerprint()
            || key_bundle.initial_state_digest != aborting_guard.initial_state_digest()
        {
            return Err(initialization_abort_ambiguous());
        }
        let expected_state_digest = aborting_guard
            .initial_state_digest()
            .ok_or_else(initialization_abort_ambiguous)?;
        let _locks = self
            .acquire_locks(cancellation, false, true)
            .map_err(|_| initialization_abort_ambiguous())?;
        self.check_cancelled(cancellation)?;
        let artifacts = self.authenticate_initialization_artifacts_locked(
            key_bundle,
            aborting_guard.initial_commit_id(),
            expected_state_digest,
        )?;
        let present_slots: Vec<u8> = artifacts
            .generations
            .iter()
            .enumerate()
            .filter_map(|(slot, artifact)| artifact.as_ref().map(|_| slot as u8))
            .collect();
        if present_slots.is_empty() && artifacts.journal.is_none() {
            return Ok(HouseholdVaultInitializationAbortV1::VerifiedAlreadyAbsent);
        }
        let valid_committed = match (
            artifacts.generations[0].as_ref(),
            artifacts.generations[1].as_ref(),
            artifacts.generations[2].as_ref(),
            artifacts.journal.as_ref(),
        ) {
            (Some(current), Some(previous), None, Some(journal)) => {
                let expected = VaultJournalV1::new(1, current.reference(), previous.reference())
                    .map_err(|_| initialization_abort_ambiguous())?;
                journal == &expected
            }
            _ => false,
        };
        if valid_committed {
            return Err(PortError::new(
                "household_vault_initialization_committed",
                "exact committed household vault initialization must not be deleted",
            ));
        }

        // Remove the journal first so no partial cleanup can leave it
        // authoritatively referencing generations that are being deleted.
        let mut artifact_paths = Vec::with_capacity(4);
        if artifacts.journal.is_some() {
            artifact_paths.push(self.journal_path());
        }
        for (slot, artifact) in artifacts.generations.iter().enumerate() {
            if artifact.is_some() {
                artifact_paths.push(self.generation_path(slot as u8)?);
            }
        }
        self.check_cancelled(cancellation)?;
        let artifact_count =
            u8::try_from(artifact_paths.len()).map_err(|_| vault_format_error())?;
        let mut removed_any = false;
        for path in &artifact_paths {
            if std::fs::remove_file(path).is_err() {
                return Err(if removed_any {
                    initialization_abort_uncertain()
                } else {
                    PortError::new(
                        "household_vault_initialization_abort",
                        "household vault initialization artifact could not be removed",
                    )
                });
            }
            removed_any = true;
            if cancellation.is_cancelled() {
                return Err(initialization_abort_uncertain());
            }
        }
        File::open(self.household_directory())
            .and_then(|directory| directory.sync_all())
            .map_err(|_| initialization_abort_uncertain())?;
        self.verify_initialization_artifacts_absent_blocking()
            .map_err(|_| initialization_abort_uncertain())?;
        Ok(HouseholdVaultInitializationAbortV1::DeletedAndVerified { artifact_count })
    }

    fn authenticate_initialization_artifacts_locked(
        &self,
        key_bundle: &HouseholdKeyBundle,
        expected_commit_id: Uuid,
        expected_state_digest: [u8; 32],
    ) -> Result<AuthenticatedInitializationArtifactsV1, PortError> {
        let mut generations: [Option<DecryptedArtifact>; 3] = [None, None, None];
        for slot in 0..=2 {
            let path = self.generation_path(slot)?;
            if self
                .artifact_is_absent(&path)
                .map_err(|_| initialization_abort_ambiguous())?
            {
                continue;
            }
            let artifact = self
                .read_artifact(&path, key_bundle)
                .map_err(|_| initialization_abort_ambiguous())?;
            if artifact.header.artifact_kind != VaultArtifactKindV1::Generation
                || artifact.header.slot != slot
                || artifact.header.revision != 1
                || artifact.header.commit_id != expected_commit_id
                || artifact.header.key_id != key_bundle.active_key_id
                || sha256(&artifact.plaintext) != expected_state_digest
            {
                return Err(initialization_abort_ambiguous());
            }
            generations[usize::from(slot)] = Some(artifact);
        }

        let journal = if self
            .artifact_is_absent(&self.journal_path())
            .map_err(|_| initialization_abort_ambiguous())?
        {
            None
        } else {
            let artifact = self
                .read_artifact(&self.journal_path(), key_bundle)
                .map_err(|_| initialization_abort_ambiguous())?;
            let journal = VaultJournalV1::decode(&artifact.plaintext)
                .and_then(|journal| {
                    journal.validate_against_header(artifact.header)?;
                    Ok(journal)
                })
                .map_err(|_| initialization_abort_ambiguous())?;
            let expected_digest = lower_hex(&expected_state_digest);
            if artifact.header.key_id != key_bundle.active_key_id
                || artifact.header.revision != 1
                || artifact.header.commit_id != expected_commit_id
                || journal.current.slot != 0
                || journal.previous.slot != 1
                || journal.current.state_revision != 1
                || journal.previous.state_revision != 1
                || journal.current.commit_id != expected_commit_id
                || journal.previous.commit_id != expected_commit_id
                || journal.current.plaintext_sha256 != expected_digest
                || journal.previous.plaintext_sha256 != expected_digest
            {
                return Err(initialization_abort_ambiguous());
            }
            Some(journal)
        };
        Ok(AuthenticatedInitializationArtifactsV1 {
            generations,
            journal,
        })
    }

    fn recover_uncommitted_initialization_write_blocking(
        &self,
        key_bundle: &HouseholdKeyBundle,
        guard: &HouseholdMigrationGuardDocument,
        cancellation: &CancellationToken,
    ) -> Result<HouseholdVaultWrite, PortError> {
        self.check_cancelled(cancellation)?;
        guard.validate_for(&self.account_slot)?;
        key_bundle.validate_initial_for(&self.account_slot, guard)?;
        let expected_state_digest = guard
            .initial_state_digest()
            .ok_or_else(initialization_resume_error)?;
        self.validate_startup_directory_entries()?;
        let mut artifacts = self
            .authenticate_initialization_artifacts_locked(
                key_bundle,
                guard.initial_commit_id(),
                expected_state_digest,
            )
            .map_err(|_| initialization_resume_error())?;
        if artifacts.journal.is_some()
            || artifacts.generations[2].is_some()
            || artifacts.generations[0].is_none()
        {
            return Err(initialization_resume_error());
        }
        let current = artifacts.generations[0]
            .take()
            .ok_or_else(initialization_resume_error)?;
        if artifacts.generations[1]
            .as_ref()
            .is_some_and(|previous| previous.plaintext != current.plaintext)
        {
            return Err(initialization_resume_error());
        }
        let write = HouseholdVaultWrite::new(
            current.header.revision,
            current.header.commit_id,
            current.plaintext.to_vec(),
        )?;
        if write.state_revision != 1
            || write.commit_id != guard.initial_commit_id()
            || write.plaintext_sha256() != expected_state_digest
        {
            return Err(initialization_resume_error());
        }
        let state = decode_canonical_household_state_v1(&write.canonical_state)
            .map_err(|_| initialization_resume_error())?;
        if state.account_binding != self.account_id
            || state.revision.get() != 1
            || state.migration_provenance.migration_id != guard.migration_id()
            || state.migration_provenance.initialization_id != guard.initialization_id()
            || state.migration_provenance.initial_commit_id.as_uuid() != guard.initial_commit_id()
            || state.migration_provenance.migration_frozen_at != *guard.migration_frozen_at()
            || !migration_source_matches_guard(guard, &state)?
            || state.bounded_applied_commits.len() != 1
        {
            return Err(initialization_resume_error());
        }
        let initial = &state.bounded_applied_commits[0];
        if initial.commit_id.as_uuid() != guard.initial_commit_id()
            || initial.resulting_revision.get() != 1
            || initial.outcome != heyfood_core::AppliedCommitOutcomeV1::Initialized
            || guard.initial_effect_fingerprint() != Some(*initial.fingerprint.as_bytes())
        {
            return Err(initialization_resume_error());
        }
        self.check_cancelled(cancellation)?;
        Ok(write)
    }

    fn load_blocking(
        &self,
        key_bundle: &HouseholdKeyBundle,
        cancellation: &CancellationToken,
        vault_lease_held: bool,
    ) -> Result<HouseholdVaultLoad, PortError> {
        key_bundle.validate_for(&self.account_slot)?;
        let _locks = self.acquire_locks(cancellation, false, vault_lease_held)?;
        let opened = self.open_authoritative_locked(key_bundle, cancellation, true)?;
        Ok(opened.into_load())
    }

    fn commit_blocking(
        &self,
        key_bundle: &HouseholdKeyBundle,
        expected_revision: u64,
        state: &HouseholdVaultWrite,
        cancellation: &CancellationToken,
        vault_lease_held: bool,
    ) -> Result<HouseholdVaultLoad, PortError> {
        key_bundle.validate_for(&self.account_slot)?;
        if key_bundle.phase != HouseholdKeyBundlePhase::Stable {
            return Err(PortError::new(
                "household_vault_key_phase",
                "household vault commit requires a stable key bundle",
            ));
        }
        let _locks = self.acquire_locks(cancellation, false, vault_lease_held)?;
        let opened = self.open_authoritative_locked(key_bundle, cancellation, true)?;
        if opened.current.header.revision != expected_revision
            || state.state_revision
                != expected_revision.checked_add(1).ok_or_else(|| {
                    PortError::new(
                        "household_vault_revision",
                        "household vault revision is exhausted",
                    )
                })?
        {
            return Err(PortError::new(
                "household_vault_revision_conflict",
                "household vault revision changed concurrently",
            ));
        }
        self.check_cancelled(cancellation)?;
        let staging_slot =
            unreferenced_slot(opened.journal.current.slot, opened.journal.previous.slot)?;
        let staging = self.write_generation(staging_slot, key_bundle, state)?;
        self.verify_generation_file(staging_slot, key_bundle, &staging.reference())?;
        self.check_cancelled(cancellation)?;
        let journal_revision = opened
            .journal
            .journal_revision
            .checked_add(1)
            .ok_or_else(|| {
                PortError::new(
                    "household_vault_journal_revision",
                    "household vault journal revision is exhausted",
                )
            })?;
        let replacement = VaultJournalV1::new(
            journal_revision,
            staging.reference(),
            opened.journal.current.clone(),
        )?;
        self.write_journal(key_bundle, &replacement)?;
        let verified = self
            .open_authoritative_locked(key_bundle, cancellation, false)
            .map_err(|_| {
                PortError::uncertain(
                    "household_vault_commit_verify",
                    "household vault commit could not be verified",
                )
            })?;
        Ok(verified.into_load())
    }

    fn rotate_blocking(
        &self,
        key_bundle: &HouseholdKeyBundle,
        cancellation: &CancellationToken,
        vault_lease_held: bool,
    ) -> Result<HouseholdVaultLoad, PortError> {
        key_bundle.validate_for(&self.account_slot)?;
        if key_bundle.phase != HouseholdKeyBundlePhase::Rewriting {
            return Err(PortError::new(
                "household_vault_key_phase",
                "household vault rotation requires a rewriting key bundle",
            ));
        }
        let _locks = self.acquire_locks(cancellation, false, vault_lease_held)?;
        let opened = self.open_authoritative_locked(key_bundle, cancellation, false)?;
        self.check_cancelled(cancellation)?;
        let current_input = HouseholdVaultWrite::new(
            opened.current.header.revision,
            opened.current.header.commit_id,
            opened.current.plaintext.to_vec(),
        )?;
        let previous_input = HouseholdVaultWrite::new(
            opened.previous.header.revision,
            opened.previous.header.commit_id,
            opened.previous.plaintext.to_vec(),
        )?;
        let current =
            self.write_generation(opened.journal.current.slot, key_bundle, &current_input)?;
        self.verify_generation_file(
            opened.journal.current.slot,
            key_bundle,
            &current.reference(),
        )?;
        self.check_cancelled(cancellation)?;
        let previous =
            self.write_generation(opened.journal.previous.slot, key_bundle, &previous_input)?;
        self.verify_generation_file(
            opened.journal.previous.slot,
            key_bundle,
            &previous.reference(),
        )?;
        self.check_cancelled(cancellation)?;
        let journal_revision = opened
            .journal
            .journal_revision
            .checked_add(1)
            .ok_or_else(|| {
                PortError::new(
                    "household_vault_journal_revision",
                    "household vault journal revision is exhausted",
                )
            })?;
        let replacement =
            VaultJournalV1::new(journal_revision, current.reference(), previous.reference())?;
        self.write_journal(key_bundle, &replacement)?;
        let verified = self
            .open_authoritative_locked(key_bundle, cancellation, false)
            .map_err(|_| {
                PortError::uncertain(
                    "household_vault_rotation_verify",
                    "household vault rotation could not be verified",
                )
            })?;
        Ok(verified.into_load())
    }

    fn open_authoritative_locked(
        &self,
        key_bundle: &HouseholdKeyBundle,
        cancellation: &CancellationToken,
        repair_previous: bool,
    ) -> Result<OpenedVault, PortError> {
        self.check_cancelled(cancellation)?;
        let journal_artifact = self.read_artifact(&self.journal_path(), key_bundle)?;
        if journal_artifact.header.artifact_kind != VaultArtifactKindV1::Journal {
            return Err(vault_format_error());
        }
        let journal = VaultJournalV1::decode(&journal_artifact.plaintext)?;
        journal.validate_against_header(journal_artifact.header)?;
        self.check_cancelled(cancellation)?;

        let current_result = self.read_referenced_generation(&journal.current, key_bundle);
        let current = match current_result {
            Ok(current) => current,
            Err(_) => {
                let previous_is_authenticated = self
                    .read_referenced_generation(&journal.previous, key_bundle)
                    .is_ok();
                return Err(PortError::new(
                    "vault_current_corrupt",
                    if previous_is_authenticated {
                        "current household state is unavailable; authenticated manual recovery material exists"
                    } else {
                        "current household state is unavailable"
                    },
                ));
            }
        };
        self.check_cancelled(cancellation)?;
        match self.read_referenced_generation(&journal.previous, key_bundle) {
            Ok(previous) => Ok(OpenedVault {
                journal,
                current,
                previous,
                health: HouseholdVaultHealthV1::Healthy,
            }),
            Err(_) if repair_previous => {
                self.repair_previous_locked(key_bundle, cancellation, journal, current)
            }
            Err(_) => Err(PortError::new(
                "vault_previous_corrupt",
                "previous household generation is unavailable",
            )),
        }
    }

    fn repair_previous_locked(
        &self,
        key_bundle: &HouseholdKeyBundle,
        cancellation: &CancellationToken,
        journal: VaultJournalV1,
        current: DecryptedArtifact,
    ) -> Result<OpenedVault, PortError> {
        self.check_cancelled(cancellation)?;
        let repair_slot = unreferenced_slot(journal.current.slot, journal.previous.slot)?;
        let state = HouseholdVaultWrite::new(
            current.header.revision,
            current.header.commit_id,
            current.plaintext.to_vec(),
        )?;
        let repair = self.write_generation(repair_slot, key_bundle, &state)?;
        self.verify_generation_file(repair_slot, key_bundle, &repair.reference())?;
        self.check_cancelled(cancellation)?;
        let journal_revision = journal.journal_revision.checked_add(1).ok_or_else(|| {
            PortError::new(
                "household_vault_journal_revision",
                "household vault journal revision is exhausted",
            )
        })?;
        let replacement = VaultJournalV1::new(
            journal_revision,
            journal.current.clone(),
            repair.reference(),
        )?;
        self.write_journal(key_bundle, &replacement)?;
        let opened = self
            .open_authoritative_locked(key_bundle, cancellation, false)
            .map_err(|_| {
                PortError::uncertain(
                    "household_vault_repair_verify",
                    "household vault repair could not be verified",
                )
            })?;
        Ok(OpenedVault {
            journal: opened.journal,
            current: opened.current,
            previous: opened.previous,
            health: HouseholdVaultHealthV1::PreviousRepairedFromAuthoritativeCurrent,
        })
    }

    fn write_generation(
        &self,
        slot: u8,
        key_bundle: &HouseholdKeyBundle,
        state: &HouseholdVaultWrite,
    ) -> Result<DecryptedArtifact, PortError> {
        self.validate_generation_plaintext(&state.canonical_state, state.state_revision)?;
        let envelope = encrypt_artifact(
            &self.account_id,
            &self.account_slot,
            VaultArtifactKindV1::Generation,
            slot,
            state.state_revision,
            state.commit_id,
            key_bundle.active_key_id,
            &key_bundle.active_key,
            &state.canonical_state,
        )?;
        let path = self.generation_path(slot)?;
        AtomicFile::replace(&path, &envelope)?;
        self.read_artifact(&path, key_bundle)
    }

    fn validate_generation_plaintext(
        &self,
        plaintext: &[u8],
        expected_revision: u64,
    ) -> Result<(), PortError> {
        let state = decode_canonical_household_state_v1(plaintext).map_err(|_| {
            PortError::new(
                "household_vault_state",
                "canonical household state is invalid",
            )
        })?;
        if state.account_binding != self.account_id || state.revision.get() != expected_revision {
            return Err(PortError::new(
                "household_vault_state",
                "canonical household state does not match its vault envelope",
            ));
        }
        Ok(())
    }

    fn write_journal(
        &self,
        key_bundle: &HouseholdKeyBundle,
        journal: &VaultJournalV1,
    ) -> Result<(), PortError> {
        let plaintext = journal.encode()?;
        let envelope = encrypt_artifact(
            &self.account_id,
            &self.account_slot,
            VaultArtifactKindV1::Journal,
            JOURNAL_SLOT,
            journal.journal_revision,
            journal.current.commit_id,
            key_bundle.active_key_id,
            &key_bundle.active_key,
            &plaintext,
        )?;
        AtomicFile::replace(&self.journal_path(), &envelope)?;
        (|| {
            let verified = self.read_artifact(&self.journal_path(), key_bundle)?;
            let decoded = VaultJournalV1::decode(&verified.plaintext)?;
            decoded.validate_against_header(verified.header)?;
            if &decoded != journal {
                return Err(vault_format_error());
            }
            Ok(())
        })()
        .map_err(|_| {
            PortError::uncertain(
                "household_vault_journal_verify",
                "household vault journal could not be verified",
            )
        })
    }

    fn verify_generation_file(
        &self,
        slot: u8,
        key_bundle: &HouseholdKeyBundle,
        reference: &GenerationReferenceV1,
    ) -> Result<(), PortError> {
        self.read_referenced_generation(reference, key_bundle)
            .and_then(|artifact| {
                if artifact.header.slot == slot {
                    Ok(())
                } else {
                    Err(vault_format_error())
                }
            })
    }

    fn read_referenced_generation(
        &self,
        reference: &GenerationReferenceV1,
        key_bundle: &HouseholdKeyBundle,
    ) -> Result<DecryptedArtifact, PortError> {
        let artifact = self.read_artifact(&self.generation_path(reference.slot)?, key_bundle)?;
        if artifact.header.artifact_kind != VaultArtifactKindV1::Generation
            || artifact.header.slot != reference.slot
            || artifact.header.revision != reference.state_revision
            || artifact.header.commit_id != reference.commit_id
            || lower_hex(&sha256(&artifact.plaintext)) != reference.plaintext_sha256
        {
            return Err(vault_format_error());
        }
        Ok(artifact)
    }

    fn read_artifact(
        &self,
        path: &Path,
        key_bundle: &HouseholdKeyBundle,
    ) -> Result<DecryptedArtifact, PortError> {
        let bytes = self.read_bounded_regular_file(
            path,
            VAULT_ENVELOPE_HEADER_BYTES + MAX_HOUSEHOLD_VAULT_CIPHERTEXT_BYTES,
        )?;
        if bytes.len() < VAULT_ENVELOPE_HEADER_BYTES {
            return Err(vault_format_error());
        }
        let header = VaultEnvelopeHeaderV1::decode(&bytes[..VAULT_ENVELOPE_HEADER_BYTES])?;
        let ciphertext_length =
            usize::try_from(header.ciphertext_length).map_err(|_| vault_format_error())?;
        if bytes.len() != VAULT_ENVELOPE_HEADER_BYTES + ciphertext_length {
            return Err(vault_format_error());
        }
        let key = key_bundle.key_for(header.key_id).ok_or_else(|| {
            PortError::new(
                "household_vault_key_mismatch",
                "household vault key is unavailable",
            )
        })?;
        let plaintext = decrypt_artifact(
            &self.account_id,
            &self.account_slot,
            header,
            key,
            &bytes[VAULT_ENVELOPE_HEADER_BYTES..],
        )?;
        if plaintext.is_empty() || plaintext.len() > MAX_HOUSEHOLD_VAULT_PLAINTEXT_BYTES {
            return Err(vault_format_error());
        }
        if header.artifact_kind == VaultArtifactKindV1::Generation {
            self.validate_generation_plaintext(&plaintext, header.revision)?;
        }
        Ok(DecryptedArtifact { header, plaintext })
    }

    fn read_bounded_regular_file(&self, path: &Path, maximum: usize) -> Result<Vec<u8>, PortError> {
        let link_metadata = std::fs::symlink_metadata(path)
            .map_err(|_| PortError::new("household_vault_read", "vault artifact is unavailable"))?;
        if link_metadata.file_type().is_symlink() || !link_metadata.is_file() {
            return Err(PortError::new(
                "household_vault_path",
                "vault artifact is not a regular file",
            ));
        }
        self.validate_owner_only_metadata(&link_metadata, false)?;
        let file = File::open(path)
            .map_err(|_| PortError::new("household_vault_read", "vault artifact is unavailable"))?;
        let opened_metadata = file
            .metadata()
            .map_err(|_| PortError::new("household_vault_read", "vault artifact is unavailable"))?;
        if !opened_metadata.is_file() {
            return Err(PortError::new(
                "household_vault_path",
                "vault artifact is not a regular file",
            ));
        }
        self.validate_same_file(&link_metadata, &opened_metadata)?;
        self.validate_owner_only_metadata(&opened_metadata, false)?;
        let file_length = usize::try_from(opened_metadata.len()).map_err(|_| {
            PortError::new("household_vault_size", "vault artifact exceeds its limit")
        })?;
        if file_length > maximum {
            return Err(PortError::new(
                "household_vault_size",
                "vault artifact exceeds its limit",
            ));
        }
        let mut bytes = Vec::with_capacity(file_length);
        file.take((maximum + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| PortError::new("household_vault_read", "vault artifact is unavailable"))?;
        if bytes.len() != file_length || bytes.len() > maximum {
            return Err(PortError::new(
                "household_vault_size",
                "vault artifact changed or exceeds its limit",
            ));
        }
        Ok(bytes)
    }

    fn acquire_locks(
        &self,
        cancellation: &CancellationToken,
        create_household: bool,
        vault_lease_held: bool,
    ) -> Result<VaultLocks, PortError> {
        let accounts_directory = self.native_root.join("accounts");
        let account_directory = self.account_directory();
        let household_directory = self.household_directory();
        if vault_lease_held {
            validate_private_directory(&accounts_directory)?;
            validate_private_directory(&account_directory)?;
            validate_private_directory(&household_directory)?;
            self.check_cancelled(cancellation)?;
            return Ok(VaultLocks {
                _lifecycle: None,
                _vault: None,
            });
        }
        ensure_private_directory(&accounts_directory)?;
        ensure_private_directory(&account_directory)?;
        let lifecycle = self.acquire_lock(
            &account_directory.join("account-lifecycle.lock"),
            cancellation,
        )?;
        self.check_cancelled(cancellation)?;
        if create_household {
            ensure_private_directory(&household_directory)?;
        } else {
            validate_private_directory(&household_directory)?;
        }
        let vault = self.acquire_lock(&household_directory.join("vault.lock"), cancellation)?;
        Ok(VaultLocks {
            _lifecycle: Some(lifecycle),
            _vault: Some(vault),
        })
    }

    fn acquire_lifecycle_lease_blocking(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<HouseholdLifecycleLease, PortError> {
        let accounts_directory = self.native_root.join("accounts");
        let account_directory = self.account_directory();
        let lock_path = account_directory.join("account-lifecycle.lock");
        ensure_private_directory(&accounts_directory)?;
        ensure_private_directory(&account_directory)?;
        let lock = self.acquire_lock(&lock_path, cancellation)?;
        validate_private_directory(&accounts_directory)?;
        validate_private_directory(&account_directory)?;
        self.check_cancelled(cancellation)?;
        Ok(HouseholdLifecycleLease {
            account_slot: self.account_slot.clone(),
            lock: Arc::new(lock),
            lock_path,
            #[cfg(unix)]
            owner_uid: self.owner_uid,
        })
    }

    fn acquire_vault_lease_blocking(
        &self,
        lifecycle_lease: HouseholdLifecycleLease,
        mode: HouseholdVaultLeaseModeV1,
        cancellation: &CancellationToken,
    ) -> Result<HouseholdVaultLease, PortError> {
        lifecycle_lease.validate_for(&self.account_slot)?;
        let accounts_directory = self.native_root.join("accounts");
        let account_directory = self.account_directory();
        let household_directory = self.household_directory();
        validate_private_directory(&accounts_directory)?;
        validate_private_directory(&account_directory)?;
        match mode {
            HouseholdVaultLeaseModeV1::CreateIfMissing => {
                ensure_private_directory(&household_directory)?;
            }
            HouseholdVaultLeaseModeV1::RequireExisting => {
                validate_private_directory(&household_directory)?;
            }
        }
        let vault_lock_path = household_directory.join("vault.lock");
        let vault_lock = self.acquire_lock(&vault_lock_path, cancellation)?;
        validate_private_directory(&accounts_directory)?;
        validate_private_directory(&account_directory)?;
        validate_private_directory(&household_directory)?;
        self.check_cancelled(cancellation)?;
        Ok(HouseholdVaultLease {
            lifecycle_lease,
            vault_lock: Arc::new(vault_lock),
            vault_lock_path,
            #[cfg(unix)]
            owner_uid: self.owner_uid,
            operation_gate: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    fn acquire_existing_vault_lease_if_present_blocking(
        &self,
        lifecycle_lease: HouseholdLifecycleLease,
        cancellation: &CancellationToken,
    ) -> Result<Option<HouseholdVaultLease>, PortError> {
        lifecycle_lease.validate_for(&self.account_slot)?;
        match std::fs::symlink_metadata(self.household_directory()) {
            Ok(_) => self
                .acquire_vault_lease_blocking(
                    lifecycle_lease,
                    HouseholdVaultLeaseModeV1::RequireExisting,
                    cancellation,
                )
                .map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                lifecycle_lease.validate_for(&self.account_slot)?;
                Ok(None)
            }
            Err(_) => Err(PortError::new(
                "household_vault_evidence",
                "household vault evidence is unavailable",
            )),
        }
    }

    fn classify_startup_artifacts_blocking(
        &self,
        key_bundle: Option<&HouseholdKeyBundle>,
        expected_commit_id: Option<Uuid>,
        expected_state_digest: Option<[u8; 32]>,
    ) -> Result<HouseholdVaultStartupArtifactsV1, PortError> {
        self.validate_startup_directory_entries()?;
        let present_count = self.startup_artifact_presence_count()?;
        if present_count == 0 {
            return Ok(HouseholdVaultStartupArtifactsV1::Absent);
        }
        let (Some(key_bundle), Some(expected_commit_id), Some(expected_state_digest)) =
            (key_bundle, expected_commit_id, expected_state_digest)
        else {
            return Err(PortError::new(
                "household_native_evidence_contradiction",
                "native household artifacts do not have an exact initialization authority",
            ));
        };
        key_bundle.validate_for(&self.account_slot)?;
        let artifacts = self
            .authenticate_initialization_artifacts_locked(
                key_bundle,
                expected_commit_id,
                expected_state_digest,
            )
            .map_err(|_| {
                PortError::new(
                    "household_native_evidence_contradiction",
                    "native household initialization artifacts are contradictory",
                )
            })?;
        match (
            artifacts.generations[0].as_ref(),
            artifacts.generations[1].as_ref(),
            artifacts.generations[2].as_ref(),
            artifacts.journal.as_ref(),
        ) {
            (Some(_), None, None, None) | (Some(_), Some(_), None, None) => {
                Ok(HouseholdVaultStartupArtifactsV1::MatchingUncommitted)
            }
            (Some(current), Some(previous), None, Some(journal))
                if current.reference() == journal.current
                    && previous.reference() == journal.previous =>
            {
                Ok(HouseholdVaultStartupArtifactsV1::MatchingCommitted)
            }
            _ => Err(PortError::new(
                "household_native_evidence_contradiction",
                "native household initialization artifacts have a contradictory topology",
            )),
        }
    }

    fn validate_startup_directory_entries(&self) -> Result<(), PortError> {
        const EXPECTED_NAMES: [&str; 5] = [
            "commit.hfj",
            "generation-0.hfv",
            "generation-1.hfv",
            "generation-2.hfv",
            "vault.lock",
        ];
        let directory = self.household_directory();
        let entries = std::fs::read_dir(&directory).map_err(|_| {
            PortError::new(
                "household_vault_evidence",
                "household vault directory is unavailable",
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|_| {
                PortError::new(
                    "household_vault_evidence",
                    "household vault directory is unavailable",
                )
            })?;
            let name = entry.file_name();
            if !EXPECTED_NAMES
                .iter()
                .any(|expected| name == std::ffi::OsStr::new(expected))
            {
                return Err(PortError::new(
                    "household_native_evidence_contradiction",
                    "household vault directory contains an unknown artifact",
                ));
            }
        }
        Ok(())
    }

    fn startup_artifact_presence_count(&self) -> Result<u8, PortError> {
        let mut count = 0_u8;
        for path in [
            self.generation_path(0)?,
            self.generation_path(1)?,
            self.generation_path(2)?,
            self.journal_path(),
        ] {
            match std::fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink() || !metadata.is_file() {
                        return Err(PortError::new(
                            "household_native_evidence_contradiction",
                            "household vault artifact is not a regular physical file",
                        ));
                    }
                    self.validate_owner_only_metadata(&metadata, false)?;
                    count = count.checked_add(1).ok_or_else(vault_format_error)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {
                    return Err(PortError::new(
                        "household_vault_evidence",
                        "household vault artifact could not be inspected",
                    ));
                }
            }
        }
        Ok(count)
    }

    fn acquire_lock(
        &self,
        path: &Path,
        cancellation: &CancellationToken,
    ) -> Result<LockedFile, PortError> {
        let file = open_private_lock(path)?;
        let link_metadata = std::fs::symlink_metadata(path)
            .map_err(|_| PortError::new("household_vault_lock", "vault lock is unavailable"))?;
        if link_metadata.file_type().is_symlink() || !link_metadata.is_file() {
            return Err(PortError::new(
                "household_vault_lock",
                "vault lock is not a regular physical file",
            ));
        }
        let opened_metadata = file
            .metadata()
            .map_err(|_| PortError::new("household_vault_lock", "vault lock is unavailable"))?;
        self.validate_same_file(&link_metadata, &opened_metadata)?;
        self.validate_owner_only_metadata(&opened_metadata, false)?;
        let started = Instant::now();
        loop {
            self.check_cancelled(cancellation)?;
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(LockedFile(file)),
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && started.elapsed() < LOCK_TIMEOUT =>
                {
                    thread::sleep(LOCK_RETRY_INTERVAL);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    return Err(PortError::new(
                        "household_vault_lock_timeout",
                        "household vault lock acquisition timed out",
                    ));
                }
                Err(_) => {
                    return Err(PortError::new(
                        "household_vault_lock",
                        "household vault lock is unavailable",
                    ));
                }
            }
        }
    }

    fn check_cancelled(&self, cancellation: &CancellationToken) -> Result<(), PortError> {
        if cancellation.is_cancelled() {
            Err(cancelled_error())
        } else {
            Ok(())
        }
    }

    fn generation_path(&self, slot: u8) -> Result<PathBuf, PortError> {
        if slot > 2 {
            return Err(vault_format_error());
        }
        Ok(self
            .household_directory()
            .join(format!("generation-{slot}.hfv")))
    }

    fn journal_path(&self) -> PathBuf {
        self.household_directory().join("commit.hfj")
    }

    #[cfg(unix)]
    fn validate_owner_only_metadata(
        &self,
        metadata: &std::fs::Metadata,
        directory: bool,
    ) -> Result<(), PortError> {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

        let expected_type = if directory {
            metadata.is_dir()
        } else {
            metadata.is_file()
        };
        if !expected_type
            || metadata.uid() != self.owner_uid
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(PortError::new(
                "household_vault_permissions",
                "vault path is not owner-only",
            ));
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn validate_owner_only_metadata(
        &self,
        metadata: &std::fs::Metadata,
        directory: bool,
    ) -> Result<(), PortError> {
        if (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
            return Err(PortError::new(
                "household_vault_permissions",
                "vault path has an invalid type",
            ));
        }
        Ok(())
    }

    #[cfg(unix)]
    fn validate_same_file(
        &self,
        before: &std::fs::Metadata,
        opened: &std::fs::Metadata,
    ) -> Result<(), PortError> {
        use std::os::unix::fs::MetadataExt as _;

        if before.dev() != opened.dev() || before.ino() != opened.ino() {
            return Err(PortError::new(
                "household_vault_path",
                "vault artifact identity changed during open",
            ));
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn validate_same_file(
        &self,
        _before: &std::fs::Metadata,
        _opened: &std::fs::Metadata,
    ) -> Result<(), PortError> {
        Ok(())
    }
}

impl fmt::Debug for HouseholdVault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdVault")
            .field("account_slot", &self.account_slot)
            .finish_non_exhaustive()
    }
}

struct LockedFile(File);

impl Drop for LockedFile {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

struct VaultLocks {
    _lifecycle: Option<LockedFile>,
    _vault: Option<LockedFile>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct GenerationReferenceV1 {
    commit_id: Uuid,
    plaintext_sha256: String,
    slot: u8,
    state_revision: u64,
}

impl GenerationReferenceV1 {
    fn validate(&self) -> Result<(), PortError> {
        if self.slot > 2
            || self.state_revision == 0
            || self.commit_id.is_nil()
            || decode_lower_hex_32(&self.plaintext_sha256).is_err()
        {
            return Err(vault_format_error());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct VaultJournalV1 {
    current: GenerationReferenceV1,
    journal_revision: u64,
    previous: GenerationReferenceV1,
    schema_version: u16,
}

impl VaultJournalV1 {
    fn new(
        journal_revision: u64,
        current: GenerationReferenceV1,
        previous: GenerationReferenceV1,
    ) -> Result<Self, PortError> {
        let value = Self {
            current,
            journal_revision,
            previous,
            schema_version: JOURNAL_SCHEMA_VERSION,
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), PortError> {
        self.current.validate()?;
        self.previous.validate()?;
        if self.schema_version != JOURNAL_SCHEMA_VERSION
            || self.journal_revision == 0
            || self.current.slot == self.previous.slot
        {
            return Err(vault_format_error());
        }
        Ok(())
    }

    fn encode(&self) -> Result<Vec<u8>, PortError> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|_| vault_format_error())
    }

    fn decode(bytes: &[u8]) -> Result<Self, PortError> {
        let decoded: Self = serde_json::from_slice(bytes).map_err(|_| vault_format_error())?;
        decoded.validate()?;
        if decoded.encode()?.as_slice() != bytes {
            return Err(vault_format_error());
        }
        Ok(decoded)
    }

    fn validate_against_header(&self, header: VaultEnvelopeHeaderV1) -> Result<(), PortError> {
        self.validate()?;
        if header.artifact_kind != VaultArtifactKindV1::Journal
            || header.slot != JOURNAL_SLOT
            || header.revision != self.journal_revision
            || header.commit_id != self.current.commit_id
        {
            return Err(vault_format_error());
        }
        Ok(())
    }
}

struct DecryptedArtifact {
    header: VaultEnvelopeHeaderV1,
    plaintext: Zeroizing<Vec<u8>>,
}

impl DecryptedArtifact {
    fn reference(&self) -> GenerationReferenceV1 {
        GenerationReferenceV1 {
            commit_id: self.header.commit_id,
            plaintext_sha256: lower_hex(&sha256(&self.plaintext)),
            slot: self.header.slot,
            state_revision: self.header.revision,
        }
    }
}

struct OpenedVault {
    journal: VaultJournalV1,
    current: DecryptedArtifact,
    previous: DecryptedArtifact,
    health: HouseholdVaultHealthV1,
}

impl OpenedVault {
    fn into_load(self) -> HouseholdVaultLoad {
        HouseholdVaultLoad {
            state_revision: self.current.header.revision,
            commit_id: self.current.header.commit_id,
            journal_revision: self.journal.journal_revision,
            canonical_state: self.current.plaintext,
            health: self.health,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn encrypt_artifact(
    account: &AccountId,
    account_slot: &HouseholdAccountSlotV1,
    artifact_kind: VaultArtifactKindV1,
    slot: u8,
    revision: u64,
    commit_id: Uuid,
    key_id: KeyId,
    root_key: &HouseholdKeyMaterial,
    plaintext: &[u8],
) -> Result<Vec<u8>, PortError> {
    if plaintext.is_empty() || plaintext.len() > MAX_HOUSEHOLD_VAULT_PLAINTEXT_BYTES {
        return Err(PortError::new(
            "household_vault_size",
            "household vault plaintext exceeds its limit",
        ));
    }
    let nonce_value = XNonce::try_generate().map_err(|_| vault_crypto_error())?;
    let mut nonce = [0_u8; 24];
    nonce.copy_from_slice(nonce_value.as_slice());
    let ciphertext_length = u32::try_from(plaintext.len() + 16).map_err(|_| {
        PortError::new(
            "household_vault_size",
            "household vault ciphertext exceeds its limit",
        )
    })?;
    let header = match artifact_kind {
        VaultArtifactKindV1::Generation => VaultEnvelopeHeaderV1::generation(
            slot,
            key_id,
            revision,
            commit_id,
            nonce,
            ciphertext_length,
        )?,
        VaultArtifactKindV1::Journal => {
            VaultEnvelopeHeaderV1::journal(key_id, revision, commit_id, nonce, ciphertext_length)?
        }
    };
    let subkey = derive_subkey(root_key, artifact_kind)?;
    let cipher =
        XChaCha20Poly1305::new_from_slice(subkey.as_slice()).map_err(|_| vault_crypto_error())?;
    let aad = household_vault_aad_v1(account, account_slot, header)?;
    let ciphertext = cipher
        .encrypt(
            &nonce_value,
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| vault_crypto_error())?;
    if ciphertext.len() != usize::try_from(ciphertext_length).map_err(|_| vault_crypto_error())? {
        return Err(vault_crypto_error());
    }
    let mut envelope = Vec::with_capacity(VAULT_ENVELOPE_HEADER_BYTES + ciphertext.len());
    envelope.extend_from_slice(&header.encode());
    envelope.extend_from_slice(&ciphertext);
    Ok(envelope)
}

fn decrypt_artifact(
    account: &AccountId,
    account_slot: &HouseholdAccountSlotV1,
    header: VaultEnvelopeHeaderV1,
    root_key: &HouseholdKeyMaterial,
    ciphertext: &[u8],
) -> Result<Zeroizing<Vec<u8>>, PortError> {
    if ciphertext.len()
        != usize::try_from(header.ciphertext_length).map_err(|_| vault_format_error())?
    {
        return Err(vault_format_error());
    }
    let subkey = derive_subkey(root_key, header.artifact_kind)?;
    let cipher =
        XChaCha20Poly1305::new_from_slice(subkey.as_slice()).map_err(|_| vault_crypto_error())?;
    let aad = household_vault_aad_v1(account, account_slot, header)?;
    cipher
        .decrypt(
            &XNonce::from(header.nonce),
            Payload {
                msg: ciphertext,
                aad: &aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| vault_crypto_error())
}

fn derive_subkey(
    root_key: &HouseholdKeyMaterial,
    artifact_kind: VaultArtifactKindV1,
) -> Result<Zeroizing<[u8; 32]>, PortError> {
    let hkdf = Hkdf::<Sha256>::new(Some(HKDF_SALT), root_key.expose());
    let mut output = Zeroizing::new([0_u8; 32]);
    hkdf.expand(artifact_kind.hkdf_info(), output.as_mut())
        .map_err(|_| vault_crypto_error())?;
    Ok(output)
}

fn domain_hash_v1(label: &[u8], parts: &[&[u8]]) -> Result<[u8; 32], PortError> {
    if label.is_empty() || !label.is_ascii() || label.contains(&0) {
        return Err(PortError::new(
            "household_domain_hash",
            "household domain-hash label is invalid",
        ));
    }
    let mut hasher = Sha256::new();
    hasher.update(label);
    hasher.update([0]);
    for part in parts {
        let length = u32::try_from(part.len()).map_err(|_| {
            PortError::new(
                "household_domain_hash",
                "household domain-hash part is too large",
            )
        })?;
        hasher.update(length.to_be_bytes());
        hasher.update(part);
    }
    Ok(hasher.finalize().into())
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

#[cfg(all(feature = "native-credentials", unix))]
fn same_file_metadata(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.uid() == right.uid()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
}

#[cfg(all(feature = "native-credentials", not(unix)))]
fn same_file_metadata(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    left.len() == right.len() && left.modified().ok() == right.modified().ok()
}

#[cfg(feature = "native-credentials")]
fn path_present(path: &Path) -> Result<bool, PortError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(PortError::new(
            "household_teardown_artifact",
            "household teardown artifact evidence is unavailable",
        )),
    }
}

#[cfg(unix)]
#[cfg(feature = "native-credentials")]
fn sync_teardown_directory(path: &Path) -> std::io::Result<()> {
    File::open(path).and_then(|directory| directory.sync_all())
}

#[cfg(not(unix))]
fn sync_teardown_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn lower_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn decode_lower_hex_32(value: &str) -> Result<[u8; 32], PortError> {
    if value.len() != 64
        || value
            .as_bytes()
            .iter()
            .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(byte))
    {
        return Err(vault_format_error());
    }
    let mut output = [0_u8; 32];
    for (index, byte) in output.iter_mut().enumerate() {
        let start = index * 2;
        *byte =
            u8::from_str_radix(&value[start..start + 2], 16).map_err(|_| vault_format_error())?;
    }
    Ok(output)
}

fn unreferenced_slot(current: u8, previous: u8) -> Result<u8, PortError> {
    if current > 2 || previous > 2 || current == previous {
        return Err(vault_format_error());
    }
    (0..=2)
        .find(|slot| *slot != current && *slot != previous)
        .ok_or_else(vault_format_error)
}

fn ensure_private_directory(path: &Path) -> Result<(), PortError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(PortError::new(
                "household_vault_path",
                "vault directory must be a regular physical directory",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt as _;

                let mut builder = std::fs::DirBuilder::new();
                builder.recursive(true).mode(0o700);
                builder.create(path).map_err(|_| {
                    PortError::new(
                        "household_vault_path",
                        "vault directory could not be created",
                    )
                })?;
            }
            #[cfg(not(unix))]
            std::fs::create_dir_all(path).map_err(|_| {
                PortError::new(
                    "household_vault_path",
                    "vault directory could not be created",
                )
            })?;
        }
        Err(_) => {
            return Err(PortError::new(
                "household_vault_path",
                "vault directory could not be inspected",
            ));
        }
    }
    validate_private_directory(path)
}

fn validate_private_directory(path: &Path) -> Result<(), PortError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| {
        PortError::new(
            "household_vault_path",
            "vault directory could not be inspected",
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(PortError::new(
            "household_vault_path",
            "vault directory must be a regular physical directory",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(PortError::new(
                "household_vault_permissions",
                "vault directory is not owner-only",
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn validate_held_lock(lock: &LockedFile, path: &Path, owner_uid: u32) -> Result<(), PortError> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let path_metadata = std::fs::symlink_metadata(path)
        .map_err(|_| PortError::new("household_vault_lease", "held lock path is unavailable"))?;
    let file_metadata = lock
        .0
        .metadata()
        .map_err(|_| PortError::new("household_vault_lease", "held lock is unavailable"))?;
    if path_metadata.file_type().is_symlink()
        || !path_metadata.is_file()
        || !file_metadata.is_file()
        || path_metadata.uid() != owner_uid
        || file_metadata.uid() != owner_uid
        || path_metadata.permissions().mode() & 0o077 != 0
        || file_metadata.permissions().mode() & 0o077 != 0
        || path_metadata.dev() != file_metadata.dev()
        || path_metadata.ino() != file_metadata.ino()
    {
        return Err(PortError::new(
            "household_vault_lease",
            "held lock identity or permissions changed",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_held_lock(lock: &LockedFile, path: &Path) -> Result<(), PortError> {
    let path_metadata = std::fs::symlink_metadata(path)
        .map_err(|_| PortError::new("household_vault_lease", "held lock path is unavailable"))?;
    let file_metadata = lock
        .0
        .metadata()
        .map_err(|_| PortError::new("household_vault_lease", "held lock is unavailable"))?;
    if path_metadata.file_type().is_symlink()
        || !path_metadata.is_file()
        || !file_metadata.is_file()
    {
        return Err(PortError::new(
            "household_vault_lease",
            "held lock identity or permissions changed",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn open_private_lock(path: &Path) -> Result<File, PortError> {
    use std::os::unix::fs::OpenOptionsExt as _;

    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .map_err(|_| PortError::new("household_vault_lock", "vault lock is unavailable"))
}

#[cfg(not(unix))]
fn open_private_lock(path: &Path) -> Result<File, PortError> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|_| PortError::new("household_vault_lock", "vault lock is unavailable"))
}

fn cancelled_error() -> PortError {
    PortError::new(
        "household_operation_cancelled",
        "household vault operation was cancelled",
    )
}

fn migration_source_matches_guard(
    guard: &HouseholdMigrationGuardDocument,
    state: &heyfood_core::HouseholdStateV1,
) -> Result<bool, PortError> {
    let guard_value: serde_json::Value = serde_json::from_slice(&guard.canonical_bytes()?)
        .map_err(|_| initialization_resume_error())?;
    let state_source = serde_json::to_value(&state.migration_provenance.source_identity)
        .map_err(|_| initialization_resume_error())?;
    Ok(guard_value.get("source_identity") == Some(&state_source))
}

fn initialization_resume_error() -> PortError {
    PortError::new(
        "household_vault_initialization_resume_mismatch",
        "household vault initialization artifacts do not match the exact ready transaction",
    )
}

fn initialization_abort_uncertain() -> PortError {
    PortError::uncertain(
        "household_vault_initialization_abort",
        "household vault initialization cleanup requires reconciliation",
    )
}

fn initialization_abort_ambiguous() -> PortError {
    PortError::uncertain(
        "household_vault_initialization_abort_ambiguous",
        "household vault initialization artifacts are foreign, mismatched, or ambiguous",
    )
}

fn vault_lease_post_error() -> PortError {
    PortError::uncertain(
        "household_vault_lease",
        "household vault operation requires reconciliation",
    )
}

fn vault_format_error() -> PortError {
    PortError::new(
        "household_vault_format",
        "household vault artifact is invalid",
    )
}

fn vault_crypto_error() -> PortError {
    PortError::new(
        "household_vault_authentication",
        "household vault artifact could not be authenticated",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential_broker::{
        InMemoryHouseholdSecureStore, KeyBundleRevision, KeyStoreExpectation,
    };
    use heyfood_application::{
        HouseholdInitialize, HouseholdRepositoryResolutionV1, resolve_household_initialize_v1,
    };
    use heyfood_core::{CanonicalTimestampV1, CommitId, HouseholdEffectV1, HouseholdStateV1};

    fn decode_hex<const N: usize>(value: &str) -> [u8; N] {
        assert_eq!(value.len(), N * 2);
        let mut output = [0_u8; N];
        for (index, byte) in output.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).unwrap();
        }
        output
    }

    #[test]
    #[cfg(feature = "native-credentials")]
    fn teardown_snapshot_evidence_is_rechecked_before_journal_authority() {
        let root = std::env::temp_dir().join(format!(
            "heyfood-vault-teardown-snapshot-{}",
            Uuid::new_v4()
        ));
        let vault = HouseholdVault::open(
            &root.join("data"),
            AccountId::parse("acct_snapshot_teardown").unwrap(),
        )
        .unwrap();
        let target =
            HouseholdTeardownVaultTargetV1::open(vault.native_root(), vault.account_slot().clone())
                .unwrap();
        let snapshot_path = root.join("python-state-import.v1.json");
        let snapshot = b"{\"schema_version\":1}\n";
        AtomicFile::replace(&snapshot_path, snapshot).unwrap();
        let cancellation = CancellationToken::new();

        target
            .verify_snapshot_evidence_blocking(
                &snapshot_path,
                Some(sha256(snapshot)),
                &cancellation,
            )
            .unwrap();
        assert!(
            target
                .verify_snapshot_evidence_blocking(
                    &snapshot_path,
                    Some(sha256(b"different")),
                    &cancellation,
                )
                .is_err()
        );
        assert!(
            target
                .verify_snapshot_evidence_blocking(&snapshot_path, None, &cancellation)
                .is_err()
        );
        std::fs::remove_file(&snapshot_path).unwrap();
        target
            .verify_snapshot_evidence_blocking(
                &snapshot_path,
                Some(sha256(snapshot)),
                &cancellation,
            )
            .unwrap();
    }

    fn initialization_resume_fixture(
        name: &str,
    ) -> (
        PathBuf,
        HouseholdVault,
        HouseholdMigrationGuardDocument,
        HouseholdKeyBundle,
        HouseholdVaultWrite,
        HouseholdStateV1,
    ) {
        let root = std::env::temp_dir().join(format!(
            "heyfood-vault-authenticated-resume-{name}-{}",
            Uuid::new_v4()
        ));
        let account = AccountId::parse("acct_example_01").unwrap();
        let vault = HouseholdVault::open(&root.join("data"), account.clone()).unwrap();
        let golden: serde_json::Value = serde_json::from_str(include_str!(
            "../../../schemas/v1/household-canonical-v1.golden.json"
        ))
        .unwrap();
        let semantic_candidate = decode_canonical_household_state_v1(
            golden["state"]["canonical_utf8"]
                .as_str()
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
        let migration_id = Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap();
        let initialization_id = Uuid::parse_str("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb").unwrap();
        let initial_commit = Uuid::parse_str("cccccccc-cccc-4ccc-8ccc-cccccccccccc").unwrap();
        let frozen_at = CanonicalTimestampV1::parse("2026-07-30T12:00:00.000Z").unwrap();
        let command = HouseholdInitialize::new(
            account,
            CommitId::from_uuid(initial_commit),
            semantic_candidate.clone(),
            HouseholdEffectV1::Initialize,
            frozen_at.clone(),
        )
        .unwrap();
        let HouseholdRepositoryResolutionV1::Write {
            state: resolved, ..
        } = resolve_household_initialize_v1(None, &command).unwrap()
        else {
            unreachable!()
        };
        let resolved = *resolved;
        let write =
            HouseholdVaultWrite::new(1, initial_commit, resolved.canonical_bytes().unwrap())
                .unwrap();
        let reserved = HouseholdMigrationGuardDocument::initializing_reserved(
            vault.account_slot(),
            crate::HouseholdMigrationSourceIdentityV1::no_source([7; 32]),
            migration_id,
            initialization_id,
            initial_commit,
            frozen_at,
        )
        .unwrap();
        let ready = reserved
            .ready_to_initialize(
                *command.claimed_effect_fingerprint.as_digest().as_bytes(),
                write.plaintext_sha256(),
            )
            .unwrap();
        let key = HouseholdKeyBundle::initializing(
            vault.account_slot(),
            KeyBundleRevision::new(1).unwrap(),
            KeyId::new(),
            HouseholdKeyMaterial::from_bytes([0x71; 32]),
            initialization_id,
            initial_commit,
            *command.claimed_effect_fingerprint.as_digest().as_bytes(),
            write.plaintext_sha256(),
        );
        (root, vault, ready, key, write, semantic_candidate)
    }

    #[tokio::test]
    async fn authenticated_uncommitted_resume_continues_exact_gen0_and_gen0_gen1() {
        for generation_count in [1_u8, 2] {
            let (root, vault, guard, key, write, _) =
                initialization_resume_fixture(&format!("positive-{generation_count}"));
            let lifecycle = vault
                .acquire_lifecycle_lease(CancellationToken::new())
                .await
                .unwrap();
            let mut lease = vault
                .acquire_vault_lease(
                    lifecycle,
                    HouseholdVaultLeaseModeV1::CreateIfMissing,
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            vault.write_generation(0, &key, &write).unwrap();
            if generation_count == 2 {
                vault.write_generation(1, &key, &write).unwrap();
            }
            let before = (0..generation_count)
                .map(|slot| {
                    std::fs::read(
                        vault
                            .household_directory()
                            .join(format!("generation-{slot}.hfv")),
                    )
                    .unwrap()
                })
                .collect::<Vec<_>>();

            let recovered = vault
                .recover_uncommitted_initialization_write(
                    &mut lease,
                    key.clone(),
                    guard,
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            assert_eq!(recovered, write);
            for (slot, expected) in before.iter().enumerate() {
                assert_eq!(
                    std::fs::read(
                        vault
                            .household_directory()
                            .join(format!("generation-{slot}.hfv"))
                    )
                    .unwrap(),
                    *expected
                );
            }
            let loaded = vault
                .initialize(&mut lease, key, recovered, CancellationToken::new())
                .await
                .unwrap();
            assert_eq!(loaded.canonical_state, write.canonical_state);
            assert!(vault.household_directory().join("commit.hfj").is_file());
            drop(lease);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[tokio::test]
    async fn authenticated_uncommitted_resume_rejects_wrong_topology_without_mutation() {
        for slot in [1_u8, 2] {
            let (root, vault, guard, key, write, _) =
                initialization_resume_fixture(&format!("wrong-topology-{slot}"));
            let lifecycle = vault
                .acquire_lifecycle_lease(CancellationToken::new())
                .await
                .unwrap();
            let mut lease = vault
                .acquire_vault_lease(
                    lifecycle,
                    HouseholdVaultLeaseModeV1::CreateIfMissing,
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            vault.write_generation(slot, &key, &write).unwrap();
            let path = vault
                .household_directory()
                .join(format!("generation-{slot}.hfv"));
            let before = std::fs::read(&path).unwrap();
            assert_eq!(
                vault
                    .recover_uncommitted_initialization_write(
                        &mut lease,
                        key,
                        guard,
                        CancellationToken::new(),
                    )
                    .await
                    .unwrap_err()
                    .code,
                "household_vault_initialization_resume_mismatch"
            );
            assert_eq!(std::fs::read(path).unwrap(), before);
            drop(lease);
            std::fs::remove_dir_all(root).unwrap();
        }

        let (root, vault, guard, key, write, _) =
            initialization_resume_fixture("unexpected-journal");
        let lifecycle = vault
            .acquire_lifecycle_lease(CancellationToken::new())
            .await
            .unwrap();
        let mut lease = vault
            .acquire_vault_lease(
                lifecycle,
                HouseholdVaultLeaseModeV1::CreateIfMissing,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        vault
            .initialize(&mut lease, key.clone(), write, CancellationToken::new())
            .await
            .unwrap();
        let paths = ["generation-0.hfv", "generation-1.hfv", "commit.hfj"]
            .map(|name| vault.household_directory().join(name));
        let before = paths
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            vault
                .recover_uncommitted_initialization_write(
                    &mut lease,
                    key,
                    guard,
                    CancellationToken::new(),
                )
                .await
                .unwrap_err()
                .code,
            "household_vault_initialization_resume_mismatch"
        );
        for (path, expected) in paths.iter().zip(before) {
            assert_eq!(std::fs::read(path).unwrap(), expected);
        }
        drop(lease);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn authenticated_uncommitted_resume_rejects_binding_and_cancellation_without_mutation() {
        let (root, vault, guard, key, write, semantic_candidate) =
            initialization_resume_fixture("binding");
        let lifecycle = vault
            .acquire_lifecycle_lease(CancellationToken::new())
            .await
            .unwrap();
        let mut lease = vault
            .acquire_vault_lease(
                lifecycle,
                HouseholdVaultLeaseModeV1::CreateIfMissing,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        vault.write_generation(0, &key, &write).unwrap();
        let path = vault.household_directory().join("generation-0.hfv");
        let before = std::fs::read(&path).unwrap();

        let wrong_key = HouseholdKeyBundle::initializing(
            vault.account_slot(),
            KeyBundleRevision::new(1).unwrap(),
            KeyId::new(),
            HouseholdKeyMaterial::from_bytes([0x72; 32]),
            guard.initialization_id(),
            guard.initial_commit_id(),
            guard.initial_effect_fingerprint().unwrap(),
            guard.initial_state_digest().unwrap(),
        );
        assert!(
            vault
                .recover_uncommitted_initialization_write(
                    &mut lease,
                    wrong_key,
                    guard.clone(),
                    CancellationToken::new(),
                )
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);

        let wrong_commit_write =
            HouseholdVaultWrite::new(1, Uuid::new_v4(), write.canonical_state.to_vec()).unwrap();
        std::fs::remove_file(&path).unwrap();
        vault
            .write_generation(0, &key, &wrong_commit_write)
            .unwrap();
        let wrong_commit_before = std::fs::read(&path).unwrap();
        assert!(
            vault
                .recover_uncommitted_initialization_write(
                    &mut lease,
                    key.clone(),
                    guard.clone(),
                    CancellationToken::new(),
                )
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), wrong_commit_before);

        let wrong_digest_write = HouseholdVaultWrite::new(
            1,
            guard.initial_commit_id(),
            semantic_candidate.canonical_bytes().unwrap(),
        )
        .unwrap();
        std::fs::remove_file(&path).unwrap();
        vault
            .write_generation(0, &key, &wrong_digest_write)
            .unwrap();
        let wrong_digest_before = std::fs::read(&path).unwrap();
        assert!(
            vault
                .recover_uncommitted_initialization_write(
                    &mut lease,
                    key.clone(),
                    guard.clone(),
                    CancellationToken::new(),
                )
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), wrong_digest_before);

        std::fs::remove_file(&path).unwrap();
        vault.write_generation(0, &key, &write).unwrap();
        let cancelled_before = std::fs::read(&path).unwrap();
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert_eq!(
            vault
                .recover_uncommitted_initialization_write(
                    &mut lease,
                    key.clone(),
                    guard.clone(),
                    cancelled,
                )
                .await
                .unwrap_err()
                .code,
            "household_operation_cancelled"
        );
        assert_eq!(std::fs::read(&path).unwrap(), cancelled_before);

        let foreign_account = AccountId::parse("acct_foreign_resume").unwrap();
        let foreign_vault =
            HouseholdVault::open(&root.join("foreign-data"), foreign_account).unwrap();
        let foreign_lifecycle = foreign_vault
            .acquire_lifecycle_lease(CancellationToken::new())
            .await
            .unwrap();
        let mut foreign_lease = foreign_vault
            .acquire_vault_lease(
                foreign_lifecycle,
                HouseholdVaultLeaseModeV1::CreateIfMissing,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            vault
                .recover_uncommitted_initialization_write(
                    &mut foreign_lease,
                    key,
                    guard,
                    CancellationToken::new(),
                )
                .await
                .unwrap_err()
                .code,
            "household_lifecycle_lease_mismatch"
        );
        assert_eq!(std::fs::read(&path).unwrap(), cancelled_before);

        drop(foreign_lease);
        drop(lease);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn initializing_bundle_resumes_partial_and_committed_exact_artifacts() {
        let test_root = std::env::temp_dir().join(format!(
            "heyfood-vault-initialization-resume-{}",
            Uuid::new_v4()
        ));
        let account = AccountId::parse("acct_example_01").unwrap();
        let vault = HouseholdVault::open(&test_root.join("data"), account).unwrap();
        let golden: serde_json::Value = serde_json::from_str(include_str!(
            "../../../schemas/v1/household-canonical-v1.golden.json"
        ))
        .unwrap();
        let state = HouseholdVaultWrite::new(
            1,
            Uuid::new_v4(),
            golden["state"]["canonical_utf8"]
                .as_str()
                .unwrap()
                .as_bytes()
                .to_vec(),
        )
        .unwrap();
        let bundle = HouseholdKeyBundle::initializing(
            vault.account_slot(),
            KeyBundleRevision::new(1).unwrap(),
            KeyId::new(),
            HouseholdKeyMaterial::from_bytes([0x91; 32]),
            Uuid::new_v4(),
            state.commit_id,
            [0x92; 32],
            state.plaintext_sha256(),
        );

        {
            let cancellation = CancellationToken::new();
            let _locks = vault.acquire_locks(&cancellation, true, false).unwrap();
            vault.write_generation(0, &bundle, &state).unwrap();
        }
        assert!(
            vault
                .household_directory()
                .join("generation-0.hfv")
                .is_file()
        );
        assert!(
            !vault
                .household_directory()
                .join("generation-1.hfv")
                .exists()
        );

        let lifecycle_lease = vault
            .acquire_lifecycle_lease(CancellationToken::new())
            .await
            .unwrap();
        let mut vault_lease = vault
            .acquire_vault_lease(
                lifecycle_lease,
                HouseholdVaultLeaseModeV1::RequireExisting,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let loaded = vault
            .initialize(
                &mut vault_lease,
                bundle.clone(),
                state.clone(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(loaded.canonical_state, state.canonical_state);
        let before = [
            std::fs::read(vault.household_directory().join("generation-0.hfv")).unwrap(),
            std::fs::read(vault.household_directory().join("generation-1.hfv")).unwrap(),
            std::fs::read(vault.household_directory().join("commit.hfj")).unwrap(),
        ];

        let replayed = vault
            .initialize(
                &mut vault_lease,
                bundle,
                state.clone(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(replayed.canonical_state, state.canonical_state);
        let after = [
            std::fs::read(vault.household_directory().join("generation-0.hfv")).unwrap(),
            std::fs::read(vault.household_directory().join("generation-1.hfv")).unwrap(),
            std::fs::read(vault.household_directory().join("commit.hfj")).unwrap(),
        ];
        assert_eq!(after, before);
        let _ = std::fs::remove_dir_all(test_root);
    }

    #[tokio::test]
    async fn startup_evidence_rejects_initial_staging_and_altered_journal_topology() {
        let test_root =
            std::env::temp_dir().join(format!("heyfood-vault-startup-evidence-{}", Uuid::new_v4()));
        let account = AccountId::parse("acct_example_01").unwrap();
        let vault = HouseholdVault::open(&test_root.join("data"), account).unwrap();
        let golden: serde_json::Value = serde_json::from_str(include_str!(
            "../../../schemas/v1/household-canonical-v1.golden.json"
        ))
        .unwrap();
        let state = HouseholdVaultWrite::new(
            1,
            Uuid::new_v4(),
            golden["state"]["canonical_utf8"]
                .as_str()
                .unwrap()
                .as_bytes()
                .to_vec(),
        )
        .unwrap();
        let bundle = HouseholdKeyBundle::initializing(
            vault.account_slot(),
            KeyBundleRevision::new(1).unwrap(),
            KeyId::new(),
            HouseholdKeyMaterial::from_bytes([0x81; 32]),
            Uuid::new_v4(),
            state.commit_id,
            [0x82; 32],
            state.plaintext_sha256(),
        );
        let lifecycle = vault
            .acquire_lifecycle_lease(CancellationToken::new())
            .await
            .unwrap();
        let mut lease = vault
            .acquire_vault_lease(
                lifecycle,
                HouseholdVaultLeaseModeV1::CreateIfMissing,
                CancellationToken::new(),
            )
            .await
            .unwrap();

        vault.write_generation(2, &bundle, &state).unwrap();
        let staging = vault
            .classify_startup_artifacts(
                &mut lease,
                Some(bundle.clone()),
                Some(state.commit_id),
                Some(state.plaintext_sha256()),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(staging.code, "household_native_evidence_contradiction");

        std::fs::remove_file(vault.generation_path(2).unwrap()).unwrap();
        vault.write_generation(0, &bundle, &state).unwrap();
        vault.write_generation(2, &bundle, &state).unwrap();
        let digest = lower_hex(&state.plaintext_sha256());
        let altered = VaultJournalV1::new(
            1,
            GenerationReferenceV1 {
                commit_id: state.commit_id,
                plaintext_sha256: digest.clone(),
                slot: 0,
                state_revision: 1,
            },
            GenerationReferenceV1 {
                commit_id: state.commit_id,
                plaintext_sha256: digest,
                slot: 2,
                state_revision: 1,
            },
        )
        .unwrap();
        vault.write_journal(&bundle, &altered).unwrap();
        let topology = vault
            .classify_startup_artifacts(
                &mut lease,
                Some(bundle),
                Some(state.commit_id),
                Some(state.plaintext_sha256()),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(topology.code, "household_native_evidence_contradiction");
        drop(lease);
        let _ = std::fs::remove_dir_all(test_root);
    }

    #[tokio::test]
    async fn startup_evidence_accepts_only_the_closed_initialization_topology_matrix() {
        let test_root =
            std::env::temp_dir().join(format!("heyfood-vault-startup-matrix-{}", Uuid::new_v4()));
        let account = AccountId::parse("acct_example_01").unwrap();
        let vault = HouseholdVault::open(&test_root.join("data"), account).unwrap();
        let golden: serde_json::Value = serde_json::from_str(include_str!(
            "../../../schemas/v1/household-canonical-v1.golden.json"
        ))
        .unwrap();
        let state = HouseholdVaultWrite::new(
            1,
            Uuid::new_v4(),
            golden["state"]["canonical_utf8"]
                .as_str()
                .unwrap()
                .as_bytes()
                .to_vec(),
        )
        .unwrap();
        let bundle = HouseholdKeyBundle::initializing(
            vault.account_slot(),
            KeyBundleRevision::new(1).unwrap(),
            KeyId::new(),
            HouseholdKeyMaterial::from_bytes([0x83; 32]),
            Uuid::new_v4(),
            state.commit_id,
            [0x84; 32],
            state.plaintext_sha256(),
        );
        let lifecycle = vault
            .acquire_lifecycle_lease(CancellationToken::new())
            .await
            .unwrap();
        let mut lease = vault
            .acquire_vault_lease(
                lifecycle,
                HouseholdVaultLeaseModeV1::CreateIfMissing,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            vault
                .classify_startup_artifacts(
                    &mut lease,
                    Some(bundle.clone()),
                    Some(state.commit_id),
                    Some(state.plaintext_sha256()),
                    CancellationToken::new(),
                )
                .await
                .unwrap(),
            HouseholdVaultStartupArtifactsV1::Absent
        );
        vault.write_generation(0, &bundle, &state).unwrap();
        assert_eq!(
            vault
                .classify_startup_artifacts(
                    &mut lease,
                    Some(bundle.clone()),
                    Some(state.commit_id),
                    Some(state.plaintext_sha256()),
                    CancellationToken::new(),
                )
                .await
                .unwrap(),
            HouseholdVaultStartupArtifactsV1::MatchingUncommitted
        );
        vault.write_generation(1, &bundle, &state).unwrap();
        assert_eq!(
            vault
                .classify_startup_artifacts(
                    &mut lease,
                    Some(bundle.clone()),
                    Some(state.commit_id),
                    Some(state.plaintext_sha256()),
                    CancellationToken::new(),
                )
                .await
                .unwrap(),
            HouseholdVaultStartupArtifactsV1::MatchingUncommitted
        );

        std::fs::remove_file(vault.generation_path(0).unwrap()).unwrap();
        let generation_one = std::fs::read(vault.generation_path(1).unwrap()).unwrap();
        assert_eq!(
            vault
                .classify_startup_artifacts(
                    &mut lease,
                    Some(bundle.clone()),
                    Some(state.commit_id),
                    Some(state.plaintext_sha256()),
                    CancellationToken::new(),
                )
                .await
                .unwrap_err()
                .code,
            "household_native_evidence_contradiction"
        );
        assert_eq!(
            std::fs::read(vault.generation_path(1).unwrap()).unwrap(),
            generation_one
        );

        vault.write_generation(0, &bundle, &state).unwrap();
        let digest = lower_hex(&state.plaintext_sha256());
        let journal = VaultJournalV1::new(
            1,
            GenerationReferenceV1 {
                commit_id: state.commit_id,
                plaintext_sha256: digest.clone(),
                slot: 0,
                state_revision: 1,
            },
            GenerationReferenceV1 {
                commit_id: state.commit_id,
                plaintext_sha256: digest,
                slot: 1,
                state_revision: 1,
            },
        )
        .unwrap();
        vault.write_journal(&bundle, &journal).unwrap();
        assert_eq!(
            vault
                .classify_startup_artifacts(
                    &mut lease,
                    Some(bundle.clone()),
                    Some(state.commit_id),
                    Some(state.plaintext_sha256()),
                    CancellationToken::new(),
                )
                .await
                .unwrap(),
            HouseholdVaultStartupArtifactsV1::MatchingCommitted
        );

        vault.write_generation(2, &bundle, &state).unwrap();
        let before = [
            std::fs::read(vault.generation_path(0).unwrap()).unwrap(),
            std::fs::read(vault.generation_path(1).unwrap()).unwrap(),
            std::fs::read(vault.generation_path(2).unwrap()).unwrap(),
            std::fs::read(vault.journal_path()).unwrap(),
        ];
        assert_eq!(
            vault
                .classify_startup_artifacts(
                    &mut lease,
                    Some(bundle),
                    Some(state.commit_id),
                    Some(state.plaintext_sha256()),
                    CancellationToken::new(),
                )
                .await
                .unwrap_err()
                .code,
            "household_native_evidence_contradiction"
        );
        let after = [
            std::fs::read(vault.generation_path(0).unwrap()).unwrap(),
            std::fs::read(vault.generation_path(1).unwrap()).unwrap(),
            std::fs::read(vault.generation_path(2).unwrap()).unwrap(),
            std::fs::read(vault.journal_path()).unwrap(),
        ];
        assert_eq!(after, before);
        drop(lease);
        let _ = std::fs::remove_dir_all(test_root);
    }

    #[tokio::test]
    async fn abort_deletes_only_authenticated_exact_invalid_initialization_topology() {
        let test_root = std::env::temp_dir().join(format!(
            "heyfood-vault-initialization-abort-{}",
            Uuid::new_v4()
        ));
        let account = AccountId::parse("acct_example_01").unwrap();
        let vault = HouseholdVault::open(&test_root.join("data"), account).unwrap();
        let golden: serde_json::Value = serde_json::from_str(include_str!(
            "../../../schemas/v1/household-canonical-v1.golden.json"
        ))
        .unwrap();
        let state = HouseholdVaultWrite::new(
            1,
            Uuid::new_v4(),
            golden["state"]["canonical_utf8"]
                .as_str()
                .unwrap()
                .as_bytes()
                .to_vec(),
        )
        .unwrap();
        let initialization_id = Uuid::new_v4();
        let migration_id = Uuid::new_v4();
        let reserved_guard = HouseholdMigrationGuardDocument::initializing_reserved(
            vault.account_slot(),
            crate::credential_broker::HouseholdMigrationSourceIdentityV1::present([0xa0; 32]),
            migration_id,
            initialization_id,
            state.commit_id,
            CanonicalTimestampV1::parse("2026-07-30T12:00:00.000Z").unwrap(),
        )
        .unwrap();
        let ready_guard = reserved_guard
            .ready_to_initialize([0xa2; 32], state.plaintext_sha256())
            .unwrap();
        let bundle = HouseholdKeyBundle::initializing(
            vault.account_slot(),
            KeyBundleRevision::new(1).unwrap(),
            KeyId::new(),
            HouseholdKeyMaterial::from_bytes([0xa1; 32]),
            initialization_id,
            state.commit_id,
            [0xa2; 32],
            state.plaintext_sha256(),
        );
        let lifecycle_lease = vault
            .acquire_lifecycle_lease(CancellationToken::new())
            .await
            .unwrap();
        let mut vault_lease = vault
            .acquire_vault_lease(
                lifecycle_lease,
                HouseholdVaultLeaseModeV1::CreateIfMissing,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let store = InMemoryHouseholdSecureStore::default();
        HouseholdMigrationGuardStore::compare_exchange(
            &store,
            &mut vault_lease,
            MigrationGuardExpectation::Absent,
            Some(reserved_guard.clone()),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        HouseholdMigrationGuardStore::compare_exchange(
            &store,
            &mut vault_lease,
            MigrationGuardExpectation::Revision(reserved_guard.guard_revision()),
            Some(ready_guard.clone()),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        HouseholdKeyStore::initialize(
            &store,
            &mut vault_lease,
            KeyStoreExpectation::Absent,
            ready_guard.clone(),
            bundle.clone(),
            CancellationToken::new(),
        )
        .await
        .unwrap();

        vault.write_generation(2, &bundle, &state).unwrap();
        let staging_path = vault.generation_path(2).unwrap();
        let mut malformed = std::fs::read(&staging_path).unwrap();
        *malformed.last_mut().unwrap() ^= 0x80;
        std::fs::write(&staging_path, &malformed).unwrap();
        let error = vault
            .abort_invalid_initialization_to_blocked_repair(
                &mut vault_lease,
                &store,
                initialization_id,
                Some(state.clone()),
                HouseholdMigrationRepairFailureCategoryV1::CanonicalConstruction,
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "household_vault_initialization_abort_ambiguous");
        assert!(error.outcome_uncertain);
        assert_eq!(std::fs::read(&staging_path).unwrap(), malformed);

        vault.write_generation(2, &bundle, &state).unwrap();
        let aborting = vault
            .record_initialization_abort_intent(
                &mut vault_lease,
                &store,
                ready_guard,
                Some(&state),
                HouseholdMigrationRepairFailureCategoryV1::CanonicalConstruction,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let mut post_intent_mismatch = std::fs::read(&staging_path).unwrap();
        *post_intent_mismatch.last_mut().unwrap() ^= 0x40;
        std::fs::write(&staging_path, &post_intent_mismatch).unwrap();
        let error = vault
            .abort_invalid_initialization_to_blocked_repair(
                &mut vault_lease,
                &store,
                initialization_id,
                None,
                HouseholdMigrationRepairFailureCategoryV1::CanonicalConstruction,
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "household_vault_initialization_abort_ambiguous");
        assert!(error.outcome_uncertain);
        assert_eq!(std::fs::read(&staging_path).unwrap(), post_intent_mismatch);
        assert_eq!(
            HouseholdMigrationGuardStore::load(
                &store,
                vault_lease.lifecycle_lease(),
                CancellationToken::new(),
            )
            .await
            .unwrap(),
            Some(aborting)
        );
        assert!(
            HouseholdKeyStore::load(
                &store,
                vault_lease.lifecycle_lease(),
                CancellationToken::new(),
            )
            .await
            .unwrap()
            .is_some()
        );

        vault.write_generation(2, &bundle, &state).unwrap();
        let blocked = vault
            .abort_invalid_initialization_to_blocked_repair(
                &mut vault_lease,
                &store,
                initialization_id,
                None,
                HouseholdMigrationRepairFailureCategoryV1::CanonicalConstruction,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            blocked.state(),
            HouseholdMigrationGuardStateV1::BlockedRepair
        );
        assert!(!staging_path.exists());
        assert!(
            HouseholdKeyStore::load(
                &store,
                vault_lease.lifecycle_lease(),
                CancellationToken::new(),
            )
            .await
            .unwrap()
            .is_none()
        );
        drop(
            vault_lease
                .release_vault(CancellationToken::new())
                .await
                .unwrap(),
        );
        let _ = std::fs::remove_dir_all(test_root);
    }

    #[tokio::test]
    async fn abort_restart_uses_only_durable_guard_key_and_artifacts_at_every_crash_cut() {
        for cut in 0..3 {
            let test_root = std::env::temp_dir()
                .join(format!("heyfood-vault-abort-cut-{cut}-{}", Uuid::new_v4()));
            let account = AccountId::parse("acct_example_01").unwrap();
            let vault = HouseholdVault::open(&test_root.join("data"), account.clone()).unwrap();
            let golden: serde_json::Value = serde_json::from_str(include_str!(
                "../../../schemas/v1/household-canonical-v1.golden.json"
            ))
            .unwrap();
            let state = HouseholdVaultWrite::new(
                1,
                Uuid::new_v4(),
                golden["state"]["canonical_utf8"]
                    .as_str()
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
            )
            .unwrap();
            let initialization_id = Uuid::new_v4();
            let reserved = HouseholdMigrationGuardDocument::initializing_reserved(
                vault.account_slot(),
                crate::credential_broker::HouseholdMigrationSourceIdentityV1::present([0xb0; 32]),
                Uuid::new_v4(),
                initialization_id,
                state.commit_id,
                CanonicalTimestampV1::parse("2026-07-30T12:00:00.000Z").unwrap(),
            )
            .unwrap();
            let ready = reserved
                .ready_to_initialize([0xb1; 32], state.plaintext_sha256())
                .unwrap();
            let bundle = HouseholdKeyBundle::initializing(
                vault.account_slot(),
                KeyBundleRevision::new(1).unwrap(),
                KeyId::new(),
                HouseholdKeyMaterial::from_bytes([0xb2; 32]),
                initialization_id,
                state.commit_id,
                [0xb1; 32],
                state.plaintext_sha256(),
            );
            let lifecycle_lease = vault
                .acquire_lifecycle_lease(CancellationToken::new())
                .await
                .unwrap();
            let mut vault_lease = vault
                .acquire_vault_lease(
                    lifecycle_lease,
                    HouseholdVaultLeaseModeV1::CreateIfMissing,
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            let store = InMemoryHouseholdSecureStore::default();
            HouseholdMigrationGuardStore::compare_exchange(
                &store,
                &mut vault_lease,
                MigrationGuardExpectation::Absent,
                Some(reserved.clone()),
                CancellationToken::new(),
            )
            .await
            .unwrap();
            HouseholdMigrationGuardStore::compare_exchange(
                &store,
                &mut vault_lease,
                MigrationGuardExpectation::Revision(reserved.guard_revision()),
                Some(ready.clone()),
                CancellationToken::new(),
            )
            .await
            .unwrap();
            HouseholdKeyStore::initialize(
                &store,
                &mut vault_lease,
                KeyStoreExpectation::Absent,
                ready.clone(),
                bundle.clone(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
            vault.write_generation(2, &bundle, &state).unwrap();

            let aborting = vault
                .record_initialization_abort_intent(
                    &mut vault_lease,
                    &store,
                    ready.clone(),
                    Some(&state),
                    HouseholdMigrationRepairFailureCategoryV1::CanonicalConstruction,
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            if cut >= 1 {
                vault
                    .delete_aborting_initialization_artifacts(
                        &mut vault_lease,
                        bundle.clone(),
                        aborting.clone(),
                        CancellationToken::new(),
                    )
                    .await
                    .unwrap();
            }
            if cut >= 2 {
                HouseholdKeyStore::abort_initialization_and_verify(
                    &store,
                    &mut vault_lease,
                    bundle.revision,
                    initialization_id,
                    aborting.clone(),
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            }

            let durable = HouseholdMigrationGuardStore::load(
                &store,
                vault_lease.lifecycle_lease(),
                CancellationToken::new(),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(durable, aborting);
            assert_eq!(durable.state(), HouseholdMigrationGuardStateV1::Aborting);
            let remint = HouseholdKeyStore::initialize(
                &store,
                &mut vault_lease,
                KeyStoreExpectation::Absent,
                ready.clone(),
                bundle.clone(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
            assert_eq!(remint.code, "household_key_guard_mismatch");

            drop(
                vault_lease
                    .release_vault(CancellationToken::new())
                    .await
                    .unwrap(),
            );
            let original_state_digest = state.plaintext_sha256();
            drop(state);
            drop(bundle);
            drop(ready);
            drop(reserved);
            drop(durable);
            drop(aborting);
            drop(remint);
            drop(vault);
            drop(golden);

            // A restarted process may find the legacy source unavailable or
            // may reconstruct a different candidate. Neither is cleanup
            // authority once the exact Aborting guard is durable.
            if cut == 1 {
                let changed_golden: serde_json::Value = serde_json::from_str(include_str!(
                    "../../../schemas/v1/household-canonical-v1.golden.json"
                ))
                .unwrap();
                let mut changed: serde_json::Value = serde_json::from_str(
                    changed_golden["state"]["canonical_utf8"].as_str().unwrap(),
                )
                .unwrap();
                changed["owner"]["display_name"] = serde_json::json!("Changed Owner");
                let changed_state = HouseholdVaultWrite::new(
                    1,
                    Uuid::new_v4(),
                    heyfood_core::canonicalize_json_value_v1(&changed).unwrap(),
                )
                .unwrap();
                assert_ne!(changed_state.plaintext_sha256(), original_state_digest);
                drop(changed_state);
            }

            let restarted = HouseholdVault::open(&test_root.join("data"), account).unwrap();
            let restarted_lifecycle = restarted
                .acquire_lifecycle_lease(CancellationToken::new())
                .await
                .unwrap();
            let mut restarted_lease = restarted
                .acquire_vault_lease(
                    restarted_lifecycle,
                    HouseholdVaultLeaseModeV1::RequireExisting,
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            let cancelled = CancellationToken::new();
            cancelled.cancel();
            assert_eq!(
                restarted
                    .abort_invalid_initialization_to_blocked_repair(
                        &mut restarted_lease,
                        &store,
                        initialization_id,
                        None,
                        HouseholdMigrationRepairFailureCategoryV1::CanonicalConstruction,
                        cancelled,
                    )
                    .await
                    .unwrap_err()
                    .code,
                "household_operation_cancelled"
            );
            assert_eq!(
                HouseholdMigrationGuardStore::load(
                    &store,
                    restarted_lease.lifecycle_lease(),
                    CancellationToken::new(),
                )
                .await
                .unwrap()
                .unwrap()
                .state(),
                HouseholdMigrationGuardStateV1::Aborting
            );

            let blocked = restarted
                .abort_invalid_initialization_to_blocked_repair(
                    &mut restarted_lease,
                    &store,
                    initialization_id,
                    None,
                    HouseholdMigrationRepairFailureCategoryV1::CanonicalConstruction,
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            assert_eq!(
                blocked.state(),
                HouseholdMigrationGuardStateV1::BlockedRepair
            );
            assert!(
                HouseholdKeyStore::load(
                    &store,
                    restarted_lease.lifecycle_lease(),
                    CancellationToken::new(),
                )
                .await
                .unwrap()
                .is_none()
            );
            restarted
                .verify_initialization_artifacts_absent(
                    &mut restarted_lease,
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            drop(
                restarted_lease
                    .release_vault(CancellationToken::new())
                    .await
                    .unwrap(),
            );
            let _ = std::fs::remove_dir_all(test_root);
        }
    }

    #[tokio::test]
    async fn abort_reconciles_lost_guard_cas_and_key_abort_results() {
        let test_root =
            std::env::temp_dir().join(format!("heyfood-vault-abort-cas-{}", Uuid::new_v4()));
        let vault = HouseholdVault::open(
            &test_root.join("data"),
            AccountId::parse("acct_example_01").unwrap(),
        )
        .unwrap();
        let golden: serde_json::Value = serde_json::from_str(include_str!(
            "../../../schemas/v1/household-canonical-v1.golden.json"
        ))
        .unwrap();
        let state = HouseholdVaultWrite::new(
            1,
            Uuid::new_v4(),
            golden["state"]["canonical_utf8"]
                .as_str()
                .unwrap()
                .as_bytes()
                .to_vec(),
        )
        .unwrap();
        let initialization_id = Uuid::new_v4();
        let reserved = HouseholdMigrationGuardDocument::initializing_reserved(
            vault.account_slot(),
            crate::credential_broker::HouseholdMigrationSourceIdentityV1::present([0xc0; 32]),
            Uuid::new_v4(),
            initialization_id,
            state.commit_id,
            CanonicalTimestampV1::parse("2026-07-30T12:00:00.000Z").unwrap(),
        )
        .unwrap();
        let ready = reserved
            .ready_to_initialize([0xc1; 32], state.plaintext_sha256())
            .unwrap();
        let bundle = HouseholdKeyBundle::initializing(
            vault.account_slot(),
            KeyBundleRevision::new(1).unwrap(),
            KeyId::new(),
            HouseholdKeyMaterial::from_bytes([0xc2; 32]),
            initialization_id,
            state.commit_id,
            [0xc1; 32],
            state.plaintext_sha256(),
        );
        let lifecycle_lease = vault
            .acquire_lifecycle_lease(CancellationToken::new())
            .await
            .unwrap();
        let mut vault_lease = vault
            .acquire_vault_lease(
                lifecycle_lease,
                HouseholdVaultLeaseModeV1::CreateIfMissing,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let store = InMemoryHouseholdSecureStore::default();
        HouseholdMigrationGuardStore::compare_exchange(
            &store,
            &mut vault_lease,
            MigrationGuardExpectation::Absent,
            Some(reserved.clone()),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        HouseholdMigrationGuardStore::compare_exchange(
            &store,
            &mut vault_lease,
            MigrationGuardExpectation::Revision(reserved.guard_revision()),
            Some(ready.clone()),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        HouseholdKeyStore::initialize(
            &store,
            &mut vault_lease,
            KeyStoreExpectation::Absent,
            ready,
            bundle.clone(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        vault.write_generation(2, &bundle, &state).unwrap();
        store.inject_next_guard_cas_uncertain_after_commit();
        store.inject_next_key_abort_uncertain_after_delete();

        let blocked = vault
            .abort_invalid_initialization_to_blocked_repair(
                &mut vault_lease,
                &store,
                initialization_id,
                Some(state),
                HouseholdMigrationRepairFailureCategoryV1::CanonicalConstruction,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            blocked.state(),
            HouseholdMigrationGuardStateV1::BlockedRepair
        );
        assert!(
            HouseholdKeyStore::load(
                &store,
                vault_lease.lifecycle_lease(),
                CancellationToken::new(),
            )
            .await
            .unwrap()
            .is_none()
        );
        drop(
            vault_lease
                .release_vault(CancellationToken::new())
                .await
                .unwrap(),
        );
        let _ = std::fs::remove_dir_all(test_root);
    }

    #[tokio::test]
    async fn detached_operation_guard_retains_both_locks_after_outer_lease_drop() {
        let test_root = std::env::temp_dir().join(format!(
            "heyfood-vault-operation-retention-{}",
            Uuid::new_v4()
        ));
        let vault = HouseholdVault::open(
            &test_root.join("data"),
            AccountId::parse("acct_example_01").unwrap(),
        )
        .unwrap();
        let lifecycle_lease = vault
            .acquire_lifecycle_lease(CancellationToken::new())
            .await
            .unwrap();
        let vault_lease = vault
            .acquire_vault_lease(
                lifecycle_lease,
                HouseholdVaultLeaseModeV1::CreateIfMissing,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let detached_operation = vault_lease
            .acquire_operation(&CancellationToken::new())
            .await
            .unwrap();
        drop(vault_lease);

        let cancellation = CancellationToken::new();
        let trigger = cancellation.clone();
        let waiting_vault = vault.clone();
        let waiter =
            tokio::spawn(async move { waiting_vault.acquire_lifecycle_lease(cancellation).await });
        tokio::time::sleep(Duration::from_millis(40)).await;
        trigger.cancel();
        assert_eq!(
            waiter.await.unwrap().unwrap_err().code,
            "household_operation_cancelled"
        );

        drop(detached_operation);
        drop(
            vault
                .acquire_lifecycle_lease(CancellationToken::new())
                .await
                .unwrap(),
        );
        let _ = std::fs::remove_dir_all(test_root);
    }

    #[test]
    fn full_width_identity_vectors_match_the_frozen_contract() {
        let account = AccountId::parse("acct_example_01").unwrap();
        let macos = HouseholdAccountSlotV1::from_root_bytes(
            &account,
            NativeRootPlatformV1::Macos,
            b"/Users/alice/Library/Application Support/ai.frntr.heyfood",
        )
        .unwrap();
        assert_eq!(
            lower_hex(&macos.account_digest),
            "2fac3a067b2de70732b3ce5846d8acd3ae98e700748a37bafe31b00bbeb5909b"
        );
        assert_eq!(
            lower_hex(&macos.native_root_instance_digest),
            "61c1a73e0f6dc4059111ba62a9c1f79bf06da4e65f90bcbc0da0cba6dab13a9a"
        );
        assert_eq!(
            lower_hex(&macos.account_locator_digest),
            "91ea4f9a8ba072d501475d70042ae061555ec7995995f3d995230c7844a39420"
        );
        assert_eq!(macos.directory_name.len(), 64);

        let linux = HouseholdAccountSlotV1::from_root_bytes(
            &account,
            NativeRootPlatformV1::Linux,
            b"/home/alice/.local/share/heyfood",
        )
        .unwrap();
        assert_eq!(linux.account_digest, macos.account_digest);
        assert_eq!(
            lower_hex(&linux.native_root_instance_digest),
            "eca9baf8e73318a57e522116993dddf48f7dcc833b89d12123e4bd424ac39ad8"
        );
        assert_eq!(
            lower_hex(&linux.account_locator_digest),
            "3ebdb4e0de17178d13fb15aa5295258afd7ea3b3e5d69b1efa358c2e18b3fbaa"
        );
    }

    #[test]
    fn header_and_aad_vectors_are_literal_and_big_endian() {
        let key_id =
            KeyId::from_uuid(Uuid::parse_str("00112233-4455-6677-8899-aabbccddeeff").unwrap());
        let commit = Uuid::parse_str("01234567-89ab-4def-8123-456789abcdef").unwrap();
        let generation = VaultEnvelopeHeaderV1::generation(
            2,
            key_id,
            42,
            commit,
            std::array::from_fn(|index| u8::try_from(index).unwrap()),
            32,
        )
        .unwrap();
        assert_eq!(
            lower_hex(&generation.encode()),
            "48465641554c5431000100020001000100112233445566778899aabbccddeeff000000000000002a0123456789ab4def8123456789abcdef000102030405060708090a0b0c0d0e0f101112131415161700000020"
        );
        let journal = VaultEnvelopeHeaderV1::journal(
            key_id,
            7,
            commit,
            std::array::from_fn(|index| u8::try_from(index + 24).unwrap()),
            48,
        )
        .unwrap();
        assert_eq!(
            lower_hex(&journal.encode()),
            "48465641554c5431000101ff0001000100112233445566778899aabbccddeeff00000000000000070123456789ab4def8123456789abcdef18191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f00000030"
        );

        let account = AccountId::parse("acct_example_01").unwrap();
        let slot = HouseholdAccountSlotV1::from_root_bytes(
            &account,
            NativeRootPlatformV1::Macos,
            b"/Users/alice/Library/Application Support/ai.frntr.heyfood",
        )
        .unwrap();
        assert_eq!(
            lower_hex(&household_vault_aad_v1(&account, &slot, generation).unwrap()),
            "686579666f6f642e686f757365686f6c642e7661756c742e6161642e76310000000f616363745f6578616d706c655f30312fac3a067b2de70732b3ce5846d8acd3ae98e700748a37bafe31b00bbeb5909b61c1a73e0f6dc4059111ba62a9c1f79bf06da4e65f90bcbc0da0cba6dab13a9a48465641554c5431000100020001000100112233445566778899aabbccddeeff000000000000002a0123456789ab4def8123456789abcdef000102030405060708090a0b0c0d0e0f101112131415161700000020"
        );
        assert_eq!(
            lower_hex(&household_vault_aad_v1(&account, &slot, journal).unwrap()),
            "686579666f6f642e686f757365686f6c642e7661756c742e6161642e76310000000f616363745f6578616d706c655f30312fac3a067b2de70732b3ce5846d8acd3ae98e700748a37bafe31b00bbeb5909b61c1a73e0f6dc4059111ba62a9c1f79bf06da4e65f90bcbc0da0cba6dab13a9a48465641554c5431000101ff0001000100112233445566778899aabbccddeeff00000000000000070123456789ab4def8123456789abcdef18191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f00000030"
        );
    }

    #[test]
    fn hkdf_subkey_vectors_match_the_frozen_contract() {
        let root = HouseholdKeyMaterial::from_bytes(std::array::from_fn(|index| {
            u8::try_from(index).unwrap()
        }));
        assert_eq!(
            lower_hex(
                derive_subkey(&root, VaultArtifactKindV1::Generation)
                    .unwrap()
                    .as_slice()
            ),
            "61e6bd9a668675016d2e4988740979177cc53599fb0504fab232abb63668365b"
        );
        assert_eq!(
            lower_hex(
                derive_subkey(&root, VaultArtifactKindV1::Journal)
                    .unwrap()
                    .as_slice()
            ),
            "17b17c6f07db7115736b390583806d1f876965b2abb64c353a73d762d5cd5603"
        );
    }

    #[test]
    fn header_decoder_rejects_trailing_and_reserved_values() {
        let mut header = decode_hex::<84>(
            "48465641554c5431000100020001000100112233445566778899aabbccddeeff000000000000002a0123456789ab4def8123456789abcdef000102030405060708090a0b0c0d0e0f101112131415161700000020",
        );
        assert!(VaultEnvelopeHeaderV1::decode(&header).is_ok());
        header[10] = 2;
        assert!(VaultEnvelopeHeaderV1::decode(&header).is_err());
        let mut trailing = header.to_vec();
        trailing.push(0);
        assert!(VaultEnvelopeHeaderV1::decode(&trailing).is_err());
    }
}
