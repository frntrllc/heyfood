//! Lock-bound native-household startup evidence.
//!
//! This module is the only composition seam that combines the secure
//! migration guard, key bundle, encrypted vault artifacts, and the
//! globally-discoverable teardown barrier. It returns a closed application
//! evidence class and never exposes paths, ciphertext, keys, or household
//! values to the executable composition root.

use heyfood_application::{
    HouseholdLoad, NativeHouseholdEvidenceV1, NativeHouseholdInitializationPhaseV1, PortError,
};
use heyfood_core::{AccountId, AppliedCommitOutcomeV1, decode_canonical_household_state_v1};
use tokio_util::sync::CancellationToken;

use crate::household_vault::HouseholdVaultStartupArtifactsV1;
use crate::{
    HouseholdKeyBundle, HouseholdKeyBundlePhase, HouseholdKeyStore, HouseholdLifecycleLease,
    HouseholdMigrationGuardDocument, HouseholdMigrationGuardStateV1, HouseholdMigrationGuardStore,
    HouseholdMigrationInitializationPhaseV1, HouseholdSecureStore, HouseholdVault,
};

const TEARDOWN_DIRECTORY: &str = "household-teardown";
const TEARDOWN_PREFIX: &str = "teardown-";
const TEARDOWN_SUFFIX: &str = ".htj";
const MAX_TEARDOWN_JOURNAL_BYTES: u64 = 16 * 1024;

/// Exact account-bound evidence observed while the lifecycle lock is held.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClassifiedNativeHouseholdEvidenceV1 {
    pub teardown_journal_present: bool,
    pub evidence: NativeHouseholdEvidenceV1,
}

/// Nonmutating compatibility fast-path proof used only while the immutable
/// native-state floor is absent. Because the floor must commit before any D2
/// account artifact or teardown journal, an existing exact account directory
/// or journal is a contradiction rather than legacy-compatible absence.
pub fn pre_floor_native_account_provenance_absent_v1(
    vault: &HouseholdVault,
) -> Result<bool, PortError> {
    if path_exists_without_following(&vault.account_directory())? {
        return Ok(false);
    }
    let teardown_path = vault.native_root().join(TEARDOWN_DIRECTORY).join(format!(
        "{TEARDOWN_PREFIX}{}{TEARDOWN_SUFFIX}",
        lower_hex(&vault.account_slot().account_digest())
    ));
    Ok(!path_exists_without_following(&teardown_path)?)
}

