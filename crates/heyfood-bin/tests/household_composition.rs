#[cfg(not(windows))]
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
#[cfg(not(windows))]
use std::sync::Arc;
#[cfg(not(windows))]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use heyfood_application::NativeHouseholdModeV1;
#[cfg(not(windows))]
use heyfood_application::{
    BoxFuture, HouseholdInitialize, HouseholdRepositoryResolutionV1, PortError,
    resolve_household_initialize_v1,
};
use heyfood_bin::native_household_composition::{
    NativeHouseholdCompositionV1, compose_native_household_v1,
};
#[cfg(not(windows))]
use heyfood_bin::native_household_composition::{
    compose_verified_native_household_v1, compose_verified_native_household_with_migration_v1,
};
use heyfood_core::{AccountId, NativeHouseholdRolloutV1};
#[cfg(not(windows))]
use heyfood_core::{
    CanonicalDigestV1, CanonicalTimestampV1, CommitId, DisplayName, HOUSEHOLD_STATE_SCHEMA_VERSION,
    HouseholdEffectV1, HouseholdOwnerV1, HouseholdProfileStateV1, HouseholdRevision,
    HouseholdScope, HouseholdStateV1, HouseholdSubjectId, ImportedCompatibilityStateV1,
    LegacySourceIdentityV1, MigrationDispositionManifestV1, MigrationProvenanceV1, RelationshipV1,
    canonical_sha256_v1,
};
use heyfood_platform::NativePaths;
#[cfg(not(windows))]
use heyfood_platform::{
    HouseholdKeyBundle, HouseholdKeyMaterial, HouseholdKeyStore, HouseholdMigrationGuardDocument,
    HouseholdMigrationGuardStore, HouseholdMigrationSourceIdentityV1, HouseholdSecureStore,
    HouseholdVault, HouseholdVaultLeaseModeV1, HouseholdVaultWrite, InMemoryHouseholdSecureStore,
    KeyBundleRevision, KeyId, KeyStoreExpectation, LegacyPythonConfigKindV1,
    LegacyPythonConfigRootV1, LegacyPythonHouseholdMigrationV1,
    LegacyPythonHouseholdSourceBrokerV1, LegacyPythonKeyringProbeOutcomeV1,
    MigrationGuardExpectation,
};
use tokio_util::sync::CancellationToken;

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "heyfood-native-composition-{name}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("temp root");
        Self(path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(not(windows))]
#[derive(Default)]
struct RejectingSourceBroker {
    calls: AtomicUsize,
}

#[cfg(not(windows))]
impl LegacyPythonHouseholdSourceBrokerV1 for RejectingSourceBroker {
    fn probe_and_load<'a>(
        &'a self,
        _lifecycle_lease: &'a heyfood_platform::HouseholdLifecycleLease,
        _config_kind: LegacyPythonConfigKindV1,
        _resolved_config_path: &'a Path,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<LegacyPythonKeyringProbeOutcomeV1, PortError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(PortError::new(
                "unexpected_legacy_source_read",
                "authenticated native artifacts must not reopen legacy sources",
            ))
        })
    }
}

fn account() -> AccountId {
    AccountId::parse("composition-account").expect("account")
}

fn ready_mode(result: NativeHouseholdCompositionV1) -> NativeHouseholdModeV1 {
    match result {
        NativeHouseholdCompositionV1::Ready(prepared) => prepared.mode(),
        NativeHouseholdCompositionV1::LifecycleRequired(required) => required.mode,
    }
}

#[cfg(not(windows))]
fn timestamp() -> CanonicalTimestampV1 {
    CanonicalTimestampV1::parse("2026-07-30T12:00:00.000Z").expect("timestamp")
}

