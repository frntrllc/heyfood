use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use heyfood_core::{PythonFieldAction, PythonImportOutcome};
use heyfood_platform::PythonStateImporter;

const BOUND_SOURCE: &[u8] = include_bytes!("../../../fixtures/config/python-0.3.2-file-state.json");
const UNBOUND_SOURCE: &[u8] =
    include_bytes!("../../../fixtures/config/python-0.3.2-unbound-state.json");
const KEYRING_SOURCE: &[u8] =
    include_bytes!("../../../fixtures/config/python-0.3.2-keyring-metadata.json");

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
