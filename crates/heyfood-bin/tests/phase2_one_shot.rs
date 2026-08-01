use std::collections::BTreeMap;
use std::io::{Cursor, ErrorKind, Read};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::Parser;
use heyfood_agent_runtime::{CliAuthContext, HttpDeadlines, HttpService};
use heyfood_application::{
    BoxFuture, ClockPort, CredentialCommit, CredentialPort, EnsureSession, EnsureSessionError,
    PortError,
};
use heyfood_bin::{
    OneShotError, OneShotExecutor, execute_qualified_one_shot, execute_qualified_prepared_log,
    prepare_log_command, prepare_qualified_log,
};
use heyfood_cli::{CommandLine, OutputMode, render_agent_result, render_item_result};
use heyfood_core::{
    AccountId, CredentialVersion, ImportedPythonState, NetworkPolicy, SensitiveString, ServiceUrl,
    SessionCredentials, SessionSnapshot,
};
use heyfood_platform::PythonStateImporter;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
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

#[test]
fn legacy_oracle_keeps_its_reviewed_provenance_manifest() {
    let oracle = python_oracle();
    let commit = oracle["provenance"]["repository_commit"].as_str().unwrap();
    assert_eq!(commit, "73494a57468dac83b4904ce6c390e36926f5c6fe");
    assert_eq!(
        oracle["provenance"]["archive_tag"],
        "archive/python-cli-73494a57"
    );

    let archive_bytes = include_bytes!("../../../tests/fixtures/python-cli-73494a57.tar");
    let archive_digest = format!("{:x}", Sha256::digest(archive_bytes));
    assert_eq!(
        archive_digest,
        oracle["provenance"]["source_archive"]["sha256"]
    );

    let expected = oracle["provenance"]["sources"].as_object().unwrap();
    assert_eq!(expected.len(), 4);

    let mut archive = tar::Archive::new(Cursor::new(archive_bytes));
    let mut actual = BTreeMap::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap().to_string_lossy().into_owned();
        if expected.contains_key(&path) {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            let previous = actual.insert(path, format!("{:x}", Sha256::digest(bytes)));
            assert!(previous.is_none(), "duplicate archived source path");
        }
    }

    assert_eq!(actual.len(), expected.len());
    for (path, expected_digest) in expected {
        assert_eq!(actual.get(path).unwrap(), expected_digest.as_str().unwrap());
    }
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

struct LogTempRoot(PathBuf);

