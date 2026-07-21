use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::Parser;
use heyfood_agent_runtime::{CliAuthContext, HttpDeadlines, HttpService};
use heyfood_application::{
    BoxFuture, ClockPort, CredentialCommit, CredentialPort, EnsureSession, PortError,
};
use heyfood_bin::{OneShotExecutor, execute_qualified_one_shot};
use heyfood_cli::{CommandLine, OutputMode};
use heyfood_core::{
    AccountId, CredentialVersion, NetworkPolicy, SensitiveString, ServiceUrl, SessionCredentials,
    SessionSnapshot,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

fn credentials() -> SessionCredentials {
    SessionCredentials::from_unix_expiry(
        AccountId::parse("one-shot-account").unwrap(),
        SensitiveString::new("access"),
        SensitiveString::new("refresh"),
        CredentialVersion::new(1),
        4_102_444_800,
    )
    .unwrap()
}

async fn fixture_service() -> (TcpListener, HttpService) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = ServiceUrl::parse(
        &format!("http://{}/", listener.local_addr().unwrap()),
        NetworkPolicy::DEVELOPMENT,
    )
    .unwrap();
    let service = HttpService::new(
        base,
        NetworkPolicy::DEVELOPMENT,
        HttpDeadlines {
            connect: Duration::from_secs(1),
            request: Duration::from_secs(2),
            pool_idle: Duration::from_secs(1),
            sse_inactivity: Duration::from_secs(2),
        },
    )
    .unwrap()
    .with_cli_auth(
        CliAuthContext::new(
            "one-shot-device",
            SensitiveString::new("channel"),
            Some(SensitiveString::new("api-key")),
        )
        .unwrap(),
    );
    (listener, service)
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

async fn respond(socket: &mut TcpStream, body: Value) {
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

async fn respond_capabilities(socket: &mut TcpStream) {
    respond(
        socket,
        json!({
            "schema_version": 1,
            "self_registration": {"status": "disabled", "regions": [], "identity_methods": []},
            "authorization": {"loopback_pkce": true, "device_code": true, "identity_methods": []},
            "profile_readiness": true,
            "application_capabilities": {"grocery": "v1"}
        }),
    )
    .await;
}

fn proposal() -> Value {
    json!({
        "confirmation_id": "00000000-0000-4000-8000-000000000001",
        "idempotency_key": "00000000-0000-4000-8000-000000000002",
        "operation": "add_items",
        "expires_at": "2026-07-21T12:05:00Z",
        "structured_preview": {"items": [{"name": "onion"}]},
        "preconditions": [{"type": "list_version", "expected_version": 4}],
        "confirmation_token": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    })
}

#[tokio::test]
async fn json_add_is_one_value_and_preserves_server_confirmation_authority() {
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        assert!(
            read_request(&mut socket)
                .await
                .starts_with("GET /v1/auth/capabilities ")
        );
        respond_capabilities(&mut socket).await;

        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("POST /v1/grocery/items "));
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["expected_version"], 4);
        assert_eq!(body["items"][0]["name"], "onion");
        respond(&mut socket, proposal()).await;
    });
    let parsed = CommandLine::try_parse_from([
        "heyfood",
        "--json",
        "grocery",
        "add",
        "--list-id",
        "00000000-0000-4000-8000-000000000123",
        "--version",
        "4",
        "onion",
    ])
    .unwrap();
    let output = OneShotExecutor::new(&service, &credentials(), OutputMode::Json)
        .execute(parsed.command.unwrap(), &[], CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(output.lines().count(), 1);
    let decoded: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(
        decoded["confirmation_token"],
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    server.await.unwrap();
}

#[tokio::test]
async fn confirmation_consumes_proposal_from_stdin_and_not_process_arguments() {
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut socket).await;
        respond_capabilities(&mut socket).await;

        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("POST /v1/grocery/confirm "));
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["decision"], "cancel");
        assert_eq!(
            body["confirmation_token"],
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        respond(
            &mut socket,
            json!({
                "status": "cancelled",
                "operation": "add_items",
                "confirmation_id": "00000000-0000-4000-8000-000000000001",
                "list": null,
                "exclusions": null
            }),
        )
        .await;
    });
    let parsed =
        CommandLine::try_parse_from(["heyfood", "grocery", "confirm", "--decision", "cancel"])
            .unwrap();
    let stdin = serde_json::to_vec(&proposal()).unwrap();
    let output = OneShotExecutor::new(&service, &credentials(), OutputMode::Json)
        .execute(parsed.command.unwrap(), &stdin, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&output).unwrap()["status"],
        "cancelled"
    );
    server.await.unwrap();
}

