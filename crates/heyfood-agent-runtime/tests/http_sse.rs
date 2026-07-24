use std::sync::{Arc, Mutex};
use std::time::Duration;

use heyfood_agent_runtime::{CliAuthContext, HttpDeadlines, HttpService};
use heyfood_application::{
    BoxFuture, ClockPort, ConfigCommit, ConfigPort, CredentialCommit, CredentialPort, PortError,
    RefreshPolicy, RunTurn, RunTurnOutcome, SerializedStateWriter, ServicePort, TurnContext,
    TurnRequest,
};
use heyfood_core::{
    AccountId, AgentConfirmationCommandWire, AgentEvent, ClientConfig, ConfigRevision,
    ConfirmationDecisionWire, CredentialVersion, GenerationId, GroceryConfirmationId,
    GroceryEditPatch, GroceryIdempotencyKey, NetworkPolicy, OperationId, RefreshOutcome,
    RefreshRequest, SensitiveString, ServiceUrl, SessionCredentials, SessionSnapshot,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

fn deadlines() -> HttpDeadlines {
    HttpDeadlines {
        connect: Duration::from_secs(1),
        request: Duration::from_secs(2),
        pool_idle: Duration::from_secs(1),
        sse_inactivity: Duration::from_secs(2),
    }
}

fn credentials(version: u64) -> SessionCredentials {
    SessionCredentials::from_unix_expiry(
        AccountId::parse("account-fixture").unwrap(),
        SensitiveString::new(format!("access-{version}")),
        SensitiveString::new(format!("refresh-{version}")),
        CredentialVersion::new(version),
        4_102_444_800,
    )
    .unwrap()
}

fn config(base: &ServiceUrl) -> ClientConfig {
    ClientConfig {
        active_context: "fixture".into(),
        api_url: base.clone(),
        auth_url: base.clone(),
        revision: ConfigRevision::new(1),
    }
}

#[derive(Default)]
struct MemoryCredentials {
    stored: Mutex<Option<SessionCredentials>>,
    commits: Mutex<usize>,
}

impl CredentialPort for MemoryCredentials {
    fn load(&self) -> BoxFuture<'_, Result<Option<SessionCredentials>, PortError>> {
        Box::pin(async { Ok(self.stored.lock().unwrap().clone()) })
    }

    fn commit(&self, commit: CredentialCommit) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            *self.stored.lock().unwrap() = Some(commit.credentials);
            *self.commits.lock().unwrap() += 1;
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

#[derive(Default)]
struct MemoryConfig;

impl ConfigPort for MemoryConfig {
    fn load(&self) -> BoxFuture<'_, Result<ClientConfig, PortError>> {
        Box::pin(async { Err(PortError::new("unused", "unused")) })
    }

    fn commit(&self, _commit: ConfigCommit) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async { Ok(()) })
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
        1
    }
}

async fn read_request(socket: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 1024];
        let count = socket.read(&mut chunk).await.unwrap();
        assert!(count > 0, "fixture peer closed before request was complete");
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
        })
        .unwrap_or(0);
    while bytes.len() - header_end < content_length {
        let mut chunk = vec![0; content_length - (bytes.len() - header_end)];
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
    socket.flush().await.unwrap();
}

fn auth_fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/python_backend_refresh.json")).unwrap()
}

fn assert_frozen_request(request: &str, fixture: &serde_json::Value) {
    let mut parts = request.split("\r\n\r\n");
    let head = parts.next().unwrap();
    let body = parts.next().unwrap_or_default();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap();
    assert!(request_line.starts_with(&format!(
        "{} {} ",
        fixture["method"].as_str().unwrap(),
        fixture["path"].as_str().unwrap()
    )));
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .collect::<std::collections::HashMap<_, _>>();
    for (name, expected) in fixture["headers"].as_object().unwrap() {
        assert_eq!(
            headers.get(name),
            Some(&expected.as_str().unwrap().to_owned())
        );
    }
    assert!(headers.contains_key("x-request-id"));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(body).unwrap(),
        fixture["body"]
    );
}

fn cli_auth(api_key: Option<&str>) -> CliAuthContext {
    CliAuthContext::new(
        "hellofood-cli-fixture-device",
        SensitiveString::new("channel-access-fixture"),
        api_key.map(SensitiveString::new),
    )
    .unwrap()
}

async fn fixture_service() -> (TcpListener, ServiceUrl) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = ServiceUrl::parse(
        &format!("http://{}/", listener.local_addr().unwrap()),
        NetworkPolicy::DEVELOPMENT,
    )
    .unwrap();
    (listener, base)
}