impl LogTempRoot {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "heyfood-phase2-prepared-log-{name}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for LogTempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn prepared_log_importer(
    name: &str,
    state: &ImportedPythonState,
) -> (LogTempRoot, PythonStateImporter) {
    let root = LogTempRoot::new(name);
    let source = root.0.join("config.json");
    let mut document = serde_json::Map::new();
    document.insert(
        "account_user_id".into(),
        Value::String(
            state
                .account_user_id
                .clone()
                .expect("fixture state is account bound"),
        ),
    );
    for (field, value) in &state.account_scoped {
        document.insert(field.clone(), value.clone());
    }
    std::fs::write(
        &source,
        serde_json::to_vec(&Value::Object(document)).unwrap(),
    )
    .unwrap();
    let importer = PythonStateImporter::under(&source, root.0.join("native"));
    importer.import().unwrap();
    (root, importer)
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
            transcription: Duration::from_secs(2),
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
    if let Err(error) = socket.write_all(b"0\r\n\r\n").await {
        assert!(
            matches!(
                error.kind(),
                ErrorKind::BrokenPipe
                    | ErrorKind::ConnectionAborted
                    | ErrorKind::ConnectionReset
                    | ErrorKind::NotConnected
            ),
            "chunked fixture terminator failed unexpectedly: {error}"
        );
    }
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
async fn exact_abby_jane_ask_renders_the_complete_current_menu_in_human_output() {
    const QUERY: &str = "Show me this week's menu at Abby Jane Bakeshop in Dripping Springs, Texas";

    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("POST /v1/agent/converse "));
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["query"], QUERY);
        assert_eq!(body["input_mode"], "text");

        let result = json!({
            "conversation_id": "abby-jane-menu-conversation",
            "message": "I found the current menu.",
            "structured": {
                "type": "household_menu",
                "presentation": "full_menu",
                "restaurant_name": "Abby Jane Bakeshop",
                "source_url": "https://www.abbyjanebakes.com/menu",
                "source_lineage": "hunter_toast_sites",
                "menu_freshness": "Menu updated 2 hours ago",
                "captured_at": "2026-07-26T17:27:14Z",
                "freshness_hours": 2.0,
                "requested_max_age_seconds": 86400,
                "is_stale": false,
                "sections": [
                    {
                        "name": "Bread",
                        "items": [
                            {
                                "name": "Big Country",
                                "description": "Naturally leavened country sourdough.",
                                "price_cents": 900,
                                "composite_level": "avoid",
                                "safety": {
                                    "member-jane": {
                                        "member_id": "member-jane",
                                        "label": "Jane",
                                        "level": "avoid",
                                        "reason": "Contains wheat flour (Celiac)",
                                        "chips": ["Contains gluten"],
                                        "conflicts": []
                                    }
                                },
                                "allergen_detail": [{
                                    "allergen_code": "wheat",
                                    "confidence": "high",
                                    "source": "owner_added",
                                    "evidence": "Owner-confirmed wheat flour"
                                }]
                            },
                            {
                                "name": "Baguette",
                                "price_cents": 400,
                                "composite_level": "caution"
                            }
                        ]
                    },
                    {
                        "name": "Pastries",
                        "items": [
                            {
                                "name": "Chocolate Croissant",
                                "price_cents": 650,
                                "composite_level": "generally_safer"
                            }
                        ]
                    }
                ]
            }
        });
        let stream =
            format!("event: result\ndata: {result}\n\nevent: done\ndata: {{}}\n\n").into_bytes();
        respond_stream(&mut socket, &stream).await;
    });

    let parsed = CommandLine::try_parse_from(["heyfood", "ask", QUERY]).unwrap();
    let output = OneShotExecutor::new(&service, &credentials(), OutputMode::HumanPlain)
        .execute(parsed.command.unwrap(), &[], CancellationToken::new())
        .await
        .unwrap();

    for expected in [
        "I found the current menu.",
        "Current menu at Abby Jane Bakeshop",
        "Source: https://www.abbyjanebakes.com/menu",
        "Menu source: Restaurant ordering page",
        "Freshness: Menu updated 2 hours ago",
        "Captured: 2026-07-26T17:27:14Z",
        "Bread",
        "• Big Country  $9.00  [avoid]",
        "  Why for Jane (avoid): Contains wheat flour (Celiac)",
        "    Flags: Contains gluten",
        "  Allergen flag: wheat (high confidence, restaurant-confirmed)",
        "    Evidence: Owner-confirmed wheat flour",
        "• Baguette  $4.00  [caution]",
        "Pastries",
        "• Chocolate Croissant  $6.50  [generally safer]",
    ] {
        assert!(
            output.lines().any(|line| line == expected),
            "missing {expected:?} from:\n{output}"
        );
    }
    assert_eq!(output.matches("• ").count(), 3);
    assert!(!output.contains('\u{1b}'));
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

    let expected = &oracle["stream"]["normalized_detailed_choice_extension"];
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut socket).await;
        respond_stream(
            &mut socket,
            b"event: choices\ndata: {\"choices\":[{\"label\":\"First\",\"value\":\"1\"}],\"allow_multiple\":false}\n\nevent: result\ndata: {\"message\":\"Choose.\"}\n\nevent: done\ndata: {}\n\n",
        )
        .await;
    });
    let parsed = CommandLine::try_parse_from(["heyfood", "--json", "ask", "choose"]).unwrap();
    let output = OneShotExecutor::new(&service, &credentials(), OutputMode::Json)
        .execute(parsed.command.unwrap(), &[], CancellationToken::new())
        .await
        .unwrap();
    let document: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(document["choices"]["choices"], expected["choices"]);
    assert_eq!(
        document["choices"]["choice_details"],
        expected["choice_details"]
    );
    server.await.unwrap();
}

