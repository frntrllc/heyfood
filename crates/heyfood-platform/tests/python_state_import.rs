use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use heyfood_application::HouseholdLoad;
#[cfg(unix)]
use heyfood_core::{
    AccountId, AppliedCommitOutcomeV1, AppliedCommitRecordV1, CanonicalDigestV1,
    CanonicalTimestampV1, CommitId, DisplayName, HouseholdProfileStateV1, HouseholdRevision,
    HouseholdScope, HouseholdSubjectId, LegacyPythonSnapshotProvenanceV1, LegacySourceIdentityV1,
};
use heyfood_core::{MigrationDispositionKindV1, PythonFieldAction, PythonImportOutcome};
#[cfg(unix)]
use heyfood_platform::{
    HouseholdAccountSlotV1, HouseholdMigrationGuardDocument, HouseholdMigrationSourceIdentityV1,
    HouseholdVault, HouseholdVaultLeaseModeV1, LegacyPythonConfigKindV1, LegacyPythonConfigRootV1,
    LegacyPythonHouseholdMigrationV1, LegacyPythonKeyringProbeOutcomeV1,
    LegacyPythonPhaseAResultV1, LegacyPythonPhaseBContextV1,
};
use heyfood_platform::{ProtectedHouseholdReason, PythonStateImporter, PythonStatePreview};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use tokio_util::sync::CancellationToken;
#[cfg(unix)]
use uuid::Uuid;

const BOUND_SOURCE: &[u8] = include_bytes!("../../../fixtures/config/python-0.3.2-file-state.json");
const UNBOUND_SOURCE: &[u8] =
    include_bytes!("../../../fixtures/config/python-0.3.2-unbound-state.json");
const KEYRING_SOURCE: &[u8] =
    include_bytes!("../../../fixtures/config/python-0.3.2-keyring-metadata.json");
#[cfg(unix)]
const D2_OWNER_ONLY: &[u8] = include_bytes!(
    "../../../fixtures/config/household-migration-v1/python-normalized-owner-only-valid.json"
);
#[cfg(unix)]
const D2_FUTURE_TIMESTAMP: &[u8] = include_bytes!(
    "../../../fixtures/config/household-migration-v1/python-normalized-future-timestamp.json"
);
#[cfg(unix)]
const D2_FUTURE_DOB: &[u8] = include_bytes!(
    "../../../fixtures/config/household-migration-v1/python-normalized-future-dob.json"
);
#[cfg(unix)]
const D2_MALFORMED_DUPLICATE: &[u8] = include_bytes!(
    "../../../fixtures/config/household-migration-v1/malformed-duplicate-account.json"
);
#[cfg(unix)]
const D2_RUST_SHOWCASE: &[u8] = include_bytes!(
    "../../../fixtures/config/household-migration-v1/rust-installed-showcase-partial-v1.json"
);
#[cfg(unix)]
const D2_RUST_FIXTURE_V4: &[u8] =
    include_bytes!("../../../fixtures/config/household-migration-v1/rust-fixture-v4.json");
#[cfg(unix)]
const D2_RUST_EXPLICIT: &[u8] = include_bytes!(
    "../../../fixtures/config/household-migration-v1/rust-unversioned-explicit-owner-v0.json"
);
#[cfg(unix)]
const D2_RUST_IMPLICIT: &[u8] = include_bytes!(
    "../../../fixtures/config/household-migration-v1/rust-unversioned-implicit-owner-v0.json"
);
#[cfg(unix)]
const D2_KEYRING_METADATA: &[u8] = include_bytes!(
    "../../../fixtures/config/household-migration-v1/python-keyring-metadata-v0.json"
);
#[cfg(unix)]
const D2_KEYRING_HOUSEHOLD: &[u8] = include_bytes!(
    "../../../fixtures/config/household-migration-v1/python-keyring-household-v0.json"
);
#[cfg(unix)]
const D2_SECRET_DISPOSITION_RESTART: &[u8] = include_bytes!(
    "../../../fixtures/config/household-migration-v1/python-secret-disposition-restart-valid.json"
);

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "heyfood-python-import-{name}-{}-{nonce}",
            std::process::id()
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

fn fixture(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = root.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

#[cfg(unix)]
fn d2_migration(
    root: &TempRoot,
    source: Option<&[u8]>,
) -> (
    LegacyPythonHouseholdMigrationV1,
    AccountId,
    HouseholdAccountSlotV1,
    HouseholdVault,
) {
    let config_root = root.0.join("legacy-config");
    std::fs::create_dir_all(&config_root).unwrap();
    if let Some(source) = source {
        let current = config_root.join("heyfood");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::write(current.join("config.json"), source).unwrap();
    }
    let config_root = LegacyPythonConfigRootV1::from_absolute_root(config_root).unwrap();
    let snapshot = root.0.join("native").join("python-state-import.v1.json");
    let migration = LegacyPythonHouseholdMigrationV1::new(config_root, snapshot);
    let account = AccountId::parse("acct-migration-owner").unwrap();
    let vault_root = root.0.join("native-vault");
    let vault = HouseholdVault::open(&vault_root, account.clone()).unwrap();
    let slot = vault.account_slot().clone();
    (migration, account, slot, vault)
}

#[cfg(unix)]
fn d2_guard_source_identity(
    phase_a: &LegacyPythonPhaseAResultV1,
) -> HouseholdMigrationSourceIdentityV1 {
    match phase_a.source_identity() {
        LegacySourceIdentityV1::Present { source_digest, .. } => {
            HouseholdMigrationSourceIdentityV1::present(*source_digest.as_bytes())
        }
        LegacySourceIdentityV1::NoSource {
            source_set_fingerprint,
        } => HouseholdMigrationSourceIdentityV1::no_source(*source_set_fingerprint.as_bytes()),
    }
}

#[cfg(unix)]
fn d2_reserved_guard(
    phase_a: &LegacyPythonPhaseAResultV1,
    slot: &HouseholdAccountSlotV1,
) -> HouseholdMigrationGuardDocument {
    HouseholdMigrationGuardDocument::initializing_reserved(
        slot,
        d2_guard_source_identity(phase_a),
        Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
        Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap(),
        Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap(),
        CanonicalTimestampV1::parse("2026-07-30T12:00:00.000Z").unwrap(),
    )
    .unwrap()
}

#[cfg(unix)]
fn d2_context(
    phase_a: &LegacyPythonPhaseAResultV1,
    slot: &HouseholdAccountSlotV1,
) -> LegacyPythonPhaseBContextV1 {
    let guard = d2_reserved_guard(phase_a, slot);
    LegacyPythonPhaseBContextV1::from_reserved_guard(
        phase_a,
        slot,
        &guard,
        DisplayName::parse("Owner").unwrap(),
    )
    .unwrap()
}

fn safe_preview_fixture(root: &TempRoot) -> (PathBuf, PythonStateImporter) {
    let source = fixture(
        &root.0,
        "config.json",
        br#"{
            "account_user_id":"preview-account",
            "first_name":"Preview",
            "household":{
                "active_scope":"member-sarah",
                "members":[
                    {"id":"_self","name":"Preview","archived":false},
                    {"id":"member-sarah","name":"Sarah","archived":false}
                ]
            }
        }"#,
    );
    let importer = PythonStateImporter::under(&source, root.0.join("native"));
    importer.import().unwrap();
    (source, importer)
}

