use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::Parser;
use heyfood_agent_runtime::{CliAuthContext, HttpDeadlines, HttpService};
use heyfood_application::{
    BoxFuture, ClockPort, CredentialCommit, CredentialPort, EnsureSession, EnsureSessionError,
    PortError,
};
use heyfood_bin::{OneShotError, OneShotExecutor, execute_qualified_one_shot};
use heyfood_cli::{CommandLine, OutputMode, render_agent_result};
use heyfood_core::{
    AccountId, CredentialVersion, ImportedPythonState, NetworkPolicy, SensitiveString, ServiceUrl,
    SessionCredentials, SessionSnapshot,
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

fn python_oracle() -> Value {
    serde_json::from_str(include_str!(
        "fixtures/python-phase2-command-parity.v1.json"
    ))
    .unwrap()
}

fn imported_state(fields: impl IntoIterator<Item = (&'static str, Value)>) -> ImportedPythonState {
    ImportedPythonState {
        account_user_id: Some("one-shot-account".into()),
        global: BTreeMap::new(),
        account_scoped: fields
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    }
}

#[test]
fn session_reconciliation_errors_remain_uncertain_at_the_cli_boundary() {
    let cases = [
        EnsureSessionError::ReconciliationRequired,
        EnsureSessionError::ServiceReconciliationRequired(PortError::uncertain(
            "refresh_transport",
            "response was not observed",
        )),
        EnsureSessionError::CredentialReconciliationRequired(PortError::new(
            "credential_write",
            "write failed",
        )),
        EnsureSessionError::ReconciliationMarkerWrite {
            operation: PortError::uncertain("refresh_transport", "response was not observed"),
            marker: PortError::new("marker_write", "write failed"),
        },
    ];
    for error in cases {
        let converted = OneShotError::from(error);
        assert!(converted.outcome_uncertain);
        assert!(converted.code.contains("reconciliation") || converted.code.contains("uncertain"));
    }
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

async fn respond_stream(socket: &mut TcpStream, body: &[u8]) {
    socket
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    socket.write_all(body).await.unwrap();
}

async fn respond_stream_chunks(socket: &mut TcpStream, chunks: &[Vec<u8>]) {
    socket
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    for chunk in chunks {
        socket
            .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
            .await
            .unwrap();
        socket.write_all(chunk).await.unwrap();
        socket.write_all(b"\r\n").await.unwrap();
        socket.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    socket.write_all(b"0\r\n\r\n").await.unwrap();
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

#[tokio::test]
async fn reply_requires_explicit_conversation_until_native_persistence_exists() {
    let (listener, service) = fixture_service().await;
    let parsed = CommandLine::try_parse_from(["heyfood", "reply", "the", "second", "one"]).unwrap();
    let error = OneShotExecutor::new(&service, &credentials(), OutputMode::Json)
        .execute(parsed.command.unwrap(), &[], CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.code, "conversation_required");
    assert!(
        tokio::time::timeout(Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn one_shot_ask_collects_sse_into_exactly_one_json_value() {
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("POST /v1/agent/converse "));
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["query"], "what can I eat?");
        assert_eq!(body["input_mode"], "text");
        respond_stream_chunks(
            &mut socket,
            &[
                b"event: thinking\ndata: {\"stage\":\"route\"}\n\nevent: partial\ndata: {\"text\":\"Try soup.\"}\n\nevent: result\ndata: {\"conversation_id\":\"conversation-2\",\"message\":\"Try soup.\"}\n\n".to_vec(),
                b"event: done\ndata: {}\n\n".to_vec(),
            ],
        )
        .await;
    });
    let parsed =
        CommandLine::try_parse_from(["heyfood", "--json", "ask", "what", "can", "I", "eat?"])
            .unwrap();
    let output = OneShotExecutor::new(&service, &credentials(), OutputMode::Json)
        .execute(parsed.command.unwrap(), &[], CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(output.lines().count(), 1);
    assert_eq!(
        serde_json::from_str::<Value>(&output).unwrap()["message"],
        "Try soup."
    );
    server.await.unwrap();
}

#[tokio::test]
async fn streamed_choices_match_the_frozen_python_json_and_human_oracle() {
    let oracle = python_oracle();
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut socket).await;
        respond_stream(
            &mut socket,
            b"event: partial\ndata: {\"text\":\"Try soup.\"}\n\nevent: choices\ndata: {\"choices\":[\"First\",\"Second\"],\"allow_multiple\":false}\n\nevent: result\ndata: {}\n\nevent: done\ndata: {}\n\n",
        )
        .await;
    });
    let parsed = CommandLine::try_parse_from(["heyfood", "--json", "ask", "lunch"]).unwrap();
    let output = OneShotExecutor::new(&service, &credentials(), OutputMode::Json)
        .execute(parsed.command.unwrap(), &[], CancellationToken::new())
        .await
        .unwrap();
    let document: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(document["text"], oracle["stream"]["partial_text"]);
    assert_eq!(document["choices"]["choices"], oracle["stream"]["choices"]);
    assert_eq!(
        document["choices"]["allow_multiple"],
        oracle["stream"]["allow_multiple"]
    );
    let rendered = render_agent_result(&document, OutputMode::HumanPlain);
    for expected in oracle["stream"]["human_lines"].as_array().unwrap() {
        assert!(
            rendered
                .lines()
                .any(|line| line == expected.as_str().unwrap())
        );
    }
    server.await.unwrap();
}

#[tokio::test]
async fn log_preserves_the_frozen_meal_prompt_and_type_semantics() {
    let oracle = python_oracle();
    let expected_query = oracle["log"]["query"].as_str().unwrap().to_owned();
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("POST /v1/agent/converse "));
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["query"], expected_query);
        respond_stream(
            &mut socket,
            b"event: result\ndata: {\"message\":\"Logged.\"}\n\nevent: done\ndata: {}\n\n",
        )
        .await;
    });
    let parsed = CommandLine::try_parse_from([
        "heyfood",
        "--json",
        "log",
        "--type",
        "breakfast",
        oracle["log"]["meal_input"].as_str().unwrap(),
    ])
    .unwrap();
    let output = OneShotExecutor::new(&service, &credentials(), OutputMode::Json)
        .execute(parsed.command.unwrap(), &[], CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&output).unwrap()["message"],
        "Logged."
    );
    server.await.unwrap();
}

