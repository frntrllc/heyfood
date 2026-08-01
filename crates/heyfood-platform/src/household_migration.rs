//! Guard-bound legacy-Python to native-household startup transaction.
//!
//! This coordinator is the only production path allowed to turn an absent or
//! `initializing` migration guard into a committed native repository. It holds
//! the account lifecycle, exact legacy-source locks, and vault lock in the D2
//! order and performs no network operation.

use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use heyfood_application::{
    BoxFuture, HouseholdLoad, NativeHouseholdCompletionModeV1, NativeHouseholdModeV1, PortError,
};
use heyfood_core::{AccountId, CanonicalTimestampV1, DisplayName, LegacySourceIdentityV1};
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::credential_broker::{
    HouseholdMigrationGuardDocument, HouseholdMigrationGuardStateV1, HouseholdMigrationGuardStore,
    HouseholdMigrationInitializationPhaseV1, HouseholdMigrationRepairFailureCategoryV1,
    HouseholdMigrationSourceIdentityV1, HouseholdSecureStore, MigrationGuardExpectation,
};
use crate::household_repository::NativeHouseholdRepository;
use crate::household_vault::{
    HouseholdLifecycleLease, HouseholdVault, HouseholdVaultLeaseModeV1,
    HouseholdVaultStartupArtifactsV1,
};
use crate::python_import::{
    LegacyPythonConfigKindV1, LegacyPythonHouseholdMigrationV1, LegacyPythonKeyringProbeOutcomeV1,
    LegacyPythonPhaseAResultV1, LegacyPythonPhaseBContextV1,
    LegacyPythonVaultReadbackVerificationV1,
};

/// Local-only purpose-limited source broker used by migration composition.
///
/// Implementations may inspect only the closed current/legacy keyring target
/// selected by `config_kind` and `resolved_config_path`. No generic secret
/// read surface is admitted.
pub trait LegacyPythonHouseholdSourceBrokerV1: Send + Sync {
    fn probe_and_load<'a>(
        &'a self,
        lifecycle_lease: &'a HouseholdLifecycleLease,
        config_kind: LegacyPythonConfigKindV1,
        resolved_config_path: &'a Path,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<LegacyPythonKeyringProbeOutcomeV1, PortError>>;
}

#[cfg(feature = "native-credentials")]
impl LegacyPythonHouseholdSourceBrokerV1 for crate::HouseholdKeyBroker {
    fn probe_and_load<'a>(
        &'a self,
        lifecycle_lease: &'a HouseholdLifecycleLease,
        config_kind: LegacyPythonConfigKindV1,
        resolved_config_path: &'a Path,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<LegacyPythonKeyringProbeOutcomeV1, PortError>> {
        Box::pin(self.legacy_python_household_probe_and_load(
            lifecycle_lease,
            config_kind,
            resolved_config_path,
            cancellation,
        ))
    }
}

/// Immutable identity/time tuple generated only when an absent guard is about
/// to be reserved. Tests can inject a deterministic tuple; production uses
/// `generate`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeHouseholdMigrationReservationV1 {
    migration_frozen_at: CanonicalTimestampV1,
    migration_id: Uuid,
    initialization_id: Uuid,
    initial_commit_id: Uuid,
}

impl NativeHouseholdMigrationReservationV1 {
    pub fn new(
        migration_frozen_at: CanonicalTimestampV1,
        migration_id: Uuid,
        initialization_id: Uuid,
        initial_commit_id: Uuid,
    ) -> Result<Self, PortError> {
        if !is_canonical_uuid_v4(migration_id)
            || !is_canonical_uuid_v4(initialization_id)
            || !is_canonical_uuid_v4(initial_commit_id)
        {
            return Err(PortError::new(
                "household_migration_reservation",
                "native household migration identities are invalid",
            ));
        }
        Ok(Self {
            migration_frozen_at,
            migration_id,
            initialization_id,
            initial_commit_id,
        })
    }

    pub fn generate() -> Result<Self, PortError> {
        let migration_frozen_at = CanonicalTimestampV1::from_datetime(OffsetDateTime::from(
            SystemTime::now(),
        ))
        .map_err(|_| {
            PortError::new(
                "household_migration_clock",
                "native household migration time is unavailable",
            )
        })?;
        Self::new(
            migration_frozen_at,
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
        )
    }
}

