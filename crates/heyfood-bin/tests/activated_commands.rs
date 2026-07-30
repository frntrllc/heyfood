#![cfg(not(windows))]

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use heyfood_application::CredentialPort;
use heyfood_core::{
    AccountId, AuthCredentialBundle, ChannelCredentials, CredentialVersion, SensitiveString,
    SessionCredentials,
};
#[cfg(feature = "native-credentials")]
use heyfood_platform::AuthorizationSessionStore;
use heyfood_platform::{FileCredentialStore, NativeAuthStore};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;

const LIST_ID: &str = "00000000-0000-4000-8000-000000000123";
// Canonical supported v0.5 scope set; intentionally excludes deferred Health authority.
const FULL_SCOPE: &str = "account:link account:delete knowledge:read menu:read menu:watch recommend:read recipes:read recipes:write claims:read_derived profile:read profile:write meals:read meals:write audio:transcribe grocery:read grocery:write";

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "heyfood-activated-{name}-{}-{nonce}",
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

fn session() -> SessionCredentials {
    SessionCredentials::from_unix_expiry(
        AccountId::parse("activated-account").unwrap(),
        SensitiveString::new("session-access"),
        SensitiveString::new("session-refresh"),
        CredentialVersion::new(1),
        4_102_444_800,
    )
    .unwrap()
}

fn initialize(root: &Path, scope: &str) {
    let session = session();
    let bundle = AuthCredentialBundle {
        channel: ChannelCredentials::from_unix_expiry(
            "hf_cid_heyfood_cli",
            "heyfood-activated-device",
            SensitiveString::new("channel-access"),
            SensitiveString::new("channel-refresh"),
            4_102_444_800,
            scope,
        )
        .unwrap(),
        session: session.clone(),
    };
    NativeAuthStore::open(root)
        .unwrap()
        .initialize(&bundle)
        .unwrap();
    FileCredentialStore::open(root)
        .unwrap()
        .initialize(&session)
        .unwrap();
}

#[cfg(feature = "native-credentials")]
fn initialize_expired_mature_session(root: &Path, scope: &str) {
    let initial_session = SessionCredentials::from_unix_expiry(
        AccountId::parse("activated-account").unwrap(),
        SensitiveString::new("session-access-1"),
        SensitiveString::new("session-refresh-1"),
        CredentialVersion::new(1),
        1,
    )
    .unwrap();
    let bundle = AuthCredentialBundle {
        channel: ChannelCredentials::from_unix_expiry(
            "hf_cid_heyfood_cli",
            "heyfood-activated-device",
            SensitiveString::new("channel-access-old"),
            SensitiveString::new("channel-refresh-old"),
            1,
            scope,
        )
        .unwrap(),
        session: initial_session.clone(),
    };
    let auth_store = NativeAuthStore::open(root).unwrap();
    let session_store = FileCredentialStore::open(root).unwrap();
    auth_store.initialize(&bundle).unwrap();
    session_store.initialize(&initial_session).unwrap();
    session_store
        .replace_authorized_session(
            &SessionCredentials::from_unix_expiry(
                initial_session.account_id,
                SensitiveString::new("session-access-7"),
                SensitiveString::new("session-refresh-7"),
                CredentialVersion::new(7),
                1,
            )
            .unwrap(),
        )
        .unwrap();
}

async fn run(
    root: &Path,
    base_url: &str,
    args: &[&str],
    stdin: Option<&[u8]>,
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_heyfood"));
    command
        .args(args)
        .env("HEYFOOD_STATE_DIR", root)
        .env("HEYFOOD_CREDENTIAL_STORE", "file")
        .env("HEYFOOD_API_URL", base_url)
        .env("HEYFOOD_API_KEY", "fixture-api-key")
        .env("HEYFOOD_TEST_FORCE_NO_CONTROLLING_TERMINAL", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if stdin.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }
    let mut child = command.spawn().unwrap();
    if let Some(stdin) = stdin {
        child.stdin.take().unwrap().write_all(stdin).await.unwrap();
    }
    child.wait_with_output().await.unwrap()
}

fn run_confirm_with_data_stdin_and_controlling_terminal(
    root: &Path,
    base_url: &str,
    proposal_path: &Path,
) -> Vec<u8> {
    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 30,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open controlling PTY");
    let mut command = CommandBuilder::new("/bin/sh");
    command.args([
        "-c",
        "exec \"$1\" --json grocery confirm --decision cancel --proposal-stdin < \"$2\"",
        "heyfood-confirm-pty",
    ]);
    command.arg(env!("CARGO_BIN_EXE_heyfood"));
    command.arg(proposal_path);
    command.env("HEYFOOD_STATE_DIR", root);
    command.env("HEYFOOD_CREDENTIAL_STORE", "file");
    command.env("HEYFOOD_API_URL", base_url);
    command.env("HEYFOOD_API_KEY", "fixture-api-key");
    command.env("NO_PROXY", "127.0.0.1,localhost");
    command.env("TERM", "xterm-256color");
    command.env_remove("HEYFOOD_TEST_FORCE_NO_CONTROLLING_TERMINAL");

    let mut child = pair
        .slave
        .spawn_command(command)
        .expect("spawn public binary with redirected data stdin");
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().expect("clone PTY reader");
    let writer = Arc::new(Mutex::new(
        pair.master.take_writer().expect("take PTY writer"),
    ));
    let capture = Arc::new(Mutex::new(Vec::new()));
    let reader_capture = Arc::clone(&capture);
    let reader_task = std::thread::spawn(move || {
        let mut chunk = [0_u8; 4096];
        loop {
            let count = reader.read(&mut chunk).expect("read public binary PTY");
            if count == 0 {
                break;
            }
            reader_capture
                .lock()
                .expect("lock PTY capture")
                .extend_from_slice(&chunk[..count]);
        }
    });

    let prompt_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let observed = capture.lock().expect("lock prompt capture").clone();
        if String::from_utf8_lossy(&observed).contains("Type CANCEL to continue:") {
            break;
        }
        assert!(
            Instant::now() < prompt_deadline,
            "public binary never requested controlling-terminal authority: {:?}",
            String::from_utf8_lossy(&observed)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    {
        let mut terminal = writer.lock().expect("lock controlling terminal");
        terminal
            .write_all(b"CANCEL\r")
            .expect("write exact terminal decision");
        terminal.flush().expect("flush terminal decision");
    }

    let exit_deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll public binary PTY") {
            break status;
        }
        if Instant::now() >= exit_deadline {
            let _ = child.kill();
            panic!("public binary did not exit after terminal decision");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "public binary PTY failed: {status:?}");
    drop(writer);
    drop(pair.master);
    reader_task.join().expect("join public binary PTY reader");
    Arc::try_unwrap(capture)
        .expect("release PTY capture")
        .into_inner()
        .expect("unlock PTY capture")
}

