//! D2 native-household startup composition.
//!
//! This seam runs only after authentication has identified the exact account
//! and before profile/consent or ordinary service traffic. It preserves the
//! released flag-off path only when the immutable native-state floor and all
//! account provenance are absent.

use std::sync::Arc;
#[cfg(feature = "native-credentials")]
use std::time::Duration;

use heyfood_application::{
    HouseholdSession, NativeHouseholdCompletionModeV1, NativeHouseholdInitializationPhaseV1,
    NativeHouseholdModeFactsV1, NativeHouseholdModeV1, PortError, resolve_native_household_mode_v1,
};
use heyfood_core::{AccountId, DisplayName, NativeHouseholdRolloutV1};
use heyfood_platform::{
    HouseholdSecureStore, HouseholdVault, LegacyPythonHouseholdMigrationV1,
    LegacyPythonHouseholdSourceBrokerV1, NativeHouseholdMutationAuthorityV1,
    NativeHouseholdRepository, NativePaths, NativeStateFloorStore,
    classify_native_household_evidence_v1, complete_native_household_initialization_v1,
    pre_floor_native_account_provenance_absent_v1, resume_native_household_artifacts_v1,
};
use tokio_util::sync::CancellationToken;

#[cfg(feature = "native-credentials")]
const HOUSEHOLD_BROKER_DEADLINE: Duration = Duration::from_secs(15);

struct OpenedHouseholdSecureStoreV1 {
    #[cfg(feature = "native-credentials")]
    store: Arc<dyn HouseholdSecureStore>,
    #[cfg(feature = "native-credentials")]
    broker: Arc<heyfood_platform::HouseholdKeyBroker>,
}

/// A usable startup composition. Legacy mode intentionally has no native
/// repository handle; committed native modes always carry one live,
/// account-bound session and cache no household state.
#[derive(Clone)]
pub struct PreparedNativeHouseholdV1 {
    mode: NativeHouseholdModeV1,
    household_session: Option<HouseholdSession>,
}

impl PreparedNativeHouseholdV1 {
    #[must_use]
    pub const fn mode(&self) -> NativeHouseholdModeV1 {
        self.mode
    }

    #[must_use]
    pub fn household_session(&self) -> Option<&HouseholdSession> {
        self.household_session.as_ref()
    }
}

impl std::fmt::Debug for PreparedNativeHouseholdV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedNativeHouseholdV1")
            .field("mode", &self.mode)
            .field(
                "household_session_present",
                &self.household_session.is_some(),
            )
            .finish()
    }
}

/// Startup rows that require an audited migration, cleanup, teardown, or
/// post-logout transition before any household view can be rendered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeHouseholdLifecycleRequiredV1 {
    pub mode: NativeHouseholdModeV1,
}

#[derive(Clone, Debug)]
pub enum NativeHouseholdCompositionV1 {
    Ready(PreparedNativeHouseholdV1),
    LifecycleRequired(NativeHouseholdLifecycleRequiredV1),
}

/// Parse the exact rollout switch without Unicode coercion.
pub fn native_household_rollout_from_environment_v1() -> Result<NativeHouseholdRolloutV1, PortError>
{
    NativeHouseholdRolloutV1::parse_environment_value(
        std::env::var_os("HEYFOOD_NATIVE_HOUSEHOLD_V1").as_deref(),
    )
    .map_err(|message| PortError::new("native_household_rollout", message))
}