#[tokio::test]
async fn refresh_rotation_integrates_with_run_turn_and_normalized_sse() {
    let (listener, base) = fixture_service().await;
    let fixture = auth_fixture();
    let refresh_fixture = fixture["refresh"].clone();
    let server = tokio::spawn(async move {
        let (mut refresh, _) = listener.accept().await.unwrap();
        let request = read_request(&mut refresh).await;
        assert_frozen_request(&request, &refresh_fixture);
        let refresh_body = serde_json::to_vec(&refresh_fixture["response"]).unwrap();
        respond(&mut refresh, "application/json", &refresh_body).await;

        let (mut converse, _) = listener.accept().await.unwrap();
        let request = read_request(&mut converse).await;
        assert!(request.starts_with("POST /v1/agent/converse "));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer access-2")
        );
        assert!(request.contains("\"query\":\"what can I eat?\""));
        let stream = b": heartbeat\r\nevent: thinking\r\ndata: {\"stage\":\"route\",\"message\":\"Working\"}\r\n\r\nevent: partial\ndata: {\"text\":\"Try soup.\"}\n\nevent: result\ndata: {\"conversation_id\":\"conversation-2\",\"message\":\"Try soup.\"}\n\n";
        respond(&mut converse, "text/event-stream; charset=utf-8", stream).await;
    });

    let service = Arc::new(
        HttpService::new(base.clone(), NetworkPolicy::DEVELOPMENT, deadlines())
            .unwrap()
            .with_cli_auth(cli_auth(Some("fixture-api-key"))),
    );
    let credential_port = Arc::new(MemoryCredentials {
        stored: Mutex::new(Some(credentials(1))),
        commits: Mutex::new(0),
    });
    let writer = Arc::new(SerializedStateWriter::new(
        credential_port.clone(),
        Arc::new(MemoryConfig),
        GenerationId::INITIAL,
        Some(&credentials(1)),
    ));
    let run_turn = RunTurn::new(service, Arc::new(FixedClock), writer);
    let snapshot = heyfood_application::OperationSnapshot {
        operation_id: OperationId::new(),
        generation: GenerationId::INITIAL,
        config: config(&base),
        session: SessionSnapshot {
            credentials: credentials(1),
            reconciliation_required: false,
        },
    };
    let (sender, mut receiver) = mpsc::channel(8);
    let outcome = run_turn
        .execute(
            TurnRequest {
                prompt: "what can I eat?".into(),
                conversation_id: None,
                context: Default::default(),
                refresh: RefreshPolicy::Required,
            },
            snapshot,
            CancellationToken::new(),
            sender,
        )
        .await
        .unwrap();
    assert_eq!(outcome, RunTurnOutcome::Completed);
    assert_eq!(*credential_port.commits.lock().unwrap(), 1);
    assert_eq!(
        credential_port
            .stored
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .version,
        CredentialVersion::new(2)
    );
    assert!(matches!(
        receiver.recv().await.unwrap().event,
        AgentEvent::Thinking { .. }
    ));
    assert_eq!(
        receiver.recv().await.unwrap().event,
        AgentEvent::Partial {
            text: "Try soup.".into()
        }
    );
    assert!(matches!(
        receiver.recv().await.unwrap().event,
        AgentEvent::Result { .. }
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn known_api_key_gate_failure_uses_frozen_channel_reexchange_contract() {
    let (listener, base) = fixture_service().await;
    let fixture = auth_fixture();
    let fallback_fixture = fixture["fallback"].clone();
    let server = tokio::spawn(async move {
        let (mut refresh, _) = listener.accept().await.unwrap();
        let request = read_request(&mut refresh).await;
        assert!(request.starts_with("POST /v1/auth/session/refresh "));
        assert!(request.contains("\"refresh_token\":\"refresh-1\""));
        assert!(!request.to_ascii_lowercase().contains("x-api-key:"));
        let failure = serde_json::to_vec(&fallback_fixture["refresh_response"]).unwrap();
        respond_status(
            &mut refresh,
            401,
            "Unauthorized",
            "application/json",
            &failure,
        )
        .await;

        let (mut reexchange, _) = listener.accept().await.unwrap();
        let request = read_request(&mut reexchange).await;
        assert_frozen_request(&request, &fallback_fixture);
        let response = serde_json::to_vec(&fallback_fixture["response"]).unwrap();
        respond(&mut reexchange, "application/json", &response).await;
    });

    let service = HttpService::new(base, NetworkPolicy::DEVELOPMENT, deadlines())
        .unwrap()
        .with_cli_auth(cli_auth(None));
    let outcome = service
        .refresh_session(
            RefreshRequest::from(&credentials(1)),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let RefreshOutcome::Refreshed(result) = outcome else {
        panic!("fallback unexpectedly cancelled before dispatch");
    };
    assert_eq!(
        result.rotated().access_token.expose_secret(),
        "access-fallback"
    );
    assert_eq!(result.rotated().version, CredentialVersion::new(2));
    server.await.unwrap();
}

#[tokio::test]
async fn peer_consumes_refresh_token_and_withholds_response_is_uncertain() {
    let (listener, base) = fixture_service().await;
    let (consumed_sender, consumed_receiver) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut refresh, _) = listener.accept().await.unwrap();
        let request = read_request(&mut refresh).await;
        assert!(request.contains("\"refresh_token\":\"refresh-1\""));
        consumed_sender.send(()).unwrap();
        // The peer may have rotated the one-time token. Closing without a
        // response makes that server outcome unknowable to the client.
    });

    let service = HttpService::new(base, NetworkPolicy::DEVELOPMENT, deadlines())
        .unwrap()
        .with_cli_auth(cli_auth(Some("fixture-api-key")));
    let refresh = tokio::spawn(async move {
        service
            .refresh_session(
                RefreshRequest::from(&credentials(1)),
                CancellationToken::new(),
            )
            .await
    });
    consumed_receiver.await.unwrap();
    let error = refresh.await.unwrap().unwrap_err();
    assert!(error.outcome_uncertain);
    assert_eq!(error.code, "refresh_transport");
    server.await.unwrap();
}