async fn read_request(socket: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 1024];
        let count = socket.read(&mut chunk).await.unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
        })
        .unwrap_or(0);
    while bytes.len() - header_end < length {
        let mut chunk = vec![0; length - (bytes.len() - header_end)];
        let count = socket.read(&mut chunk).await.unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&chunk[..count]);
    }
    String::from_utf8(bytes).unwrap()
}

async fn respond(socket: &mut TcpStream, content_type: &str, body: &[u8]) {
    respond_status(socket, 200, "OK", content_type, body).await;
}

async fn respond_status(
    socket: &mut TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
) {
    socket
        .write_all(
            format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    socket.write_all(body).await.unwrap();
}

fn old_scope() -> &'static str {
    "account:link account:delete knowledge:read menu:read recommend:read recipes:read recipes:write claims:read_derived profile:read profile:write meals:read meals:write audio:transcribe health:read integrations:manage"
}

fn assert_legacy_health_authority(scope: &str) {
    let scopes = scope.split_whitespace().collect::<BTreeSet<_>>();
    assert!(scopes.contains("health:read"));
    assert!(scopes.contains("integrations:manage"));
}

fn capabilities(grocery: bool) -> Value {
    json!({
        "schema_version": 1,
        "self_registration": {"status": "disabled", "regions": [], "identity_methods": []},
        "authorization": {"loopback_pkce": true, "device_code": true, "identity_methods": []},
        "profile_readiness": true,
        "application_capabilities": if grocery { json!({"grocery": "v1"}) } else { json!({}) }
    })
}

fn list() -> Value {
    json!({
        "id": LIST_ID,
        "title": "Grocery List",
        "state": "active",
        "version": 4,
        "items": [],
        "created_at": "2026-07-21T12:00:00Z",
        "updated_at": "2026-07-21T12:00:00Z"
    })
}

fn watch() -> Value {
    json!({
        "id": "00000000-0000-4000-8000-000000000010",
        "restaurant_id": "0c1cb790-0000-4000-8000-000000000000",
        "cadence": {"weekday": 3, "hour": 9},
        "tz": "America/Chicago",
        "active": true,
        "notify": true,
        "next_run_at": "2026-07-30T14:00:00Z",
        "last_run_at": null,
        "last_snapshot_id": null,
        "created_at": "2026-07-23T12:00:00Z",
        "identity_verdict": "verified",
        "identity_confidence": 0.92
    })
}

fn proposal(operation: &str) -> Value {
    json!({
        "confirmation_id": "00000000-0000-4000-8000-000000000001",
        "idempotency_key": "00000000-0000-4000-8000-000000000002",
        "operation": operation,
        "expires_at": "2026-07-21T12:05:00Z",
        "structured_preview": {"items": []},
        "preconditions": [{"type": "list_version", "expected_version": 4}],
        "confirmation_token": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    })
}

fn response_for(method: &str, path: &str) -> (&'static str, Vec<u8>) {
    if path == "/v1/auth/capabilities" {
        return (
            "application/json",
            serde_json::to_vec(&capabilities(true)).unwrap(),
        );
    }
    let value = match (method, path.split('?').next().unwrap()) {
        ("GET", "/v1/grocery/list") => list(),
        ("GET", "/v1/grocery/exclusions") => json!({"exclusions": ["pork"]}),
        ("POST", "/v1/grocery/items") => proposal("add_items"),
        ("POST", "/v1/grocery/items/remove") => proposal("remove_items"),
        ("POST", "/v1/grocery/items/state") => proposal("update_item_state"),
        ("POST", "/v1/grocery/exclusions") => proposal("add_exclusion"),
        ("POST", "/v1/grocery/exclusions/remove") => proposal("remove_exclusion"),
        ("POST", "/v1/grocery/confirm") => json!({
            "status": "cancelled",
            "operation": "add_items",
            "confirmation_id": "00000000-0000-4000-8000-000000000001",
            "list": null,
            "exclusions": null
        }),
        ("GET", path) if path.starts_with("/v1/grocery/lists/") => list(),
        ("GET", "/v1/integrations") => json!({"integrations": []}),
        ("GET", "/v1/health/context") => json!({
            "status": "not_connected", "provider": null, "stale_since": null,
            "data_freshness_hours": null, "sleep_avg": null, "readiness_avg": null,
            "activity_avg": null, "sleep_label": null, "readiness_label": null,
            "activity_label": null, "steps_avg": null, "active_calories_avg": null,
            "stress_label": null, "deep_sleep_label": null, "goals": []
        }),
        ("POST", "/v1/integrations/authorize") => {
            json!({"auth_url": "https://provider.invalid/authorize", "provider": "oura"})
        }
        ("POST", "/v1/integrations/oura/sync") => json!({
            "provider": "oura", "suggested_goals": [],
            "data_period_start": null, "data_period_end": null
        }),
        ("DELETE", "/v1/integrations/oura") => json!({
            "provider": "oura", "status": "disconnected", "message": "disconnected"
        }),
        ("GET", "/v1/menu/watch") => json!({"watches": [watch()], "count": 1}),
        ("POST", "/v1/menu/watch") => watch(),
        _ => panic!("unexpected binary route {method} {path}"),
    };
    ("application/json", serde_json::to_vec(&value).unwrap())
}

