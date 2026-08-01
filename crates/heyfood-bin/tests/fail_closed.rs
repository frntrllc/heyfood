use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

struct TempHome(std::path::PathBuf);

impl TempHome {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "heyfood-functional-cut-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn bare_binary_prints_only_runnable_native_next_steps() {
    let output = Command::new(env!("CARGO_BIN_EXE_heyfood"))
        .output()
        .expect("native binary should run");

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).expect("guidance should be UTF-8");
    assert!(stdout.contains("heyfood register"));
    assert!(stdout.contains("heyfood ask"));
    assert!(stdout.contains("heyfood --help"));
    assert!(!stdout.contains('\u{1b}'), "must not enter terminal modes");
    assert!(!stdout.contains("██"), "must not emit a giant banner");
}

#[test]
fn authenticated_one_shot_route_fails_with_account_connection_guidance_when_disconnected() {
    let root = TempHome::new();
    let mut command = Command::new(env!("CARGO_BIN_EXE_heyfood"));
    command
        .args(["ask", "What can I eat?"])
        .env("HOME", &root.0)
        .env("XDG_CONFIG_HOME", &root.0)
        .env("HEYFOOD_STATE_DIR", &root.0);
    #[cfg(not(windows))]
    command.env("HEYFOOD_CREDENTIAL_STORE", "file");
    #[cfg(windows)]
    command.env("HEYFOOD_CREDENTIAL_STORE", "native");
    let output = command.output().expect("native binary should run");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("diagnostic should be UTF-8");
    assert!(stderr.contains("heyfood login"));
    assert!(!stderr.contains("qualification"));
    assert!(!stderr.contains("cannot start"));
}

#[test]
fn interactive_chat_rejects_json_without_entering_terminal_mode() {
    let output = Command::new(env!("CARGO_BIN_EXE_heyfood"))
        .args(["--json", "chat"])
        .output()
        .expect("native binary should run");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["type"], "interactive_json_unsupported");
}

#[test]
fn interactive_chat_requires_a_tty() {
    let output = Command::new(env!("CARGO_BIN_EXE_heyfood"))
        .arg("chat")
        .output()
        .expect("native binary should run");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("requires terminal input and output"));
    assert!(!stderr.contains('\u{1b}'));
}

#[test]
fn json_completion_is_rejected_as_one_json_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_heyfood"))
        .args(["--json", "completion", "bash"])
        .output()
        .expect("native binary should run");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["type"], "completion_json_unsupported");
}

#[cfg(all(not(feature = "native-credentials"), not(windows)))]
fn create_private_directory(path: &std::path::Path) {
    use std::os::unix::fs::DirBuilderExt as _;

    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(path).unwrap();
}

#[cfg(all(not(feature = "native-credentials"), not(windows)))]
fn portable_authorization_bundle() -> heyfood_core::AuthCredentialBundle {
    heyfood_core::AuthCredentialBundle {
        channel: heyfood_core::ChannelCredentials::from_unix_expiry(
            "portable-client",
            "portable-device",
            heyfood_core::SensitiveString::new("channel-access"),
            heyfood_core::SensitiveString::new("channel-refresh"),
            4_102_444_800,
            "account:link profile:read",
        )
        .unwrap(),
        session: heyfood_core::SessionCredentials::from_unix_expiry(
            heyfood_core::AccountId::parse("portable-account").unwrap(),
            heyfood_core::SensitiveString::new("session-access"),
            heyfood_core::SensitiveString::new("session-refresh"),
            heyfood_core::CredentialVersion::new(1),
            4_102_444_800,
        )
        .unwrap(),
    }
}

#[cfg(all(not(feature = "native-credentials"), not(windows)))]
fn install_pending_authorization_replacement(root: &std::path::Path) {
    let auth = heyfood_platform::NativeAuthStore::open(root).unwrap();
    let session = heyfood_platform::FileCredentialStore::open(root).unwrap();
    let expected = portable_authorization_bundle();
    auth.initialize_account_bound(&expected, &session).unwrap();
    auth.begin_authorization_intent()
        .unwrap()
        .begin_authorization_replacement_if_current(
            "portable-pending-replacement".to_owned(),
            &expected,
            &session,
        )
        .unwrap();
}

#[cfg(all(not(feature = "native-credentials"), not(windows)))]
fn assert_fresh_authorization_blocked_without_network(root: &std::path::Path, command: &str) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_heyfood"))
        .args(["--json", command, "--no-browser", "--timeout", "1"])
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root)
        .env("HEYFOOD_STATE_DIR", root)
        .env("HEYFOOD_CREDENTIAL_STORE", "file")
        .env(
            "HEYFOOD_API_URL",
            format!("http://{}", listener.local_addr().unwrap()),
        )
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let rendered: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        rendered["error"]["type"],
        "household_native_credentials_required"
    );
    assert!(matches!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    ));
}

#[cfg(all(not(feature = "native-credentials"), not(windows)))]
#[test]
fn portable_fresh_login_and_registration_stop_before_provider_dispatch() {
    for evidence in ["compatibility", "household-teardown", "accounts"] {
        let root = TempHome::new();
        let native_root = root.0.join("data");
        create_private_directory(&native_root);
        let evidence_path = native_root.join(evidence);
        create_private_directory(&evidence_path);
        if evidence == "household-teardown" {
            use std::os::unix::fs::PermissionsExt as _;

            let journal = evidence_path.join("teardown-pending.htj");
            std::fs::write(&journal, b"pending").unwrap();
            std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        for command in ["login", "register"] {
            assert_fresh_authorization_blocked_without_network(&root.0, command);
        }
    }
}

#[cfg(all(not(feature = "native-credentials"), not(windows)))]
#[test]
fn portable_login_checks_native_barrier_before_resuming_pending_replacement() {
    let root = TempHome::new();
    install_pending_authorization_replacement(&root.0);
    create_private_directory(&root.0.join("data").join("compatibility"));

    assert_fresh_authorization_blocked_without_network(&root.0, "login");
}