/// Verified completion returned only after authenticated vault readback and
/// exact snapshot retirement.
#[derive(Clone, Debug)]
pub struct NativeHouseholdMigrationCompletionV1 {
    repository: NativeHouseholdRepository,
    verification: LegacyPythonVaultReadbackVerificationV1,
    mode: NativeHouseholdModeV1,
}

impl NativeHouseholdMigrationCompletionV1 {
    #[must_use]
    pub const fn mode(&self) -> NativeHouseholdModeV1 {
        self.mode
    }

    #[must_use]
    pub fn repository(&self) -> &NativeHouseholdRepository {
        &self.repository
    }

    #[must_use]
    pub const fn verification(&self) -> &LegacyPythonVaultReadbackVerificationV1 {
        &self.verification
    }

    #[must_use]
    pub fn into_repository(self) -> NativeHouseholdRepository {
        self.repository
    }
}

/// Verified zero-source-read completion for a canonical initialization
/// generation that committed, or began committing, before startup crashed.
#[derive(Clone, Debug)]
pub struct NativeHouseholdArtifactResumeCompletionV1 {
    repository: NativeHouseholdRepository,
    readback: HouseholdLoad,
    mode: NativeHouseholdModeV1,
}

impl NativeHouseholdArtifactResumeCompletionV1 {
    #[must_use]
    pub const fn mode(&self) -> NativeHouseholdModeV1 {
        self.mode
    }

    #[must_use]
    pub fn repository(&self) -> &NativeHouseholdRepository {
        &self.repository
    }

    #[must_use]
    pub const fn readback(&self) -> &HouseholdLoad {
        &self.readback
    }

    #[must_use]
    pub fn into_repository(self) -> NativeHouseholdRepository {
        self.repository
    }
}

/// Complete a new or resumable native-household initialization with a
/// production reservation generated lazily only if no guard exists.
#[allow(clippy::too_many_arguments)]
pub async fn complete_native_household_initialization_v1(
    vault: &HouseholdVault,
    account: &AccountId,
    secure_store: Arc<dyn HouseholdSecureStore>,
    source_broker: &dyn LegacyPythonHouseholdSourceBrokerV1,
    migration: &LegacyPythonHouseholdMigrationV1,
    owner_display_name: DisplayName,
    completion: NativeHouseholdCompletionModeV1,
    cancellation: CancellationToken,
) -> Result<NativeHouseholdMigrationCompletionV1, PortError> {
    complete_native_household_initialization_with_reservation_v1(
        vault,
        account,
        secure_store,
        source_broker,
        migration,
        owner_display_name,
        completion,
        NativeHouseholdMigrationReservationV1::generate,
        cancellation,
    )
    .await
}