#[tokio::test]
async fn public_binary_dispatches_the_unattended_grocery_and_watch_read_routes() {
    let root = TempRoot::new("routes");
    initialize(&root.0, FULL_SCOPE);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut product_routes = BTreeSet::new();
        for _ in 0..7 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            let mut request_line = request.lines().next().unwrap().split_whitespace();
            let method = request_line.next().unwrap();
            let path = request_line.next().unwrap();
            if path != "/v1/auth/capabilities" && path != "/v1/grocery/list" {
                product_routes.insert(format!("{method} {}", path.split('?').next().unwrap()));
            }
            if path == "/v1/grocery/list" {
                product_routes.insert(format!("{method} {path}"));
            }
            if method == "DELETE" && path.starts_with("/v1/menu/watch/") {
                respond_status(&mut socket, 204, "No Content", "application/json", b"").await;
            } else {
                let (content_type, body) = response_for(method, path);
                respond(&mut socket, content_type, &body).await;
            }
        }
        product_routes
    });

    let cases: Vec<(Vec<&str>, Option<Vec<u8>>)> = vec![
        (vec!["--json", "grocery"], None),
        (
            vec!["--json", "grocery", "export", LIST_ID, "--format", "json"],
            None,
        ),
        (vec!["--json", "grocery", "exclusions"], None),
        (vec!["--json", "watch"], None),
    ];
    for (args, stdin) in cases {
        let output = run(&root.0, &base_url, &args, stdin.as_deref()).await;
        assert!(
            output.status.success(),
            "{} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let routes = server.await.unwrap();
    let expected = BTreeSet::from([
        "GET /v1/grocery/list".into(),
        "GET /v1/grocery/exclusions".into(),
        format!("GET /v1/grocery/lists/{LIST_ID}/export"),
        "GET /v1/menu/watch".into(),
    ]);
    assert_eq!(routes, expected);
}

#[tokio::test]
async fn public_binary_separates_proposal_stdin_from_controlling_terminal_decision() {
    let root = TempRoot::new("positive-human-authority");
    initialize(&root.0, FULL_SCOPE);
    let proposal_path = root.0.join("proposal.json");
    std::fs::write(
        &proposal_path,
        serde_json::to_vec(&json!({
            "confirmation_id": "00000000-0000-4000-8000-000000000001",
            "idempotency_key": "00000000-0000-4000-8000-000000000002",
            "operation": "add_items",
            "expires_at": "2026-07-21T12:05:00Z",
            "structured_preview": {
                "items": [{
                    "requested_name": "milk",
                    "quantity": 2.0,
                    "unit": "cartons",
                    "note": "lactose-free",
                    "sources": [{"source_type": "manual"}]
                }]
            },
            "preconditions": [
                {"type": "list_version", "expected_version": 4},
                {"type": "context_hash", "expected": "context-v4"}
            ],
            "confirmation_token": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        }))
        .unwrap(),
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut routes = Vec::new();
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            let mut request_line = request.lines().next().unwrap().split_whitespace();
            let method = request_line.next().unwrap();
            let path = request_line.next().unwrap();
            routes.push(format!("{method} {path}"));
            let (content_type, body) = response_for(method, path);
            respond(&mut socket, content_type, &body).await;
        }
        routes
    });

    let pty_root = root.0.clone();
    let pty_url = base_url.clone();
    let terminal = tokio::task::spawn_blocking(move || {
        run_confirm_with_data_stdin_and_controlling_terminal(&pty_root, &pty_url, &proposal_path)
    })
    .await
    .unwrap();
    let terminal = String::from_utf8_lossy(&terminal);
    let routes = server.await.unwrap();

    assert_eq!(
        routes,
        vec![
            "GET /v1/auth/capabilities".to_owned(),
            "POST /v1/grocery/confirm".to_owned()
        ]
    );
    assert!(terminal.contains("Type CANCEL to continue:"));
    assert!(terminal.contains("\"quantity\": 2.0"));
    assert!(terminal.contains("\"expected\": \"context-v4\""));
    assert!(terminal.contains("\"status\":\"cancelled\""));
    assert!(!terminal.contains("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
}

#[tokio::test]
async fn public_binary_rejects_human_only_mutations_without_a_controlling_terminal() {
    let root = TempRoot::new("human-authority");
    initialize(&root.0, FULL_SCOPE);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let proposal = serde_json::to_vec(&proposal("add_items")).unwrap();
    let cases: Vec<(Vec<&str>, Option<&[u8]>)> = vec![
        (vec!["--json", "log", "oatmeal"], None),
        (
            vec![
                "--json",
                "grocery",
                "add",
                "--list-id",
                LIST_ID,
                "--version",
                "4",
                "onion",
            ],
            None,
        ),
        (
            vec![
                "--json",
                "grocery",
                "remove",
                "--list-id",
                LIST_ID,
                "--version",
                "4",
                "item-1",
            ],
            None,
        ),
        (
            vec![
                "--json",
                "grocery",
                "state",
                "--list-id",
                LIST_ID,
                "--version",
                "4",
                "item-1",
                "purchased",
            ],
            None,
        ),
        (
            vec![
                "--json",
                "grocery",
                "never",
                "--list-id",
                LIST_ID,
                "--version",
                "4",
                "pork",
            ],
            None,
        ),
        (
            vec!["--json", "grocery", "confirm", "--decision", "cancel"],
            Some(proposal.as_slice()),
        ),
        (
            vec![
                "--json",
                "watch",
                "add",
                "0c1cb790-0000-4000-8000-000000000000",
                "--weekday",
                "thursday",
                "--hour",
                "9",
                "--notify",
            ],
            None,
        ),
        (
            vec![
                "--json",
                "watch",
                "remove",
                "00000000-0000-4000-8000-000000000010",
            ],
            None,
        ),
    ];

    for (arguments, stdin) in cases {
        let output = run(&root.0, &base_url, &arguments, stdin).await;
        assert!(!output.status.success(), "{}", arguments.join(" "));
        let error: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            error["error"]["type"],
            "human_terminal_required",
            "{}",
            arguments.join(" ")
        );
    }
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
            .await
            .is_err(),
        "a human-only command reached the network without a controlling terminal"
    );
}