#[tokio::test]
async fn item_uses_the_channel_tool_and_preserves_restaurant_context() {
    let oracle = python_oracle();
    let expected = oracle["item"]["request"].clone();
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("POST /v1/channel/tools/explain_item "));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer channel")
        );
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body, expected);
        respond(
            &mut socket,
            json!({
                "item_name": "veggie burger",
                "status": "compatible",
                "summary": "This item fits the profile."
            }),
        )
        .await;
    });
    let parsed = CommandLine::try_parse_from([
        "heyfood",
        "--json",
        "item",
        "--restaurant",
        oracle["item"]["restaurant_input"].as_str().unwrap(),
        oracle["item"]["item_input"].as_str().unwrap(),
    ])
    .unwrap();
    let output = OneShotExecutor::new(&service, &credentials(), OutputMode::Json)
        .execute(parsed.command.unwrap(), &[], CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&output).unwrap()["status"],
        "compatible"
    );
    server.await.unwrap();
}

#[tokio::test]
async fn item_at_resolves_the_account_bound_imported_search() {
    let state = imported_state([(
        "last_restaurant_search",
        json!({"restaurants": [{"id": "restaurant-1", "name": "Cafe One"}]}),
    )]);
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["restaurant_name"], "Cafe One");
        respond(
            &mut socket,
            json!({
                "item_name": "soup",
                "status": "compatible",
                "summary": "Fits."
            }),
        )
        .await;
    });
    let parsed =
        CommandLine::try_parse_from(["heyfood", "--json", "item", "--at", "1", "soup"]).unwrap();
    OneShotExecutor::new(&service, &credentials(), OutputMode::Json)
        .with_imported_state(Some(&state))
        .execute(parsed.command.unwrap(), &[], CancellationToken::new())
        .await
        .unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn log_for_builds_consent_aware_household_context() {
    let state = imported_state([
        ("first_name", json!("Justin")),
        (
            "household",
            json!({
                "members": [
                    {"id": "_self", "name": "Justin", "relationship": "self", "archived": false},
                    {"id": "member-sarah", "name": "Sarah", "relationship": "partner", "archived": false}
                ]
            }),
        ),
    ]);
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("GET /v1/profile/consent "));
        respond(&mut socket, json!({"has_consent": true})).await;

        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("GET /v1/profile/sync?member_id=member-sarah "));
        respond(
            &mut socket,
            json!({"profile_data": {"preferences": ["vegetarian"]}}),
        )
        .await;

        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["dietary_context"]["name"], "Sarah");
        assert_eq!(body["dietary_context"]["owner_name"], "Justin");
        assert_eq!(body["dietary_context"]["preferences"][0], "vegetarian");
        assert_eq!(body["meal_context"]["active_member_id"], "member-sarah");
        assert_eq!(
            body["device_context"]["household"]["members"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        respond_stream(
            &mut socket,
            b"event: result\ndata: {\"message\":\"Logged.\"}\n\nevent: done\ndata: {}\n\n",
        )
        .await;
    });
    let parsed =
        CommandLine::try_parse_from(["heyfood", "--json", "log", "--for", "Sarah", "oatmeal"])
            .unwrap();
    OneShotExecutor::new(&service, &credentials(), OutputMode::Json)
        .with_imported_state(Some(&state))
        .execute(parsed.command.unwrap(), &[], CancellationToken::new())
        .await
        .unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn oracle_text_limits_trim_and_count_unicode_characters() {
    let (listener, service) = fixture_service().await;
    let over_query = "x".repeat(501);
    let parsed = CommandLine::try_parse_from(["heyfood", "ask", over_query.as_str()]).unwrap();
    let error = OneShotExecutor::new(&service, &credentials(), OutputMode::Json)
        .execute(parsed.command.unwrap(), &[], CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid_argument");

    let valid_unicode_item = "é".repeat(200);
    let parsed =
        CommandLine::try_parse_from(["heyfood", "item", valid_unicode_item.as_str()]).unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["item_name"].as_str().unwrap().chars().count(), 200);
        respond(
            &mut socket,
            json!({"item_name": "unicode", "status": "unknown", "summary": "ok"}),
        )
        .await;
    });
    OneShotExecutor::new(&service, &credentials(), OutputMode::Json)
        .execute(parsed.command.unwrap(), &[], CancellationToken::new())
        .await
        .unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn invalid_terminal_events_fail_without_returning_partial_machine_output() {
    for (terminal, expected_code) in [
        (
            "event: done\ndata: {\"unexpected\":true}\n\n",
            "sse_payload",
        ),
        ("event: future_terminal\ndata: {}\n\n", "sse_event"),
    ] {
        let (listener, service) = fixture_service().await;
        let result = b"event: partial\ndata: {\"text\":\"Do not emit me.\"}\n\nevent: result\ndata: {\"message\":\"Do not emit me.\"}\n\n".to_vec();
        let terminal = terminal.as_bytes().to_vec();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request(&mut socket).await;
            respond_stream_chunks(&mut socket, &[result, terminal]).await;
        });
        let parsed = CommandLine::try_parse_from(["heyfood", "--json", "ask", "fixture"]).unwrap();
        let error = OneShotExecutor::new(&service, &credentials(), OutputMode::Json)
            .execute(parsed.command.unwrap(), &[], CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(error.code, expected_code);
        assert!(error.outcome_uncertain);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn clean_legacy_eof_after_result_preserves_one_value_json_output() {
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_request(&mut socket).await;
        respond_stream(
            &mut socket,
            b"event: partial\ndata: {\"text\":\"Legacy.\"}\n\nevent: result\ndata: {\"message\":\"Legacy.\"}\n\n",
        )
        .await;
    });
    let parsed = CommandLine::try_parse_from(["heyfood", "--json", "ask", "legacy"]).unwrap();
    let output = OneShotExecutor::new(&service, &credentials(), OutputMode::Json)
        .execute(parsed.command.unwrap(), &[], CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(output.lines().count(), 1);
    assert_eq!(
        serde_json::from_str::<Value>(&output).unwrap()["message"],
        "Legacy."
    );
    server.await.unwrap();
}

#[tokio::test]
async fn split_error_and_done_preserve_authoritative_error_semantics() {
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_request(&mut socket).await;
        respond_stream_chunks(
            &mut socket,
            &[
                b"event: error\ndata: {\"code\":\"service_error\",\"message\":\"Unable to answer.\",\"retryable\":false}\n\n".to_vec(),
                b"event: done\ndata: {}\n\n".to_vec(),
            ],
        )
        .await;
    });
    let parsed = CommandLine::try_parse_from(["heyfood", "--json", "ask", "fixture"]).unwrap();
    let error = OneShotExecutor::new(&service, &credentials(), OutputMode::Json)
        .execute(parsed.command.unwrap(), &[], CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.code, "agent_error");
    assert!(!error.outcome_uncertain);
    server.await.unwrap();
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
