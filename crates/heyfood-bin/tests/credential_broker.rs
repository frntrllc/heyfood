#![cfg(feature = "native-credentials")]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "heyfood-credential-broker-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = broker("delete", &self.0, &[]);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn broker(action: &str, root: &Path, input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_heyfood"))
        .args(["__heyfood_credential_broker", action])
        .arg(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

fn hex(value: &str) -> String {
    value.bytes().map(|value| format!("{value:02x}")).collect()
}

fn credential_document(version: u64) -> Vec<u8> {
    format!(
        "schema=2\naccount={}\naccess={}\nrefresh={}\nversion={version}\nexpires=4102444800\n",
        hex("broker-account"),
        hex(&format!("access-private-{version}")),
        hex(&format!("refresh-private-{version}")),
    )
    .into_bytes()
}

#[test]
fn native_credentials_round_trip_through_anonymous_pipes_only() {
    let root = TempRoot::new();
    let initialized = broker("initialize", &root.0, &credential_document(1));
    assert!(initialized.status.success());
    assert!(initialized.stdout.is_empty());

    let loaded = broker("load", &root.0, &[]);
    assert!(loaded.status.success());
    let loaded = String::from_utf8(loaded.stdout).unwrap();
    assert!(loaded.contains(&hex("refresh-private-1")));

    let mut rotation = b"expected=1\ncommit=00000000-0000-4000-8000-000000000001\n".to_vec();
    rotation.extend_from_slice(&credential_document(2));
    assert!(broker("commit", &root.0, &rotation).status.success());
    let rotated = broker("load", &root.0, &[]);
    assert!(rotated.status.success());
    let rotated = String::from_utf8(rotated.stdout).unwrap();
    assert!(rotated.contains(&hex("refresh-private-2")));
    assert!(!root.0.join("credentials.native").exists());
}