#[tokio::test]
async fn cancellation_after_peer_consumes_refresh_token_is_uncertain() {
    let (listener, base) = fixture_service().await;
    let (consumed_sender, consumed_receiver) = oneshot::channel();
    let (release_sender, release_receiver) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut refresh, _) = listener.accept().await.unwrap();
        let request = read_request(&mut refresh).await;
        assert!(request.contains("\"refresh_token\":\"refresh-1\""));
        consumed_sender.send(()).unwrap();
        let _ = release_receiver.await;
    });

    let service = HttpService::new(base, NetworkPolicy::DEVELOPMENT, deadlines())
        .unwrap()
        .with_cli_auth(cli_auth(Some("fixture-api-key")));
    let cancellation = CancellationToken::new();
    let request_cancellation = cancellation.clone();
    let refresh = tokio::spawn(async move {
        service
            .refresh_session(RefreshRequest::from(&credentials(1)), request_cancellation)
            .await
    });
    consumed_receiver.await.unwrap();
    cancellation.cancel();
    let error = refresh.await.unwrap().unwrap_err();
    assert!(error.outcome_uncertain);
    assert_eq!(error.code, "refresh_cancelled_after_dispatch");
    release_sender.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn missing_session_refresh_token_reexchanges_without_primary_request() {
    let (listener, base) = fixture_service().await;
    let fixture = auth_fixture();
    let fallback_fixture = fixture["fallback"].clone();
    let server = tokio::spawn(async move {
        let (mut reexchange, _) = listener.accept().await.unwrap();
        let request = read_request(&mut reexchange).await;
        assert_frozen_request(&request, &fallback_fixture);
        let response = serde_json::to_vec(&fallback_fixture["response"]).unwrap();
        respond(&mut reexchange, "application/json", &response).await;
    });

    let service = HttpService::new(base, NetworkPolicy::DEVELOPMENT, deadlines())
        .unwrap()
        .with_cli_auth(cli_auth(None));
    let mut without_refresh = credentials(1);
    without_refresh.refresh_token = SensitiveString::new("");
    let outcome = service
        .refresh_session(
            RefreshRequest::from(&without_refresh),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(matches!(outcome, RefreshOutcome::Refreshed(_)));
    server.await.unwrap();
}

#[tokio::test]
async fn every_nonterminal_sse_type_is_normalized_before_result_and_done() {
    let (listener, base) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_request(&mut socket).await;
        let stream = b": comment\nevent: thinking\ndata: {\"stage\":\"one\"}\n\nevent: progress\ndata: {\"message\":\"two\",\"current\":1,\"total\":2}\n\nevent: partial\ndata: {\"delta\":\"three\",\ndata: \"ignored\":true}\n\nevent: choices\ndata: {\"choices\":[\"four\",{\"label\":\"five\",\"value\":\"5\"}]}\n\nevent: result\ndata: {\"conversation_id\":\"six\"}\n\nevent: done\ndata: {}\n\n";
        respond(&mut socket, "text/event-stream", stream).await;
    });
    let service = HttpService::new(base, NetworkPolicy::DEVELOPMENT, deadlines())
        .unwrap()
        .with_cli_auth(cli_auth(None));
    let accepted = service
        .open_turn(
            TurnRequest {
                prompt: "fixture".into(),
                conversation_id: None,
                context: Default::default(),
                refresh: RefreshPolicy::Never,
            },
            credentials(1),
            OperationId::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let mut events = accepted.events;
    let mut received = Vec::new();
    while let Some(event) = events.next().await.unwrap() {
        received.push(event);
    }
    assert_eq!(received.len(), 5);
    assert!(matches!(received[0], AgentEvent::Thinking { .. }));
    assert!(matches!(received[1], AgentEvent::Progress { .. }));
    assert_eq!(
        received[2],
        AgentEvent::Partial {
            text: "three".into()
        }
    );
    assert!(matches!(received[3], AgentEvent::Choices { .. }));
    assert!(matches!(received[4], AgentEvent::Result { .. }));
    events.close().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn conversational_confirmation_uses_the_frozen_confirm_xor_query_shape() {
    let (listener, base) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        let body: serde_json::Value = serde_json::from_str(
            request
                .split("\r\n\r\n")
                .nth(1)
                .expect("confirmation request body"),
        )
        .unwrap();
        assert!(body.get("query").is_none());
        assert_eq!(
            body["confirm"],
            serde_json::json!({
                "confirmation_id": "00000000-0000-4000-8000-000000000001",
                "idempotency_key": "00000000-0000-4000-8000-000000000002",
                "decision": "accept",
                "edits": {
                    "items": [
                        {"name": "scallion greens", "source_type": "manual"}
                    ]
                }
            })
        );
        assert_eq!(body["conversation_id"], "conversation-grocery");
        let stream = b"event: result\ndata: {\"conversation_id\":\"conversation-grocery\",\"text\":\"Grocery list updated.\",\"structured\":{\"type\":\"general_response\"}}\n\nevent: done\ndata: {}\n\n";
        respond(&mut socket, "text/event-stream", stream).await;
    });
    let service = HttpService::new(base, NetworkPolicy::DEVELOPMENT, deadlines())
        .unwrap()
        .with_cli_auth(cli_auth(None));
    let accepted = service
        .open_turn(
            TurnRequest {
                prompt: String::new(),
                conversation_id: Some("conversation-grocery".into()),
                context: TurnContext {
                    confirmation: Some(AgentConfirmationCommandWire {
                        confirmation_id: GroceryConfirmationId::parse(
                            "00000000-0000-4000-8000-000000000001",
                        )
                        .unwrap(),
                        idempotency_key: GroceryIdempotencyKey::parse(
                            "00000000-0000-4000-8000-000000000002",
                        )
                        .unwrap(),
                        decision: ConfirmationDecisionWire::Accept,
                        edits: Some(
                            GroceryEditPatch::new(
                                serde_json::from_value(serde_json::json!({
                                    "items": [
                                        {"name": "scallion greens", "source_type": "manual"}
                                    ]
                                }))
                                .unwrap(),
                            )
                            .unwrap(),
                        ),
                    }),
                    ..TurnContext::default()
                },
                refresh: RefreshPolicy::Never,
            },
            credentials(1),
            OperationId::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let mut events = accepted.events;
    assert!(matches!(
        events.next().await.unwrap(),
        Some(AgentEvent::Result { .. })
    ));
    assert!(events.next().await.unwrap().is_none());
    events.close().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn uncertain_conversational_post_is_never_retried() {
    let (listener, base) = fixture_service().await;
    let (count_sender, count_receiver) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_request(&mut socket).await;
        drop(socket);
        let retried = tokio::time::timeout(Duration::from_millis(400), listener.accept())
            .await
            .is_ok();
        count_sender.send(retried).unwrap();
    });
    let service = HttpService::new(base, NetworkPolicy::DEVELOPMENT, deadlines())
        .unwrap()
        .with_cli_auth(cli_auth(None));
    let error = match service
        .open_turn(
            TurnRequest {
                prompt: "do not replay".into(),
                conversation_id: None,
                context: Default::default(),
                refresh: RefreshPolicy::Never,
            },
            credentials(1),
            OperationId::new(),
            CancellationToken::new(),
        )
        .await
    {
        Ok(_) => panic!("dropped fixture connection must fail"),
        Err(error) => error,
    };
    assert!(error.outcome_uncertain);
    assert!(!count_receiver.await.unwrap());
    server.await.unwrap();
}