/// Classify one authenticated account without performing migration, profile,
/// consent, or network work.
///
/// A teardown journal preempts ordinary mode selection. Otherwise guard and
/// key state are reloaded under `account-lifecycle.lock`; an existing vault
/// directory is then inspected only under `vault.lock`. Lock-only directories
/// with no guard, key, or encrypted artifacts do not become provenance.
pub async fn classify_native_household_evidence_v1(
    vault: &HouseholdVault,
    secure_store: &dyn HouseholdSecureStore,
    cancellation: CancellationToken,
) -> Result<ClassifiedNativeHouseholdEvidenceV1, PortError> {
    check_cancelled(&cancellation)?;
    let lifecycle = vault
        .acquire_lifecycle_lease(cancellation.child_token())
        .await?;
    let teardown_journal_present = scan_global_teardown_journals_v1(vault, &lifecycle)?;
    if teardown_journal_present {
        return Ok(ClassifiedNativeHouseholdEvidenceV1 {
            teardown_journal_present: true,
            evidence: NativeHouseholdEvidenceV1::Contradictory,
        });
    }

    let guard =
        HouseholdMigrationGuardStore::load(secure_store, &lifecycle, cancellation.child_token())
            .await?;
    check_cancelled(&cancellation)?;
    let key = HouseholdKeyStore::load(secure_store, &lifecycle, cancellation.child_token()).await?;
    if let Some(guard) = guard.as_ref() {
        guard.validate_for(vault.account_slot())?;
    }
    if let Some(key) = key.as_ref() {
        key.validate_for(vault.account_slot())?;
    }
    check_cancelled(&cancellation)?;

    let mut vault_lease = vault
        .acquire_existing_vault_lease_if_present(lifecycle, cancellation.child_token())
        .await?;
    let artifact_count = match vault_lease.as_mut() {
        Some(lease) => {
            vault
                .startup_artifact_count(lease, cancellation.child_token())
                .await?
        }
        None => 0,
    };

    let evidence = match guard.as_ref() {
        None => {
            if key.is_none() && artifact_count == 0 {
                NativeHouseholdEvidenceV1::NoNativeState
            } else {
                NativeHouseholdEvidenceV1::Contradictory
            }
        }
        Some(guard) => match guard.state() {
            HouseholdMigrationGuardStateV1::Initializing => {
                classify_initializing(
                    vault,
                    vault_lease.as_mut(),
                    guard,
                    key,
                    artifact_count,
                    &cancellation,
                )
                .await?
            }
            HouseholdMigrationGuardStateV1::Aborting => {
                classify_aborting(
                    vault,
                    vault_lease.as_mut(),
                    guard,
                    key,
                    artifact_count,
                    &cancellation,
                )
                .await?
            }
            HouseholdMigrationGuardStateV1::Migrated
            | HouseholdMigrationGuardStateV1::InitializedNoSource => {
                classify_committed(vault, vault_lease.as_mut(), guard, key, &cancellation).await?
            }
            HouseholdMigrationGuardStateV1::BlockedRepair => {
                if key.is_none() && artifact_count == 0 {
                    NativeHouseholdEvidenceV1::RepairBlocked
                } else {
                    NativeHouseholdEvidenceV1::Contradictory
                }
            }
            HouseholdMigrationGuardStateV1::BlockedAfterLogout => {
                if key.is_none() && artifact_count == 0 {
                    NativeHouseholdEvidenceV1::PostLogout
                } else {
                    NativeHouseholdEvidenceV1::Contradictory
                }
            }
        },
    };
    Ok(ClassifiedNativeHouseholdEvidenceV1 {
        teardown_journal_present: false,
        evidence,
    })
}

async fn classify_initializing(
    vault: &HouseholdVault,
    vault_lease: Option<&mut crate::HouseholdVaultLease>,
    guard: &HouseholdMigrationGuardDocument,
    key: Option<HouseholdKeyBundle>,
    artifact_count: u8,
    cancellation: &CancellationToken,
) -> Result<NativeHouseholdEvidenceV1, PortError> {
    let Some(phase) = guard.initialization_phase() else {
        return Ok(NativeHouseholdEvidenceV1::Contradictory);
    };
    match phase {
        HouseholdMigrationInitializationPhaseV1::ReservedSource => {
            if key.is_none() && artifact_count == 0 {
                Ok(NativeHouseholdEvidenceV1::ResumableInitialization {
                    phase: NativeHouseholdInitializationPhaseV1::ReservedSource,
                })
            } else {
                Ok(NativeHouseholdEvidenceV1::Contradictory)
            }
        }
        HouseholdMigrationInitializationPhaseV1::ReadyToInitialize => {
            let expected_commit_id = Some(guard.initial_commit_id());
            let expected_state_digest = guard.initial_state_digest();
            let Some(expected_state_digest) = expected_state_digest else {
                return Ok(NativeHouseholdEvidenceV1::Contradictory);
            };
            match key {
                None if artifact_count == 0 => {
                    Ok(NativeHouseholdEvidenceV1::ResumableInitialization {
                        phase: NativeHouseholdInitializationPhaseV1::ReadyToInitialize,
                    })
                }
                None => Ok(NativeHouseholdEvidenceV1::Contradictory),
                Some(key) => {
                    if !initializing_key_matches_guard(&key, guard)
                        && key.phase != HouseholdKeyBundlePhase::Stable
                    {
                        return Ok(NativeHouseholdEvidenceV1::Contradictory);
                    }
                    let Some(lease) = vault_lease else {
                        if artifact_count == 0 && key.phase == HouseholdKeyBundlePhase::Initializing
                        {
                            return Ok(NativeHouseholdEvidenceV1::ResumableInitialization {
                                phase: NativeHouseholdInitializationPhaseV1::ReadyToInitialize,
                            });
                        }
                        return Ok(NativeHouseholdEvidenceV1::Contradictory);
                    };
                    let artifacts = vault
                        .classify_startup_artifacts(
                            lease,
                            Some(key.clone()),
                            expected_commit_id,
                            Some(expected_state_digest),
                            cancellation.child_token(),
                        )
                        .await?;
                    match (key.phase, artifacts) {
                        (
                            HouseholdKeyBundlePhase::Initializing,
                            HouseholdVaultStartupArtifactsV1::Absent,
                        ) => Ok(NativeHouseholdEvidenceV1::ResumableInitialization {
                            phase: NativeHouseholdInitializationPhaseV1::ReadyToInitialize,
                        }),
                        (
                            HouseholdKeyBundlePhase::Initializing,
                            HouseholdVaultStartupArtifactsV1::MatchingUncommitted,
                        ) => Ok(NativeHouseholdEvidenceV1::ResumableInitialization {
                            phase: NativeHouseholdInitializationPhaseV1::UncommittedArtifacts,
                        }),
                        (
                            HouseholdKeyBundlePhase::Initializing | HouseholdKeyBundlePhase::Stable,
                            HouseholdVaultStartupArtifactsV1::MatchingCommitted,
                        ) => Ok(NativeHouseholdEvidenceV1::ResumableInitialization {
                            phase:
                                NativeHouseholdInitializationPhaseV1::CommittedAwaitingFinalization,
                        }),
                        _ => Ok(NativeHouseholdEvidenceV1::Contradictory),
                    }
                }
            }
        }
    }
}