#[test]
fn python_state_preview_safe_snapshot_reads_no_mixed_source_bytes() {
    let root = TempRoot::new("preview-safe-no-read");
    let (source, importer) = safe_preview_fixture(&root);
    #[cfg(not(unix))]
    let _ = &source;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&source).unwrap().permissions();
        permissions.set_mode(0o000);
        std::fs::set_permissions(&source, permissions).unwrap();
    }
    let preview = importer.preview_state().unwrap();
    assert!(matches!(preview, PythonStatePreview::SafeSnapshot { .. }));
}

#[test]
fn python_state_preview_stat_visible_invalid_json_without_snapshot_is_protected() {
    let root = TempRoot::new("preview-invalid-protected");
    let source = fixture(&root.0, "config.json", b"{credential-canary: not-json}");
    let importer = PythonStateImporter::under(source, root.0.join("native"));
    let preview = importer.preview_state().unwrap();
    assert!(matches!(
        preview,
        PythonStatePreview::ProtectedUninspectedMixedSource {
            reason: ProtectedHouseholdReason::UninspectedMixedSource,
            ..
        }
    ));
    assert!(!format!("{preview:?}").contains("credential-canary"));
}

#[test]
fn python_state_preview_mixed_source_read_probe_count_is_zero_before_log() {
    let root = TempRoot::new("preview-unreadable-probe");
    let source = fixture(&root.0, "config.json", b"probe-canary");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&source).unwrap().permissions();
        permissions.set_mode(0o000);
        std::fs::set_permissions(&source, permissions).unwrap();
    }
    let importer = PythonStateImporter::under(source, root.0.join("native"));
    assert!(matches!(
        importer.preview_state().unwrap(),
        PythonStatePreview::ProtectedUninspectedMixedSource { .. }
    ));
}

#[test]
fn python_state_import_read_probe_activates_only_after_log() {
    let root = TempRoot::new("post-review-read");
    let source = fixture(&root.0, "config.json", b"{invalid-after-review");
    let importer = PythonStateImporter::under(source, root.0.join("native"));
    let preview = importer.preview_state().unwrap();
    let error = importer.verify_after_review(&preview).unwrap_err();
    assert_eq!(error.code, "python_import_format");
}

#[test]
fn python_state_preview_returns_explicit_no_source_without_synthetic_identity() {
    let root = TempRoot::new("preview-no-source");
    let importer = PythonStateImporter::under(root.0.join("missing.json"), root.0.join("native"));
    let preview = importer.preview_state().unwrap();
    assert!(matches!(preview, PythonStatePreview::NoSource { .. }));
    assert!(
        importer
            .verify_after_review(&preview)
            .unwrap()
            .state()
            .is_none()
    );
}

#[test]
fn python_state_preview_mixed_source_without_safe_snapshot_is_uninspected_protected() {
    let root = TempRoot::new("preview-mixed-protected");
    let source = fixture(&root.0, "config.json", BOUND_SOURCE);
    let importer = PythonStateImporter::under(source, root.0.join("native"));
    assert!(matches!(
        importer.preview_state().unwrap(),
        PythonStatePreview::ProtectedUninspectedMixedSource {
            reason: ProtectedHouseholdReason::UninspectedMixedSource,
            ..
        }
    ));
    assert!(!importer.destination_path().exists());
}

#[test]
fn python_state_preview_prior_keyring_not_read_returns_protected() {
    let root = TempRoot::new("preview-keyring");
    let source = fixture(&root.0, "config.json", KEYRING_SOURCE);
    let importer = PythonStateImporter::under(source, root.0.join("native"));
    importer.import().unwrap();
    assert!(matches!(
        importer.preview_state().unwrap(),
        PythonStatePreview::ProtectedUninspectedMixedSource {
            reason: ProtectedHouseholdReason::PriorImporterSkippedKeyring,
            ..
        }
    ));
}

#[test]
fn python_state_preview_never_collapses_protected_source_to_no_source() {
    let root = TempRoot::new("preview-no-collapse");
    let source = fixture(&root.0, "config.json", b"");
    let importer = PythonStateImporter::under(source, root.0.join("native"));
    assert!(!matches!(
        importer.preview_state().unwrap(),
        PythonStatePreview::NoSource { .. }
    ));
}

#[test]
fn python_state_preview_normalized_state_digest_has_golden_preimage() {
    let root = TempRoot::new("preview-normalized-golden");
    let (_source, importer) = safe_preview_fixture(&root);
    let preview = importer.preview_state().unwrap();
    let PythonStatePreview::SafeSnapshot {
        state,
        normalized_state_digest,
        ..
    } = preview
    else {
        panic!("expected safe snapshot");
    };
    let encoded = serde_json::to_vec(&state).unwrap();
    let mut preimage = b"heyfood.log.python-state.v1\0".to_vec();
    preimage.extend_from_slice(&(encoded.len() as u64).to_be_bytes());
    preimage.extend_from_slice(&encoded);
    assert_eq!(
        normalized_state_digest.as_str(),
        format!("{:x}", Sha256::digest(&preimage))
    );
}

#[test]
fn python_state_preview_never_places_mixed_content_digest_in_source_set() {
    let root = TempRoot::new("preview-no-content-digest");
    let (_source, importer) = safe_preview_fixture(&root);
    let preview = importer.preview_state().unwrap();
    let debug = format!("{:?}", preview.checked_source_set());
    assert!(debug.contains("content_digest_present: false"));
    assert_eq!(debug.matches("content_digest_present: true").count(), 1);
}

#[test]
fn python_state_no_source_detects_source_appearance_after_review() {
    let root = TempRoot::new("preview-source-appearance");
    let source = root.0.join("config.json");
    let importer = PythonStateImporter::under(&source, root.0.join("native"));
    let preview = importer.preview_state().unwrap();
    std::fs::write(source, BOUND_SOURCE).unwrap();
    assert_eq!(
        importer.verify_after_review(&preview).unwrap_err().code,
        "python_state_changed"
    );
}

#[test]
fn python_state_protected_metadata_or_snapshot_binding_change_after_review_fails() {
    let root = TempRoot::new("preview-protected-drift");
    let source = fixture(&root.0, "config.json", BOUND_SOURCE);
    let importer = PythonStateImporter::under(&source, root.0.join("native"));
    let preview = importer.preview_state().unwrap();
    std::fs::write(source, UNBOUND_SOURCE).unwrap();
    assert_eq!(
        importer.verify_after_review(&preview).unwrap_err().code,
        "python_state_changed"
    );
}

#[test]
fn python_state_preview_rejects_malformed_native_snapshot() {
    let root = TempRoot::new("preview-malformed-native");
    let native = root.0.join("native");
    std::fs::create_dir(&native).unwrap();
    std::fs::write(native.join("python-state-import.v1.json"), b"{broken").unwrap();
    let importer = PythonStateImporter::under(root.0.join("missing.json"), native);
    assert_eq!(
        importer.preview_state().unwrap_err().code,
        "python_snapshot_invalid"
    );
}