#[tokio::test]
async fn cancellation_after_peer_consumes_converse_body_reports_unknown_outcome() {
    let (listener, base) = fixture_service().await;
    let (consumed_sender, consumed_receiver) = oneshot::channel();
    let (release_sender, release_receiver) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.contains("\"query\":\"possibly mutating turn\""));
        consumed_sender.send(()).unwrap();
        let _ = release_receiver.await;
    });

    let service = Arc::new(
        HttpService::new(base.clone(), NetworkPolicy::DEVELOPMENT, deadlines())
            .unwrap()
            .with_cli_auth(cli_auth(Some("fixture-api-key"))),
    );
    let initial = credentials(1);
    let writer = Arc::new(SerializedStateWriter::new(
        Arc::new(MemoryCredentials {
            stored: Mutex::new(Some(initial.clone())),
            commits: Mutex::new(0),
        }),
        Arc::new(MemoryConfig),
        GenerationId::INITIAL,
        Some(&initial),
    ));
    let run_turn = RunTurn::new(service, Arc::new(FixedClock), writer);
    let cancellation = CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let (sender, _receiver) = mpsc::channel(4);
    let task = tokio::spawn(async move {
        run_turn
            .execute(
                TurnRequest {
                    prompt: "possibly mutating turn".into(),
                    conversation_id: None,
                    context: Default::default(),
                    refresh: RefreshPolicy::Never,
                },
                heyfood_application::OperationSnapshot {
                    operation_id: OperationId::new(),
                    generation: GenerationId::INITIAL,
                    config: config(&base),
                    session: SessionSnapshot {
                        credentials: initial,
                        reconciliation_required: false,
                    },
                },
                task_cancellation,
                sender,
            )
            .await
    });

    consumed_receiver.await.unwrap();
    cancellation.cancel();
    assert_eq!(
        task.await.unwrap().unwrap(),
        RunTurnOutcome::CancelledAfterDispatchOutcomeUnknown
    );
    release_sender.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn cancellation_drops_the_sse_response_and_closes_the_peer_socket() {
    let (listener, base) = fixture_service().await;
    let (headers_sender, headers_receiver) = oneshot::channel();
    let (closed_sender, closed_receiver) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_request(&mut socket).await;
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\nevent: thinking\ndata: {\"stage\":\"accepted\"}\n\n",
            )
            .await
            .unwrap();
        socket.flush().await.unwrap();
        headers_sender.send(()).unwrap();
        let mut byte = [0_u8; 1];
        let closed = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut byte))
            .await
            .is_ok_and(|result| result.is_ok_and(|count| count == 0));
        closed_sender.send(closed).unwrap();
    });

    let service = Arc::new(
        HttpService::new(base.clone(), NetworkPolicy::DEVELOPMENT, deadlines())
            .unwrap()
            .with_cli_auth(cli_auth(None)),
    );
    let credential_port = Arc::new(MemoryCredentials {
        stored: Mutex::new(Some(credentials(1))),
        commits: Mutex::new(0),
    });
    let writer = Arc::new(SerializedStateWriter::new(
        credential_port,
        Arc::new(MemoryConfig),
        GenerationId::INITIAL,
        Some(&credentials(1)),
    ));
    let run_turn = RunTurn::new(service, Arc::new(FixedClock), writer);
    let cancellation = CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let (sender, mut receiver) = mpsc::channel(4);
    let task = tokio::spawn(async move {
        run_turn
            .execute(
                TurnRequest {
                    prompt: "wait".into(),
                    conversation_id: None,
                    context: Default::default(),
                    refresh: RefreshPolicy::Never,
                },
                heyfood_application::OperationSnapshot {
                    operation_id: OperationId::new(),
                    generation: GenerationId::INITIAL,
                    config: config(&base),
                    session: SessionSnapshot {
                        credentials: credentials(1),
                        reconciliation_required: false,
                    },
                },
                task_cancellation,
                sender,
            )
            .await
    });
    headers_receiver.await.unwrap();
    assert!(matches!(
        receiver.recv().await.unwrap().event,
        AgentEvent::Thinking { .. }
    ));
    cancellation.cancel();
    assert_eq!(
        task.await.unwrap().unwrap(),
        RunTurnOutcome::CancelledAfterServerAcceptance
    );
    assert!(closed_receiver.await.unwrap());
    server.await.unwrap();
}