#[cfg(not(windows))]
fn initial_state(
    account: AccountId,
    commit_id: CommitId,
    migration_id: uuid::Uuid,
    initialization_id: uuid::Uuid,
) -> HouseholdStateV1 {
    let at = timestamp();
    HouseholdStateV1 {
        schema_version: HOUSEHOLD_STATE_SCHEMA_VERSION,
        account_binding: account,
        revision: HouseholdRevision::new(1).expect("revision"),
        owner: HouseholdOwnerV1 {
            display_name: DisplayName::parse("Owner").expect("name"),
            relationship: RelationshipV1::Self_,
            profile_state: HouseholdProfileStateV1::Incomplete,
            created_at: at.clone(),
            updated_at: at.clone(),
        },
        active_scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
        members: Vec::new(),
        profiles: Vec::new(),
        outbox: Vec::new(),
        bounded_applied_commits: Vec::new(),
        imported_compatibility: ImportedCompatibilityStateV1 {
            fields: Vec::new(),
            legacy_python_applied_mutation_ids: Vec::new(),
            legacy_python_applied_mutation_ids_digest: None,
            legacy_remote_profile_references: Vec::new(),
            legacy_timestamp_provenance: Vec::new(),
        },
        migration_dispositions: MigrationDispositionManifestV1 {
            dispositions: Vec::new(),
        },
        migration_provenance: MigrationProvenanceV1 {
            source_identity: LegacySourceIdentityV1::NoSource {
                source_set_fingerprint: CanonicalDigestV1::from_bytes([7; 32]),
            },
            legacy_python_snapshot: None,
            migration_id,
            initialization_id,
            initial_commit_id: commit_id,
            migration_frozen_at: at.clone(),
        },
        updated_at: at,
    }
}

#[cfg(not(windows))]
#[derive(Clone, Copy)]
enum NativeFixtureCompletionV1 {
    UncommittedArtifacts,
    CommittedAwaitingFinalization,
    Completed,
}

