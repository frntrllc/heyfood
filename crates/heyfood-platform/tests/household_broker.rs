use std::path::PathBuf;
use std::time::Duration;

use heyfood_core::{AccountId, CanonicalTimestampV1, canonicalize_json_value_v1};
#[cfg(feature = "native-credentials")]
use heyfood_platform::HouseholdKeyBroker;
#[cfg(not(feature = "native-credentials"))]
use heyfood_platform::open_production_household_secure_store;
use heyfood_platform::{
    HouseholdAccountSlotV1, HouseholdBrokerOperationV1, HouseholdKeyBundle, HouseholdKeyMaterial,
    HouseholdKeyStore, HouseholdKeyringLocatorsV1, HouseholdMigrationGuardDocument,
    HouseholdMigrationGuardStateV1, HouseholdMigrationGuardStore,
    HouseholdMigrationInitializationPhaseV1, HouseholdMigrationRepairFailureCategoryV1,
    HouseholdMigrationSourceIdentityV1, HouseholdVault, HouseholdVaultLeaseModeV1,
    InMemoryHouseholdSecureStore, KeyBundleRevision, KeyId, KeyStoreExpectation,
    LegacyPythonKeyringLocatorV1, MAX_BROKER_DOCUMENT_BYTES, MigrationGuardExpectation,
    NativePaths, NativeRootPlatformV1,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "heyfood-household-broker-root-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        std::fs::create_dir_all(&path).expect("temp root");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .expect("temp root permissions");
        }
        Self(path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn macos_slot() -> HouseholdAccountSlotV1 {
    HouseholdAccountSlotV1::from_root_bytes(
        &AccountId::parse("acct_example_01").expect("account"),
        NativeRootPlatformV1::Macos,
        b"/Users/alice/Library/Application Support/ai.frntr.heyfood",
    )
    .expect("slot")
}

#[test]
fn native_and_historical_keyring_locators_match_the_frozen_vectors() {
    let macos = HouseholdKeyringLocatorsV1::from_account_slot(&macos_slot()).expect("locators");
    assert_eq!(macos.service, "ai.frntr.heyfood.household.v1");
    assert_eq!(
        macos.key_bundle_username,
        "key-91ea4f9a8ba072d501475d70042ae061555ec7995995f3d995230c7844a39420"
    );
    assert_eq!(
        macos.migration_guard_username,
        "migration-guard-91ea4f9a8ba072d501475d70042ae061555ec7995995f3d995230c7844a39420"
    );
    assert_eq!(macos.key_bundle_username.len(), 68);
    assert_eq!(macos.migration_guard_username.len(), 80);

    let linux_slot = HouseholdAccountSlotV1::from_root_bytes(
        &AccountId::parse("acct_example_01").expect("account"),
        NativeRootPlatformV1::Linux,
        b"/home/alice/.local/share/heyfood",
    )
    .expect("slot");
    let linux = HouseholdKeyringLocatorsV1::from_account_slot(&linux_slot).expect("locators");
    assert_eq!(
        linux.key_bundle_username,
        "key-3ebdb4e0de17178d13fb15aa5295258afd7ea3b3e5d69b1efa358c2e18b3fbaa"
    );
    assert_eq!(
        linux.migration_guard_username,
        "migration-guard-3ebdb4e0de17178d13fb15aa5295258afd7ea3b3e5d69b1efa358c2e18b3fbaa"
    );

    for (path, expected) in [
        (
            b"/Users/alice/.config/heyfood/config.json".as_slice(),
            "config-708cb8bd7cc7bdc9b42f",
        ),
        (
            b"/Users/alice/.config/hellofood/config.json".as_slice(),
            "config-f4f9aebd2346a951e341",
        ),
        (
            b"/home/alice/.config/heyfood/config.json".as_slice(),
            "config-d12b1ca554fb38660c4d",
        ),
        (
            b"/home/alice/.config/hellofood/config.json".as_slice(),
            "config-cc800c000e80f0a94363",
        ),
    ] {
        let locator =
            LegacyPythonKeyringLocatorV1::from_resolved_config_path_bytes(path).expect("legacy");
        assert_eq!(locator.service, "heyfood-cli");
        assert_eq!(locator.username, expected);
    }
}

#[test]
fn broker_operation_limits_are_closed_and_only_legacy_load_is_large() {
    let operations = [
        HouseholdBrokerOperationV1::SecureStoreProbe,
        HouseholdBrokerOperationV1::KeyLoad,
        HouseholdBrokerOperationV1::KeyInitialize,
        HouseholdBrokerOperationV1::KeyAbortInitialization,
        HouseholdBrokerOperationV1::KeyReplace,
        HouseholdBrokerOperationV1::KeyDelete,
        HouseholdBrokerOperationV1::KeyVerifyAbsent,
        HouseholdBrokerOperationV1::MigrationGuardLoad,
        HouseholdBrokerOperationV1::MigrationGuardCompareExchange,
        HouseholdBrokerOperationV1::LegacyPythonHouseholdProbe,
        HouseholdBrokerOperationV1::LegacyPythonHouseholdLoad,
        HouseholdBrokerOperationV1::LegacyPythonCredentialsScrubAndVerify,
    ];
    for operation in operations {
        let expected = if operation == HouseholdBrokerOperationV1::LegacyPythonHouseholdLoad {
            (4 * 1024 * 1024) + (64 * 1024)
        } else {
            16 * 1024
        };
        assert_eq!(operation.response_limit(), expected);
        assert!(!operation.action().is_empty());
    }
}

#[test]
fn migration_guard_is_typed_canonical_and_rejects_incomplete_or_noncanonical_documents() {
    let slot = macos_slot();
    let reserved = HouseholdMigrationGuardDocument::initializing_reserved(
        &slot,
        HouseholdMigrationSourceIdentityV1::no_source([0x31; 32]),
        Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").expect("migration"),
        Uuid::parse_str("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb").expect("initialization"),
        Uuid::parse_str("cccccccc-cccc-4ccc-8ccc-cccccccccccc").expect("commit"),
        CanonicalTimestampV1::parse("2026-07-30T12:00:00.000Z").expect("timestamp"),
    )
    .expect("reserved");
    let canonical = reserved.canonical_bytes().expect("canonical");
    assert_eq!(
        HouseholdMigrationGuardDocument::from_canonical_bytes(&slot, &canonical)
            .expect("canonical round trip"),
        reserved
    );

    let pretty = serde_json::to_vec_pretty(
        &serde_json::from_slice::<serde_json::Value>(&canonical).expect("value"),
    )
    .expect("pretty");
    assert_eq!(
        HouseholdMigrationGuardDocument::from_canonical_bytes(&slot, &pretty)
            .expect_err("whitespace is noncanonical")
            .code,
        "household_migration_guard_quarantined"
    );
    assert_eq!(
        HouseholdMigrationGuardDocument::from_canonical_bytes(&slot, br#"{"guard_revision":1}"#,)
            .expect_err("incomplete guard")
            .code,
        "household_migration_guard_quarantined"
    );

    let duplicate = String::from_utf8(canonical.clone())
        .expect("utf8")
        .replacen(
            "\"schema_version\":1",
            "\"schema_version\":1,\"schema_version\":1",
            1,
        );
    assert_eq!(
        HouseholdMigrationGuardDocument::from_canonical_bytes(&slot, duplicate.as_bytes(),)
            .expect_err("duplicate key")
            .code,
        "household_migration_guard_quarantined"
    );

    let mut unknown: serde_json::Value = serde_json::from_slice(&canonical).expect("unknown value");
    unknown["future_field"] = serde_json::json!(true);
    let unknown = canonicalize_json_value_v1(&unknown).expect("canonical unknown");
    assert_eq!(
        HouseholdMigrationGuardDocument::from_canonical_bytes(&slot, &unknown)
            .expect_err("unknown guard field")
            .code,
        "household_migration_guard_quarantined"
    );
    let oversized_guard = vec![b' '; MAX_BROKER_DOCUMENT_BYTES + 1];
    assert_eq!(
        HouseholdMigrationGuardDocument::from_canonical_bytes(&slot, &oversized_guard)
            .expect_err("oversized guard")
            .code,
        "household_migration_guard_quarantined"
    );

    let mut illegal: serde_json::Value = serde_json::from_slice(&canonical).expect("illegal value");
    illegal["state"] = serde_json::Value::String("migrated".to_owned());
    let illegal = canonicalize_json_value_v1(&illegal).expect("canonical illegal");
    assert_eq!(
        HouseholdMigrationGuardDocument::from_canonical_bytes(&slot, &illegal)
            .expect_err("illegal state/nullability")
            .code,
        "household_migration_guard_quarantined"
    );

    let mut unreachable_logout: serde_json::Value =
        serde_json::from_slice(&canonical).expect("unreachable logout");
    unreachable_logout["state"] = serde_json::json!("blocked_after_logout");
    unreachable_logout["initialization_phase"] = serde_json::Value::Null;
    let unreachable_logout =
        canonicalize_json_value_v1(&unreachable_logout).expect("canonical unreachable logout");
    assert_eq!(
        HouseholdMigrationGuardDocument::from_canonical_bytes(&slot, &unreachable_logout)
            .expect_err("logout tombstone must retain completed or repair provenance")
            .code,
        "household_migration_guard_quarantined"
    );

    for (field, replacement) in [
        ("guard_revision", serde_json::json!(0)),
        ("initialization_phase", serde_json::Value::Null),
        (
            "migration_frozen_at",
            serde_json::json!("2026-07-30T12:00:00Z"),
        ),
        (
            "initial_commit_id",
            serde_json::json!("00000000-0000-0000-0000-000000000000"),
        ),
    ] {
        let mut invalid: serde_json::Value =
            serde_json::from_slice(&canonical).expect("invalid value");
        invalid[field] = replacement;
        let invalid = canonicalize_json_value_v1(&invalid).expect("canonical invalid");
        assert_eq!(
            HouseholdMigrationGuardDocument::from_canonical_bytes(&slot, &invalid)
                .expect_err("invalid guard field")
                .code,
            "household_migration_guard_quarantined",
            "{field}"
        );
    }

    let uppercase_uuid = String::from_utf8(canonical.clone()).expect("utf8").replace(
        "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA",
    );
    assert_eq!(
        HouseholdMigrationGuardDocument::from_canonical_bytes(&slot, uppercase_uuid.as_bytes(),)
            .expect_err("alternate UUID spelling")
            .code,
        "household_migration_guard_quarantined"
    );

    let mut one_digest: serde_json::Value = serde_json::from_slice(&canonical).expect("one digest");
    one_digest["initial_state_digest"] = serde_json::json!("44".repeat(32));
    let one_digest = canonicalize_json_value_v1(&one_digest).expect("canonical one digest");
    assert_eq!(
        HouseholdMigrationGuardDocument::from_canonical_bytes(&slot, &one_digest)
            .expect_err("partial ready tuple")
            .code,
        "household_migration_guard_quarantined"
    );

    let other_slot = HouseholdAccountSlotV1::from_root_bytes(
        &AccountId::parse("acct_other").expect("other account"),
        NativeRootPlatformV1::Macos,
        b"/Users/alice/Library/Application Support/ai.frntr.heyfood",
    )
    .expect("other slot");
    assert_eq!(
        HouseholdMigrationGuardDocument::from_canonical_bytes(&other_slot, &canonical)
            .expect_err("account binding")
            .code,
        "household_migration_guard_quarantined"
    );

    let ready = reserved
        .ready_to_initialize([0x41; 32], [0x42; 32])
        .expect("ready");
    let initialized = ready.complete_initialization().expect("initialized");
    assert_eq!(
        initialized.state(),
        HouseholdMigrationGuardStateV1::InitializedNoSource
    );
    assert_eq!(
        initialized
            .blocked_after_logout()
            .expect("blocked after logout")
            .state(),
        HouseholdMigrationGuardStateV1::BlockedAfterLogout
    );
    let aborting = ready
        .begin_aborting(HouseholdMigrationRepairFailureCategoryV1::SemanticValidation)
        .expect("aborting");
    assert_eq!(aborting.state(), HouseholdMigrationGuardStateV1::Aborting);
    assert_eq!(
        aborting.initialization_phase(),
        Some(HouseholdMigrationInitializationPhaseV1::ReadyToInitialize)
    );
    assert_eq!(
        HouseholdMigrationGuardDocument::from_canonical_bytes(
            &slot,
            &aborting.canonical_bytes().expect("aborting canonical"),
        )
        .expect("aborting round trip"),
        aborting
    );
}

#[tokio::test]
async fn in_memory_key_and_guard_stores_are_account_bound_cancellable_and_cas_only() {
    let root = TempRoot::new();
    let vault = HouseholdVault::open(
        &root.0.join("data"),
        AccountId::parse("acct_example_01").expect("account"),
    )
    .expect("vault");
    let lifecycle_lease = vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .expect("lifecycle lease");
    let mut vault_lease = vault
        .acquire_vault_lease(
            lifecycle_lease,
            HouseholdVaultLeaseModeV1::CreateIfMissing,
            CancellationToken::new(),
        )
        .await
        .expect("vault lease");
    let slot = vault_lease.account_slot().clone();
    let store = InMemoryHouseholdSecureStore::default();
    let key_id = KeyId::new();
    let key = HouseholdKeyMaterial::from_bytes([0x42; 32]);
    let migration_id = Uuid::new_v4();
    let initialization_id = Uuid::new_v4();
    let initial_commit_id = Uuid::new_v4();
    let reserved_guard = HouseholdMigrationGuardDocument::initializing_reserved(
        &slot,
        HouseholdMigrationSourceIdentityV1::no_source([0x33; 32]),
        migration_id,
        initialization_id,
        initial_commit_id,
        CanonicalTimestampV1::parse("2026-07-30T12:00:00.000Z").expect("timestamp"),
    )
    .expect("reserved guard");
    let ready_guard = reserved_guard
        .ready_to_initialize([0x11; 32], [0x22; 32])
        .expect("ready guard");
    let initial = HouseholdKeyBundle::initializing(
        &slot,
        KeyBundleRevision::new(1).expect("revision"),
        key_id,
        key.clone(),
        initialization_id,
        initial_commit_id,
        [0x11; 32],
        [0x22; 32],
    );
    let invalid_initial = HouseholdKeyBundle::stable(
        &slot,
        KeyBundleRevision::new(1).expect("revision"),
        key_id,
        key.clone(),
    );
    let invalid_first_guard = HouseholdMigrationGuardStore::compare_exchange(
        &store,
        &mut vault_lease,
        MigrationGuardExpectation::Absent,
        Some(ready_guard.clone()),
        CancellationToken::new(),
    )
    .await
    .expect_err("first guard must be reserved");
    assert_eq!(
        invalid_first_guard.code,
        "household_migration_guard_revision"
    );
    HouseholdMigrationGuardStore::compare_exchange(
        &store,
        &mut vault_lease,
        MigrationGuardExpectation::Absent,
        Some(reserved_guard.clone()),
        CancellationToken::new(),
    )
    .await
    .expect("reserved guard CAS");
    HouseholdMigrationGuardStore::compare_exchange(
        &store,
        &mut vault_lease,
        MigrationGuardExpectation::Revision(reserved_guard.guard_revision()),
        Some(ready_guard.clone()),
        CancellationToken::new(),
    )
    .await
    .expect("ready guard CAS");
    let invalid_phase = store
        .initialize(
            &mut vault_lease,
            KeyStoreExpectation::Absent,
            ready_guard.clone(),
            invalid_initial,
            CancellationToken::new(),
        )
        .await
        .expect_err("initial phase");
    assert_eq!(invalid_phase.code, "household_key_bundle_invalid");
    store
        .initialize(
            &mut vault_lease,
            KeyStoreExpectation::Absent,
            ready_guard.clone(),
            initial.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("initialize");
    assert_eq!(
        HouseholdKeyStore::load(
            &store,
            vault_lease.lifecycle_lease(),
            CancellationToken::new()
        )
        .await
        .expect("load"),
        Some(initial.clone())
    );
    let duplicate = store
        .initialize(
            &mut vault_lease,
            KeyStoreExpectation::Absent,
            ready_guard.clone(),
            initial,
            CancellationToken::new(),
        )
        .await
        .expect_err("duplicate");
    assert_eq!(duplicate.code, "household_key_exists");

    let stable = HouseholdKeyBundle::stable(
        &slot,
        KeyBundleRevision::new(2).expect("revision"),
        key_id,
        key,
    );
    HouseholdKeyStore::compare_exchange(
        &store,
        &mut vault_lease,
        KeyBundleRevision::new(1).expect("revision"),
        stable.clone(),
        CancellationToken::new(),
    )
    .await
    .expect("key CAS");
    assert_eq!(
        HouseholdKeyStore::load(
            &store,
            vault_lease.lifecycle_lease(),
            CancellationToken::new()
        )
        .await
        .expect("load"),
        Some(stable)
    );

    assert_eq!(
        HouseholdMigrationGuardStore::load(
            &store,
            vault_lease.lifecycle_lease(),
            CancellationToken::new()
        )
        .await
        .expect("guard load"),
        Some(ready_guard)
    );

    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let error = HouseholdKeyStore::load(&store, vault_lease.lifecycle_lease(), cancellation)
        .await
        .expect_err("cancelled");
    assert_eq!(error.code, "household_operation_cancelled");
}

#[tokio::test]
async fn aborting_guard_blocks_key_remint_before_and_after_exact_key_abort() {
    let root = TempRoot::new();
    let vault = HouseholdVault::open(
        &root.0.join("data"),
        AccountId::parse("acct_abort_key").expect("account"),
    )
    .expect("vault");
    let lifecycle_lease = vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .expect("lifecycle");
    let mut vault_lease = vault
        .acquire_vault_lease(
            lifecycle_lease,
            HouseholdVaultLeaseModeV1::CreateIfMissing,
            CancellationToken::new(),
        )
        .await
        .expect("vault lease");
    let store = InMemoryHouseholdSecureStore::default();
    let initialization_id = Uuid::new_v4();
    let commit_id = Uuid::new_v4();
    let reserved = HouseholdMigrationGuardDocument::initializing_reserved(
        vault.account_slot(),
        HouseholdMigrationSourceIdentityV1::present([0x51; 32]),
        Uuid::new_v4(),
        initialization_id,
        commit_id,
        CanonicalTimestampV1::parse("2026-07-30T12:00:00.000Z").expect("timestamp"),
    )
    .expect("reserved");
    let ready = reserved
        .ready_to_initialize([0x52; 32], [0x53; 32])
        .expect("ready");
    HouseholdMigrationGuardStore::compare_exchange(
        &store,
        &mut vault_lease,
        MigrationGuardExpectation::Absent,
        Some(reserved.clone()),
        CancellationToken::new(),
    )
    .await
    .expect("reserve");
    HouseholdMigrationGuardStore::compare_exchange(
        &store,
        &mut vault_lease,
        MigrationGuardExpectation::Revision(reserved.guard_revision()),
        Some(ready.clone()),
        CancellationToken::new(),
    )
    .await
    .expect("ready");
    let bundle = HouseholdKeyBundle::initializing(
        vault.account_slot(),
        KeyBundleRevision::new(1).expect("revision"),
        KeyId::new(),
        HouseholdKeyMaterial::from_bytes([0x54; 32]),
        initialization_id,
        commit_id,
        [0x52; 32],
        [0x53; 32],
    );
    HouseholdKeyStore::initialize(
        &store,
        &mut vault_lease,
        KeyStoreExpectation::Absent,
        ready.clone(),
        bundle.clone(),
        CancellationToken::new(),
    )
    .await
    .expect("key");

    let aborting = ready
        .begin_aborting(HouseholdMigrationRepairFailureCategoryV1::CanonicalConstruction)
        .expect("aborting");
    HouseholdMigrationGuardStore::compare_exchange(
        &store,
        &mut vault_lease,
        MigrationGuardExpectation::Revision(ready.guard_revision()),
        Some(aborting.clone()),
        CancellationToken::new(),
    )
    .await
    .expect("record abort");

    let denied_while_key_present = HouseholdKeyStore::initialize(
        &store,
        &mut vault_lease,
        KeyStoreExpectation::Absent,
        ready.clone(),
        bundle.clone(),
        CancellationToken::new(),
    )
    .await
    .expect_err("stale ready guard cannot initialize");
    assert_eq!(
        denied_while_key_present.code,
        "household_key_guard_mismatch"
    );

    HouseholdKeyStore::abort_initialization_and_verify(
        &store,
        &mut vault_lease,
        bundle.revision,
        initialization_id,
        aborting.clone(),
        CancellationToken::new(),
    )
    .await
    .expect("abort key");
    assert!(
        HouseholdKeyStore::load(
            &store,
            vault_lease.lifecycle_lease(),
            CancellationToken::new(),
        )
        .await
        .expect("load")
        .is_none()
    );

    let denied_after_key_absence = HouseholdKeyStore::initialize(
        &store,
        &mut vault_lease,
        KeyStoreExpectation::Absent,
        ready,
        bundle,
        CancellationToken::new(),
    )
    .await
    .expect_err("cleanup-pending guard prevents remint");
    assert_eq!(
        denied_after_key_absence.code,
        "household_key_guard_mismatch"
    );
    assert_eq!(
        HouseholdMigrationGuardStore::load(
            &store,
            vault_lease.lifecycle_lease(),
            CancellationToken::new(),
        )
        .await
        .expect("guard")
        .expect("present")
        .state(),
        HouseholdMigrationGuardStateV1::Aborting
    );
}

#[cfg(feature = "native-credentials")]
#[tokio::test]
async fn production_broker_rejects_an_account_slot_bound_to_another_native_root_before_spawn() {
    let root = TempRoot::new();
    let paths = NativePaths::under(&root.0);
    std::fs::create_dir(paths.data_dir()).expect("data root");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        std::fs::set_permissions(paths.data_dir(), std::fs::Permissions::from_mode(0o700))
            .expect("data root permissions");
    }
    let broker =
        HouseholdKeyBroker::from_native_paths(&paths, Duration::from_secs(1)).expect("open broker");
    let other_root = TempRoot::new();
    let other_vault = HouseholdVault::open(
        &other_root.0.join("data"),
        AccountId::parse("acct_example_01").expect("account"),
    )
    .expect("other vault");
    let other_lifecycle_lease = other_vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .expect("other lifecycle lease");
    let error = HouseholdKeyStore::load(&broker, &other_lifecycle_lease, CancellationToken::new())
        .await
        .expect_err("root mismatch");
    assert_eq!(error.code, "household_broker_root_mismatch");
}

#[test]
fn secure_values_are_redacted_in_diagnostics() {
    let slot = macos_slot();
    let bundle = HouseholdKeyBundle::stable(
        &slot,
        KeyBundleRevision::new(1).expect("revision"),
        KeyId::new(),
        HouseholdKeyMaterial::from_bytes([0xab; 32]),
    );
    let debug = format!("{bundle:?}");
    assert!(!debug.contains(&"ab".repeat(32)));
    assert!(!debug.contains("acct_example_01"));
    assert!(debug.contains("account_digest"));
}

#[cfg(not(feature = "native-credentials"))]
#[test]
fn no_default_build_returns_unavailable_before_creating_any_store_artifact() {
    let root = std::env::temp_dir().join(format!(
        "heyfood-household-secure-store-unavailable-{}",
        Uuid::new_v4()
    ));
    assert!(!root.exists());
    let paths = NativePaths::under(root.clone());
    let error = match open_production_household_secure_store(&paths, Duration::from_secs(1)) {
        Ok(_) => panic!("secure store must be unavailable"),
        Err(error) => error,
    };
    assert_eq!(error.code, "household_secure_store_unavailable");
    assert!(!root.exists());
}