/// Compose the live household source for one authenticated account.
///
/// This function performs no profile, consent, or hosted service operation.
/// It may create the immutable floor only for rollout `1`, after the
/// production secure-store probe succeeds. Existing native provenance always
/// bypasses the Python compatibility path under both rollout values.
pub async fn compose_native_household_v1(
    paths: &NativePaths,
    account: AccountId,
    rollout: NativeHouseholdRolloutV1,
    cancellation: CancellationToken,
) -> Result<NativeHouseholdCompositionV1, PortError> {
    check_cancelled(&cancellation)?;
    let root_present = native_root_present(paths.data_dir())?;
    if !root_present && !rollout.is_enabled() {
        return Ok(legacy_ready());
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    if root_present || rollout.is_enabled() {
        return Err(PortError::new(
            "household_secure_store_unavailable",
            "native household root identity is unavailable on this platform",
        ));
    }
    if !root_present {
        require_native_credentials_feature()?;
    }

    let vault = HouseholdVault::from_native_paths(paths, account.clone())?;
    let floor = NativeStateFloorStore::open(
        paths.data_dir(),
        vault.account_slot().native_root_instance_digest(),
    )?;
    let current_floor = floor.load(cancellation.child_token()).await?;
    if current_floor.is_none() && !rollout.is_enabled() {
        if pre_floor_native_account_provenance_absent_v1(&vault)? {
            return Ok(legacy_ready());
        }
        return Err(PortError::new(
            "household_native_evidence_contradiction",
            "native household account evidence exists without its compatibility floor",
        ));
    }

    let secure_store = open_secure_store(paths)?;
    if current_floor.is_none() {
        secure_store
            .ensure_native_floor(&floor, cancellation.child_token())
            .await?;
    }
    #[cfg(feature = "native-credentials")]
    {
        let migration = LegacyPythonHouseholdMigrationV1::discover(paths)?;
        let owner_display_name = DisplayName::parse("Me").map_err(|_| {
            PortError::new(
                "household_owner_display_name",
                "native household owner display name is invalid",
            )
        })?;
        return compose_verified_native_household_with_migration_v1(
            vault,
            account,
            rollout,
            secure_store.store,
            secure_store.broker.as_ref(),
            &migration,
            owner_display_name,
            cancellation,
        )
        .await;
    }
    #[cfg(not(feature = "native-credentials"))]
    {
        let _ = (vault, account, secure_store, cancellation);
        Err(secure_store_unavailable())
    }
}

/// Compose after the immutable floor and secure-store capability have already
/// been verified. This injection seam keeps production and deterministic
/// fake-store tests on the same lock-bound evidence classifier.
pub async fn compose_verified_native_household_v1(
    vault: HouseholdVault,
    account: AccountId,
    rollout: NativeHouseholdRolloutV1,
    secure_store: Arc<dyn HouseholdSecureStore>,
    cancellation: CancellationToken,
) -> Result<NativeHouseholdCompositionV1, PortError> {
    check_cancelled(&cancellation)?;
    let classified = classify_native_household_evidence_v1(
        &vault,
        secure_store.as_ref(),
        cancellation.child_token(),
    )
    .await?;
    let mode = resolve_native_household_mode_v1(
        rollout,
        NativeHouseholdModeFactsV1 {
            teardown_journal_present: classified.teardown_journal_present,
            evidence: classified.evidence,
        },
    )
    .map_err(|_| {
        PortError::new(
            "household_native_evidence_contradiction",
            "native household state evidence is contradictory",
        )
    })?;

    match mode {
        NativeHouseholdModeV1::LegacyCompatibility => Ok(legacy_ready()),
        NativeHouseholdModeV1::NativeEnabled | NativeHouseholdModeV1::NativeRollbackReadOnly => {
            let repository =
                NativeHouseholdRepository::from_vault(account, vault, secure_store, mode)?;
            Ok(NativeHouseholdCompositionV1::Ready(
                PreparedNativeHouseholdV1 {
                    mode,
                    household_session: Some(
                        repository
                            .into_session(Arc::new(NativeHouseholdMutationAuthorityV1::new())),
                    ),
                },
            ))
        }
        mode => Ok(NativeHouseholdCompositionV1::LifecycleRequired(
            NativeHouseholdLifecycleRequiredV1 { mode },
        )),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeHouseholdInitializationExecutionV1 {
    SourceMigration,
    AuthenticatedArtifacts,
}

const fn initialization_execution_v1(
    phase: NativeHouseholdInitializationPhaseV1,
) -> NativeHouseholdInitializationExecutionV1 {
    match phase {
        NativeHouseholdInitializationPhaseV1::ReservedSource
        | NativeHouseholdInitializationPhaseV1::ReadyToInitialize => {
            NativeHouseholdInitializationExecutionV1::SourceMigration
        }
        NativeHouseholdInitializationPhaseV1::UncommittedArtifacts
        | NativeHouseholdInitializationPhaseV1::CommittedAwaitingFinalization => {
            NativeHouseholdInitializationExecutionV1::AuthenticatedArtifacts
        }
    }
}

/// Production-equivalent lifecycle composition with injectable, purpose-
/// limited migration dependencies for deterministic tests.
///
/// The typed phase split makes the source broker unreachable once an
/// authenticated generation exists. Committed modes also take the
/// snapshot-only verifier on every startup, closing the crash gap between
/// guard finalization and exact released-snapshot retirement.
#[allow(clippy::too_many_arguments)]
pub async fn compose_verified_native_household_with_migration_v1(
    vault: HouseholdVault,
    account: AccountId,
    rollout: NativeHouseholdRolloutV1,
    secure_store: Arc<dyn HouseholdSecureStore>,
    source_broker: &dyn LegacyPythonHouseholdSourceBrokerV1,
    migration: &LegacyPythonHouseholdMigrationV1,
    owner_display_name: DisplayName,
    cancellation: CancellationToken,
) -> Result<NativeHouseholdCompositionV1, PortError> {
    check_cancelled(&cancellation)?;
    let classified = classify_native_household_evidence_v1(
        &vault,
        secure_store.as_ref(),
        cancellation.child_token(),
    )
    .await?;
    let mode = resolve_native_household_mode_v1(
        rollout,
        NativeHouseholdModeFactsV1 {
            teardown_journal_present: classified.teardown_journal_present,
            evidence: classified.evidence,
        },
    )
    .map_err(|_| {
        PortError::new(
            "household_native_evidence_contradiction",
            "native household state evidence is contradictory",
        )
    })?;

    match mode {
        NativeHouseholdModeV1::LegacyCompatibility => Ok(legacy_ready()),
        NativeHouseholdModeV1::NativeEnable => {
            let completed = complete_native_household_initialization_v1(
                &vault,
                &account,
                secure_store,
                source_broker,
                migration,
                owner_display_name,
                NativeHouseholdCompletionModeV1::NativeEnabled,
                cancellation,
            )
            .await?;
            Ok(native_ready(completed.mode(), completed.into_repository()))
        }
        NativeHouseholdModeV1::ResumeNativeInitialization { phase, completion } => {
            match initialization_execution_v1(phase) {
                NativeHouseholdInitializationExecutionV1::SourceMigration => {
                    let completed = complete_native_household_initialization_v1(
                        &vault,
                        &account,
                        secure_store,
                        source_broker,
                        migration,
                        owner_display_name,
                        completion,
                        cancellation,
                    )
                    .await?;
                    Ok(native_ready(completed.mode(), completed.into_repository()))
                }
                NativeHouseholdInitializationExecutionV1::AuthenticatedArtifacts => {
                    let completed = resume_native_household_artifacts_v1(
                        &vault,
                        &account,
                        secure_store,
                        migration,
                        completion,
                        cancellation,
                    )
                    .await?;
                    Ok(native_ready(completed.mode(), completed.into_repository()))
                }
            }
        }
        NativeHouseholdModeV1::NativeEnabled | NativeHouseholdModeV1::NativeRollbackReadOnly => {
            let completion = if mode == NativeHouseholdModeV1::NativeEnabled {
                NativeHouseholdCompletionModeV1::NativeEnabled
            } else {
                NativeHouseholdCompletionModeV1::NativeRollbackReadOnly
            };
            let completed = resume_native_household_artifacts_v1(
                &vault,
                &account,
                secure_store,
                migration,
                completion,
                cancellation,
            )
            .await?;
            Ok(native_ready(completed.mode(), completed.into_repository()))
        }
        mode => Ok(NativeHouseholdCompositionV1::LifecycleRequired(
            NativeHouseholdLifecycleRequiredV1 { mode },
        )),
    }
}

fn native_ready(
    mode: NativeHouseholdModeV1,
    repository: NativeHouseholdRepository,
) -> NativeHouseholdCompositionV1 {
    NativeHouseholdCompositionV1::Ready(PreparedNativeHouseholdV1 {
        mode,
        household_session: Some(
            repository.into_session(Arc::new(NativeHouseholdMutationAuthorityV1::new())),
        ),
    })
}

fn legacy_ready() -> NativeHouseholdCompositionV1 {
    NativeHouseholdCompositionV1::Ready(PreparedNativeHouseholdV1 {
        mode: NativeHouseholdModeV1::LegacyCompatibility,
        household_session: None,
    })
}

fn native_root_present(root: &std::path::Path) -> Result<bool, PortError> {
    match std::fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(PortError::new(
                "household_native_root",
                "native household root must be a physical directory",
            ))
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(PortError::new(
            "household_native_root",
            "native household root is unavailable",
        )),
    }
}

#[cfg(feature = "native-credentials")]
fn open_secure_store(paths: &NativePaths) -> Result<OpenedHouseholdSecureStoreV1, PortError> {
    let broker = Arc::new(heyfood_platform::HouseholdKeyBroker::from_native_paths(
        paths,
        HOUSEHOLD_BROKER_DEADLINE,
    )?);
    let store: Arc<dyn HouseholdSecureStore> = broker.clone();
    Ok(OpenedHouseholdSecureStoreV1 { store, broker })
}

#[cfg(not(feature = "native-credentials"))]
fn open_secure_store(_paths: &NativePaths) -> Result<OpenedHouseholdSecureStoreV1, PortError> {
    Err(secure_store_unavailable())
}

impl OpenedHouseholdSecureStoreV1 {
    #[cfg(feature = "native-credentials")]
    async fn ensure_native_floor(
        &self,
        floor: &NativeStateFloorStore,
        cancellation: CancellationToken,
    ) -> Result<(), PortError> {
        floor
            .ensure_after_secure_store_probe(cancellation, |probe_cancellation| {
                self.broker.probe(probe_cancellation)
            })
            .await
            .map(|_| ())
    }

    #[cfg(not(feature = "native-credentials"))]
    async fn ensure_native_floor(
        &self,
        _floor: &NativeStateFloorStore,
        _cancellation: CancellationToken,
    ) -> Result<(), PortError> {
        let _ = self;
        Err(secure_store_unavailable())
    }
}

fn require_native_credentials_feature() -> Result<(), PortError> {
    #[cfg(feature = "native-credentials")]
    {
        Ok(())
    }
    #[cfg(not(feature = "native-credentials"))]
    {
        Err(secure_store_unavailable())
    }
}

#[cfg(not(feature = "native-credentials"))]
fn secure_store_unavailable() -> PortError {
    PortError::new(
        "household_secure_store_unavailable",
        "native household secure storage is unavailable in this build",
    )
}

fn check_cancelled(cancellation: &CancellationToken) -> Result<(), PortError> {
    if cancellation.is_cancelled() {
        Err(PortError::new(
            "household_operation_cancelled",
            "native household startup was cancelled",
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialization_phase_routing_never_reopens_sources_after_artifacts_exist() {
        for phase in [
            NativeHouseholdInitializationPhaseV1::ReservedSource,
            NativeHouseholdInitializationPhaseV1::ReadyToInitialize,
        ] {
            assert_eq!(
                initialization_execution_v1(phase),
                NativeHouseholdInitializationExecutionV1::SourceMigration
            );
        }
        for phase in [
            NativeHouseholdInitializationPhaseV1::UncommittedArtifacts,
            NativeHouseholdInitializationPhaseV1::CommittedAwaitingFinalization,
        ] {
            assert_eq!(
                initialization_execution_v1(phase),
                NativeHouseholdInitializationExecutionV1::AuthenticatedArtifacts
            );
        }
    }
}