async fn classify_aborting(
    vault: &HouseholdVault,
    vault_lease: Option<&mut crate::HouseholdVaultLease>,
    guard: &HouseholdMigrationGuardDocument,
    key: Option<HouseholdKeyBundle>,
    artifact_count: u8,
    cancellation: &CancellationToken,
) -> Result<NativeHouseholdEvidenceV1, PortError> {
    match key {
        None if artifact_count == 0 => Ok(NativeHouseholdEvidenceV1::AbortingCleanup),
        None => Ok(NativeHouseholdEvidenceV1::Contradictory),
        Some(key) if initializing_key_matches_guard(&key, guard) => {
            let Some(lease) = vault_lease else {
                return Ok(NativeHouseholdEvidenceV1::Contradictory);
            };
            let _ = vault
                .classify_startup_artifacts(
                    lease,
                    Some(key),
                    Some(guard.initial_commit_id()),
                    guard.initial_state_digest(),
                    cancellation.child_token(),
                )
                .await?;
            Ok(NativeHouseholdEvidenceV1::AbortingCleanup)
        }
        Some(_) => Ok(NativeHouseholdEvidenceV1::Contradictory),
    }
}

async fn classify_committed(
    vault: &HouseholdVault,
    vault_lease: Option<&mut crate::HouseholdVaultLease>,
    guard: &HouseholdMigrationGuardDocument,
    key: Option<HouseholdKeyBundle>,
    cancellation: &CancellationToken,
) -> Result<NativeHouseholdEvidenceV1, PortError> {
    let (Some(lease), Some(key)) = (vault_lease, key) else {
        return Ok(NativeHouseholdEvidenceV1::Contradictory);
    };
    if key.phase != HouseholdKeyBundlePhase::Stable {
        return Ok(NativeHouseholdEvidenceV1::Contradictory);
    }
    let loaded = vault.load(lease, key, cancellation.child_token()).await?;
    let state =
        decode_canonical_household_state_v1(&loaded.canonical_state).map_err(state_error)?;
    if state.account_binding != vault_account(vault)
        || state.revision.get() != loaded.state_revision
        || !state.bounded_applied_commits.iter().any(|record| {
            record.commit_id.as_uuid() == loaded.commit_id
                && record.resulting_revision == state.revision
        })
    {
        return Ok(NativeHouseholdEvidenceV1::Contradictory);
    }
    let load = HouseholdLoad::from_state(state)?;
    if load.state_digest.as_bytes() != &loaded.plaintext_sha256()
        || !guard_matches_state_provenance(guard, &load.state)?
    {
        return Ok(NativeHouseholdEvidenceV1::Contradictory);
    }
    Ok(NativeHouseholdEvidenceV1::ValidCommitted)
}