/// Resume only from authenticated native artifacts and persisted snapshot
/// provenance. This path has no source-broker parameter by construction and
/// never reads either legacy config file or historical keyring target.
pub async fn resume_native_household_artifacts_v1(
    vault: &HouseholdVault,
    account: &AccountId,
    secure_store: Arc<dyn HouseholdSecureStore>,
    migration: &LegacyPythonHouseholdMigrationV1,
    completion: NativeHouseholdCompletionModeV1,
    cancellation: CancellationToken,
) -> Result<NativeHouseholdArtifactResumeCompletionV1, PortError> {
    check_cancelled(&cancellation)?;
    if vault.account_id() != account {
        return Err(PortError::new(
            "household_account_mismatch",
            "native household resume belongs to another account",
        ));
    }
    let lifecycle = vault
        .acquire_lifecycle_lease(cancellation.child_token())
        .await?;
    if crate::household_teardown_barrier_present_v1(vault, &lifecycle)? {
        return Err(PortError::new(
            "household_account_teardown_in_progress",
            "native household resume is blocked by account teardown",
        ));
    }
    let mut snapshot_lease = migration
        .acquire_snapshot_retirement_lease(lifecycle, cancellation.child_token())
        .await?;
    let lifecycle = migration.take_snapshot_lifecycle_for_vault(&mut snapshot_lease)?;
    let mut vault_lease = vault
        .acquire_vault_lease(
            lifecycle,
            HouseholdVaultLeaseModeV1::RequireExisting,
            cancellation.child_token(),
        )
        .await?;
    let guard = HouseholdMigrationGuardStore::load(
        secure_store.as_ref(),
        vault_lease.lifecycle_lease(),
        cancellation.child_token(),
    )
    .await?
    .ok_or_else(|| {
        PortError::new(
            "household_initialization_protocol_required",
            "native household artifact resume requires its migration guard",
        )
    })?;
    guard.validate_for(vault.account_slot())?;
    let key = crate::HouseholdKeyStore::load(
        secure_store.as_ref(),
        vault_lease.lifecycle_lease(),
        cancellation.child_token(),
    )
    .await?;
    let mode = completion_mode(completion);
    let repository = NativeHouseholdRepository::from_vault(
        account.clone(),
        vault.clone(),
        secure_store.clone(),
        mode,
    )?;

    let readback = match guard.state() {
        HouseholdMigrationGuardStateV1::Initializing
            if guard.initialization_phase()
                == Some(HouseholdMigrationInitializationPhaseV1::ReadyToInitialize) =>
        {
            let key = key.ok_or_else(|| {
                PortError::new(
                    "household_initialization_source_resume_required",
                    "ready native household initialization has no authenticated artifact key",
                )
            })?;
            let topology = vault
                .classify_startup_artifacts(
                    &mut vault_lease,
                    Some(key),
                    Some(guard.initial_commit_id()),
                    guard.initial_state_digest(),
                    cancellation.child_token(),
                )
                .await?;
            match topology {
                HouseholdVaultStartupArtifactsV1::MatchingUncommitted => {
                    repository
                        .resume_uncommitted_initialization_with_retained_leases(
                            &mut vault_lease,
                            cancellation.child_token(),
                        )
                        .await?
                }
                HouseholdVaultStartupArtifactsV1::MatchingCommitted => {
                    repository
                        .finalize_committed_initialization_with_retained_leases(
                            &mut vault_lease,
                            cancellation.child_token(),
                        )
                        .await?
                }
                HouseholdVaultStartupArtifactsV1::Absent => {
                    return Err(PortError::new(
                        "household_initialization_source_resume_required",
                        "ready native household initialization has no committed source-independent artifact",
                    ));
                }
            }
        }
        HouseholdMigrationGuardStateV1::Migrated
        | HouseholdMigrationGuardStateV1::InitializedNoSource => {
            repository
                .load_committed_with_retained_leases(&mut vault_lease, cancellation.child_token())
                .await?
        }
        _ => {
            return Err(PortError::new(
                "household_initialization_protocol_required",
                "native household artifacts are not in a resumable initialization state",
            ));
        }
    };
    let committed_guard = HouseholdMigrationGuardStore::load(
        secure_store.as_ref(),
        vault_lease.lifecycle_lease(),
        CancellationToken::new(),
    )
    .await?
    .ok_or_else(|| {
        PortError::uncertain(
            "household_migration_guard_missing",
            "native household artifact finalization requires reconciliation",
        )
    })?;
    let authority = migration.committed_snapshot_retirement_authority(
        &snapshot_lease,
        &vault_lease,
        &committed_guard,
        &readback,
    )?;
    migration
        .retire_committed_snapshot(&snapshot_lease, &authority, CancellationToken::new())
        .await?;

    Ok(NativeHouseholdArtifactResumeCompletionV1 {
        repository,
        readback,
        mode,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn complete_native_household_initialization_with_reservation_v1<Factory>(
    vault: &HouseholdVault,
    account: &AccountId,
    secure_store: Arc<dyn HouseholdSecureStore>,
    source_broker: &dyn LegacyPythonHouseholdSourceBrokerV1,
    migration: &LegacyPythonHouseholdMigrationV1,
    owner_display_name: DisplayName,
    completion: NativeHouseholdCompletionModeV1,
    reservation_factory: Factory,
    cancellation: CancellationToken,
) -> Result<NativeHouseholdMigrationCompletionV1, PortError>
where
    Factory: FnOnce() -> Result<NativeHouseholdMigrationReservationV1, PortError>,
{
    check_cancelled(&cancellation)?;
    if vault.account_id() != account {
        return Err(PortError::new(
            "household_account_mismatch",
            "native household migration belongs to another account",
        ));
    }
    let lifecycle = vault
        .acquire_lifecycle_lease(cancellation.child_token())
        .await?;
    if crate::household_teardown_barrier_present_v1(vault, &lifecycle)? {
        return Err(PortError::new(
            "household_account_teardown_in_progress",
            "native household migration is blocked by account teardown",
        ));
    }
    let existing_guard = HouseholdMigrationGuardStore::load(
        secure_store.as_ref(),
        &lifecycle,
        cancellation.child_token(),
    )
    .await?;
    if existing_guard
        .as_ref()
        .is_some_and(|guard| guard.state() != HouseholdMigrationGuardStateV1::Initializing)
    {
        return Err(PortError::new(
            "household_initialization_protocol_required",
            "native household migration requires an initializing guard",
        ));
    }

    let mut source_lease = migration
        .acquire_source_lease(lifecycle, cancellation.child_token())
        .await?;
    let source_lifecycle = migration.lifecycle_for_phase_a(&source_lease)?;
    let current = source_broker
        .probe_and_load(
            source_lifecycle,
            LegacyPythonConfigKindV1::Current,
            migration.config_path(LegacyPythonConfigKindV1::Current),
            cancellation.child_token(),
        )
        .await?;
    let legacy = source_broker
        .probe_and_load(
            source_lifecycle,
            LegacyPythonConfigKindV1::Legacy,
            migration.config_path(LegacyPythonConfigKindV1::Legacy),
            cancellation.child_token(),
        )
        .await?;
    let probes = migration.bind_keyring_probes(vault.account_slot(), current, legacy)?;
    let phase_a_result = migration
        .phase_a(
            account,
            vault.account_slot(),
            &source_lease,
            &probes,
            cancellation.child_token(),
        )
        .await;
    let phase_a = match phase_a_result {
        Ok(phase_a) => phase_a,
        Err(error) => {
            if let (Some(guard), Some(failure)) = (
                existing_guard.as_ref(),
                deterministic_phase_a_failure(&error),
            ) {
                let lifecycle = migration.take_lifecycle_for_vault(&mut source_lease)?;
                let mut vault_lease = vault
                    .acquire_vault_lease(
                        lifecycle,
                        HouseholdVaultLeaseModeV1::CreateIfMissing,
                        CancellationToken::new(),
                    )
                    .await?;
                vault
                    .abort_invalid_initialization_to_blocked_repair(
                        &mut vault_lease,
                        secure_store.as_ref(),
                        guard.initialization_id(),
                        None,
                        failure,
                        CancellationToken::new(),
                    )
                    .await?;
            }
            return Err(error);
        }
    };

    let (reserved, source_mismatch) = match existing_guard {
        Some(guard) => {
            let mismatch = require_reserved_or_ready_source(&guard, &phase_a).err();
            (guard, mismatch)
        }
        None => {
            let reservation = reservation_factory()?;
            (
                HouseholdMigrationGuardDocument::initializing_reserved_with_snapshot(
                    vault.account_slot(),
                    migration_source_identity(&phase_a)?,
                    phase_a.snapshot_provenance(),
                    reservation.migration_id,
                    reservation.initialization_id,
                    reservation.initial_commit_id,
                    reservation.migration_frozen_at,
                )?,
                None,
            )
        }
    };

    // The source lease no longer borrows the lifecycle after acquisition; it
    // remains held while the lifecycle authority moves into the narrower
    // vault lease.
    let lifecycle = migration.take_lifecycle_for_vault(&mut source_lease)?;
    let mut vault_lease = vault
        .acquire_vault_lease(
            lifecycle,
            HouseholdVaultLeaseModeV1::CreateIfMissing,
            cancellation.child_token(),
        )
        .await?;
    if let Some(error) = source_mismatch {
        vault
            .abort_invalid_initialization_to_blocked_repair(
                &mut vault_lease,
                secure_store.as_ref(),
                reserved.initialization_id(),
                None,
                HouseholdMigrationRepairFailureCategoryV1::SourceChanged,
                CancellationToken::new(),
            )
            .await?;
        return Err(error);
    }
    let reserved = if reserved.guard_revision() == 1
        && HouseholdMigrationGuardStore::load(
            secure_store.as_ref(),
            vault_lease.lifecycle_lease(),
            CancellationToken::new(),
        )
        .await?
        .is_none()
    {
        compare_exchange_guard_and_reconcile(
            secure_store.as_ref(),
            &mut vault_lease,
            MigrationGuardExpectation::Absent,
            None,
            reserved,
            cancellation.child_token(),
        )
        .await?
    } else {
        reserved
    };

    let context = LegacyPythonPhaseBContextV1::from_reserved_guard(
        &phase_a,
        vault.account_slot(),
        &reserved,
        owner_display_name,
    )?;
    let phase_b = match migration
        .phase_b(
            &phase_a,
            &context,
            vault.account_slot(),
            &vault_lease,
            &source_lease,
            &probes,
            cancellation.child_token(),
        )
        .await
    {
        Ok(result) => result,
        Err(error) => {
            if let Some(failure) = deterministic_phase_b_failure(&error) {
                vault
                    .abort_invalid_initialization_to_blocked_repair(
                        &mut vault_lease,
                        secure_store.as_ref(),
                        reserved.initialization_id(),
                        None,
                        failure,
                        CancellationToken::new(),
                    )
                    .await?;
            }
            return Err(error);
        }
    };
    let resolved = phase_b.resolve_initialization()?;
    let ready = match reserved.initialization_phase() {
        Some(HouseholdMigrationInitializationPhaseV1::ReservedSource) => {
            let ready_candidate = reserved.ready_to_initialize(
                *resolved.initial_effect_fingerprint.as_digest().as_bytes(),
                *resolved.canonical_state_digest.as_bytes(),
            )?;
            compare_exchange_guard_and_reconcile(
                secure_store.as_ref(),
                &mut vault_lease,
                MigrationGuardExpectation::Revision(reserved.guard_revision()),
                Some(reserved.clone()),
                ready_candidate,
                cancellation.child_token(),
            )
            .await?
        }
        Some(HouseholdMigrationInitializationPhaseV1::ReadyToInitialize)
            if reserved.initial_effect_fingerprint()
                == Some(*resolved.initial_effect_fingerprint.as_digest().as_bytes())
                && reserved.initial_state_digest()
                    == Some(*resolved.canonical_state_digest.as_bytes()) =>
        {
            reserved
        }
        _ => {
            return Err(PortError::new(
                "household_initialization_guard_mismatch",
                "native household initialization guard does not match its phase-B result",
            ));
        }
    };
    debug_assert_eq!(
        ready.initial_state_digest(),
        Some(*resolved.canonical_state_digest.as_bytes())
    );

    let mode = completion_mode(completion);
    let repository =
        NativeHouseholdRepository::from_vault(account.clone(), vault.clone(), secure_store, mode)?;
    repository
        .initialize_with_retained_leases(
            resolved.command.clone(),
            &mut vault_lease,
            cancellation.child_token(),
        )
        .await?;
    let readback = repository
        .load_committed_with_retained_leases(&mut vault_lease, CancellationToken::new())
        .await?;
    let verification = phase_b.verify_vault_readback(&resolved.command, &readback.state)?;
    migration
        .retire_verified_snapshot(&source_lease, &verification, CancellationToken::new())
        .await?;
    drop(vault_lease);
    drop(source_lease);

    Ok(NativeHouseholdMigrationCompletionV1 {
        repository,
        verification,
        mode,
    })
}

async fn compare_exchange_guard_and_reconcile(
    secure_store: &dyn HouseholdSecureStore,
    vault_lease: &mut crate::HouseholdVaultLease,
    expected: MigrationGuardExpectation,
    prior: Option<HouseholdMigrationGuardDocument>,
    replacement: HouseholdMigrationGuardDocument,
    cancellation: CancellationToken,
) -> Result<HouseholdMigrationGuardDocument, PortError> {
    let exchange = HouseholdMigrationGuardStore::compare_exchange(
        secure_store,
        vault_lease,
        expected,
        Some(replacement.clone()),
        cancellation,
    )
    .await;
    let observed = HouseholdMigrationGuardStore::load(
        secure_store,
        vault_lease.lifecycle_lease(),
        CancellationToken::new(),
    )
    .await?;
    match (exchange, observed) {
        (_, Some(observed)) if observed == replacement => Ok(replacement),
        (Err(error), observed) if observed == prior => Err(error),
        _ => Err(PortError::uncertain(
            "household_migration_guard_cas",
            "native household migration guard transition requires reconciliation",
        )),
    }
}

fn require_reserved_or_ready_source(
    guard: &HouseholdMigrationGuardDocument,
    phase_a: &LegacyPythonPhaseAResultV1,
) -> Result<(), PortError> {
    let snapshot_provenance = phase_a.snapshot_provenance();
    if guard.source_identity() != &migration_source_identity(phase_a)?
        || guard.legacy_python_snapshot() != snapshot_provenance.as_ref()
    {
        return Err(PortError::new(
            "legacy_python_guard_reservation_mismatch",
            "legacy household source does not match its reserved migration guard",
        ));
    }
    if guard.state() != HouseholdMigrationGuardStateV1::Initializing
        || !matches!(
            guard.initialization_phase(),
            Some(
                HouseholdMigrationInitializationPhaseV1::ReservedSource
                    | HouseholdMigrationInitializationPhaseV1::ReadyToInitialize
            )
        )
    {
        return Err(PortError::new(
            "household_initialization_protocol_required",
            "native household migration guard is not resumable",
        ));
    }
    Ok(())
}

fn migration_source_identity(
    phase_a: &LegacyPythonPhaseAResultV1,
) -> Result<HouseholdMigrationSourceIdentityV1, PortError> {
    match phase_a.source_identity() {
        LegacySourceIdentityV1::Present {
            source_kind,
            source_digest,
        } if source_kind == "legacy_python_source_bundle_v1" => Ok(
            HouseholdMigrationSourceIdentityV1::present(*source_digest.as_bytes()),
        ),
        LegacySourceIdentityV1::NoSource {
            source_set_fingerprint,
        } => Ok(HouseholdMigrationSourceIdentityV1::no_source(
            *source_set_fingerprint.as_bytes(),
        )),
        LegacySourceIdentityV1::Present { .. } => Err(PortError::new(
            "legacy_python_source_identity",
            "legacy household source identity is unsupported",
        )),
    }
}

fn deterministic_phase_b_failure(
    error: &PortError,
) -> Option<HouseholdMigrationRepairFailureCategoryV1> {
    match error.code {
        "legacy_python_source_changed" => {
            Some(HouseholdMigrationRepairFailureCategoryV1::SourceChanged)
        }
        "legacy_python_semantic_validation" => {
            Some(HouseholdMigrationRepairFailureCategoryV1::SemanticValidation)
        }
        "legacy_python_canonical_construction" | "legacy_python_initialization_mismatch" => {
            Some(HouseholdMigrationRepairFailureCategoryV1::CanonicalConstruction)
        }
        _ => None,
    }
}

fn deterministic_phase_a_failure(
    error: &PortError,
) -> Option<HouseholdMigrationRepairFailureCategoryV1> {
    match error.code {
        "legacy_python_source_changed"
        | "legacy_python_source_conflict"
        | "legacy_household_source_unbound"
        | "legacy_python_account_mismatch" => {
            Some(HouseholdMigrationRepairFailureCategoryV1::SourceChanged)
        }
        "legacy_python_source_syntax"
        | "legacy_python_source_shape"
        | "legacy_python_keyring_format"
        | "python_snapshot_invalid" => {
            Some(HouseholdMigrationRepairFailureCategoryV1::SemanticValidation)
        }
        _ => None,
    }
}

fn is_canonical_uuid_v4(value: Uuid) -> bool {
    value.get_version_num() == 4
        && value.get_variant() == uuid::Variant::RFC4122
        && value != Uuid::nil()
}

fn check_cancelled(cancellation: &CancellationToken) -> Result<(), PortError> {
    if cancellation.is_cancelled() {
        Err(PortError::new(
            "household_operation_cancelled",
            "native household migration was cancelled",
        ))
    } else {
        Ok(())
    }
}

const fn completion_mode(completion: NativeHouseholdCompletionModeV1) -> NativeHouseholdModeV1 {
    match completion {
        NativeHouseholdCompletionModeV1::NativeEnabled => NativeHouseholdModeV1::NativeEnabled,
        NativeHouseholdCompletionModeV1::NativeRollbackReadOnly => {
            NativeHouseholdModeV1::NativeRollbackReadOnly
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::{
        HouseholdMigrationGuardStore, InMemoryHouseholdSecureStore, LegacyPythonConfigRootV1,
    };

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "heyfood-household-migration-{name}-{}",
                Uuid::new_v4()
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

    struct FixedSourceBroker {
        outcome: LegacyPythonKeyringProbeOutcomeV1,
        calls: AtomicUsize,
    }

    impl FixedSourceBroker {
        fn missing() -> Self {
            Self {
                outcome: LegacyPythonKeyringProbeOutcomeV1::AuthoritativeMissing,
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl LegacyPythonHouseholdSourceBrokerV1 for FixedSourceBroker {
        fn probe_and_load<'a>(
            &'a self,
            _lifecycle_lease: &'a HouseholdLifecycleLease,
            _config_kind: LegacyPythonConfigKindV1,
            _resolved_config_path: &'a Path,
            cancellation: CancellationToken,
        ) -> BoxFuture<'a, Result<LegacyPythonKeyringProbeOutcomeV1, PortError>> {
            Box::pin(async move {
                if cancellation.is_cancelled() {
                    return Err(PortError::new(
                        "household_operation_cancelled",
                        "test source broker was cancelled",
                    ));
                }
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(self.outcome.clone())
            })
        }
    }

    fn fixture(
        name: &str,
    ) -> (
        TempRoot,
        LegacyPythonHouseholdMigrationV1,
        AccountId,
        HouseholdVault,
        Arc<InMemoryHouseholdSecureStore>,
    ) {
        let root = TempRoot::new(name);
        let config_root = root.0.join("legacy-config");
        std::fs::create_dir_all(&config_root).unwrap();
        let migration = LegacyPythonHouseholdMigrationV1::new(
            LegacyPythonConfigRootV1::from_absolute_root(config_root).unwrap(),
            root.0.join("native").join("python-state-import.v1.json"),
        );
        let account = AccountId::parse(format!("acct-{name}")).unwrap();
        let vault = HouseholdVault::open(&root.0.join("vault"), account.clone()).unwrap();
        (
            root,
            migration,
            account,
            vault,
            Arc::new(InMemoryHouseholdSecureStore::default()),
        )
    }

    fn reservation() -> Result<NativeHouseholdMigrationReservationV1, PortError> {
        NativeHouseholdMigrationReservationV1::new(
            CanonicalTimestampV1::parse("2026-07-30T12:00:00.000Z").unwrap(),
            Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
            Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap(),
            Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap(),
        )
    }

    #[tokio::test]
    async fn no_source_initialization_is_deadlock_free_in_enabled_and_rollback_modes() {
        for (name, completion, expected_mode) in [
            (
                "enabled",
                NativeHouseholdCompletionModeV1::NativeEnabled,
                NativeHouseholdModeV1::NativeEnabled,
            ),
            (
                "rollback",
                NativeHouseholdCompletionModeV1::NativeRollbackReadOnly,
                NativeHouseholdModeV1::NativeRollbackReadOnly,
            ),
        ] {
            let (_root, migration, account, vault, store) = fixture(name);
            let broker = FixedSourceBroker::missing();
            store.inject_next_guard_cas_uncertain_after_commit();
            let completion = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                complete_native_household_initialization_with_reservation_v1(
                    &vault,
                    &account,
                    store,
                    &broker,
                    &migration,
                    DisplayName::parse("Me").unwrap(),
                    completion,
                    reservation,
                    CancellationToken::new(),
                ),
            )
            .await
            .expect("retained lifecycle/source/vault transaction deadlocked")
            .unwrap();
            assert_eq!(completion.mode(), expected_mode);
            assert_eq!(broker.calls.load(Ordering::SeqCst), 2);
            let load = completion
                .repository()
                .clone()
                .into_session(Arc::new(crate::NativeHouseholdMutationAuthorityV1::new()))
                .load_required(CancellationToken::new())
                .await
                .unwrap();
            assert_eq!(load.state.account_binding, account);
            assert!(load.state.members.is_empty());
            assert!(load.state.profiles.is_empty());
        }
    }

    #[tokio::test]
    async fn unavailable_or_cancelled_probe_never_creates_a_guard() {
        for (name, outcome, cancellation) in [
            (
                "unavailable",
                LegacyPythonKeyringProbeOutcomeV1::Unavailable,
                CancellationToken::new(),
            ),
            (
                "cancelled",
                LegacyPythonKeyringProbeOutcomeV1::AuthoritativeMissing,
                {
                    let token = CancellationToken::new();
                    token.cancel();
                    token
                },
            ),
        ] {
            let (_root, migration, account, vault, store) = fixture(name);
            let broker = FixedSourceBroker {
                outcome,
                calls: AtomicUsize::new(0),
            };
            assert!(
                complete_native_household_initialization_with_reservation_v1(
                    &vault,
                    &account,
                    store.clone(),
                    &broker,
                    &migration,
                    DisplayName::parse("Me").unwrap(),
                    NativeHouseholdCompletionModeV1::NativeEnabled,
                    reservation,
                    cancellation,
                )
                .await
                .is_err()
            );
            let lifecycle = vault
                .acquire_lifecycle_lease(CancellationToken::new())
                .await
                .unwrap();
            assert!(
                HouseholdMigrationGuardStore::load(
                    store.as_ref(),
                    &lifecycle,
                    CancellationToken::new(),
                )
                .await
                .unwrap()
                .is_none()
            );
        }
    }

    #[tokio::test]
    async fn exact_reserved_guard_resume_never_mints_a_second_identity_tuple() {
        let (_root, migration, account, vault, store) = fixture("exact-resume");
        let lifecycle = vault
            .acquire_lifecycle_lease(CancellationToken::new())
            .await
            .unwrap();
        let mut source_lease = migration
            .acquire_source_lease(lifecycle, CancellationToken::new())
            .await
            .unwrap();
        let probes = migration
            .authoritative_missing_keyring_probes(vault.account_slot())
            .unwrap();
        let phase_a = migration
            .phase_a(
                &account,
                vault.account_slot(),
                &source_lease,
                &probes,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let expected = reservation().unwrap();
        let guard = HouseholdMigrationGuardDocument::initializing_reserved_with_snapshot(
            vault.account_slot(),
            migration_source_identity(&phase_a).unwrap(),
            phase_a.snapshot_provenance(),
            expected.migration_id,
            expected.initialization_id,
            expected.initial_commit_id,
            expected.migration_frozen_at.clone(),
        )
        .unwrap();
        let lifecycle = migration
            .take_lifecycle_for_vault(&mut source_lease)
            .unwrap();
        let mut vault_lease = vault
            .acquire_vault_lease(
                lifecycle,
                HouseholdVaultLeaseModeV1::CreateIfMissing,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        HouseholdMigrationGuardStore::compare_exchange(
            store.as_ref(),
            &mut vault_lease,
            MigrationGuardExpectation::Absent,
            Some(guard.clone()),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        drop(vault_lease);
        drop(source_lease);

        let broker = FixedSourceBroker::missing();
        let completion = complete_native_household_initialization_with_reservation_v1(
            &vault,
            &account,
            store.clone(),
            &broker,
            &migration,
            DisplayName::parse("Me").unwrap(),
            NativeHouseholdCompletionModeV1::NativeEnabled,
            || panic!("resume must not mint another reservation"),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(completion.mode(), NativeHouseholdModeV1::NativeEnabled);
        let lifecycle = vault
            .acquire_lifecycle_lease(CancellationToken::new())
            .await
            .unwrap();
        let observed = HouseholdMigrationGuardStore::load(
            store.as_ref(),
            &lifecycle,
            CancellationToken::new(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(observed.migration_id(), guard.migration_id());
        assert_eq!(observed.initialization_id(), guard.initialization_id());
        assert_eq!(observed.initial_commit_id(), guard.initial_commit_id());
    }
}
