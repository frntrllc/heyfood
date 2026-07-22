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
fn authenticated_one_shot_route_fails_with_registration_guidance_when_disconnected() {
    let root = TempHome::new();
    let mut command = Command::new(env!("CARGO_BIN_EXE_heyfood"));
    command
        .args(["ask", "What can I eat?"])
        .env("HOME", &root.0)
        .env("XDG_CONFIG_HOME", &root.0);
    #[cfg(not(windows))]
    command.env("HEYFOOD_CREDENTIAL_STORE", "file");
    #[cfg(windows)]
    command.env("HEYFOOD_CREDENTIAL_STORE", "native");
    let output = command.output().expect("native binary should run");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("diagnostic should be UTF-8");
    assert!(stderr.contains("heyfood register"));
    assert!(!stderr.contains("qualification"));
    assert!(!stderr.contains("cannot start"));
}

#[test]
fn placeholder_command_returns_a_typed_failure() {
    let output = Command::new(env!("CARGO_BIN_EXE_heyfood"))
        .args(["--json", "chat"])
        .output()
        .expect("native binary should run");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["type"], "command_not_available");
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