fn vault_account(vault: &HouseholdVault) -> AccountId {
    vault.account_id().clone()
}

fn guard_matches_state_provenance(
    guard: &HouseholdMigrationGuardDocument,
    state: &heyfood_core::HouseholdStateV1,
) -> Result<bool, PortError> {
    if state.migration_provenance.initialization_id != guard.initialization_id()
        || state.migration_provenance.initial_commit_id.as_uuid() != guard.initial_commit_id()
    {
        return Ok(false);
    }
    let Some(expected_fingerprint) = guard.initial_effect_fingerprint() else {
        return Ok(false);
    };
    let Some(initial_record) = state
        .bounded_applied_commits
        .iter()
        .find(|record| record.commit_id.as_uuid() == guard.initial_commit_id())
    else {
        return Ok(false);
    };
    if initial_record.outcome != AppliedCommitOutcomeV1::Initialized
        || initial_record.resulting_revision.get() != 1
        || !constant_time_eq_32(initial_record.fingerprint.as_bytes(), &expected_fingerprint)
    {
        return Ok(false);
    }
    let guard_value: serde_json::Value =
        serde_json::from_slice(&guard.canonical_bytes()?).map_err(state_error)?;
    let state_source =
        serde_json::to_value(&state.migration_provenance.source_identity).map_err(state_error)?;
    let state_migration_id =
        serde_json::to_value(state.migration_provenance.migration_id).map_err(state_error)?;
    let state_frozen_at = serde_json::to_value(&state.migration_provenance.migration_frozen_at)
        .map_err(state_error)?;
    Ok(guard_value.get("source_identity") == Some(&state_source)
        && guard_value.get("migration_id") == Some(&state_migration_id)
        && guard_value.get("migration_frozen_at") == Some(&state_frozen_at))
}

fn constant_time_eq_32(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn initializing_key_matches_guard(
    key: &HouseholdKeyBundle,
    guard: &HouseholdMigrationGuardDocument,
) -> bool {
    key.phase == HouseholdKeyBundlePhase::Initializing
        && key.initialization_id == Some(guard.initialization_id())
        && key.initial_commit_id == Some(guard.initial_commit_id())
        && key.initial_effect_fingerprint == guard.initial_effect_fingerprint()
        && key.initial_state_digest == guard.initial_state_digest()
}

/// Authoritative per-account teardown write barrier.
///
/// Any exact journal file blocks startup/commit even before its content can be
/// resumed. This check must run while the matching lifecycle lease is held, so
/// another conforming process cannot create or remove the journal between the
/// check and the protected operation.
pub fn household_teardown_barrier_present_v1(
    vault: &HouseholdVault,
    lifecycle: &HouseholdLifecycleLease,
) -> Result<bool, PortError> {
    lifecycle.validate_for(vault.account_slot())?;
    let directory = vault.native_root().join(TEARDOWN_DIRECTORY);
    let root_metadata =
        std::fs::symlink_metadata(vault.native_root()).map_err(|_| teardown_unavailable())?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(teardown_unavailable());
    }
    #[cfg(unix)]
    let expected_owner = {
        use std::os::unix::fs::MetadataExt as _;
        Some(root_metadata.uid())
    };
    #[cfg(not(unix))]
    let expected_owner = None;
    match std::fs::symlink_metadata(&directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(teardown_unavailable()),
        Ok(metadata) => validate_private_directory(&metadata, expected_owner)?,
    }
    let filename = format!(
        "{TEARDOWN_PREFIX}{}{TEARDOWN_SUFFIX}",
        lower_hex(&vault.account_slot().account_digest())
    );
    let path = directory.join(filename);
    let metadata = match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(teardown_unavailable()),
        Ok(metadata) => metadata,
    };
    validate_private_file(&metadata, expected_owner)?;
    if metadata.len() == 0 || metadata.len() > MAX_TEARDOWN_JOURNAL_BYTES {
        return Err(teardown_unavailable());
    }
    lifecycle.validate_for(vault.account_slot())?;
    Ok(true)
}

