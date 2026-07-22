#![cfg(feature = "native-credentials")]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use heyfood_platform::NativeAuthStore;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "heyfood-native-composition-{}-{nonce}",
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

async fn respond_json(socket: &mut TcpStream, body: Value) {
    let body = serde_json::to_vec(&body).unwrap();
    socket
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    socket.write_all(&body).await.unwrap();
}

async fn run(root: &Path, base_url: &str, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_heyfood"))
        .args(args)
        .env("HEYFOOD_STATE_DIR", root)
        .env("HEYFOOD_CREDENTIAL_STORE", "native")
        .env("HEYFOOD_API_URL", base_url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .unwrap()
}

async fn cleanup(root: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_heyfood"))
        .arg("--version")
        .env("HEYFOOD_STATE_DIR", root)
        .env("HEYFOOD_TEST_DELETE_NATIVE_CREDENTIALS", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .unwrap()
}

#[tokio::test]
async fn default_executable_uses_brokered_native_account_bound_credentials() {
    let root = TempRoot::new();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/", listener.local_addr().unwrap());
    let service_url = base_url.clone();
    let server = tokio::spawn(async move {
        let full_scope = "account:link account:delete knowledge:read menu:read recommend:read recipes:read recipes:write claims:read_derived profile:read profile:write meals:read meals:write audio:transcribe health:read integrations:manage grocery:read grocery:write";
        let verification_uri = format!("{service_url}authorize");
        for _ in 0..6 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            let path = request
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap();
            match path {
                "/v1/auth/capabilities" => {
                    respond_json(&mut socket, json!({
                        "schema_version": 1,
                        "self_registration": {"status": "available", "regions": ["US"], "identity_methods": ["sms", "email"]},
                        "authorization": {"loopback_pkce": true, "device_code": true, "identity_methods": ["sms", "email"]},
                        "profile_readiness": true,
                        "application_capabilities": {}
                    })).await;
                }
                "/v1/channel/oauth/device/authorize" => {
                    respond_json(
                        &mut socket,
                        json!({
                            "device_code": "hf_dc_01234567890123456789",
                            "user_code": "ABCD-EFGH",
                            "verification_uri": verification_uri,
                            "verification_uri_complete": null,
                            "expires_in": 600,
                            "interval": 1
                        }),
                    )
                    .await;
                }
                "/v1/channel/oauth/device/token" => {
                    respond_json(
                        &mut socket,
                        json!({
                            "access_token": "channel-access",
                            "token_type": "bearer",
                            "refresh_token": "channel-refresh",
                            "expires_in": 3600,
                            "scope": full_scope
                        }),
                    )
                    .await;
                }
                "/v1/channel/oauth/cli/session" => {
                    let request_body: Value =
                        serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
                    let device_id = request_body["device_id"].as_str().unwrap();
                    respond_json(
                        &mut socket,
                        json!({
                            "user_id": "native-composition-account",
                            "device_id": device_id,
                            "session_id": "native-composition-session",
                            "access_token": "session-access",
                            "refresh_token": "session-refresh",
                            "access_expires_at": "2999-01-01T00:00:00Z",
                            "refresh_expires_at": "2999-02-01T00:00:00Z",
                            "scopes": full_scope.split_whitespace().collect::<Vec<_>>(),
                            "is_anonymous": false
                        }),
                    )
                    .await;
                }
                "/v1/channel/tools/profile/readiness" => {
                    respond_json(
                        &mut socket,
                        json!({
                            "schema_version": 1,
                            "status": "ready",
                            "member_id": "_self",
                            "has_profile_sync_consent": true,
                            "profile_version": 1
                        }),
                    )
                    .await;
                }
                "/v1/agent/converse" => {
                    assert!(
                        request
                            .to_ascii_lowercase()
                            .contains("authorization: bearer session-access")
                    );
                    let body = b"event: result\ndata: {\"message\":\"native broker ok\"}\n\nevent: done\ndata: {}\n\n";
                    socket.write_all(format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    ).as_bytes()).await.unwrap();
                    socket.write_all(body).await.unwrap();
                }
                _ => panic!("unexpected path: {path}"),
            }
        }
    });
    let registration = run(
        &root.0,
        &base_url,
        &[
            "--json",
            "register",
            "--device",
            "--no-browser",
            "--timeout",
            "5",
            "--no-onboard",
        ],
    )
    .await;
    assert!(
        registration.status.success(),
        "registration stdout: {}; stderr: {}",
        String::from_utf8_lossy(&registration.stdout),
        String::from_utf8_lossy(&registration.stderr)
    );
    let output = run(
        &root.0,
        &base_url,
        &["--json", "ask", "native", "composition"],
    )
    .await;
    if !output.status.success() {
        server.abort();
        let _ = cleanup(&root.0).await;
        panic!(
            "status: {}; stdout: {}; stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    server.await.unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["message"],
        "native broker ok"
    );
    assert!(cleanup(&root.0).await.status.success());
    #[cfg(windows)]
    NativeAuthStore::open(&root.0).unwrap().delete().unwrap();
}
