use std::sync::{Arc, Mutex};
use std::time::Duration;

use heyfood_agent_runtime::{HttpDeadlines, HttpService};
use heyfood_application::{
    BoxFuture, ClockPort, ConfigCommit, ConfigPort, CredentialCommit, CredentialPort, PortError,
    RefreshPolicy, RunTurn, RunTurnOutcome, SerializedStateWriter, ServicePort, TurnRequest,
};
use heyfood_core::{
    AccountId, AgentEvent, ClientConfig, ConfigRevision, CredentialVersion, GenerationId,
    NetworkPolicy, OperationId, SensitiveString, ServiceUrl, SessionCredentials, SessionSnapshot,
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
    socket
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    socket.write_all(body).await.unwrap();
    socket.flush().await.unwrap();
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
    let server = tokio::spawn(async move {
        let (mut refresh, _) = listener.accept().await.unwrap();
        let request = read_request(&mut refresh).await;
        assert!(request.starts_with("POST /v1/auth/session/refresh "));
        assert!(request.contains("\"current_version\":1"));
        assert!(request.contains("\"refresh_token\":\"refresh-1\""));
        let refresh_body = br#"{"account_id":"account-fixture","access_token":"access-2","refresh_token":"refresh-2","credential_version":2,"expires_at_unix":4102444800}"#;
        respond(&mut refresh, "application/json", refresh_body).await;

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

    let service =
        Arc::new(HttpService::new(base.clone(), NetworkPolicy::DEVELOPMENT, deadlines()).unwrap());
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
async fn every_raw_sse_type_is_normalized_including_multiline_and_error() {
    let (listener, base) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_request(&mut socket).await;
        let stream = b": comment\nevent: thinking\ndata: {\"stage\":\"one\"}\n\nevent: progress\ndata: {\"message\":\"two\",\"current\":1,\"total\":2}\n\nevent: partial\ndata: {\"delta\":\"three\",\ndata: \"ignored\":true}\n\nevent: choices\ndata: {\"choices\":[\"four\",{\"label\":\"five\",\"value\":\"5\"}]}\n\nevent: result\ndata: {\"conversation_id\":\"six\"}\n\nevent: error\ndata: {\"code\":\"seven\",\"message\":\"failed\",\"retryable\":false}\n\n";
        respond(&mut socket, "text/event-stream", stream).await;
    });
    let service = HttpService::new(base, NetworkPolicy::DEVELOPMENT, deadlines()).unwrap();
    let accepted = service
        .open_turn(
            TurnRequest {
                prompt: "fixture".into(),
                conversation_id: None,
                refresh: RefreshPolicy::Never,
            },
            credentials(1),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let mut events = accepted.events;
    let mut received = Vec::new();
    while let Some(event) = events.next().await.unwrap() {
        received.push(event);
    }
    assert_eq!(received.len(), 6);
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
    assert!(matches!(received[5], AgentEvent::Error { .. }));
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
    let service = HttpService::new(base, NetworkPolicy::DEVELOPMENT, deadlines()).unwrap();
    let error = match service
        .open_turn(
            TurnRequest {
                prompt: "do not replay".into(),
                conversation_id: None,
                refresh: RefreshPolicy::Never,
            },
            credentials(1),
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

    let service =
        Arc::new(HttpService::new(base.clone(), NetworkPolicy::DEVELOPMENT, deadlines()).unwrap());
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