fn scan_global_teardown_journals_v1(
    vault: &HouseholdVault,
    lifecycle: &HouseholdLifecycleLease,
) -> Result<bool, PortError> {
    lifecycle.validate_for(vault.account_slot())?;
    let directory = vault.native_root().join(TEARDOWN_DIRECTORY);
    let root_metadata =
        std::fs::symlink_metadata(vault.native_root()).map_err(|_| teardown_unavailable())?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(teardown_unavailable());
    }
    #[cfg(unix)]
    let expected_owner = {
        use std::os::unix::fs::MetadataExt as _;
        Some(root_metadata.uid())
    };
    #[cfg(not(unix))]
    let expected_owner = None;
    let directory_metadata = match std::fs::symlink_metadata(&directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(teardown_unavailable()),
        Ok(metadata) => metadata,
    };
    validate_private_directory(&directory_metadata, expected_owner)?;

    let mut names = Vec::new();
    for entry in std::fs::read_dir(&directory).map_err(|_| teardown_unavailable())? {
        let entry = entry.map_err(|_| teardown_unavailable())?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| teardown_unavailable())?;
        names.push(name);
        if names.len() > 64 {
            return Err(teardown_unavailable());
        }
    }
    names.sort();
    let current_digest = lower_hex(&vault.account_slot().account_digest());
    let mut current_present = false;
    let mut seen = std::collections::BTreeSet::new();
    for name in names {
        let digest = name
            .strip_prefix(TEARDOWN_PREFIX)
            .and_then(|value| value.strip_suffix(TEARDOWN_SUFFIX))
            .filter(|value| {
                value.len() == 64
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
            .ok_or_else(teardown_unavailable)?;
        if !seen.insert(digest.to_owned()) {
            return Err(teardown_unavailable());
        }
        let metadata =
            std::fs::symlink_metadata(directory.join(&name)).map_err(|_| teardown_unavailable())?;
        validate_private_file(&metadata, expected_owner)?;
        if metadata.len() == 0 || metadata.len() > MAX_TEARDOWN_JOURNAL_BYTES {
            return Err(teardown_unavailable());
        }
        if digest == current_digest {
            current_present = true;
        } else {
            return Err(PortError::new(
                "household_teardown_resume_required",
                "another native household teardown must be resumed before account startup",
            ));
        }
    }
    lifecycle.validate_for(vault.account_slot())?;
    Ok(current_present)
}

fn validate_private_directory(
    metadata: &std::fs::Metadata,
    expected_owner: Option<u32>,
) -> Result<(), PortError> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(teardown_unavailable());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

        if Some(metadata.uid()) != expected_owner || metadata.permissions().mode() & 0o777 != 0o700
        {
            return Err(teardown_unavailable());
        }
    }
    #[cfg(not(unix))]
    let _ = expected_owner;
    Ok(())
}

fn validate_private_file(
    metadata: &std::fs::Metadata,
    expected_owner: Option<u32>,
) -> Result<(), PortError> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(teardown_unavailable());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

        if Some(metadata.uid()) != expected_owner || metadata.permissions().mode() & 0o777 != 0o600
        {
            return Err(teardown_unavailable());
        }
    }
    #[cfg(not(unix))]
    let _ = expected_owner;
    Ok(())
}

fn lower_hex(bytes: &[u8; 32]) -> String {
    let mut output = String::with_capacity(64);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn path_exists_without_following(path: &std::path::Path) -> Result<bool, PortError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(PortError::new(
            "household_native_evidence",
            "native household evidence is unavailable",
        )),
    }
}

fn check_cancelled(cancellation: &CancellationToken) -> Result<(), PortError> {
    if cancellation.is_cancelled() {
        Err(PortError::new(
            "household_operation_cancelled",
            "native household evidence classification was cancelled",
        ))
    } else {
        Ok(())
    }
}

fn state_error(error: impl std::fmt::Display) -> PortError {
    let _ = error;
    PortError::new(
        "household_state_invalid",
        "canonical household state is invalid",
    )
}

fn teardown_unavailable() -> PortError {
    PortError::new(
        "household_teardown_journal_invalid",
        "native household teardown evidence is invalid or unavailable",
    )
}