#[tokio::test]
async fn public_binary_writes_json_export_to_an_owner_only_file() {
    let root = TempRoot::new("export-file");
    initialize(&root.0, FULL_SCOPE);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            let mut request_line = request.lines().next().unwrap().split_whitespace();
            let method = request_line.next().unwrap();
            let path = request_line.next().unwrap();
            let (content_type, body) = response_for(method, path);
            respond(&mut socket, content_type, &body).await;
        }
    });
    let target = root.0.join("grocery.json");
    let output = run(
        &root.0,
        &base_url,
        &[
            "--json",
            "grocery",
            "export",
            LIST_ID,
            "--format",
            "json",
            "--out",
            target.to_str().unwrap(),
        ],
        None,
    )
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["written"], true);
    assert_eq!(receipt["format"], "json");
    let written: Value = serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
    assert_eq!(written, list());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    server.await.unwrap();
}

#[tokio::test]
async fn public_binary_fails_closed_before_route_dispatch_for_scope_deferral_capability_and_confirmation()
 {
    let old = TempRoot::new("old-scope");
    initialize(
        &old.0,
        "account:link profile:read profile:write meals:read meals:write audio:transcribe",
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let output = run(&old.0, &base_url, &["--json", "grocery", "list"], None).await;
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        error["error"]["type"],
        "authorization_scope_upgrade_required"
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );

    let output = run(&old.0, &base_url, &["--json", "health", "show"], None).await;
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["type"], "capability_deferred");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );

    let confirmed = TempRoot::new("confirmation");
    initialize(&confirmed.0, FULL_SCOPE);
    let confirmation_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let confirmation_url = format!("http://{}", confirmation_listener.local_addr().unwrap());
    let output = run(
        &confirmed.0,
        &confirmation_url,
        &["--json", "grocery", "confirm", "--decision", "cancel"],
        None,
    )
    .await;
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["type"], "human_terminal_required");
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            confirmation_listener.accept()
        )
        .await
        .is_err()
    );

    let capability_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let capability_url = format!("http://{}", capability_listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = capability_listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("GET /v1/auth/capabilities "));
        respond(
            &mut socket,
            "application/json",
            &serde_json::to_vec(&capabilities(false)).unwrap(),
        )
        .await;
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                capability_listener.accept()
            )
            .await
            .is_err()
        );
    });
    let output = run(
        &confirmed.0,
        &capability_url,
        &["--json", "grocery", "list"],
        None,
    )
    .await;
    assert!(!output.status.success());
    server.await.unwrap();
}