#[cfg(not(windows))]
async fn committed_native_fixture(
    name: &str,
    corrupt_initial_fingerprint: bool,
    completion: NativeFixtureCompletionV1,
) -> (TempRoot, HouseholdVault, Arc<InMemoryHouseholdSecureStore>) {
    let root = TempRoot::new(name);
    let paths = NativePaths::under(root.0.join("state"));
    let account = account();
    let vault = HouseholdVault::from_native_paths(&paths, account.clone()).expect("vault");
    let store = Arc::new(InMemoryHouseholdSecureStore::default());
    let migration_id = uuid::Uuid::new_v4();
    let initialization_id = uuid::Uuid::new_v4();
    let commit_id = CommitId::new();
    let command = HouseholdInitialize::new(
        account.clone(),
        commit_id,
        initial_state(account.clone(), commit_id, migration_id, initialization_id),
        HouseholdEffectV1::Initialize,
        timestamp(),
    )
    .expect("command");
    let HouseholdRepositoryResolutionV1::Write { state, .. } =
        resolve_household_initialize_v1(None, &command).expect("resolved")
    else {
        panic!("initialization must write");
    };
    let mut state = *state;
    if corrupt_initial_fingerprint {
        state.bounded_applied_commits[0].fingerprint = CanonicalDigestV1::from_bytes([0x99; 32]);
    }
    let state_digest = *canonical_sha256_v1(&state).expect("digest").as_bytes();
    let reserved = HouseholdMigrationGuardDocument::initializing_reserved(
        vault.account_slot(),
        HouseholdMigrationSourceIdentityV1::no_source([7; 32]),
        migration_id,
        initialization_id,
        commit_id.as_uuid(),
        timestamp(),
    )
    .expect("reserved");
    let ready = reserved
        .ready_to_initialize(
            *command.claimed_effect_fingerprint.as_digest().as_bytes(),
            state_digest,
        )
        .expect("ready");
    let lifecycle = vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .expect("lifecycle");
    let mut lease = vault
        .acquire_vault_lease(
            lifecycle,
            HouseholdVaultLeaseModeV1::CreateIfMissing,
            CancellationToken::new(),
        )
        .await
        .expect("vault lease");
    HouseholdMigrationGuardStore::compare_exchange(
        store.as_ref(),
        &mut lease,
        MigrationGuardExpectation::Absent,
        Some(reserved.clone()),
        CancellationToken::new(),
    )
    .await
    .expect("reserve guard");
    HouseholdMigrationGuardStore::compare_exchange(
        store.as_ref(),
        &mut lease,
        MigrationGuardExpectation::Revision(reserved.guard_revision()),
        Some(ready.clone()),
        CancellationToken::new(),
    )
    .await
    .expect("ready guard");
    let key = HouseholdKeyBundle::initializing(
        vault.account_slot(),
        KeyBundleRevision::new(1).expect("revision"),
        KeyId::new(),
        HouseholdKeyMaterial::from_bytes([0x5a; 32]),
        initialization_id,
        commit_id.as_uuid(),
        *command.claimed_effect_fingerprint.as_digest().as_bytes(),
        state_digest,
    )
    .expect("valid initializing key bundle");
    HouseholdKeyStore::initialize(
        store.as_ref(),
        &mut lease,
        KeyStoreExpectation::Absent,
        ready.clone(),
        key.clone(),
        CancellationToken::new(),
    )
    .await
    .expect("key");
    let write = HouseholdVaultWrite::new(
        state.revision.get(),
        commit_id.as_uuid(),
        state.canonical_bytes().expect("canonical state"),
    )
    .expect("vault write");
    vault
        .initialize(&mut lease, key.clone(), write, CancellationToken::new())
        .await
        .expect("initialize vault");
    match completion {
        NativeFixtureCompletionV1::UncommittedArtifacts => {
            std::fs::remove_file(vault.household_directory().join("generation-1.hfv"))
                .expect("remove second generation");
            std::fs::remove_file(vault.household_directory().join("commit.hfj"))
                .expect("remove initialization journal");
        }
        NativeFixtureCompletionV1::CommittedAwaitingFinalization => {}
        NativeFixtureCompletionV1::Completed => {
            let stable = HouseholdKeyBundle::stable(
                vault.account_slot(),
                key.revision().checked_next().expect("next key revision"),
                key.active_key_id(),
                key.active_key().clone(),
            )
            .expect("valid stable key bundle");
            HouseholdKeyStore::compare_exchange(
                store.as_ref(),
                &mut lease,
                key.revision(),
                stable,
                CancellationToken::new(),
            )
            .await
            .expect("stable key");
            let completed = ready.complete_initialization().expect("completed guard");
            HouseholdMigrationGuardStore::compare_exchange(
                store.as_ref(),
                &mut lease,
                MigrationGuardExpectation::Revision(ready.guard_revision()),
                Some(completed),
                CancellationToken::new(),
            )
            .await
            .expect("complete guard");
        }
    }
    drop(lease);
    (root, vault, store)
}

#[test]
fn clap_terminal_controls_leave_native_state_untouched() {
    let root = TempRoot::new("terminal-controls");
    let state = root.0.join("state");

    for argument in ["--version", "--help"] {
        let output = Command::new(env!("CARGO_BIN_EXE_heyfood"))
            .arg(argument)
            .env("HEYFOOD_STATE_DIR", &state)
            .output()
            .expect("run terminal control");
        assert!(
            output.status.success(),
            "{argument} failed: status={:?}, stdout={:?}, stderr={:?}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !state.exists(),
            "{argument} must not create or inspect native state"
        );
    }
}

#[tokio::test]
async fn clean_flag_zero_preserves_legacy_without_creating_native_root() {
    let root = TempRoot::new("clean-legacy");
    let paths = NativePaths::under(root.0.join("state"));
    assert!(!paths.data_dir().exists());

    let composed = compose_native_household_v1(
        &paths,
        account(),
        NativeHouseholdRolloutV1::Disabled,
        CancellationToken::new(),
    )
    .await
    .expect("legacy composition");

    assert_eq!(
        ready_mode(composed),
        NativeHouseholdModeV1::LegacyCompatibility
    );
    assert!(
        !paths.data_dir().exists(),
        "flag zero with no provenance must not create any D2 artifact"
    );
}