#[test]
fn python_state_preview_never_exposes_credential_fields_or_mixed_bytes() {
    let root = TempRoot::new("preview-redaction");
    let canary = "hf-secret-preview-canary";
    let source = fixture(
        &root.0,
        "config.json",
        format!("{{\"api_key\":\"{canary}\"").as_bytes(),
    );
    let importer = PythonStateImporter::under(source, root.0.join("native"));
    let preview = importer.preview_state().unwrap();
    assert!(!format!("{preview:?}").contains(canary));
}

#[test]
fn imports_bound_local_state_without_copying_credentials_or_mutating_source() {
    let root = TempRoot::new("bound");
    let source = fixture(&root.0, "config.json", BOUND_SOURCE);
    let source_before = std::fs::read(&source).unwrap();
    let destination = root.0.join("native");
    let importer = PythonStateImporter::under(&source, &destination);

    let report = importer.import().unwrap();
    assert_eq!(report.outcome, PythonImportOutcome::Imported);
    assert!(report.reauthentication_required);
    assert!(!report.requires_manual_action);
    assert_eq!(std::fs::read(&source).unwrap(), source_before);

    let state = importer.load_state().unwrap().unwrap();
    assert_eq!(state.account_user_id.as_deref(), Some("user-fixture-1"));
    assert_eq!(state.global["active_context"].as_str(), Some("production"));
    assert_eq!(
        state.account_scoped["last_conversation"]["conversation_id"].as_str(),
        Some("conversation-fixture")
    );
    assert!(
        state
            .account_scoped
            .contains_key("household_local_profiles")
    );
    assert!(
        state
            .account_scoped
            .contains_key("household_profile_outbox")
    );

    let native = std::fs::read_to_string(importer.destination_path()).unwrap();
    for secret in [
        "hf_api_fixture_secret",
        "hf_oauth_fixture_access",
        "hf_oauth_fixture_refresh",
        "hf_session_fixture_access",
        "hf_session_fixture_refresh",
    ] {
        assert!(!native.contains(secret));
        assert!(!format!("{report:?}").contains(secret));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(importer.destination_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn repeat_is_idempotent_and_a_different_source_cannot_overwrite_state() {
    let root = TempRoot::new("idempotent");
    let source = fixture(&root.0, "config.json", BOUND_SOURCE);
    let importer = PythonStateImporter::under(&source, root.0.join("native"));
    let first = importer.import().unwrap();
    let destination_before = std::fs::read(importer.destination_path()).unwrap();

    let second = importer.import().unwrap();
    assert_eq!(second.outcome, PythonImportOutcome::AlreadyImported);
    assert_eq!(first.source_sha256, second.source_sha256);
    assert_eq!(
        std::fs::read(importer.destination_path()).unwrap(),
        destination_before
    );

    std::fs::write(&source, UNBOUND_SOURCE).unwrap();
    let error = importer.import().unwrap_err();
    assert_eq!(error.code, "python_import_conflict");
    assert_eq!(
        std::fs::read(importer.destination_path()).unwrap(),
        destination_before
    );
}

#[test]
fn unbound_and_unknown_state_is_reported_and_never_silently_copied() {
    let root = TempRoot::new("unbound");
    let source = fixture(&root.0, "config.json", UNBOUND_SOURCE);
    let importer = PythonStateImporter::under(&source, root.0.join("native"));

    let report = importer.import().unwrap();
    assert!(report.requires_manual_action);
    let action = |field: &str| {
        report
            .dispositions
            .iter()
            .find(|item| item.field == field)
            .unwrap()
            .action
    };
    assert_eq!(
        action("household_local_profiles"),
        PythonFieldAction::BlockedUnbound
    );
    assert_eq!(action("location"), PythonFieldAction::BlockedUnbound);
    assert_eq!(
        action("unknown_future_state"),
        PythonFieldAction::Unsupported
    );

    let state = importer.load_state().unwrap().unwrap();
    assert!(state.account_scoped.is_empty());
    assert!(!state.global.contains_key("unknown_future_state"));
}

#[test]
fn keyring_metadata_preserves_account_binding_but_requires_manual_reconciliation() {
    let root = TempRoot::new("keyring");
    let source = fixture(&root.0, "config.json", KEYRING_SOURCE);
    let importer = PythonStateImporter::under(source, root.0.join("native"));

    let report = importer.import().unwrap();
    assert!(report.reauthentication_required);
    assert!(report.requires_manual_action);
    assert_eq!(
        report
            .dispositions
            .iter()
            .find(|item| item.field == "credential_store")
            .unwrap()
            .action,
        PythonFieldAction::KeyringNotRead
    );
    let state = importer.load_state().unwrap().unwrap();
    assert_eq!(
        state.account_user_id.as_deref(),
        Some("user-keyring-fixture")
    );
    assert!(state.account_scoped.contains_key("location"));
}

#[test]
fn missing_malformed_and_symlink_sources_fail_closed_without_writes() {
    let root = TempRoot::new("fail-closed");
    let missing = PythonStateImporter::under(root.0.join("missing.json"), root.0.join("missing"));
    assert_eq!(
        missing.import().unwrap().outcome,
        PythonImportOutcome::NoSource
    );
    assert!(!missing.destination_path().exists());

    let malformed_path = fixture(&root.0, "malformed.json", b"{not-json");
    let malformed = PythonStateImporter::under(&malformed_path, root.0.join("malformed"));
    assert_eq!(malformed.import().unwrap_err().code, "python_import_format");
    assert!(!malformed.destination_path().exists());

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let target = fixture(&root.0, "target.json", BOUND_SOURCE);
        let link = root.0.join("linked.json");
        symlink(target, &link).unwrap();
        let linked = PythonStateImporter::under(link, root.0.join("linked"));
        assert_eq!(linked.import().unwrap_err().code, "python_import_symlink");
        assert!(!linked.destination_path().exists());

        let destination_target = root.0.join("destination-target");
        std::fs::create_dir(&destination_target).unwrap();
        let destination_link = root.0.join("destination-link");
        symlink(destination_target, &destination_link).unwrap();
        let destination_source = fixture(&root.0, "destination-source.json", BOUND_SOURCE);
        let linked_destination = PythonStateImporter::under(destination_source, destination_link);
        assert_eq!(
            linked_destination.import().unwrap_err().code,
            "python_import_destination_symlink"
        );
    }
}

#[tokio::test]
#[cfg(unix)]
async fn d2_phase_a_then_b_preserves_valid_owner_only_state_and_is_replay_stable() {
    let root = TempRoot::new("d2-owner-only");
    let (migration, account, slot, vault) = d2_migration(&root, Some(D2_OWNER_ONLY));
    let lifecycle = vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .unwrap();
    let source_lease = migration
        .acquire_source_lease(lifecycle, CancellationToken::new())
        .await
        .unwrap();
    let probes = migration
        .authoritative_missing_keyring_probes(&slot)
        .unwrap();

    let phase_a = migration
        .phase_a(
            &account,
            &slot,
            &source_lease,
            &probes,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(matches!(phase_a, LegacyPythonPhaseAResultV1::Present(_)));
    assert!(
        !migration
            .phase_a(
                &account,
                &slot,
                &source_lease,
                &probes,
                CancellationToken::new(),
            )
            .await
            .unwrap()
            .is_no_source()
    );

    let context = d2_context(&phase_a, &slot);
    let source_vault_lease = migration
        .acquire_source_vault_lease(
            source_lease,
            &vault,
            HouseholdVaultLeaseModeV1::CreateIfMissing,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let first = migration
        .phase_b(
            &phase_a,
            &context,
            &slot,
            source_vault_lease.vault_lease(),
            source_vault_lease.source_lease(),
            &probes,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let replay = migration
        .phase_b(
            &phase_a,
            &context,
            &slot,
            source_vault_lease.vault_lease(),
            source_vault_lease.source_lease(),
            &probes,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(first, replay);
    assert_eq!(first.state.account_binding, account);
    assert!(first.state.members.is_empty());
    assert_eq!(
        first.state.owner.profile_state,
        HouseholdProfileStateV1::LocalOnly
    );
    assert_eq!(
        first.state.active_scope,
        HouseholdScope::Subject(HouseholdSubjectId::self_())
    );
    assert_eq!(first.state.updated_at.as_str(), "2026-07-29T10:11:12.987Z");
    assert_eq!(
        first.state.owner.created_at.as_str(),
        "2025-01-02T03:04:05.123Z"
    );
    assert_eq!(first.state.profiles.len(), 1);
    assert!(
        first.state.profiles[0]
            .document
            .effective_profile()
            .unwrap()
            .is_some()
    );
    let resolved = first.resolve_initialization().unwrap();
    assert!(
        first
            .verify_vault_readback(&resolved.command, &resolved.resolved_state)
            .is_ok()
    );
    assert!(!root.0.join("native/python-state-import.v1.json").exists());
}

#[tokio::test]
#[cfg(unix)]
async fn d2_accepts_each_frozen_household_family_and_its_only_allowed_outbox_shape() {
    for (name, source, members, profiles, outbox) in [
        ("showcase", D2_RUST_SHOWCASE, 0, 0, 0),
        ("fixture-v4", D2_RUST_FIXTURE_V4, 1, 1, 1),
        ("explicit-v0", D2_RUST_EXPLICIT, 1, 1, 1),
        ("implicit-v0", D2_RUST_IMPLICIT, 1, 0, 0),
    ] {
        let root = TempRoot::new(name);
        let (migration, account, slot, vault) = d2_migration(&root, Some(source));
        let lifecycle = vault
            .acquire_lifecycle_lease(CancellationToken::new())
            .await
            .unwrap();
        let source_lease = migration
            .acquire_source_lease(lifecycle, CancellationToken::new())
            .await
            .unwrap();
        let probes = migration
            .authoritative_missing_keyring_probes(&slot)
            .unwrap();
        let phase_a = migration
            .phase_a(
                &account,
                &slot,
                &source_lease,
                &probes,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let context = d2_context(&phase_a, &slot);
        let source_vault_lease = migration
            .acquire_source_vault_lease(
                source_lease,
                &vault,
                HouseholdVaultLeaseModeV1::CreateIfMissing,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let result = migration
            .phase_b(
                &phase_a,
                &context,
                &slot,
                source_vault_lease.vault_lease(),
                source_vault_lease.source_lease(),
                &probes,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(result.state.members.len(), members, "{name}");
        assert_eq!(result.state.profiles.len(), profiles, "{name}");
        assert_eq!(result.state.outbox.len(), outbox, "{name}");
        assert!(result.state.canonical_bytes().is_ok(), "{name}");
    }

    let root = TempRoot::new("keyring-v0");
    let (migration, account, slot, vault) = d2_migration(&root, Some(D2_KEYRING_METADATA));
    let lifecycle = vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .unwrap();
    let source_lease = migration
        .acquire_source_lease(lifecycle, CancellationToken::new())
        .await
        .unwrap();
    let keyring = migration
        .bind_keyring_probes(
            &slot,
            LegacyPythonKeyringProbeOutcomeV1::Present(D2_KEYRING_HOUSEHOLD.to_vec()),
            LegacyPythonKeyringProbeOutcomeV1::AuthoritativeMissing,
        )
        .unwrap();
    let phase_a = migration
        .phase_a(
            &account,
            &slot,
            &source_lease,
            &keyring,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let context = d2_context(&phase_a, &slot);
    let source_vault_lease = migration
        .acquire_source_vault_lease(
            source_lease,
            &vault,
            HouseholdVaultLeaseModeV1::CreateIfMissing,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let result = migration
        .phase_b(
            &phase_a,
            &context,
            &slot,
            source_vault_lease.vault_lease(),
            source_vault_lease.source_lease(),
            &keyring,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.state.members.len(), 1);
    assert_eq!(result.state.profiles.len(), 1);
    assert_eq!(result.state.outbox.len(), 1);
    assert_eq!(
        result.state.members[0].profile_state,
        HouseholdProfileStateV1::LocalOnly
    );
    assert!(
        result.state.outbox[0]
            .outbox_id
            .as_str()
            .starts_with("legacy-py-patch-v0-")
    );
}

#[tokio::test]
#[cfg(unix)]
async fn d2_rejects_family_outbox_cross_product_in_phase_b() {
    let source = br#"{
      "account_user_id":"acct-migration-owner",
      "credential_store":"file",
      "first_name":"Owner",
      "household":{
        "version":1,
        "active_scope":"_self",
        "members":[
          {"id":"_self","name":"Owner","relationship":"self","archived":false}
        ]
      },
      "household_profile_outbox":{
        "_self":{"local_context":{"restrictions":["peanut"]}}
      }
    }"#;
    let root = TempRoot::new("d2-cross-product");
    let (migration, account, slot, vault) = d2_migration(&root, Some(source));
    let lifecycle = vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .unwrap();
    let source_lease = migration
        .acquire_source_lease(lifecycle, CancellationToken::new())
        .await
        .unwrap();
    let probes = migration
        .authoritative_missing_keyring_probes(&slot)
        .unwrap();
    let phase_a = migration
        .phase_a(
            &account,
            &slot,
            &source_lease,
            &probes,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let context = d2_context(&phase_a, &slot);
    let source_vault_lease = migration
        .acquire_source_vault_lease(
            source_lease,
            &vault,
            HouseholdVaultLeaseModeV1::CreateIfMissing,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        migration
            .phase_b(
                &phase_a,
                &context,
                &slot,
                source_vault_lease.vault_lease(),
                source_vault_lease.source_lease(),
                &probes,
                CancellationToken::new(),
            )
            .await
            .unwrap_err()
            .code,
        "legacy_python_semantic_validation"
    );
}

#[tokio::test]
#[cfg(unix)]
async fn d2_no_source_requires_authoritative_probes_and_replays_exact_fingerprint() {
    let root = TempRoot::new("d2-no-source");
    let (migration, account, slot, vault) = d2_migration(&root, None);
    let lifecycle = vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .unwrap();
    let source_lease = migration
        .acquire_source_lease(lifecycle, CancellationToken::new())
        .await
        .unwrap();
    let probes = migration
        .authoritative_missing_keyring_probes(&slot)
        .unwrap();
    let first = migration
        .phase_a(
            &account,
            &slot,
            &source_lease,
            &probes,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let replay = migration
        .phase_a(
            &account,
            &slot,
            &source_lease,
            &probes,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(first, replay);
    assert!(first.is_no_source());
    assert_eq!(first.source_identity(), replay.source_identity());

    let context = d2_context(&first, &slot);
    let source_vault_lease = migration
        .acquire_source_vault_lease(
            source_lease,
            &vault,
            HouseholdVaultLeaseModeV1::CreateIfMissing,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let result = migration
        .phase_b(
            &first,
            &context,
            &slot,
            source_vault_lease.vault_lease(),
            source_vault_lease.source_lease(),
            &probes,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(result.state.members.is_empty());
    assert!(result.state.profiles.is_empty());
    assert_eq!(
        result.state.owner.profile_state,
        HouseholdProfileStateV1::Incomplete
    );
    let source_lease = migration
        .release_source_vault_lease(source_vault_lease, CancellationToken::new())
        .await
        .unwrap();

    let unavailable = migration
        .bind_keyring_probes(
            &slot,
            LegacyPythonKeyringProbeOutcomeV1::Unavailable,
            LegacyPythonKeyringProbeOutcomeV1::AuthoritativeMissing,
        )
        .unwrap();
    let error = migration
        .phase_a(
            &account,
            &slot,
            &source_lease,
            &unavailable,
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "legacy_python_source_probe_unavailable");
    assert!(!matches!(
        migration
            .phase_a(
                &account,
                &slot,
                &source_lease,
                &unavailable,
                CancellationToken::new(),
            )
            .await,
        Ok(LegacyPythonPhaseAResultV1::NoSource { .. })
    ));

    let present_empty = migration
        .bind_keyring_probes(
            &slot,
            LegacyPythonKeyringProbeOutcomeV1::Present(b"{}".to_vec()),
            LegacyPythonKeyringProbeOutcomeV1::AuthoritativeMissing,
        )
        .unwrap();
    let empty_entry = migration
        .phase_a(
            &account,
            &slot,
            &source_lease,
            &present_empty,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(empty_entry.is_no_source());
    assert_ne!(empty_entry.source_identity(), first.source_identity());
}

#[test]
#[cfg(unix)]
fn d2_relative_xdg_is_ambiguous_before_any_source_probe() {
    let error = LegacyPythonConfigRootV1::from_environment_values(
        Some(std::ffi::OsStr::new(".")),
        Some(Path::new("/tmp")),
    )
    .unwrap_err();
    assert_eq!(error.code, "legacy_python_config_root_ambiguous");
    assert!(error.message.contains("XDG_CONFIG_HOME"));
}

#[tokio::test]
#[cfg(unix)]
async fn d2_phase_a_rejects_duplicate_names_and_unsafe_numbers_without_freezing_time() {
    let root = TempRoot::new("d2-malformed");
    let (migration, account, slot, vault) = d2_migration(&root, Some(D2_MALFORMED_DUPLICATE));
    let lifecycle = vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .unwrap();
    let source_lease = migration
        .acquire_source_lease(lifecycle, CancellationToken::new())
        .await
        .unwrap();
    let probes = migration
        .authoritative_missing_keyring_probes(&slot)
        .unwrap();
    assert_eq!(
        migration
            .phase_a(
                &account,
                &slot,
                &source_lease,
                &probes,
                CancellationToken::new(),
            )
            .await
            .unwrap_err()
            .code,
        "legacy_python_source_syntax"
    );

    let unsafe_root = TempRoot::new("d2-unsafe-number");
    let unsafe_source = br#"{
        "account_user_id":"acct-migration-owner",
        "credential_store":"file",
        "extension":{"unsafe":9007199254740992}
    }"#;
    let (migration, account, slot, vault) = d2_migration(&unsafe_root, Some(unsafe_source));
    let lifecycle = vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .unwrap();
    let source_lease = migration
        .acquire_source_lease(lifecycle, CancellationToken::new())
        .await
        .unwrap();
    let probes = migration
        .authoritative_missing_keyring_probes(&slot)
        .unwrap();
    assert_eq!(
        migration
            .phase_a(
                &account,
                &slot,
                &source_lease,
                &probes,
                CancellationToken::new(),
            )
            .await
            .unwrap_err()
            .code,
        "legacy_python_source_syntax"
    );
}

#[tokio::test]
#[cfg(unix)]
async fn d2_future_timestamp_and_dob_pass_phase_a_then_fail_phase_b() {
    for (name, source) in [
        ("future-time", D2_FUTURE_TIMESTAMP),
        ("future-dob", D2_FUTURE_DOB),
    ] {
        let root = TempRoot::new(name);
        let (migration, account, slot, vault) = d2_migration(&root, Some(source));
        let lifecycle = vault
            .acquire_lifecycle_lease(CancellationToken::new())
            .await
            .unwrap();
        let source_lease = migration
            .acquire_source_lease(lifecycle, CancellationToken::new())
            .await
            .unwrap();
        let probes = migration
            .authoritative_missing_keyring_probes(&slot)
            .unwrap();
        let phase_a = migration
            .phase_a(
                &account,
                &slot,
                &source_lease,
                &probes,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let context = d2_context(&phase_a, &slot);
        let source_vault_lease = migration
            .acquire_source_vault_lease(
                source_lease,
                &vault,
                HouseholdVaultLeaseModeV1::CreateIfMissing,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let error = migration
            .phase_b(
                &phase_a,
                &context,
                &slot,
                source_vault_lease.vault_lease(),
                source_vault_lease.source_lease(),
                &probes,
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "legacy_python_semantic_validation");
        assert!(!root.0.join("native/python-state-import.v1.json").exists());
    }
}

#[tokio::test]
#[cfg(unix)]
async fn d2_phase_b_rejects_source_change_and_never_reselects_or_refreezes() {
    let root = TempRoot::new("d2-source-change");
    let (migration, account, slot, vault) = d2_migration(&root, Some(D2_OWNER_ONLY));
    let lifecycle = vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .unwrap();
    let source_lease = migration
        .acquire_source_lease(lifecycle, CancellationToken::new())
        .await
        .unwrap();
    let probes = migration
        .authoritative_missing_keyring_probes(&slot)
        .unwrap();
    let phase_a = migration
        .phase_a(
            &account,
            &slot,
            &source_lease,
            &probes,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let context = d2_context(&phase_a, &slot);
    let source = root.0.join("legacy-config/heyfood/config.json");
    let mut changed = D2_OWNER_ONLY.to_vec();
    changed.push(b'\n');
    std::fs::write(source, changed).unwrap();

    let source_vault_lease = migration
        .acquire_source_vault_lease(
            source_lease,
            &vault,
            HouseholdVaultLeaseModeV1::CreateIfMissing,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let error = migration
        .phase_b(
            &phase_a,
            &context,
            &slot,
            source_vault_lease.vault_lease(),
            source_vault_lease.source_lease(),
            &probes,
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "legacy_python_source_changed");
}

#[tokio::test]
#[cfg(unix)]
async fn d2_secret_free_candidate_complete_dispositions_and_typed_restart_verify_after_readback() {
    let root = TempRoot::new("d2-secret-disposition-restart");
    let (migration, account, slot, vault) =
        d2_migration(&root, Some(D2_SECRET_DISPOSITION_RESTART));
    let lifecycle = vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .unwrap();
    let source_lease = migration
        .acquire_source_lease(lifecycle, CancellationToken::new())
        .await
        .unwrap();
    let probes = migration
        .authoritative_missing_keyring_probes(&slot)
        .unwrap();
    let phase_a = migration
        .phase_a(
            &account,
            &slot,
            &source_lease,
            &probes,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let phase_a_debug = format!("{phase_a:?}");
    for canary in [
        "candidate-api-key-canary",
        "candidate-oauth-canary",
        "candidate-refresh-canary",
        "candidate-session-canary",
        "candidate-session-refresh-canary",
    ] {
        assert!(!phase_a_debug.contains(canary));
    }
    let context = d2_context(&phase_a, &slot);
    let source_vault_lease = migration
        .acquire_source_vault_lease(
            source_lease,
            &vault,
            HouseholdVaultLeaseModeV1::CreateIfMissing,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let phase_b = migration
        .phase_b(
            &phase_a,
            &context,
            &slot,
            source_vault_lease.vault_lease(),
            source_vault_lease.source_lease(),
            &probes,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let resolved = phase_b.resolve_initialization().unwrap();
    let readback_json =
        String::from_utf8(resolved.resolved_state.canonical_bytes().unwrap()).unwrap();
    for canary in [
        "candidate-api-key-canary",
        "candidate-oauth-canary",
        "candidate-refresh-canary",
        "candidate-session-canary",
        "candidate-session-refresh-canary",
        "retire-without-retaining",
    ] {
        assert!(!readback_json.contains(canary));
    }

    let source: serde_json::Value = serde_json::from_slice(D2_SECRET_DISPOSITION_RESTART).unwrap();
    let expected_fields = source
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let actual_fields = phase_b
        .state
        .migration_dispositions
        .dispositions
        .iter()
        .map(|disposition| disposition.field_name.clone())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(actual_fields, expected_fields);
    for credential in ["api_key", "oauth", "session"] {
        let disposition = phase_b
            .state
            .migration_dispositions
            .dispositions
            .iter()
            .find(|disposition| disposition.field_name == credential)
            .unwrap();
        assert_eq!(
            disposition.disposition,
            MigrationDispositionKindV1::ReauthenticationRequired
        );
        assert!(disposition.source_digest.is_none());
        assert!(disposition.destination_digest.is_none());
    }
    let unknown = phase_b
        .state
        .migration_dispositions
        .dispositions
        .iter()
        .find(|disposition| disposition.field_name == "unknown_extension")
        .unwrap();
    assert_eq!(unknown.disposition, MigrationDispositionKindV1::Retired);
    assert!(unknown.source_digest.is_none());
    assert!(unknown.destination_digest.is_none());
    assert!(
        phase_b
            .state
            .migration_dispositions
            .dispositions
            .iter()
            .filter_map(|disposition| {
                disposition
                    .source_digest
                    .zip(disposition.destination_digest)
            })
            .all(|(source, destination)| source != destination)
    );

    let verification = phase_b
        .verify_vault_readback(&resolved.command, &resolved.resolved_state)
        .unwrap();
    let location = verification.restart_state.location.unwrap();
    assert_eq!(location.label(), "San Luis Obispo, CA");
    assert_eq!(
        location.canonical().as_value(),
        source.get("location").unwrap()
    );
    let restaurant_search = verification.restart_state.last_restaurant_search.unwrap();
    assert_eq!(
        restaurant_search.restaurant_names(),
        &["Garden Cafe".to_owned(), "Harbor Kitchen".to_owned()]
    );
    assert_eq!(
        restaurant_search.canonical().as_value(),
        source.get("last_restaurant_search").unwrap()
    );

    let mut tampered = resolved.resolved_state.clone();
    let location = tampered
        .imported_compatibility
        .fields
        .iter_mut()
        .find(|field| field.field_name == "location")
        .unwrap();
    location.value = heyfood_core::CanonicalJsonValueV1::from_value(
        serde_json::json!({
            "label": "Wrong",
            "latitude": 1.0,
            "longitude": 2.0
        }),
        1024,
    )
    .unwrap();
    assert_eq!(
        phase_b
            .verify_vault_readback(&resolved.command, &tampered)
            .unwrap_err()
            .code,
        "legacy_python_vault_readback_mismatch"
    );
}

#[tokio::test]
#[cfg(unix)]
async fn d2_phase_a_and_phase_b_are_cancellable_at_authority_boundaries() {
    let root = TempRoot::new("d2-cancellation");
    let (migration, account, slot, vault) = d2_migration(&root, Some(D2_OWNER_ONLY));
    let lifecycle = vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .unwrap();
    let source_lease = migration
        .acquire_source_lease(lifecycle, CancellationToken::new())
        .await
        .unwrap();
    let probes = migration
        .authoritative_missing_keyring_probes(&slot)
        .unwrap();
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert_eq!(
        migration
            .phase_a(&account, &slot, &source_lease, &probes, cancelled,)
            .await
            .unwrap_err()
            .code,
        "legacy_python_migration_cancelled"
    );
    let phase_a = migration
        .phase_a(
            &account,
            &slot,
            &source_lease,
            &probes,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let context = d2_context(&phase_a, &slot);
    let source_vault_lease = migration
        .acquire_source_vault_lease(
            source_lease,
            &vault,
            HouseholdVaultLeaseModeV1::CreateIfMissing,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert_eq!(
        migration
            .phase_b(
                &phase_a,
                &context,
                &slot,
                source_vault_lease.vault_lease(),
                source_vault_lease.source_lease(),
                &probes,
                cancelled,
            )
            .await
            .unwrap_err()
            .code,
        "legacy_python_migration_cancelled"
    );
}

#[tokio::test]
#[cfg(unix)]
async fn d2_bound_keyring_evidence_rejects_wrong_native_slot_and_config_root() {
    let root_a = TempRoot::new("d2-probe-binding-a");
    let (migration_a, account, slot_a, vault_a) = d2_migration(&root_a, Some(D2_OWNER_ONLY));
    let root_b = TempRoot::new("d2-probe-binding-b");
    let (migration_b, _, slot_b, vault_b) = d2_migration(&root_b, Some(D2_OWNER_ONLY));
    let probes_a = migration_a
        .authoritative_missing_keyring_probes(&slot_a)
        .unwrap();

    let lifecycle_b = vault_b
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .unwrap();
    let source_lease_a_for_b = migration_a
        .acquire_source_lease(lifecycle_b, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        migration_a
            .phase_a(
                &account,
                &slot_b,
                &source_lease_a_for_b,
                &probes_a,
                CancellationToken::new(),
            )
            .await
            .unwrap_err()
            .code,
        "legacy_python_keyring_evidence_mismatch"
    );
    drop(source_lease_a_for_b);

    let lifecycle_a = vault_a
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .unwrap();
    let source_lease_b_for_a = migration_b
        .acquire_source_lease(lifecycle_a, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        migration_b
            .phase_a(
                &account,
                &slot_a,
                &source_lease_b_for_a,
                &probes_a,
                CancellationToken::new(),
            )
            .await
            .unwrap_err()
            .code,
        "legacy_python_keyring_evidence_mismatch"
    );
}

#[tokio::test]
#[cfg(unix)]
async fn d2_phase_b_context_rejects_wrong_source_snapshot_and_completed_guard() {
    let root = TempRoot::new("d2-context-guard-binding");
    let (migration, account, slot, vault) = d2_migration(&root, Some(D2_OWNER_ONLY));
    let lifecycle = vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .unwrap();
    let source_lease = migration
        .acquire_source_lease(lifecycle, CancellationToken::new())
        .await
        .unwrap();
    let probes = migration
        .authoritative_missing_keyring_probes(&slot)
        .unwrap();
    let phase_a = migration
        .phase_a(
            &account,
            &slot,
            &source_lease,
            &probes,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let timestamp = CanonicalTimestampV1::parse("2026-07-30T12:00:00.000Z").unwrap();
    let wrong_source = HouseholdMigrationGuardDocument::initializing_reserved(
        &slot,
        HouseholdMigrationSourceIdentityV1::present([0x55; 32]),
        Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
        Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap(),
        Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap(),
        timestamp.clone(),
    )
    .unwrap();
    assert_eq!(
        LegacyPythonPhaseBContextV1::from_reserved_guard(
            &phase_a,
            &slot,
            &wrong_source,
            DisplayName::parse("Owner").unwrap(),
        )
        .unwrap_err()
        .code,
        "legacy_python_guard_reservation_mismatch"
    );

    let wrong_snapshot = HouseholdMigrationGuardDocument::initializing_reserved_with_snapshot(
        &slot,
        d2_guard_source_identity(&phase_a),
        Some(LegacyPythonSnapshotProvenanceV1 {
            locator_digest: CanonicalDigestV1::from_bytes([0x66; 32]),
            content_digest: CanonicalDigestV1::from_bytes([0x77; 32]),
        }),
        Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
        Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap(),
        Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap(),
        timestamp,
    )
    .unwrap();
    assert_eq!(
        LegacyPythonPhaseBContextV1::from_reserved_guard(
            &phase_a,
            &slot,
            &wrong_snapshot,
            DisplayName::parse("Owner").unwrap(),
        )
        .unwrap_err()
        .code,
        "legacy_python_guard_reservation_mismatch"
    );

    let completed = d2_reserved_guard(&phase_a, &slot)
        .ready_to_initialize([0x88; 32], [0x99; 32])
        .unwrap()
        .complete_initialization()
        .unwrap();
    assert_eq!(
        LegacyPythonPhaseBContextV1::from_reserved_guard(
            &phase_a,
            &slot,
            &completed,
            DisplayName::parse("Owner").unwrap(),
        )
        .unwrap_err()
        .code,
        "legacy_python_guard_reservation_mismatch"
    );
}

#[tokio::test]
#[cfg(unix)]
async fn d2_verified_readback_retires_only_the_exact_snapshot_bytes() {
    let root = TempRoot::new("d2-verified-snapshot-retirement");
    let (migration, account, slot, vault) = d2_migration(&root, Some(D2_OWNER_ONLY));
    PythonStateImporter::under(
        migration.config_path(LegacyPythonConfigKindV1::Current),
        migration.snapshot_path().parent().unwrap(),
    )
    .import()
    .unwrap();
    let exact_snapshot = std::fs::read(migration.snapshot_path()).unwrap();
    let lifecycle = vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .unwrap();
    let source_lease = migration
        .acquire_source_lease(lifecycle, CancellationToken::new())
        .await
        .unwrap();
    let probes = migration
        .authoritative_missing_keyring_probes(&slot)
        .unwrap();
    let phase_a = migration
        .phase_a(
            &account,
            &slot,
            &source_lease,
            &probes,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(phase_a.snapshot_provenance().is_some());
    let reserved = HouseholdMigrationGuardDocument::initializing_reserved_with_snapshot(
        &slot,
        d2_guard_source_identity(&phase_a),
        phase_a.snapshot_provenance(),
        Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
        Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap(),
        Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap(),
        CanonicalTimestampV1::parse("2026-07-30T12:00:00.000Z").unwrap(),
    )
    .unwrap();
    let context = LegacyPythonPhaseBContextV1::from_reserved_guard(
        &phase_a,
        &slot,
        &reserved,
        DisplayName::parse("Owner").unwrap(),
    )
    .unwrap();
    let source_vault_lease = migration
        .acquire_source_vault_lease(
            source_lease,
            &vault,
            HouseholdVaultLeaseModeV1::CreateIfMissing,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let phase_b = migration
        .phase_b(
            &phase_a,
            &context,
            &slot,
            source_vault_lease.vault_lease(),
            source_vault_lease.source_lease(),
            &probes,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let resolved = phase_b.resolve_initialization().unwrap();
    let verification = phase_b
        .verify_vault_readback(&resolved.command, &resolved.resolved_state)
        .unwrap();

    std::fs::write(migration.snapshot_path(), b"changed-after-readback").unwrap();
    assert_eq!(
        migration
            .retire_verified_snapshot(
                source_vault_lease.source_lease(),
                &verification,
                CancellationToken::new(),
            )
            .await
            .unwrap_err()
            .code,
        "legacy_python_snapshot_retirement_mismatch"
    );
    assert!(migration.snapshot_path().exists());
    std::fs::write(migration.snapshot_path(), exact_snapshot).unwrap();
    migration
        .retire_verified_snapshot(
            source_vault_lease.source_lease(),
            &verification,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!migration.snapshot_path().exists());
}

#[tokio::test]
#[cfg(unix)]
async fn d2_committed_snapshot_resume_uses_only_guard_vault_and_exact_snapshot() {
    let root = TempRoot::new("d2-committed-snapshot-resume");
    let (migration, account, slot, vault) = d2_migration(&root, Some(D2_OWNER_ONLY));
    PythonStateImporter::under(
        migration.config_path(LegacyPythonConfigKindV1::Current),
        migration.snapshot_path().parent().unwrap(),
    )
    .import()
    .unwrap();
    let exact_snapshot = std::fs::read(migration.snapshot_path()).unwrap();
    let lifecycle = vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .unwrap();
    let source_lease = migration
        .acquire_source_lease(lifecycle, CancellationToken::new())
        .await
        .unwrap();
    let probes = migration
        .authoritative_missing_keyring_probes(&slot)
        .unwrap();
    let phase_a = migration
        .phase_a(
            &account,
            &slot,
            &source_lease,
            &probes,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let snapshot_provenance = phase_a.snapshot_provenance().unwrap();
    let reserved = HouseholdMigrationGuardDocument::initializing_reserved_with_snapshot(
        &slot,
        d2_guard_source_identity(&phase_a),
        Some(snapshot_provenance.clone()),
        Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
        Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap(),
        Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap(),
        CanonicalTimestampV1::parse("2026-07-30T12:00:00.000Z").unwrap(),
    )
    .unwrap();
    let context = LegacyPythonPhaseBContextV1::from_reserved_guard(
        &phase_a,
        &slot,
        &reserved,
        DisplayName::parse("Owner").unwrap(),
    )
    .unwrap();
    let source_vault_lease = migration
        .acquire_source_vault_lease(
            source_lease,
            &vault,
            HouseholdVaultLeaseModeV1::CreateIfMissing,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let phase_b = migration
        .phase_b(
            &phase_a,
            &context,
            &slot,
            source_vault_lease.vault_lease(),
            source_vault_lease.source_lease(),
            &probes,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let resolved = phase_b.resolve_initialization().unwrap();
    let committed = reserved
        .ready_to_initialize(
            *resolved.initial_effect_fingerprint.as_digest().as_bytes(),
            *resolved.canonical_state_digest.as_bytes(),
        )
        .unwrap()
        .complete_initialization()
        .unwrap();
    let source_lease = migration
        .release_source_vault_lease(source_vault_lease, CancellationToken::new())
        .await
        .unwrap();
    drop(source_lease);

    std::fs::remove_dir_all(root.0.join("legacy-config")).unwrap();
    let lifecycle = vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .unwrap();
    let snapshot_lease = migration
        .acquire_snapshot_retirement_lease(lifecycle, CancellationToken::new())
        .await
        .unwrap();
    let snapshot_vault_lease = migration
        .acquire_snapshot_vault_lease(
            snapshot_lease,
            &vault,
            HouseholdVaultLeaseModeV1::CreateIfMissing,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let mut later_state = resolved.resolved_state.clone();
    let later_time = CanonicalTimestampV1::parse("2026-07-30T12:01:00.000Z").unwrap();
    later_state.revision = HouseholdRevision::new(2).unwrap();
    later_state.updated_at = later_time.clone();
    later_state
        .bounded_applied_commits
        .push(AppliedCommitRecordV1 {
            commit_id: CommitId::from_uuid(
                Uuid::parse_str("44444444-4444-4444-8444-444444444444").unwrap(),
            ),
            fingerprint: CanonicalDigestV1::from_bytes([0x44; 32]),
            resulting_revision: HouseholdRevision::new(2).unwrap(),
            outcome: AppliedCommitOutcomeV1::Committed,
            committed_at: later_time,
        });
    later_state.bounded_applied_commits.sort_by(|left, right| {
        left.commit_id
            .as_uuid()
            .as_bytes()
            .cmp(right.commit_id.as_uuid().as_bytes())
    });
    let authenticated_later_load = HouseholdLoad::from_state(later_state).unwrap();

    let wrong_source = HouseholdMigrationGuardDocument::initializing_reserved_with_snapshot(
        &slot,
        HouseholdMigrationSourceIdentityV1::present([0x55; 32]),
        Some(snapshot_provenance.clone()),
        committed.migration_id(),
        committed.initialization_id(),
        committed.initial_commit_id(),
        committed.migration_frozen_at().clone(),
    )
    .unwrap()
    .ready_to_initialize(
        committed.initial_effect_fingerprint().unwrap(),
        committed.initial_state_digest().unwrap(),
    )
    .unwrap()
    .complete_initialization()
    .unwrap();
    assert_eq!(
        migration
            .committed_snapshot_retirement_authority(
                snapshot_vault_lease.snapshot_lease(),
                snapshot_vault_lease.vault_lease(),
                &wrong_source,
                &authenticated_later_load,
            )
            .unwrap_err()
            .code,
        "legacy_python_committed_snapshot_authority_mismatch"
    );

    let wrong_snapshot = HouseholdMigrationGuardDocument::initializing_reserved_with_snapshot(
        &slot,
        d2_guard_source_identity(&phase_a),
        Some(LegacyPythonSnapshotProvenanceV1 {
            locator_digest: snapshot_provenance.locator_digest,
            content_digest: CanonicalDigestV1::from_bytes([0x77; 32]),
        }),
        committed.migration_id(),
        committed.initialization_id(),
        committed.initial_commit_id(),
        committed.migration_frozen_at().clone(),
    )
    .unwrap()
    .ready_to_initialize(
        committed.initial_effect_fingerprint().unwrap(),
        committed.initial_state_digest().unwrap(),
    )
    .unwrap()
    .complete_initialization()
    .unwrap();
    assert_eq!(
        migration
            .committed_snapshot_retirement_authority(
                snapshot_vault_lease.snapshot_lease(),
                snapshot_vault_lease.vault_lease(),
                &wrong_snapshot,
                &authenticated_later_load,
            )
            .unwrap_err()
            .code,
        "legacy_python_committed_snapshot_authority_mismatch"
    );

    let authority = migration
        .committed_snapshot_retirement_authority(
            snapshot_vault_lease.snapshot_lease(),
            snapshot_vault_lease.vault_lease(),
            &committed,
            &authenticated_later_load,
        )
        .unwrap();
    std::fs::write(migration.snapshot_path(), b"changed-after-commit").unwrap();
    assert_eq!(
        migration
            .retire_committed_snapshot(
                snapshot_vault_lease.snapshot_lease(),
                &authority,
                CancellationToken::new(),
            )
            .await
            .unwrap_err()
            .code,
        "legacy_python_snapshot_retirement_mismatch"
    );
    std::fs::write(migration.snapshot_path(), exact_snapshot).unwrap();
    migration
        .retire_committed_snapshot(
            snapshot_vault_lease.snapshot_lease(),
            &authority,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!migration.snapshot_path().exists());
}

#[tokio::test]
#[cfg(unix)]
async fn d2_phase_a_rejects_authenticated_account_mismatch() {
    let root = TempRoot::new("d2-account-mismatch");
    let (migration, _account, slot, vault) = d2_migration(&root, Some(D2_OWNER_ONLY));
    let lifecycle = vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .unwrap();
    let source_lease = migration
        .acquire_source_lease(lifecycle, CancellationToken::new())
        .await
        .unwrap();
    let other = AccountId::parse("acct-other").unwrap();
    let probes = migration
        .authoritative_missing_keyring_probes(&slot)
        .unwrap();
    let error = migration
        .phase_a(
            &other,
            &slot,
            &source_lease,
            &probes,
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "legacy_python_account_mismatch");
}

#[test]
#[cfg(windows)]
fn imported_state_has_a_non_inherited_owner_only_windows_acl() {
    use std::process::Command;

    let root = TempRoot::new("windows-acl");
    let source = fixture(&root.0, "config.json", BOUND_SOURCE);
    let importer = PythonStateImporter::under(source, root.0.join("native"));
    importer.import().unwrap();

    let output = Command::new("icacls")
        .arg(importer.destination_path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let acl = String::from_utf8_lossy(&output.stdout);
    assert!(
        !acl.contains("(I)"),
        "ACL must not retain inherited entries: {acl}"
    );
    assert_eq!(
        acl.matches("(F)").count(),
        1,
        "ACL must grant full control only to the current SID: {acl}"
    );
}