#[tokio::test]
async fn public_login_preserves_old_credentials_until_complete_then_replaces_both_stores() {
    let root = TempRoot::new("login-success");
    initialize(&root.0, old_scope());
    let auth_store = NativeAuthStore::open(&root.0).unwrap();
    let session_store = FileCredentialStore::open(&root.0).unwrap();
    let old_auth = auth_store.load().unwrap().unwrap();
    let old_session = session_store.load().await.unwrap().unwrap();
    assert_legacy_health_authority(&old_auth.channel.scope);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let verification_uri = format!("{base_url}/authorize");
    let server = tokio::spawn(async move {
        let mut client_transaction_id = String::new();
        let authorization_transaction_id = "authorization-transaction-login";
        let stage_id = "stage-transaction-login";
        let bundle = json!({
            "channel": {
                "access_token": "expanded-channel-access",
                "token_type": "bearer",
                "expires_in": 3600,
                "refresh_token": "expanded-channel-refresh",
                "scope": FULL_SCOPE,
                "link_id": "link-transaction-login",
                "resource": null,
                "authorization_transaction_id": authorization_transaction_id,
                "access_expires_at": "2999-01-01T00:00:00Z",
                "refresh_expires_at": "2999-02-01T00:00:00Z"
            },
            "session": {
                "user_id": "activated-account",
                "device_id": "heyfood-activated-device",
                "session_id": "expanded-session-id",
                "access_token": "expanded-session-access",
                "refresh_token": "expanded-session-refresh",
                "access_expires_at": "2999-01-01T00:00:00Z",
                "refresh_expires_at": "2999-02-01T00:00:00Z",
                "scopes": FULL_SCOPE.split_whitespace().collect::<Vec<_>>(),
                "is_anonymous": false
            }
        });
        let bundle_digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&bundle).unwrap()));
        for expected in [
            "/v1/auth/capabilities",
            "/v1/channel/oauth/device/authorize",
            "/v1/channel/oauth/device/token",
            "/v1/channel/oauth/cli/reauthorizations",
            "/v1/channel/oauth/cli/reauthorizations/stage-transaction-login/promote",
        ] {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            let path = request
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap();
            assert_eq!(path, expected);
            let body = match path {
                "/v1/auth/capabilities" => capabilities(false),
                "/v1/channel/oauth/device/authorize" => {
                    let request: Value =
                        serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
                    assert_eq!(request["intent"], "sign_in");
                    assert_eq!(request["scope"], FULL_SCOPE);
                    assert_eq!(request["device_id"], "heyfood-activated-device");
                    client_transaction_id = request["client_transaction_id"]
                        .as_str()
                        .unwrap()
                        .to_owned();
                    json!({
                        "device_code": "hf_dc_01234567890123456789",
                        "user_code": "ABCD-EFGH",
                        "verification_uri": verification_uri,
                        "verification_uri_complete": null,
                        "expires_in": 600,
                        "interval": 1
                    })
                }
                "/v1/channel/oauth/device/token" => json!({
                    "access_token": "provisional-channel-access",
                    "token_type": "bearer",
                    "refresh_token": "provisional-channel-refresh",
                    "expires_in": 3600,
                    "scope": FULL_SCOPE,
                    "link_id": "link-transaction-login",
                    "resource": null,
                    "authorization_transaction_id": authorization_transaction_id
                }),
                "/v1/channel/oauth/cli/reauthorizations" => {
                    assert!(
                        request
                            .to_ascii_lowercase()
                            .contains("authorization: bearer provisional-channel-access\r\n")
                    );
                    let request_body: Value =
                        serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
                    assert_eq!(request_body["client_transaction_id"], client_transaction_id);
                    assert_eq!(
                        request_body["authorization_transaction_id"],
                        authorization_transaction_id
                    );
                    assert_eq!(request_body["device_id"], "heyfood-activated-device");
                    json!({
                        "stage_id": stage_id,
                        "client_transaction_id": client_transaction_id.clone(),
                        "authorization_transaction_id": authorization_transaction_id,
                        "device_id": "heyfood-activated-device",
                        "status": "staged",
                        "scopes": FULL_SCOPE.split_whitespace().collect::<Vec<_>>(),
                        "bundle_digest": bundle_digest.clone(),
                        "recovery_token": "recovery-token-login-fixture",
                        "bundle": bundle.clone(),
                        "expires_at": "2999-01-01T00:00:00Z",
                        "recoverable_until": "2999-01-02T00:00:00Z",
                        "promoted_at": null,
                        "aborted_at": null
                    })
                }
                "/v1/channel/oauth/cli/reauthorizations/stage-transaction-login/promote" => {
                    assert!(request.to_ascii_lowercase().contains(
                        "authorization: reauthorization recovery-token-login-fixture\r\n"
                    ));
                    let request_body: Value =
                        serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
                    assert_eq!(request_body["client_transaction_id"], client_transaction_id);
                    assert_eq!(request_body["device_id"], "heyfood-activated-device");
                    assert_eq!(request_body["bundle_digest"], bundle_digest);
                    json!({
                        "stage_id": stage_id,
                        "client_transaction_id": client_transaction_id.clone(),
                        "authorization_transaction_id": authorization_transaction_id,
                        "device_id": "heyfood-activated-device",
                        "status": "promoted",
                        "scopes": FULL_SCOPE.split_whitespace().collect::<Vec<_>>(),
                        "bundle_digest": bundle_digest.clone(),
                        "recovery_token": "recovery-token-login-fixture",
                        "bundle": bundle.clone(),
                        "expires_at": "2999-01-01T00:00:00Z",
                        "recoverable_until": "2999-01-02T00:00:00Z",
                        "promoted_at": "2026-07-21T00:00:00Z",
                        "aborted_at": null
                    })
                }
                _ => unreachable!(),
            };
            respond(
                &mut socket,
                "application/json",
                &serde_json::to_vec(&body).unwrap(),
            )
            .await;
        }
    });
    let output = run(
        &root.0,
        &base_url,
        &["--json", "login", "--no-browser", "--timeout", "5"],
        None,
    )
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    server.await.unwrap();

    let expanded = auth_store.load().unwrap().unwrap();
    let expanded_session = session_store.load().await.unwrap().unwrap();
    assert_ne!(expanded, old_auth);
    assert_ne!(expanded_session, old_session);
    assert_eq!(expanded.channel.scope, FULL_SCOPE);
    assert!(!expanded.channel.scope.contains("health:read"));
    assert!(!expanded.channel.scope.contains("integrations:manage"));
    assert_eq!(expanded.session, expanded_session);
    assert!(!root.0.join("auth.reconciliation").exists());
}