#[cfg(all(windows, feature = "native-credentials"))]
#[tokio::test]
async fn windows_enabled_household_fails_closed_without_creating_native_state() {
    let root = TempRoot::new("windows-native-root-unavailable");
    let paths = NativePaths::under(root.0.join("state"));

    let error = compose_native_household_v1(
        &paths,
        account(),
        NativeHouseholdRolloutV1::Enabled,
        CancellationToken::new(),
    )
    .await
    .expect_err("Windows native household storage is intentionally unavailable");

    assert_eq!(error.code, "household_secure_store_unavailable");
    assert!(
        !paths.data_dir().exists(),
        "failed composition must not create floor, key, or vault state"
    );
}

#[cfg(not(feature = "native-credentials"))]
#[tokio::test]
async fn flag_one_without_native_credentials_fails_before_any_native_write() {
    let root = TempRoot::new("no-credentials");
    let paths = NativePaths::under(root.0.join("state"));

    let error = compose_native_household_v1(
        &paths,
        account(),
        NativeHouseholdRolloutV1::Enabled,
        CancellationToken::new(),
    )
    .await
    .expect_err("secure store is unavailable");

    assert_eq!(error.code, "household_secure_store_unavailable");
    assert!(!paths.data_dir().exists());
}

// Windows native-root identity is intentionally deferred with Windows
// distribution; retain the portable flag-zero contract there without opening
// the macOS/Linux-only household vault.
#[cfg(not(windows))]
#[tokio::test]
async fn lock_only_directory_is_not_native_provenance_but_flag_one_never_falls_back() {
    let root = TempRoot::new("lock-only");
    let paths = NativePaths::under(root.0.join("state"));
    let vault = HouseholdVault::from_native_paths(&paths, account()).expect("vault");
    let store = Arc::new(InMemoryHouseholdSecureStore::default());
    let store_port: Arc<dyn HouseholdSecureStore> = store;

    let enabled = compose_verified_native_household_v1(
        vault.clone(),
        account(),
        NativeHouseholdRolloutV1::Enabled,
        store_port.clone(),
        CancellationToken::new(),
    )
    .await
    .expect("enabled classification");
    assert_eq!(
        ready_mode(enabled),
        NativeHouseholdModeV1::NativeEnable,
        "rollout one must require native initialization"
    );

    let disabled = compose_verified_native_household_v1(
        vault,
        account(),
        NativeHouseholdRolloutV1::Disabled,
        store_port,
        CancellationToken::new(),
    )
    .await
    .expect("disabled classification");
    assert_eq!(
        ready_mode(disabled),
        NativeHouseholdModeV1::LegacyCompatibility,
        "retained lock files alone are not migration provenance"
    );
}

