#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use heyfood_core::{
    AccountId, CanonicalTimestampV1, HouseholdRevision, decode_canonical_household_state_v1,
};
use heyfood_platform::{
    HouseholdKeyBundle, HouseholdKeyMaterial, HouseholdKeyStore, HouseholdMigrationGuardDocument,
    HouseholdMigrationGuardStateV1, HouseholdMigrationGuardStore,
    HouseholdMigrationRepairFailureCategoryV1, HouseholdMigrationSourceIdentityV1, HouseholdVault,
    HouseholdVaultHealthV1, HouseholdVaultLease, HouseholdVaultLeaseModeV1, HouseholdVaultWrite,
    InMemoryHouseholdSecureStore, KeyBundleRevision, KeyId, KeyStoreExpectation,
    MigrationGuardExpectation,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "heyfood-household-vault-{name}-{}-{nonce}",
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

fn canonical_state(account: &str, revision: u64) -> Vec<u8> {
    let golden: serde_json::Value = serde_json::from_str(include_str!(
        "../../../schemas/v1/household-canonical-v1.golden.json"
    ))
    .expect("golden");
    let mut state = decode_canonical_household_state_v1(
        golden["state"]["canonical_utf8"]
            .as_str()
            .expect("canonical state")
            .as_bytes(),
    )
    .expect("decode state");
    state.account_binding = AccountId::parse(account).expect("account");
    state.revision = HouseholdRevision::new(revision).expect("revision");
    state.canonical_bytes().expect("canonical state")
}

fn write(account: &str, revision: u64) -> HouseholdVaultWrite {
    HouseholdVaultWrite::new(revision, Uuid::new_v4(), canonical_state(account, revision))
        .expect("vault write")
}

fn initializing_bundle(
    vault: &HouseholdVault,
    key_id: KeyId,
    key: HouseholdKeyMaterial,
    write: &HouseholdVaultWrite,
) -> HouseholdKeyBundle {
    HouseholdKeyBundle::initializing(
        vault.account_slot(),
        KeyBundleRevision::new(1).expect("revision"),
        key_id,
        key,
        Uuid::new_v4(),
        write.commit_id,
        [0x5a; 32],
        write.plaintext_sha256(),
    )
}

fn stable_bundle(
    vault: &HouseholdVault,
    revision: u64,
    key_id: KeyId,
    key: HouseholdKeyMaterial,
) -> HouseholdKeyBundle {
    HouseholdKeyBundle::stable(
        vault.account_slot(),
        KeyBundleRevision::new(revision).expect("revision"),
        key_id,
        key,
    )
}

fn corrupt(path: &Path) {
    let mut bytes = std::fs::read(path).expect("artifact");
    let last = bytes.last_mut().expect("nonempty artifact");
    *last ^= 0x80;
    std::fs::write(path, bytes).expect("corrupt artifact");
}

fn artifact_bytes(directory: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(directory).expect("household directory") {
        let path = entry.expect("entry").path();
        if path.is_file() {
            files.push((path.clone(), std::fs::read(path).expect("artifact bytes")));
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

fn assert_no_committed_vault_artifacts(vault: &HouseholdVault) {
    let directory = vault.household_directory();
    for name in [
        "generation-0.hfv",
        "generation-1.hfv",
        "generation-2.hfv",
        "commit.hfj",
    ] {
        assert!(!directory.join(name).exists(), "{name} must be absent");
    }
}

async fn acquire_vault_lease(
    vault: &HouseholdVault,
    mode: HouseholdVaultLeaseModeV1,
) -> HouseholdVaultLease {
    let lifecycle_lease = vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .expect("lifecycle lease");
    vault
        .acquire_vault_lease(lifecycle_lease, mode, CancellationToken::new())
        .await
        .expect("vault lease")
}

#[cfg(unix)]
#[test]
fn native_root_symlink_is_rejected_without_changing_the_target_permissions() {
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    let root = TempRoot::new("root-symlink");
    let target = root.0.join("target");
    std::fs::create_dir(&target).expect("target");
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))
        .expect("target permissions");
    let redirect = root.0.join("redirect");
    symlink(&target, &redirect).expect("symlink");

    let before = std::fs::symlink_metadata(&target)
        .expect("target metadata")
        .permissions()
        .mode()
        & 0o777;
    let error = HouseholdVault::open(
        &redirect,
        AccountId::parse("account-root-symlink").expect("account"),
    )
    .expect_err("symlink root");
    assert_eq!(error.code, "household_vault_path");
    let after = std::fs::symlink_metadata(&target)
        .expect("target metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(after, before);
}

#[tokio::test]
async fn generation_writes_reject_noncanonical_and_cross_account_state_before_encryption() {
    let invalid_root = TempRoot::new("invalid-state");
    let invalid_vault = HouseholdVault::open(
        &invalid_root.0.join("data"),
        AccountId::parse("account-invalid-state").expect("account"),
    )
    .expect("vault");
    let error = HouseholdVaultWrite::new(1, Uuid::new_v4(), br#"{"schema_version":1}"#.to_vec())
        .expect_err("invalid state");
    assert_eq!(error.code, "household_vault_state");
    assert_no_committed_vault_artifacts(&invalid_vault);

    let mismatch_root = TempRoot::new("cross-account-state");
    let mismatch_vault = HouseholdVault::open(
        &mismatch_root.0.join("data"),
        AccountId::parse("account-expected").expect("account"),
    )
    .expect("vault");
    let mut mismatch_lease =
        acquire_vault_lease(&mismatch_vault, HouseholdVaultLeaseModeV1::CreateIfMissing).await;
    let mismatch_state = write("account-foreign", 1);
    let error = mismatch_vault
        .initialize(
            &mut mismatch_lease,
            initializing_bundle(
                &mismatch_vault,
                KeyId::new(),
                HouseholdKeyMaterial::from_bytes([0x0b; 32]),
                &mismatch_state,
            ),
            mismatch_state,
            CancellationToken::new(),
        )
        .await
        .expect_err("cross-account state");
    assert_eq!(error.code, "household_vault_state");
    assert_no_committed_vault_artifacts(&mismatch_vault);
}

#[tokio::test]
async fn initial_seed_uses_two_encrypted_generations_and_no_plaintext_fallback() {
    let root = TempRoot::new("seed");
    let account = "account-seed";
    let vault = HouseholdVault::open(
        &root.0.join("data"),
        AccountId::parse(account).expect("account"),
    )
    .expect("vault");
    let mut vault_lease =
        acquire_vault_lease(&vault, HouseholdVaultLeaseModeV1::CreateIfMissing).await;
    let state = write(account, 1);
    let key_id = KeyId::new();
    let key = HouseholdKeyMaterial::from_bytes([0x11; 32]);
    let initializing = initializing_bundle(&vault, key_id, key.clone(), &state);

    let loaded = vault
        .initialize(
            &mut vault_lease,
            initializing,
            state.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("initialize");
    assert_eq!(loaded.state_revision, 1);
    assert_eq!(loaded.canonical_state, state.canonical_state);
    assert_eq!(loaded.journal_revision, 1);

    let directory = vault.household_directory();
    assert!(directory.join("generation-0.hfv").is_file());
    assert!(directory.join("generation-1.hfv").is_file());
    assert!(!directory.join("generation-2.hfv").exists());
    assert!(directory.join("commit.hfj").is_file());
    let first = std::fs::read(directory.join("generation-0.hfv")).expect("generation zero");
    let second = std::fs::read(directory.join("generation-1.hfv")).expect("generation one");
    assert_ne!(&first[56..80], &second[56..80], "nonces must differ");
    for (_, bytes) in artifact_bytes(&directory) {
        assert!(
            !bytes
                .windows(account.len())
                .any(|window| window == account.as_bytes())
        );
        assert!(
            !bytes
                .windows(b"Owner".len())
                .any(|window| window == b"Owner")
        );
    }

    let stable = stable_bundle(&vault, 2, key_id, key);
    let restarted = vault
        .load(&mut vault_lease, stable, CancellationToken::new())
        .await
        .expect("restart load");
    assert_eq!(restarted.canonical_state, state.canonical_state);
}

#[tokio::test]
async fn lifecycle_lease_is_retained_and_abort_preserves_exact_resumable_or_committed_state() {
    let root = TempRoot::new("lifecycle-abort");
    let account = "account-lifecycle-abort";
    let vault = HouseholdVault::open(
        &root.0.join("data"),
        AccountId::parse(account).expect("account"),
    )
    .expect("vault");
    let lifecycle_lease = vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .expect("lifecycle lease");

    let waiter_cancellation = CancellationToken::new();
    let waiter_trigger = waiter_cancellation.clone();
    let waiting_vault = vault.clone();
    let waiter = tokio::spawn(async move {
        waiting_vault
            .acquire_lifecycle_lease(waiter_cancellation)
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    waiter_trigger.cancel();
    let error = waiter
        .await
        .expect("waiter task")
        .expect_err("lease must remain exclusive");
    assert_eq!(error.code, "household_operation_cancelled");

    let state = write(account, 1);
    let key_id = KeyId::new();
    let key = HouseholdKeyMaterial::from_bytes([0x19; 32]);
    let initializing = initializing_bundle(&vault, key_id, key, &state);
    let initialization_id = initializing
        .initialization_id
        .expect("initialization identifier");
    let mut vault_lease = vault
        .acquire_vault_lease(
            lifecycle_lease,
            HouseholdVaultLeaseModeV1::CreateIfMissing,
            CancellationToken::new(),
        )
        .await
        .expect("vault lease");
    let store = InMemoryHouseholdSecureStore::default();
    let reserved_guard = HouseholdMigrationGuardDocument::initializing_reserved(
        vault.account_slot(),
        HouseholdMigrationSourceIdentityV1::present([0x58; 32]),
        Uuid::new_v4(),
        initialization_id,
        state.commit_id,
        CanonicalTimestampV1::parse("2026-07-30T12:00:00.000Z").expect("timestamp"),
    )
    .expect("reserved guard");
    let ready_guard = reserved_guard
        .ready_to_initialize([0x5a; 32], state.plaintext_sha256())
        .expect("ready guard");
    HouseholdMigrationGuardStore::compare_exchange(
        &store,
        &mut vault_lease,
        MigrationGuardExpectation::Absent,
        Some(reserved_guard.clone()),
        CancellationToken::new(),
    )
    .await
    .expect("reserve guard");
    HouseholdMigrationGuardStore::compare_exchange(
        &store,
        &mut vault_lease,
        MigrationGuardExpectation::Revision(reserved_guard.guard_revision()),
        Some(ready_guard.clone()),
        CancellationToken::new(),
    )
    .await
    .expect("ready guard");
    HouseholdKeyStore::initialize(
        &store,
        &mut vault_lease,
        KeyStoreExpectation::Absent,
        ready_guard,
        initializing.clone(),
        CancellationToken::new(),
    )
    .await
    .expect("key initialization");
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
        .expect_err("absent artifacts remain resumable");
    assert_eq!(error.code, "household_vault_initialization_resumable");
    assert_eq!(
        HouseholdMigrationGuardStore::load(
            &store,
            vault_lease.lifecycle_lease(),
            CancellationToken::new(),
        )
        .await
        .expect("guard load")
        .expect("guard")
        .state(),
        HouseholdMigrationGuardStateV1::Initializing
    );
    vault
        .initialize(
            &mut vault_lease,
            initializing.clone(),
            state.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("initialize under lifecycle lease");

    let committed = artifact_bytes(&vault.household_directory());
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
        .expect_err("committed journal must never be aborted");
    assert_eq!(error.code, "household_vault_initialization_committed");
    assert_eq!(artifact_bytes(&vault.household_directory()), committed);

    std::fs::remove_file(vault.household_directory().join("commit.hfj"))
        .expect("simulate pre-commit interruption");
    let partial = artifact_bytes(&vault.household_directory());
    let error = vault
        .abort_invalid_initialization_to_blocked_repair(
            &mut vault_lease,
            &store,
            initialization_id,
            Some(state),
            HouseholdMigrationRepairFailureCategoryV1::CanonicalConstruction,
            CancellationToken::new(),
        )
        .await
        .expect_err("exact partial initialization is resumable");
    assert_eq!(error.code, "household_vault_initialization_resumable");
    assert_eq!(artifact_bytes(&vault.household_directory()), partial);

    let lifecycle_lease = vault_lease
        .release_vault(CancellationToken::new())
        .await
        .expect("release vault lease");
    drop(lifecycle_lease);
    vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .expect("lease can be reacquired after drop");
}

#[tokio::test]
async fn commit_stages_into_the_unreferenced_third_slot_before_journal_authority_moves() {
    let root = TempRoot::new("commit");
    let account = "account-commit";
    let vault = HouseholdVault::open(
        &root.0.join("data"),
        AccountId::parse(account).expect("account"),
    )
    .expect("vault");
    let mut vault_lease =
        acquire_vault_lease(&vault, HouseholdVaultLeaseModeV1::CreateIfMissing).await;
    let first = write(account, 1);
    let key_id = KeyId::new();
    let key = HouseholdKeyMaterial::from_bytes([0x22; 32]);
    vault
        .initialize(
            &mut vault_lease,
            initializing_bundle(&vault, key_id, key.clone(), &first),
            first,
            CancellationToken::new(),
        )
        .await
        .expect("initialize");
    let stable = stable_bundle(&vault, 2, key_id, key);
    let second = write(account, 2);
    let committed = vault
        .commit(
            &mut vault_lease,
            stable.clone(),
            1,
            second.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("commit");
    assert_eq!(committed.state_revision, 2);
    assert_eq!(committed.journal_revision, 2);
    assert_eq!(committed.canonical_state, second.canonical_state);
    assert!((0..=2).all(|slot| {
        vault
            .household_directory()
            .join(format!("generation-{slot}.hfv"))
            .is_file()
    }));

    let conflict = vault
        .commit(
            &mut vault_lease,
            stable,
            1,
            write(account, 2),
            CancellationToken::new(),
        )
        .await
        .expect_err("stale revision");
    assert_eq!(conflict.code, "household_vault_revision_conflict");
}

#[tokio::test]
async fn current_corruption_never_promotes_or_rewrites_the_valid_previous_generation() {
    let root = TempRoot::new("current-corrupt");
    let account = "account-current-corrupt";
    let vault = HouseholdVault::open(
        &root.0.join("data"),
        AccountId::parse(account).expect("account"),
    )
    .expect("vault");
    let mut vault_lease =
        acquire_vault_lease(&vault, HouseholdVaultLeaseModeV1::CreateIfMissing).await;
    let state = write(account, 1);
    let key_id = KeyId::new();
    let key = HouseholdKeyMaterial::from_bytes([0x33; 32]);
    vault
        .initialize(
            &mut vault_lease,
            initializing_bundle(&vault, key_id, key.clone(), &state),
            state,
            CancellationToken::new(),
        )
        .await
        .expect("initialize");
    let stable = stable_bundle(&vault, 2, key_id, key);
    corrupt(&vault.household_directory().join("generation-0.hfv"));
    let before = artifact_bytes(&vault.household_directory());

    for _ in 0..2 {
        let error = vault
            .load(&mut vault_lease, stable.clone(), CancellationToken::new())
            .await
            .expect_err("current corruption must fail closed");
        assert_eq!(error.code, "vault_current_corrupt");
        assert_eq!(artifact_bytes(&vault.household_directory()), before);
    }
}

#[tokio::test]
async fn previous_corruption_repairs_only_from_the_authoritative_current() {
    let root = TempRoot::new("previous-corrupt");
    let account = "account-previous-corrupt";
    let vault = HouseholdVault::open(
        &root.0.join("data"),
        AccountId::parse(account).expect("account"),
    )
    .expect("vault");
    let mut vault_lease =
        acquire_vault_lease(&vault, HouseholdVaultLeaseModeV1::CreateIfMissing).await;
    let state = write(account, 1);
    let key_id = KeyId::new();
    let key = HouseholdKeyMaterial::from_bytes([0x44; 32]);
    vault
        .initialize(
            &mut vault_lease,
            initializing_bundle(&vault, key_id, key.clone(), &state),
            state.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("initialize");
    corrupt(&vault.household_directory().join("generation-1.hfv"));

    let stable = stable_bundle(&vault, 2, key_id, key);
    let repaired = vault
        .load(&mut vault_lease, stable.clone(), CancellationToken::new())
        .await
        .expect("repair");
    assert_eq!(
        repaired.health,
        HouseholdVaultHealthV1::PreviousRepairedFromAuthoritativeCurrent
    );
    assert_eq!(repaired.journal_revision, 2);
    assert_eq!(repaired.canonical_state, state.canonical_state);
    assert!(
        vault
            .household_directory()
            .join("generation-2.hfv")
            .is_file()
    );

    let restarted = vault
        .load(&mut vault_lease, stable, CancellationToken::new())
        .await
        .expect("healthy restart");
    assert_eq!(restarted.health, HouseholdVaultHealthV1::Healthy);
    assert_eq!(restarted.canonical_state, state.canonical_state);
}

#[tokio::test]
async fn key_rotation_rewrites_both_generations_and_the_journal_before_old_key_removal() {
    let root = TempRoot::new("rotation");
    let account = "account-rotation";
    let vault = HouseholdVault::open(
        &root.0.join("data"),
        AccountId::parse(account).expect("account"),
    )
    .expect("vault");
    let mut vault_lease =
        acquire_vault_lease(&vault, HouseholdVaultLeaseModeV1::CreateIfMissing).await;
    let state = write(account, 1);
    let old_key_id = KeyId::new();
    let old_key = HouseholdKeyMaterial::from_bytes([0x55; 32]);
    vault
        .initialize(
            &mut vault_lease,
            initializing_bundle(&vault, old_key_id, old_key.clone(), &state),
            state.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("initialize");
    let new_key_id = KeyId::new();
    let new_key = HouseholdKeyMaterial::from_bytes([0x66; 32]);
    let previous = HouseholdKeyBundle::stable(
        vault.account_slot(),
        KeyBundleRevision::new(1).expect("revision"),
        old_key_id,
        old_key,
    );
    let rewriting = HouseholdKeyBundle::rewriting(
        vault.account_slot(),
        KeyBundleRevision::new(2).expect("revision"),
        new_key_id,
        new_key.clone(),
        &previous,
        Uuid::new_v4(),
    )
    .expect("rewriting bundle");
    let rotated = vault
        .rotate(
            &mut vault_lease,
            rewriting.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("rotate");
    assert_eq!(rotated.journal_revision, 2);
    assert_eq!(rotated.canonical_state, state.canonical_state);

    let finalized = rewriting
        .stabilized(
            vault.account_slot(),
            KeyBundleRevision::new(3).expect("revision"),
        )
        .expect("finalized bundle");
    let restarted = vault
        .load(&mut vault_lease, finalized, CancellationToken::new())
        .await
        .expect("new key only");
    assert_eq!(restarted.canonical_state, state.canonical_state);
}

#[tokio::test]
async fn wrong_key_and_preflight_cancellation_fail_without_plaintext_fallback() {
    let root = TempRoot::new("wrong-key");
    let account = "account-wrong-key";
    let vault = HouseholdVault::open(
        &root.0.join("data"),
        AccountId::parse(account).expect("account"),
    )
    .expect("vault");
    let mut vault_lease =
        acquire_vault_lease(&vault, HouseholdVaultLeaseModeV1::CreateIfMissing).await;
    let state = write(account, 1);
    let key_id = KeyId::new();
    let key = HouseholdKeyMaterial::from_bytes([0x77; 32]);
    vault
        .initialize(
            &mut vault_lease,
            initializing_bundle(&vault, key_id, key, &state),
            state,
            CancellationToken::new(),
        )
        .await
        .expect("initialize");
    let wrong = stable_bundle(
        &vault,
        2,
        key_id,
        HouseholdKeyMaterial::from_bytes([0x78; 32]),
    );
    assert!(
        vault
            .load(&mut vault_lease, wrong, CancellationToken::new())
            .await
            .is_err()
    );

    let cancelled_root = TempRoot::new("cancelled");
    let cancelled_account = "account-cancelled";
    let cancelled_vault = HouseholdVault::open(
        &cancelled_root.0.join("data"),
        AccountId::parse(cancelled_account).expect("account"),
    )
    .expect("vault");
    let lifecycle_lease = cancelled_vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .expect("cancelled lifecycle lease");
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let error = cancelled_vault
        .acquire_vault_lease(
            lifecycle_lease,
            HouseholdVaultLeaseModeV1::CreateIfMissing,
            cancellation,
        )
        .await
        .expect_err("preflight cancellation");
    assert_eq!(error.code, "household_operation_cancelled");
    assert!(!cancelled_vault.household_directory().exists());
}