#[tokio::test]
async fn rejected_login_leaves_both_existing_credentials_byte_for_byte_authoritative() {
    let root = TempRoot::new("login-rejected");
    initialize(&root.0, old_scope());
    let auth_store = NativeAuthStore::open(&root.0).unwrap();
    let session_store = FileCredentialStore::open(&root.0).unwrap();
    let old_auth = auth_store.load().unwrap().unwrap();
    let old_session = session_store.load().await.unwrap().unwrap();
    assert_legacy_health_authority(&old_auth.channel.scope);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let verification_uri = format!("{base_url}/authorize");
    let server = tokio::spawn(async move {
        for expected in [
            "/v1/auth/capabilities",
            "/v1/channel/oauth/device/authorize",
            "/v1/channel/oauth/device/token",
        ] {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            let path = request
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap();
            assert_eq!(path, expected);
            if path == "/v1/channel/oauth/device/token" {
                respond_status(
                    &mut socket,
                    400,
                    "Bad Request",
                    "application/json",
                    &serde_json::to_vec(&json!({"error": "access_denied"})).unwrap(),
                )
                .await;
                continue;
            }
            if path == "/v1/channel/oauth/device/authorize" {
                let request: Value =
                    serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
                assert_eq!(request["scope"], FULL_SCOPE);
            }
            let body = if path == "/v1/auth/capabilities" {
                capabilities(false)
            } else {
                json!({
                    "device_code": "hf_dc_01234567890123456789",
                    "user_code": "ABCD-EFGH",
                    "verification_uri": verification_uri,
                    "verification_uri_complete": null,
                    "expires_in": 600,
                    "interval": 1
                })
            };
            respond(
                &mut socket,
                "application/json",
                &serde_json::to_vec(&body).unwrap(),
            )
            .await;
        }
    });
    let output = run(
        &root.0,
        &base_url,
        &["--json", "login", "--no-browser", "--timeout", "5"],
        None,
    )
    .await;
    assert!(!output.status.success());
    server.await.unwrap();
    assert_eq!(auth_store.load().unwrap(), Some(old_auth));
    assert_eq!(session_store.load().await.unwrap(), Some(old_session));
    assert!(!root.0.join("auth.reconciliation").exists());
}

#[tokio::test]
#[cfg(feature = "native-credentials")]
async fn public_logout_revokes_current_authority_in_order_and_clears_both_stores() {
    let root = TempRoot::new("logout-success");
    initialize(&root.0, FULL_SCOPE);
    let auth_store = NativeAuthStore::open(&root.0).unwrap();
    let session_store = FileCredentialStore::open(&root.0).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        for expected in [
            "GET /v1/channel/oauth/whoami ",
            "DELETE /v1/channel/links/link-activated ",
            "POST /v1/auth/device/revoke ",
            "POST /v1/auth/session/revoke ",
        ] {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            assert!(request.starts_with(expected), "{request}");
            if expected.starts_with("GET ") {
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("authorization: bearer channel-access\r\n")
                );
                respond(
                    &mut socket,
                    "application/json",
                    br#"{"link_id":"link-activated"}"#,
                )
                .await;
            } else {
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("authorization: bearer session-access\r\n")
                );
                if expected.starts_with("POST ") {
                    let body: Value =
                        serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
                    assert_eq!(body["reason"], "cli_logout");
                    if expected.contains("/auth/device/revoke") {
                        assert_eq!(body["device_id"], "heyfood-activated-device");
                    } else {
                        assert!(body.get("device_id").is_none());
                    }
                }
                respond(&mut socket, "application/json", br#"{"revoked":true}"#).await;
            }
        }
    });
    let output = run(&root.0, &base_url, &["--json", "logout"], None).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    server.await.unwrap();
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["ok"], true);
    assert_eq!(document["remote_complete"], true);
    assert_eq!(document["local_credentials_cleared"], true);
    assert_eq!(auth_store.load().unwrap(), None);
    assert_eq!(session_store.load().await.unwrap(), None);

    let repeated = run(&root.0, &base_url, &["--json", "logout"], None).await;
    assert!(repeated.status.success());
    let repeated: Value = serde_json::from_slice(&repeated.stdout).unwrap();
    assert_eq!(repeated["remote_complete"], true);
    assert_eq!(repeated["teardown"]["link"]["attempted"], false);
    assert_eq!(repeated["teardown"]["device"]["attempted"], false);
    assert_eq!(repeated["teardown"]["session"]["attempted"], false);
}

#[tokio::test]
#[cfg(feature = "native-credentials")]
async fn public_logout_clears_local_credentials_after_remote_failures_without_leaking_tokens() {
    let root = TempRoot::new("logout-remote-failure");
    initialize(&root.0, FULL_SCOPE);
    let auth_store = NativeAuthStore::open(&root.0).unwrap();
    let session_store = FileCredentialStore::open(&root.0).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        for expected in [
            "GET /v1/channel/oauth/whoami ",
            "POST /v1/auth/device/revoke ",
            "POST /v1/auth/session/revoke ",
        ] {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            assert!(request.starts_with(expected), "{request}");
            respond_status(
                &mut socket,
                503,
                "Unavailable",
                "application/json",
                br#"{"detail":"sentinel-secret"}"#,
            )
            .await;
        }
    });
    let output = run(&root.0, &base_url, &["--json", "logout"], None).await;
    assert!(output.status.success());
    server.await.unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let document: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(document["remote_complete"], false);
    assert_eq!(document["local_credentials_cleared"], true);
    assert!(!stdout.contains("sentinel"));
    assert!(!stdout.contains("channel-access"));
    assert!(!stdout.contains("session-access"));
    assert_eq!(auth_store.load().unwrap(), None);
    assert_eq!(session_store.load().await.unwrap(), None);
}