#[tokio::test]
async fn health_disconnect_requires_local_confirmation_before_network() {
    let (listener, service) = fixture_service().await;
    let parsed = CommandLine::try_parse_from(["heyfood", "health", "disconnect", "oura"]).unwrap();
    let error = OneShotExecutor::new(&service, &credentials(), OutputMode::Json)
        .execute(parsed.command.unwrap(), &[], CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.code, "confirmation_required");
    assert!(
        tokio::time::timeout(Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn unported_registration_topology_is_fail_closed_without_network() {
    let (listener, service) = fixture_service().await;
    let parsed = CommandLine::try_parse_from(["heyfood", "register"]).unwrap();
    let error = OneShotExecutor::new(&service, &credentials(), OutputMode::Json)
        .execute(parsed.command.unwrap(), &[], CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.code, "phase2_parity_pending");
    assert!(
        tokio::time::timeout(Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );
}

#[derive(Default)]
struct MemoryCredentials {
    commits: Mutex<Vec<CredentialCommit>>,
}

impl CredentialPort for MemoryCredentials {
    fn load(&self) -> BoxFuture<'_, Result<Option<SessionCredentials>, PortError>> {
        Box::pin(async { Ok(None) })
    }

    fn commit(&self, commit: CredentialCommit) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            self.commits.lock().unwrap().push(commit);
            Ok(())
        })
    }

    fn mark_reconciliation_required(
        &self,
        _commit_id: heyfood_core::CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async { Ok(()) })
    }

    fn clear_reconciliation_required(
        &self,
        _commit_id: heyfood_core::CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async { Ok(()) })
    }
}

struct FixedClock;

impl ClockPort for FixedClock {
    fn unix_timestamp(&self) -> i64 {
        4_102_444_800
    }
}

#[tokio::test]
async fn qualified_one_shot_commits_rotation_before_using_the_new_access_token() {
    let (listener, service) = fixture_service().await;
    let service = Arc::new(service);
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let refresh = read_request(&mut socket).await;
        assert!(refresh.starts_with("POST /v1/auth/session/refresh "));
        respond(
            &mut socket,
            json!({
                "user_id": "one-shot-account",
                "access_token": "access-2",
                "refresh_token": "refresh-2",
                "access_expires_at": "2099-01-01T00:00:00Z"
            }),
        )
        .await;

        let (mut socket, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut socket).await;
        respond_capabilities(&mut socket).await;

        let (mut socket, _) = listener.accept().await.unwrap();
        let list = read_request(&mut socket).await;
        assert!(list.starts_with("GET /v1/grocery/list "));
        assert!(
            list.to_ascii_lowercase()
                .contains("authorization: bearer access-2")
        );
        respond(
            &mut socket,
            json!({
                "id": "00000000-0000-4000-8000-000000000123",
                "title": "Grocery List",
                "state": "active",
                "version": 4,
                "items": [],
                "created_at": "2026-07-21T12:00:00Z",
                "updated_at": "2026-07-21T12:00:00Z"
            }),
        )
        .await;
    });

    let store = Arc::new(MemoryCredentials::default());
    let ensure = EnsureSession::new(service.clone(), store.clone(), Arc::new(FixedClock));
    let parsed = CommandLine::try_parse_from(["heyfood", "--json", "grocery", "list"]).unwrap();
    let output = execute_qualified_one_shot(
        service.as_ref(),
        &ensure,
        SessionSnapshot {
            credentials: credentials(),
            reconciliation_required: false,
        },
        OutputMode::Json,
        parsed.command.unwrap(),
        &[],
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&output).unwrap()["version"],
        4
    );
    assert_eq!(store.commits.lock().unwrap().len(), 1);
    assert_eq!(
        store.commits.lock().unwrap()[0]
            .credentials
            .access_token
            .expose_secret(),
        "access-2"
    );
    server.await.unwrap();
}
