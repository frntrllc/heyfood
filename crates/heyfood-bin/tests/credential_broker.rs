#![cfg(feature = "native-credentials")]

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
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn broker(action: &str, root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_heyfood"))
        .args(["__heyfood_credential_broker", action])
        .arg(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn external_process_cannot_invoke_the_credential_broker() {
    let root = TempRoot::new();
    let loaded = broker("load", &root.0);
    assert!(!loaded.status.success());
    assert!(loaded.stdout.is_empty());
    assert!(!root.0.join("credentials.native").exists());
}