#[tokio::test]
#[cfg(feature = "native-credentials")]
async fn public_logout_refreshes_expired_mature_authority_before_remote_teardown() {
    let root = TempRoot::new("logout-refresh-mature");
    initialize_expired_mature_session(&root.0, FULL_SCOPE);
    let auth_store = NativeAuthStore::open(&root.0).unwrap();
    let session_store = FileCredentialStore::open(&root.0).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server_scope = FULL_SCOPE.to_owned();
    let server = tokio::spawn(async move {
        for expected in [
            "POST /v1/channel/oauth/token ",
            "POST /v1/auth/session/refresh ",
            "GET /v1/channel/oauth/whoami ",
            "DELETE /v1/channel/links/link-refreshed ",
            "POST /v1/auth/device/revoke ",
            "POST /v1/auth/session/revoke ",
        ] {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            assert!(request.starts_with(expected), "{request}");
            let body = request.split_once("\r\n\r\n").map_or("", |(_, body)| body);
            if expected.contains("/channel/oauth/token") {
                let body: Value = serde_json::from_str(body).unwrap();
                assert_eq!(body["grant_type"], "refresh_token");
                assert_eq!(body["refresh_token"], "channel-refresh-old");
                respond(
                    &mut socket,
                    "application/json",
                    &serde_json::to_vec(&json!({
                        "access_token": "channel-access-new",
                        "refresh_token": "channel-refresh-new",
                        "token_type": "bearer",
                        "expires_in": 3600,
                        "scope": server_scope
                    }))
                    .unwrap(),
                )
                .await;
            } else if expected.contains("/auth/session/refresh") {
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("x-device-id: heyfood-activated-device\r\n")
                );
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("x-api-key: fixture-api-key\r\n")
                );
                let body: Value = serde_json::from_str(body).unwrap();
                assert_eq!(body["refresh_token"], "session-refresh-7");
                respond(
                    &mut socket,
                    "application/json",
                    br#"{"user_id":"activated-account","access_token":"session-access-8","refresh_token":"session-refresh-8","access_expires_at":"2099-01-01T00:00:00Z"}"#,
                )
                .await;
            } else if expected.starts_with("GET ") {
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("authorization: bearer channel-access-new\r\n")
                );
                respond(
                    &mut socket,
                    "application/json",
                    br#"{"link_id":"link-refreshed"}"#,
                )
                .await;
            } else {
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("authorization: bearer session-access-8\r\n")
                );
                respond(&mut socket, "application/json", br#"{"revoked":true}"#).await;
            }
        }
    });

    let output = run(&root.0, &base_url, &["--json", "logout"], None).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    server.await.unwrap();
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["remote_complete"], true);
    assert_eq!(document["local_credentials_cleared"], true);
    assert_eq!(auth_store.load().unwrap(), None);
    assert_eq!(session_store.load().await.unwrap(), None);
    assert!(!root.0.join("auth.reconciliation").exists());
    assert!(!root.0.join("credentials.reconciliation").exists());
}

#[tokio::test]
#[cfg(feature = "native-credentials")]
async fn rejected_logout_preflight_clears_local_authority_without_remote_teardown() {
    let root = TempRoot::new("logout-refresh-rejected");
    initialize_expired_mature_session(&root.0, FULL_SCOPE);
    let auth_store = NativeAuthStore::open(&root.0).unwrap();
    let session_store = FileCredentialStore::open(&root.0).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("POST /v1/channel/oauth/token "));
        respond_status(
            &mut socket,
            401,
            "Unauthorized",
            "application/json",
            br#"{"error":"invalid_grant"}"#,
        )
        .await;
    });

    let output = run(&root.0, &base_url, &["--json", "logout"], None).await;
    assert!(output.status.success());
    server.await.unwrap();
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["remote_complete"], false);
    assert_eq!(document["local_credentials_cleared"], true);
    for step in ["link", "device", "session"] {
        assert_eq!(document["teardown"][step]["attempted"], false);
        assert_eq!(document["teardown"][step]["error"], "request_failed");
    }
    assert_eq!(auth_store.load().unwrap(), None);
    assert_eq!(session_store.load().await.unwrap(), None);
}

#[tokio::test]
#[cfg(feature = "native-credentials")]
async fn uncertain_logout_preflight_removes_refresh_markers_with_local_authority() {
    let root = TempRoot::new("logout-refresh-uncertain");
    initialize_expired_mature_session(&root.0, FULL_SCOPE);
    let auth_store = NativeAuthStore::open(&root.0).unwrap();
    let session_store = FileCredentialStore::open(&root.0).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("POST /v1/channel/oauth/token "));
        drop(socket);
    });

    let output = run(&root.0, &base_url, &["--json", "logout"], None).await;
    assert!(output.status.success());
    server.await.unwrap();
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["remote_complete"], false);
    assert_eq!(document["local_credentials_cleared"], true);
    for step in ["link", "device", "session"] {
        assert_eq!(document["teardown"][step]["attempted"], false);
        assert_eq!(document["teardown"][step]["outcome_uncertain"], true);
        assert_eq!(document["teardown"][step]["error"], "outcome_uncertain");
    }
    assert_eq!(auth_store.load().unwrap(), None);
    assert_eq!(session_store.load().await.unwrap(), None);
    assert!(!root.0.join("auth.reconciliation").exists());
    assert!(!root.0.join("credentials.reconciliation").exists());
}

#[tokio::test]
#[cfg(feature = "native-credentials")]
async fn restarted_logout_adopts_uncertain_channel_refresh_without_network_teardown() {
    let root = TempRoot::new("logout-refresh-uncertain-restart");
    initialize_expired_mature_session(&root.0, FULL_SCOPE);
    let auth_store = NativeAuthStore::open(&root.0).unwrap();
    let session_store = FileCredentialStore::open(&root.0).unwrap();
    std::fs::write(
        root.0.join("auth.reconciliation"),
        b"channel_refresh_outcome_uncertain\n",
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());

    let output = run(&root.0, &base_url, &["--json", "logout"], None).await;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["ok"], true);
    assert_eq!(document["remote_complete"], false);
    assert_eq!(document["local_credentials_cleared"], true);
    for step in ["link", "device", "session"] {
        assert_eq!(document["teardown"][step]["attempted"], false);
        assert_eq!(document["teardown"][step]["outcome_uncertain"], true);
        assert_eq!(document["teardown"][step]["error"], "outcome_uncertain");
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(100), listener.accept())
            .await
            .is_err(),
        "restart recovery dispatched a teardown request"
    );
    assert_eq!(auth_store.load().unwrap(), None);
    assert_eq!(session_store.load().await.unwrap(), None);
    assert!(!root.0.join("auth.reconciliation").exists());
    assert!(!root.0.join("credentials.reconciliation").exists());
}

