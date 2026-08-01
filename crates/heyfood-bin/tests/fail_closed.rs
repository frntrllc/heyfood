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

#[cfg(not(feature = "native-credentials"))]
fn grant_state_snapshot(root: &std::path::Path) -> Option<[u8; 32]> {
    use sha2::{Digest as _, Sha256};

    if !root.exists() {
        return None;
    }

    fn visit(base: &std::path::Path, path: &std::path::Path, digest: &mut sha2::Sha256) {
        use sha2::Digest as _;

        let metadata = std::fs::symlink_metadata(path).unwrap();
        let relative = path.strip_prefix(base).unwrap().to_string_lossy();
        digest.update(relative.len().to_be_bytes());
        digest.update(relative.as_bytes());
        let kind = if metadata.file_type().is_symlink() {
            2_u8
        } else if metadata.is_dir() {
            1_u8
        } else if metadata.is_file() {
            0_u8
        } else {
            3_u8
        };
        digest.update([kind]);
        digest.update(metadata.len().to_be_bytes());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            digest.update(metadata.permissions().mode().to_be_bytes());
        }
        #[cfg(not(unix))]
        digest.update([u8::from(metadata.permissions().readonly())]);

        if metadata.is_file() {
            digest.update(std::fs::read(path).unwrap());
        } else if metadata.is_dir() {
            let mut children = std::fs::read_dir(path)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>();
            children.sort();
            for child in children {
                visit(base, &child, digest);
            }
        }
    }

    let mut digest = Sha256::new();
    visit(root, root, &mut digest);
    Some(digest.finalize().into())
}

#[cfg(not(feature = "native-credentials"))]
fn assert_portable_grant_command_is_preflight_only(root: &std::path::Path, command: &str) {
    let before = grant_state_snapshot(root);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut invocation = Command::new(env!("CARGO_BIN_EXE_heyfood"));
    invocation
        .args([
            "--json",
            command,
            "--device",
            "--no-browser",
            "--timeout",
            "1",
        ])
        .env("HEYFOOD_STATE_DIR", root)
        .env(
            "HEYFOOD_API_URL",
            format!("http://{}", listener.local_addr().unwrap()),
        );
    #[cfg(not(windows))]
    invocation.env("HEYFOOD_CREDENTIAL_STORE", "file");
    #[cfg(windows)]
    invocation.env("HEYFOOD_CREDENTIAL_STORE", "native");
    let output = invocation.output().unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let rendered: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        rendered["error"]["type"],
        "household_native_credentials_required"
    );
    assert!(before == grant_state_snapshot(root), "grant state changed");
    assert!(matches!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    ));
}

#[cfg(not(feature = "native-credentials"))]
fn assert_both_portable_grant_commands_are_preflight_only(root: &std::path::Path) {
    for command in ["login", "register"] {
        assert_portable_grant_command_is_preflight_only(root, command);
    }
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
fn install_connected_authorization(root: &std::path::Path) {
    let auth = heyfood_platform::NativeAuthStore::open(root).unwrap();
    let session = heyfood_platform::FileCredentialStore::open(root).unwrap();
    auth.initialize_account_bound(&portable_authorization_bundle(), &session)
        .unwrap();
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

#[cfg(not(feature = "native-credentials"))]
#[test]
fn portable_grant_commands_touch_nothing_for_empty_state() {
    let home = TempHome::new();
    let absent_state = home.0.join("absent-state");

    assert_both_portable_grant_commands_are_preflight_only(&absent_state);
    assert!(!absent_state.exists());

    // NativePaths rejects relative overrides. Receiving the credential-gate
    // error proves the gate runs before even that local path validation.
    let relative_state = std::path::PathBuf::from(format!(
        "portable-relative-state-must-not-be-read-{}",
        std::process::id()
    ));
    assert!(!relative_state.exists());
    assert_both_portable_grant_commands_are_preflight_only(&relative_state);
    assert!(!relative_state.exists());
}

#[cfg(not(feature = "native-credentials"))]
#[test]
fn portable_grant_commands_do_not_inspect_or_change_native_evidence() {
    let home = TempHome::new();
    let state = home.0.join("evidence-state");
    for directory in [
        state.join("data/compatibility"),
        state.join("data/accounts/account-provenance"),
        state.join("data/household-teardown"),
    ] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::fs::write(
        state.join("data/household-teardown/teardown-pending.htj"),
        b"opaque-native-evidence",
    )
    .unwrap();

    assert_both_portable_grant_commands_are_preflight_only(&state);
}

#[cfg(all(not(feature = "native-credentials"), not(windows)))]
#[test]
fn portable_grant_commands_do_not_resume_a_pending_replacement() {
    let home = TempHome::new();
    let state = home.0.join("pending-state");
    install_pending_authorization_replacement(&state);

    assert_both_portable_grant_commands_are_preflight_only(&state);
}

#[cfg(all(not(feature = "native-credentials"), not(windows)))]
#[test]
fn portable_grant_commands_do_not_reauthorize_a_connected_account() {
    let home = TempHome::new();
    let state = home.0.join("connected-state");
    install_connected_authorization(&state);

    assert_both_portable_grant_commands_are_preflight_only(&state);
}