#[cfg(not(windows))]
#[tokio::test]
async fn committed_state_classifies_enabled_and_rollback_with_live_sessions() {
    let (_root, vault, store) = committed_native_fixture(
        "committed-modes",
        false,
        NativeFixtureCompletionV1::Completed,
    )
    .await;
    let store_port: Arc<dyn HouseholdSecureStore> = store;

    for (rollout, expected_mode) in [
        (
            NativeHouseholdRolloutV1::Enabled,
            NativeHouseholdModeV1::NativeEnabled,
        ),
        (
            NativeHouseholdRolloutV1::Disabled,
            NativeHouseholdModeV1::NativeRollbackReadOnly,
        ),
    ] {
        let result = compose_verified_native_household_v1(
            vault.clone(),
            account(),
            rollout,
            store_port.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("committed composition");
        let NativeHouseholdCompositionV1::Ready(prepared) = result else {
            panic!("committed state must be ready");
        };
        assert_eq!(prepared.mode(), expected_mode);
        assert!(
            prepared.household_agent_phase0_port().is_some(),
            "committed native composition must retain its account-bound agent read port"
        );
        let session = prepared
            .household_session()
            .expect("live household session");
        let loaded = session
            .load_required(CancellationToken::new())
            .await
            .expect("live load");
        assert_eq!(loaded.state.account_binding, account());
    }
}

#[cfg(not(windows))]
#[tokio::test]
async fn committed_startup_never_invokes_the_legacy_source_broker() {
    let (root, vault, store) = committed_native_fixture(
        "committed-zero-source",
        false,
        NativeFixtureCompletionV1::Completed,
    )
    .await;
    let migration = LegacyPythonHouseholdMigrationV1::new(
        LegacyPythonConfigRootV1::from_absolute_root(root.0.join("legacy-config"))
            .expect("legacy config root"),
        root.0.join("legacy-snapshot.json"),
    );
    let broker = RejectingSourceBroker::default();
    let store_port: Arc<dyn HouseholdSecureStore> = store;

    for (rollout, expected_mode) in [
        (
            NativeHouseholdRolloutV1::Enabled,
            NativeHouseholdModeV1::NativeEnabled,
        ),
        (
            NativeHouseholdRolloutV1::Disabled,
            NativeHouseholdModeV1::NativeRollbackReadOnly,
        ),
    ] {
        let result = compose_verified_native_household_with_migration_v1(
            vault.clone(),
            account(),
            rollout,
            store_port.clone(),
            &broker,
            &migration,
            DisplayName::parse("Owner").expect("owner name"),
            CancellationToken::new(),
        )
        .await
        .expect("committed composition");
        let NativeHouseholdCompositionV1::Ready(prepared) = result else {
            panic!("committed state must be ready");
        };
        assert_eq!(prepared.mode(), expected_mode);
        let loaded = prepared
            .household_session()
            .expect("live household session")
            .load_required(CancellationToken::new())
            .await
            .expect("live load");
        assert_eq!(loaded.state.account_binding, account());
    }

    assert_eq!(
        broker.calls.load(Ordering::SeqCst),
        0,
        "committed native startup must have no legacy source-broker path"
    );
}

#[cfg(not(windows))]
#[tokio::test]
async fn authenticated_artifact_resume_never_invokes_the_legacy_source_broker() {
    let broker = RejectingSourceBroker::default();

    for (name, fixture_completion) in [
        (
            "uncommitted-zero-source",
            NativeFixtureCompletionV1::UncommittedArtifacts,
        ),
        (
            "awaiting-finalization-zero-source",
            NativeFixtureCompletionV1::CommittedAwaitingFinalization,
        ),
    ] {
        let (root, vault, store) = committed_native_fixture(name, false, fixture_completion).await;
        let migration = LegacyPythonHouseholdMigrationV1::new(
            LegacyPythonConfigRootV1::from_absolute_root(root.0.join("legacy-config"))
                .expect("legacy config root"),
            root.0.join("legacy-snapshot.json"),
        );
        let store_port: Arc<dyn HouseholdSecureStore> = store;
        let result = compose_verified_native_household_with_migration_v1(
            vault,
            account(),
            NativeHouseholdRolloutV1::Enabled,
            store_port,
            &broker,
            &migration,
            DisplayName::parse("Owner").expect("owner name"),
            CancellationToken::new(),
        )
        .await
        .expect("authenticated artifact resume");
        let NativeHouseholdCompositionV1::Ready(prepared) = result else {
            panic!("resumed native state must be ready");
        };
        assert_eq!(prepared.mode(), NativeHouseholdModeV1::NativeEnabled);
        let loaded = prepared
            .household_session()
            .expect("live household session")
            .load_required(CancellationToken::new())
            .await
            .expect("live load");
        assert_eq!(loaded.state.account_binding, account());
    }

    assert_eq!(
        broker.calls.load(Ordering::SeqCst),
        0,
        "authenticated artifact resume must have no legacy source-broker path"
    );
}

#[cfg(not(windows))]
#[tokio::test]
async fn committed_state_with_wrong_initial_ledger_fingerprint_is_contradictory() {
    let (_root, vault, store) = committed_native_fixture(
        "wrong-initial-ledger",
        true,
        NativeFixtureCompletionV1::Completed,
    )
    .await;
    let store_port: Arc<dyn HouseholdSecureStore> = store;

    let error = compose_verified_native_household_v1(
        vault,
        account(),
        NativeHouseholdRolloutV1::Enabled,
        store_port,
        CancellationToken::new(),
    )
    .await
    .expect_err("wrong initial ledger fingerprint");

    assert_eq!(error.code, "household_native_evidence_contradiction");
}

#[cfg(unix)]
#[tokio::test]
async fn exact_current_teardown_preempts_mode_and_invalid_global_names_fail_closed() {
    use std::os::unix::fs::PermissionsExt as _;

    let root = TempRoot::new("teardown");
    let paths = NativePaths::under(root.0.join("state"));
    let vault = HouseholdVault::from_native_paths(&paths, account()).expect("vault");
    let directory = paths.data_dir().join("household-teardown");
    std::fs::create_dir_all(&directory).expect("teardown directory");
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
        .expect("directory mode");
    let digest = vault.account_slot().directory_name();
    let journal = directory.join(format!("teardown-{digest}.htj"));
    std::fs::write(&journal, b"content-free-teardown-v1").expect("journal");
    std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o600))
        .expect("journal mode");
    let store: Arc<dyn HouseholdSecureStore> = Arc::new(InMemoryHouseholdSecureStore::default());

    let result = compose_verified_native_household_v1(
        vault.clone(),
        account(),
        NativeHouseholdRolloutV1::Disabled,
        store.clone(),
        CancellationToken::new(),
    )
    .await
    .expect("teardown classification");
    assert_eq!(ready_mode(result), NativeHouseholdModeV1::ResumeTeardown);

    std::fs::remove_file(journal).expect("remove current journal");
    let invalid = directory.join("unexpected");
    std::fs::write(&invalid, b"x").expect("invalid journal");
    std::fs::set_permissions(&invalid, std::fs::Permissions::from_mode(0o600))
        .expect("invalid mode");
    let error = compose_verified_native_household_v1(
        vault,
        account(),
        NativeHouseholdRolloutV1::Disabled,
        store,
        CancellationToken::new(),
    )
    .await
    .expect_err("invalid global teardown name");
    assert_eq!(error.code, "household_teardown_journal_invalid");
}

#[cfg(unix)]
#[tokio::test]
async fn teardown_permissions_are_exact() {
    use std::os::unix::fs::PermissionsExt as _;

    let root = TempRoot::new("teardown-mode");
    let paths = NativePaths::under(root.0.join("state"));
    let vault = HouseholdVault::from_native_paths(&paths, account()).expect("vault");
    let directory = paths.data_dir().join("household-teardown");
    std::fs::create_dir_all(&directory).expect("teardown directory");
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
        .expect("directory mode");
    let journal = directory.join(format!(
        "teardown-{}.htj",
        vault.account_slot().directory_name()
    ));
    std::fs::write(&journal, b"content-free-teardown-v1").expect("journal");
    std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o400)).expect("weak mode");
    let store: Arc<dyn HouseholdSecureStore> = Arc::new(InMemoryHouseholdSecureStore::default());

    let error = compose_verified_native_household_v1(
        vault,
        account(),
        NativeHouseholdRolloutV1::Disabled,
        store,
        CancellationToken::new(),
    )
    .await
    .expect_err("journal mode must be exactly 0600");
    assert_eq!(error.code, "household_teardown_journal_invalid");
}