#[tokio::test]
async fn lost_prepare_response_then_expiry_recovers_old_authority_without_second_issuance() {
    let root = TempRoot::new("login-prepare-loss-expiry");
    initialize(&root.0, old_scope());
    let auth_store = NativeAuthStore::open(&root.0).unwrap();
    let session_store = FileCredentialStore::open(&root.0).unwrap();
    let old_auth = auth_store.load().unwrap().unwrap();
    let old_session = session_store.load().await.unwrap().unwrap();
    assert_legacy_health_authority(&old_auth.channel.scope);
    let authorization_transaction_id = "authorization-transaction-prepare-loss";

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let verification_uri = format!("{base_url}/authorize");
    let first = tokio::spawn(async move {
        let mut client_transaction_id = String::new();
        for expected in [
            "/v1/auth/capabilities",
            "/v1/channel/oauth/device/authorize",
            "/v1/channel/oauth/device/token",
            "/v1/channel/oauth/cli/reauthorizations",
        ] {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            let path = request
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap();
            assert_eq!(path, expected);
            match path {
                "/v1/auth/capabilities" => {
                    respond(
                        &mut socket,
                        "application/json",
                        &serde_json::to_vec(&capabilities(false)).unwrap(),
                    )
                    .await;
                }
                "/v1/channel/oauth/device/authorize" => {
                    let body: Value =
                        serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
                    assert_eq!(body["scope"], FULL_SCOPE);
                    client_transaction_id =
                        body["client_transaction_id"].as_str().unwrap().to_owned();
                    respond(
                        &mut socket,
                        "application/json",
                        &serde_json::to_vec(&json!({
                            "device_code": "hf_dc_01234567890123456789",
                            "user_code": "ABCD-EFGH",
                            "verification_uri": verification_uri,
                            "verification_uri_complete": null,
                            "expires_in": 600,
                            "interval": 1
                        }))
                        .unwrap(),
                    )
                    .await;
                }
                "/v1/channel/oauth/device/token" => {
                    respond(
                        &mut socket,
                        "application/json",
                        &serde_json::to_vec(&json!({
                            "access_token": "provisional-prepare-loss-access",
                            "token_type": "bearer",
                            "refresh_token": "provisional-prepare-loss-refresh",
                            "expires_in": 3600,
                            "scope": FULL_SCOPE,
                            "link_id": "link-prepare-loss",
                            "resource": null,
                            "authorization_transaction_id": authorization_transaction_id
                        }))
                        .unwrap(),
                    )
                    .await;
                }
                "/v1/channel/oauth/cli/reauthorizations" => {
                    assert!(request.contains(&client_transaction_id));
                    // The backend committed the stage but the response was
                    // lost. Closing the socket exercises idempotent replay.
                    drop(socket);
                }
                _ => unreachable!(),
            }
        }
        client_transaction_id
    });
    let first_output = run(
        &root.0,
        &base_url,
        &["--json", "login", "--no-browser", "--timeout", "5"],
        None,
    )
    .await;
    assert!(!first_output.status.success());
    let client_transaction_id = first.await.unwrap();
    let pending = auth_store
        .pending_authorization_replacement()
        .unwrap()
        .unwrap();
    assert_eq!(pending.client_transaction_id, client_transaction_id);
    assert_eq!(
        pending.phase,
        heyfood_platform::AuthorizationReplacementPhase::Preparing
    );

    let recovery_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let recovery_url = format!("http://{}", recovery_listener.local_addr().unwrap());
    let recovery_client_transaction_id = client_transaction_id.clone();
    let recovery = tokio::spawn(async move {
        let (mut socket, _) = recovery_listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("POST /v1/channel/oauth/cli/reauthorizations "));
        assert!(request.contains(&recovery_client_transaction_id));
        respond(
            &mut socket,
            "application/json",
            &serde_json::to_vec(&json!({
                "stage_id": "stage-transaction-prepare-loss",
                "client_transaction_id": recovery_client_transaction_id,
                "authorization_transaction_id": authorization_transaction_id,
                "device_id": "heyfood-activated-device",
                "status": "expired",
                "scopes": FULL_SCOPE.split_whitespace().collect::<Vec<_>>(),
                "bundle_digest": "a".repeat(64),
                "recovery_token": null,
                "bundle": null,
                "expires_at": "2026-07-21T00:00:00Z",
                "recoverable_until": "2999-01-01T00:00:00Z",
                "promoted_at": null,
                "aborted_at": null
            }))
            .unwrap(),
        )
        .await;
    });
    let recovery_output = run(
        &root.0,
        &recovery_url,
        &["--json", "login", "--no-browser", "--timeout", "5"],
        None,
    )
    .await;
    assert!(!recovery_output.status.success());
    recovery.await.unwrap();
    assert_eq!(auth_store.load().unwrap(), Some(old_auth));
    assert_eq!(session_store.load().await.unwrap(), Some(old_session));
    assert!(
        auth_store
            .pending_authorization_replacement()
            .unwrap()
            .is_none()
    );
}