#[tokio::test]
async fn log_preserves_the_frozen_meal_prompt_and_type_semantics() {
    let oracle = python_oracle();
    let expected_query = oracle["log"]["query"].as_str().unwrap().to_owned();
    let state = imported_state([
        ("first_name", json!("Justin")),
        (
            "household",
            json!({
                "active_scope": oracle["log"]["default_scope"]["active_scope"],
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
        respond(&mut socket, json!({"has_consent": false})).await;

        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("POST /v1/agent/converse "));
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["query"], expected_query);
        assert_eq!(body["dietary_context"]["name"], "Sarah");
        assert_eq!(body["meal_context"]["active_member_id"], "member-sarah");
        assert!(body.get("device_context").is_none());
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
    let Some(heyfood_cli::Command::Log(arguments)) = parsed.command else {
        panic!("expected log command");
    };
    let (_root, importer) = prepared_log_importer("prompt-semantics", &state);
    let prepared = prepare_log_command(arguments, &[], importer.preview_state().unwrap()).unwrap();
    let verified = importer
        .verify_after_review(prepared.source_preview())
        .unwrap();
    let credentials = credentials();
    let prepared = prepare_qualified_log(
        &service,
        &credentials,
        prepared,
        verified,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let output = execute_qualified_prepared_log(
        &service,
        credentials,
        OutputMode::Json,
        prepared,
        CancellationToken::new(),
    )
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
    let human = render_item_result(
        &json!({
            "item_name": "veggie burger",
            "status": "compatible",
            "summary": "This item fits the profile.",
            "confidence": 0.95,
            "member_name": "Sarah"
        }),
        OutputMode::HumanPlain,
    );
    for expected in oracle["item"]["human_lines"].as_array().unwrap() {
        assert!(human.lines().any(|line| line == expected.as_str().unwrap()));
    }
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
    let parsed = CommandLine::try_parse_from([
        "heyfood",
        "--json",
        "item",
        "--at",
        "1",
        "--restaurant",
        "Ignored Cafe",
        "soup",
    ])
    .unwrap();
    OneShotExecutor::new(&service, &credentials(), OutputMode::Json)
        .with_imported_state(Some(&state))
        .execute(parsed.command.unwrap(), &[], CancellationToken::new())
        .await
        .unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn item_nonnumeric_at_preserves_the_explicit_restaurant_like_python() {
    let oracle = python_oracle();
    let selector = &oracle["item"]["selectors"][1];
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["restaurant_name"], "Cafe One");
        respond(
            &mut socket,
            json!({"item_name": "soup", "status": "compatible", "summary": "Fits."}),
        )
        .await;
    });
    let parsed = CommandLine::try_parse_from([
        "heyfood",
        "--json",
        "item",
        "--at",
        selector["at"].as_str().unwrap(),
        "--restaurant",
        selector["explicit_restaurant"].as_str().unwrap(),
        "soup",
    ])
    .unwrap();
    OneShotExecutor::new(&service, &credentials(), OutputMode::Json)
        .execute(parsed.command.unwrap(), &[], CancellationToken::new())
        .await
        .unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn prepared_log_omitted_for_dispatches_the_reviewed_member() {
    let state = imported_state([
        ("first_name", json!("Justin")),
        (
            "household",
            json!({
                "active_scope": "member-sarah",
                "members": [
                    {"id": "_self", "name": "Justin", "relationship": "self", "archived": false},
                    {"id": "member-sarah", "name": "Sarah", "relationship": "partner", "archived": false}
                ]
            }),
        ),
        (
            "household_profile_outbox",
            json!({"_self": {"local_context": {"preferences": ["omnivore"]}}}),
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
    let parsed = CommandLine::try_parse_from(["heyfood", "--json", "log", "oatmeal"]).unwrap();
    let (_root, importer) = prepared_log_importer("household-context", &state);
    let CommandLine {
        command: Some(heyfood_cli::Command::Log(arguments)),
        ..
    } = parsed
    else {
        panic!("expected log command");
    };
    let preview = importer.preview_state().unwrap();
    let prepared = prepare_log_command(arguments, &[], preview).unwrap();
    assert!(
        prepared
            .review_document()
            .contains("\"Sarah\" [member-id-utf8-hex=6d656d6265722d7361726168]")
    );
    let verified = importer
        .verify_after_review(prepared.source_preview())
        .unwrap();
    let credentials = credentials();
    let prepared = prepare_qualified_log(
        &service,
        &credentials,
        prepared,
        verified,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    execute_qualified_prepared_log(
        &service,
        credentials,
        OutputMode::Json,
        prepared,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn prepared_log_explicit_self_overrides_saved_member() {
    let state = imported_state([
        ("first_name", json!("Justin")),
        (
            "household",
            json!({
                "active_scope": "member-sarah",
                "members": [
                    {"id": "_self", "name": "Justin", "relationship": "self", "archived": false},
                    {"id": "member-sarah", "name": "Sarah", "relationship": "partner", "archived": false}
                ]
            }),
        ),
        (
            "household_profile_outbox",
            json!({"_self": {"local_context": {"preferences": ["omnivore"]}}}),
        ),
    ]);
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        assert!(
            read_request(&mut socket)
                .await
                .starts_with("GET /v1/profile/consent ")
        );
        respond(&mut socket, json!({"has_consent": true})).await;

        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["meal_context"]["active_member_id"], "_self");
        assert_eq!(body["meal_context"]["active_member_name"], "Me");
        respond_stream(
            &mut socket,
            b"event: result\ndata: {\"message\":\"Logged.\"}\n\nevent: done\ndata: {}\n\n",
        )
        .await;
    });
    let parsed =
        CommandLine::try_parse_from(["heyfood", "--json", "log", "--for", "self", "oatmeal"])
            .unwrap();
    let Some(heyfood_cli::Command::Log(arguments)) = parsed.command else {
        panic!("expected log command");
    };
    let (_root, importer) = prepared_log_importer("explicit-self", &state);
    let prepared = prepare_log_command(arguments, &[], importer.preview_state().unwrap()).unwrap();
    assert!(prepared.review_document().contains("\"Me\" [scope=_self]"));
    let verified = importer
        .verify_after_review(prepared.source_preview())
        .unwrap();
    let credentials = credentials();
    let prepared = prepare_qualified_log(
        &service,
        &credentials,
        prepared,
        verified,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    execute_qualified_prepared_log(
        &service,
        credentials,
        OutputMode::Json,
        prepared,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn prepared_log_everyone_preserves_reviewed_cook_mode() {
    let state = imported_state([
        ("first_name", json!("Justin")),
        (
            "household",
            json!({
                "active_scope": "__everyone__",
                "members": [
                    {"id": "_self", "name": "Justin", "relationship": "self", "archived": false},
                    {"id": "member-sarah", "name": "Sarah", "relationship": "partner", "archived": false}
                ]
            }),
        ),
        (
            "household_profile_outbox",
            json!({
                "_self": {"local_context": {}},
                "member-sarah": {"local_context": {}}
            }),
        ),
    ]);
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        assert!(
            read_request(&mut socket)
                .await
                .starts_with("GET /v1/profile/consent ")
        );
        respond(&mut socket, json!({"has_consent": true})).await;

        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["meal_context"]["is_cook_mode"], true);
        assert!(body["meal_context"].get("active_member_id").is_none());
        assert_eq!(
            body["dietary_context"]["members"].as_array().unwrap().len(),
            2
        );
        respond_stream(
            &mut socket,
            b"event: result\ndata: {\"message\":\"Logged.\"}\n\nevent: done\ndata: {}\n\n",
        )
        .await;
    });
    let parsed = CommandLine::try_parse_from(["heyfood", "--json", "log", "oatmeal"]).unwrap();
    let Some(heyfood_cli::Command::Log(arguments)) = parsed.command else {
        panic!("expected log command");
    };
    let (_root, importer) = prepared_log_importer("everyone", &state);
    let prepared = prepare_log_command(arguments, &[], importer.preview_state().unwrap()).unwrap();
    assert!(
        prepared
            .review_document()
            .contains("\"Everyone\" [scope=__everyone__]")
    );
    let verified = importer
        .verify_after_review(prepared.source_preview())
        .unwrap();
    let credentials = credentials();
    let prepared = prepare_qualified_log(
        &service,
        &credentials,
        prepared,
        verified,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    execute_qualified_prepared_log(
        &service,
        credentials,
        OutputMode::Json,
        prepared,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn prepared_log_account_mismatch_dispatches_no_profile_or_converse_request() {
    let mut state = imported_state([(
        "household",
        json!({
            "active_scope": "_self",
            "members": [{"id": "_self", "name": "Justin", "archived": false}]
        }),
    )]);
    state.account_user_id = Some("different-account".into());
    let (listener, service) = fixture_service().await;
    let parsed = CommandLine::try_parse_from(["heyfood", "--json", "log", "oatmeal"]).unwrap();
    let Some(heyfood_cli::Command::Log(arguments)) = parsed.command else {
        panic!("expected log command");
    };
    let (_root, importer) = prepared_log_importer("account-mismatch", &state);
    let prepared = prepare_log_command(arguments, &[], importer.preview_state().unwrap()).unwrap();
    let verified = importer
        .verify_after_review(prepared.source_preview())
        .unwrap();
    let error = prepare_qualified_log(
        &service,
        &credentials(),
        prepared,
        verified,
        CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "python_state_account_mismatch");
    assert!(
        tokio::time::timeout(Duration::from_millis(25), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn prepared_log_malformed_consent_fails_before_profile_or_converse_dispatch() {
    let state = imported_state([(
        "household",
        json!({
            "active_scope": "member-sarah",
            "members": [
                {"id": "_self", "name": "Justin", "archived": false},
                {"id": "member-sarah", "name": "Sarah", "archived": false}
            ]
        }),
    )]);
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        assert!(
            read_request(&mut socket)
                .await
                .starts_with("GET /v1/profile/consent ")
        );
        respond(&mut socket, json!({"has_consent": "yes"})).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(25), listener.accept())
                .await
                .is_err()
        );
    });
    let parsed = CommandLine::try_parse_from(["heyfood", "--json", "log", "oatmeal"]).unwrap();
    let Some(heyfood_cli::Command::Log(arguments)) = parsed.command else {
        panic!("expected log command");
    };
    let (_root, importer) = prepared_log_importer("malformed-consent", &state);
    let prepared = prepare_log_command(arguments, &[], importer.preview_state().unwrap()).unwrap();
    let verified = importer
        .verify_after_review(prepared.source_preview())
        .unwrap();
    let error = prepare_qualified_log(
        &service,
        &credentials(),
        prepared,
        verified,
        CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "profile_consent_contract_invalid");
    server.await.unwrap();
}

#[tokio::test]
async fn prepared_log_malformed_remote_member_profile_fails_before_converse_dispatch() {
    let state = imported_state([(
        "household",
        json!({
            "active_scope": "member-sarah",
            "members": [
                {"id": "_self", "name": "Justin", "archived": false},
                {"id": "member-sarah", "name": "Sarah", "archived": false}
            ]
        }),
    )]);
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        assert!(
            read_request(&mut socket)
                .await
                .starts_with("GET /v1/profile/consent ")
        );
        respond(&mut socket, json!({"has_consent": true})).await;

        let (mut socket, _) = listener.accept().await.unwrap();
        assert!(
            read_request(&mut socket)
                .await
                .starts_with("GET /v1/profile/sync?member_id=member-sarah ")
        );
        respond(&mut socket, json!({"profile_data": []})).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(25), listener.accept())
                .await
                .is_err()
        );
    });
    let parsed = CommandLine::try_parse_from(["heyfood", "--json", "log", "oatmeal"]).unwrap();
    let Some(heyfood_cli::Command::Log(arguments)) = parsed.command else {
        panic!("expected log command");
    };
    let (_root, importer) = prepared_log_importer("malformed-member-profile", &state);
    let prepared = prepare_log_command(arguments, &[], importer.preview_state().unwrap()).unwrap();
    let verified = importer
        .verify_after_review(prepared.source_preview())
        .unwrap();
    let error = prepare_qualified_log(
        &service,
        &credentials(),
        prepared,
        verified,
        CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "household_profile_contract_invalid");
    server.await.unwrap();
}

#[tokio::test]
async fn protected_invalid_roster_does_not_persist_snapshot_or_dispatch() {
    let root = LogTempRoot::new("protected-invalid-roster");
    let source = root.0.join("config.json");
    std::fs::write(
        &source,
        serde_json::to_vec(&json!({
            "account_user_id": "one-shot-account",
            "household": {
                "active_scope": "_self",
                "members": [{"id": "_self"}]
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let importer = PythonStateImporter::under(&source, root.0.join("native"));
    let parsed =
        CommandLine::try_parse_from(["heyfood", "--json", "log", "--for", "self", "oatmeal"])
            .unwrap();
    let Some(heyfood_cli::Command::Log(arguments)) = parsed.command else {
        panic!("expected log command");
    };
    let prepared = prepare_log_command(arguments, &[], importer.preview_state().unwrap()).unwrap();
    let verified = importer
        .verify_after_review(prepared.source_preview())
        .unwrap();
    assert!(!importer.destination_path().exists());
    let (listener, service) = fixture_service().await;
    let error = prepare_qualified_log(
        &service,
        &credentials(),
        prepared,
        verified,
        CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "household_state_invalid");
    assert!(!importer.destination_path().exists());
    assert!(
        tokio::time::timeout(Duration::from_millis(25), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn protected_account_mismatch_does_not_persist_snapshot_or_dispatch() {
    let root = LogTempRoot::new("protected-account-mismatch");
    let source = root.0.join("config.json");
    std::fs::write(
        &source,
        serde_json::to_vec(&json!({
            "account_user_id": "different-account",
            "household": {
                "active_scope": "_self",
                "members": [{"id": "_self", "name": "Justin"}]
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let importer = PythonStateImporter::under(&source, root.0.join("native"));
    let parsed =
        CommandLine::try_parse_from(["heyfood", "--json", "log", "--for", "self", "oatmeal"])
            .unwrap();
    let Some(heyfood_cli::Command::Log(arguments)) = parsed.command else {
        panic!("expected log command");
    };
    let prepared = prepare_log_command(arguments, &[], importer.preview_state().unwrap()).unwrap();
    let verified = importer
        .verify_after_review(prepared.source_preview())
        .unwrap();
    assert!(!importer.destination_path().exists());
    let (listener, service) = fixture_service().await;
    let error = prepare_qualified_log(
        &service,
        &credentials(),
        prepared,
        verified,
        CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "python_state_account_mismatch");
    assert!(!importer.destination_path().exists());
    assert!(
        tokio::time::timeout(Duration::from_millis(25), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn prepared_log_snapshot_prevents_active_scope_toctou() {
    let state = imported_state([(
        "household",
        json!({
            "active_scope": "member-sarah",
            "members": [
                {"id": "_self", "name": "Justin", "archived": false},
                {"id": "member-sarah", "name": "Sarah", "archived": false}
            ]
        }),
    )]);
    let (root, importer) = prepared_log_importer("scope-toctou", &state);
    let parsed = CommandLine::try_parse_from(["heyfood", "--json", "log", "oatmeal"]).unwrap();
    let Some(heyfood_cli::Command::Log(arguments)) = parsed.command else {
        panic!("expected log command");
    };
    let prepared = prepare_log_command(arguments, &[], importer.preview_state().unwrap()).unwrap();
    std::fs::write(
        root.0.join("config.json"),
        serde_json::to_vec(&json!({
            "account_user_id": "one-shot-account",
            "household": {
                "active_scope": "_self",
                "members": [
                    {"id": "_self", "name": "Justin", "archived": false},
                    {"id": "member-sarah", "name": "Sarah", "archived": false}
                ]
            }
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        importer
            .verify_after_review(prepared.source_preview())
            .unwrap_err()
            .code,
        "python_state_changed"
    );
}

#[tokio::test]
async fn raw_log_executor_requires_prepared_command() {
    let (_listener, service) = fixture_service().await;
    let parsed = CommandLine::try_parse_from(["heyfood", "--json", "log", "oatmeal"]).unwrap();
    let error = OneShotExecutor::new(&service, &credentials(), OutputMode::Json)
        .execute(parsed.command.unwrap(), &[], CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.code, "prepared_log_required");
}

#[tokio::test]
async fn selected_household_outbox_uses_the_python_fallback_context_without_blocking() {
    let oracle = python_oracle();
    let outbox = &oracle["log"]["outbox_scope"];
    let state = imported_state([
        ("first_name", json!("Justin")),
        (
            "household",
            json!({
                "active_scope": "member-sarah",
                "members": [
                    {"id": "_self", "name": "Justin", "relationship": "self", "archived": false},
                    {"id": "member-sarah", "name": "Sarah", "relationship": "partner", "archived": false}
                ]
            }),
        ),
        (
            "household_profile_outbox",
            json!({"member-sarah": {"local_context": outbox["local_context"].clone()}}),
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
        assert!(request.starts_with("POST /v1/agent/converse "));
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["dietary_context"]["preferences"][0], "vegetarian");
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
        "--for",
        outbox["selector"].as_str().unwrap(),
        "oatmeal",
    ])
    .unwrap();
    let (_root, importer) = prepared_log_importer("outbox-context", &state);
    let Some(heyfood_cli::Command::Log(arguments)) = parsed.command else {
        panic!("expected log command");
    };
    let preview = importer.preview_state().unwrap();
    let prepared = prepare_log_command(arguments, &[], preview).unwrap();
    let verified = importer
        .verify_after_review(prepared.source_preview())
        .unwrap();
    let credentials = credentials();
    let prepared = prepare_qualified_log(
        &service,
        &credentials,
        prepared,
        verified,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    execute_qualified_prepared_log(
        &service,
        credentials,
        OutputMode::Json,
        prepared,
        CancellationToken::new(),
    )
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
