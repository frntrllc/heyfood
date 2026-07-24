#![cfg(not(windows))]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};

use heyfood_application::CredentialPort;
use heyfood_core::{
    AccountId, AuthCredentialBundle, ChannelCredentials, CredentialVersion, SensitiveString,
    SessionCredentials,
};
use heyfood_platform::{FileCredentialStore, NativeAuthStore};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;

const LIST_ID: &str = "00000000-0000-4000-8000-000000000123";
const FULL_SCOPE: &str = "account:link account:delete knowledge:read menu:read menu:watch recommend:read recipes:read recipes:write claims:read_derived profile:read profile:write meals:read meals:write audio:transcribe health:read integrations:manage grocery:read grocery:write";

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
    "account:link account:delete knowledge:read menu:read recommend:read recipes:read recipes:write claims:read_derived profile:read profile:write meals:read meals:write audio:transcribe"
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
async fn public_binary_dispatches_all_seventeen_health_grocery_and_watch_routes() {
    let root = TempRoot::new("routes");
    initialize(&root.0, FULL_SCOPE);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut product_routes = BTreeSet::new();
        for _ in 0..28 {
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
            vec!["--json", "grocery", "export", LIST_ID, "--format", "json"],
            None,
        ),
        (
            vec!["--json", "grocery", "confirm", "--decision", "cancel"],
            Some(serde_json::to_vec(&proposal("add_items")).unwrap()),
        ),
        (vec!["--json", "grocery", "exclusions"], None),
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
            vec![
                "--json",
                "grocery",
                "never",
                "--list-id",
                LIST_ID,
                "--version",
                "4",
                "--remove",
                "pork",
            ],
            None,
        ),
        (vec!["--json", "health", "status"], None),
        (vec!["--json", "health", "show"], None),
        (vec!["--json", "health", "connect", "oura"], None),
        (vec!["--json", "health", "sync", "oura"], None),
        (
            vec!["--json", "health", "disconnect", "oura", "--yes"],
            None,
        ),
        (vec!["--json", "watch"], None),
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
        "POST /v1/grocery/items".into(),
        "POST /v1/grocery/items/remove".into(),
        "POST /v1/grocery/items/state".into(),
        "GET /v1/grocery/exclusions".into(),
        "POST /v1/grocery/exclusions".into(),
        "POST /v1/grocery/exclusions/remove".into(),
        "POST /v1/grocery/confirm".into(),
        format!("GET /v1/grocery/lists/{LIST_ID}/export"),
        "GET /v1/health/context".into(),
        "GET /v1/integrations".into(),
        "POST /v1/integrations/authorize".into(),
        "POST /v1/integrations/oura/sync".into(),
        "DELETE /v1/integrations/oura".into(),
        "GET /v1/menu/watch".into(),
        "POST /v1/menu/watch".into(),
        "DELETE /v1/menu/watch/00000000-0000-4000-8000-000000000010".into(),
    ]);
    assert_eq!(routes, expected);
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
async fn public_binary_fails_closed_before_route_dispatch_for_scope_capability_and_confirmation() {
    let old = TempRoot::new("old-scope");
    initialize(
        &old.0,
        "account:link profile:read profile:write meals:read meals:write audio:transcribe",
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let output = run(&old.0, &base_url, &["--json", "health", "show"], None).await;
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

    let confirmed = TempRoot::new("confirmation");
    initialize(&confirmed.0, FULL_SCOPE);
    let output = run(
        &confirmed.0,
        &base_url,
        &["--json", "health", "disconnect", "oura"],
        None,
    )
    .await;
    assert!(!output.status.success());
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
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
async fn lost_prepare_response_then_expiry_recovers_old_authority_without_second_issuance() {
    let root = TempRoot::new("login-prepare-loss-expiry");
    initialize(&root.0, old_scope());
    let auth_store = NativeAuthStore::open(&root.0).unwrap();
    let session_store = FileCredentialStore::open(&root.0).unwrap();
    let old_auth = auth_store.load().unwrap().unwrap();
    let old_session = session_store.load().await.unwrap().unwrap();
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
