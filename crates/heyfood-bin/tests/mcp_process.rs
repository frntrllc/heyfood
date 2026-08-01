#[cfg(feature = "native-credentials")]
use std::fs;
#[cfg(feature = "native-credentials")]
use std::io::{BufRead, BufReader, Read, Write};
#[cfg(feature = "native-credentials")]
use std::path::PathBuf;
use std::process::Command;
#[cfg(feature = "native-credentials")]
use std::process::Stdio;
#[cfg(feature = "native-credentials")]
use std::sync::mpsc;
#[cfg(feature = "native-credentials")]
use std::thread;
#[cfg(feature = "native-credentials")]
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(feature = "native-credentials")]
use serde_json::{Value, json};

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_heyfood"));
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("HEYFOOD_") {
            command.env_remove(name);
        }
    }
    command
}

#[cfg(feature = "native-credentials")]
struct TempRoot(PathBuf);

#[cfg(feature = "native-credentials")]
impl TempRoot {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "heyfood-mcp-clean-profile-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        for child in [
            "config",
            "data",
            "cache",
            "AppData/Roaming",
            "AppData/Local",
        ] {
            fs::create_dir_all(path.join(child)).unwrap();
        }
        Self(path)
    }
}

#[cfg(feature = "native-credentials")]
impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(feature = "native-credentials")]
fn send_request(stdin: &mut impl Write, id: u64, method: &str, params: Value) {
    serde_json::to_writer(
        &mut *stdin,
        &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
    )
    .unwrap();
    stdin.write_all(b"\n").unwrap();
    stdin.flush().unwrap();
}

#[cfg(feature = "native-credentials")]
fn response(receiver: &mpsc::Receiver<Value>, id: u64, deadline: Duration) -> Value {
    loop {
        let value = receiver
            .recv_timeout(deadline)
            .unwrap_or_else(|_| panic!("timed out waiting for MCP response {id}"));
        if value.get("id").and_then(Value::as_u64) == Some(id) {
            return value;
        }
    }
}

#[test]
fn inherited_heyfood_environment_fails_before_protocol_stdout() {
    for name in [
        "HEYFOOD_API_URL",
        "HEYFOOD_API_KEY",
        "HEYFOOD_CREDENTIAL_STORE",
        "HEYFOOD_STATE_DIR",
        "HEYFOOD_UNKNOWN_OVERRIDE",
    ] {
        let output = command()
            .args(["mcp", "serve"])
            .env(name, "must-not-be-read")
            .output()
            .unwrap();
        assert!(!output.status.success(), "{name}");
        assert!(output.stdout.is_empty(), "{name}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        if cfg!(feature = "native-credentials") {
            assert!(stderr.contains(name), "{name}: {stderr}");
        } else {
            assert!(stderr.contains("requires the native credential feature"));
        }
        assert!(!stderr.contains("must-not-be-read"), "{name}");
    }
    for arguments in [
        vec!["mcp", "--json", "serve"],
        vec!["--json", "mcp", "serve"],
        vec!["--verbose", "mcp", "serve"],
    ] {
        let output = command()
            .args(arguments)
            .env("HEYFOOD_TEST_DELETE_NATIVE_CREDENTIALS", "1")
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        if cfg!(feature = "native-credentials") {
            assert!(
                stderr.contains("HEYFOOD_TEST_DELETE_NATIVE_CREDENTIALS"),
                "{stderr}"
            );
        }
    }
}

#[test]
fn one_shot_and_human_output_modifiers_never_start_mcp_stdout() {
    for arguments in [
        vec!["--json", "mcp", "serve"],
        vec!["--raw", "mcp", "serve"],
        vec!["--no-color", "mcp", "serve"],
        vec!["--no-banner", "mcp", "serve"],
        vec!["--no-input", "mcp", "serve"],
    ] {
        let output = command().args(arguments).output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        if cfg!(feature = "native-credentials") {
            assert!(stderr.contains("stdout is reserved for MCP"));
        } else {
            assert!(stderr.contains("requires the native credential feature"));
        }
    }
}

#[cfg(feature = "native-credentials")]
#[test]
fn clean_profile_discovers_the_exact_protocol_and_gets_a_typed_auth_handoff() {
    let root = TempRoot::new();
    let mut command = command();
    command
        .args(["mcp", "serve"])
        .env("HOME", &root.0)
        .env("USERPROFILE", &root.0)
        .env("XDG_CONFIG_HOME", root.0.join("config"))
        .env("XDG_DATA_HOME", root.0.join("data"))
        .env("XDG_CACHE_HOME", root.0.join("cache"))
        .env("APPDATA", root.0.join("AppData").join("Roaming"))
        .env("LOCALAPPDATA", root.0.join("AppData").join("Local"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let line = line.unwrap();
            sender.send(serde_json::from_str(&line).unwrap()).unwrap();
        }
    });

    send_request(
        &mut stdin,
        1,
        "initialize",
        json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "heyfood-test", "version": "1"}
        }),
    );
    let initialized = response(&receiver, 1, Duration::from_secs(5));
    assert_eq!(
        initialized["result"]["protocolVersion"],
        Value::String("2025-11-25".to_owned())
    );
    writeln!(
        stdin,
        "{}",
        json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        })
    )
    .unwrap();

    send_request(&mut stdin, 2, "tools/list", json!({}));
    let listed = response(&receiver, 2, Duration::from_secs(5));
    let names = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "heyfood_get_manifest",
            "heyfood_get_status",
            "heyfood_get_capabilities",
            "heyfood_get_grocery_list",
            "heyfood_get_grocery_exclusions",
            "heyfood_list_menu_watches",
        ]
    );

    send_request(
        &mut stdin,
        3,
        "tools/call",
        json!({"name": "heyfood_get_manifest", "arguments": {}}),
    );
    let manifest = response(&receiver, 3, Duration::from_secs(5));
    assert_eq!(manifest["result"]["structuredContent"]["schema_version"], 1);
    assert!(
        manifest["result"]["structuredContent"]
            .get("native_state_compatibility")
            .is_none(),
        "MCP must retain the closed v1 default used by installed v0.6.2 skills"
    );
    assert_eq!(manifest["result"]["isError"], false);

    send_request(
        &mut stdin,
        4,
        "tools/call",
        json!({"name": "heyfood_get_status", "arguments": {}}),
    );
    let missing_auth = response(&receiver, 4, Duration::from_secs(20));
    assert_eq!(missing_auth["result"]["isError"], true);
    let error_code = missing_auth["result"]["structuredContent"]["error"]["code"]
        .as_str()
        .unwrap();
    if cfg!(target_os = "macos") {
        // Unsigned local/test binaries can be denied by Keychain before a
        // missing entry is observable. Protected signed archives must return
        // `login_required`; the archive qualification records that stronger
        // assertion.
        assert!(
            matches!(error_code, "login_required" | "credential_broker_failed"),
            "{error_code}"
        );
    } else {
        assert_eq!(error_code, "login_required");
    }
    if error_code == "login_required" {
        assert_eq!(
            missing_auth["result"]["structuredContent"]["error"]["user_action"],
            "heyfood login"
        );
    }

    drop(stdin);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "MCP process did not terminate after clean EOF"
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success());
    reader.join().unwrap();
    let mut diagnostic = String::new();
    BufReader::new(stderr)
        .read_to_string(&mut diagnostic)
        .unwrap();
    assert!(diagnostic.is_empty(), "{diagnostic}");
}
