//! Internal executable qualification harness. Cargo builds this file as a test
//! executable; it is not a `[[bin]]` target and cannot be installed as a user
//! command.

#![forbid(unsafe_code)]

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::Command;
#[cfg(unix)]
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use heyfood_agent_runtime::{CliAuthContext, HttpDeadlines, HttpService};
use heyfood_application::{
    ClockPort, ConfigPort, CredentialPort, OperationSnapshot, RefreshPolicy, RunTurn,
    RunTurnOutcome, SerializedStateWriter, TurnContext, TurnRequest,
};
use heyfood_bin::{QualifiedTurnDriver, run_qualified_session};
use heyfood_core::{
    AccountId, ClientConfig, ConfigRevision, CredentialVersion, GenerationId, NetworkPolicy,
    OperationId, SensitiveString, ServiceUrl, SessionCredentials, SessionSnapshot,
};
#[cfg(not(all(windows, feature = "native-credentials")))]
use heyfood_platform::FileCredentialStore as QualificationCredentialStore;
#[cfg(all(windows, feature = "native-credentials"))]
use heyfood_platform::WindowsCredentialStore as QualificationCredentialStore;
use heyfood_platform::{NativeConfigStore, NativeSignalSource, SignalEvent};
use heyfood_tui::{
    Action, AppModel, ExitReason, RuntimeEvent, SemanticEntry, Speaker, dispatch, render,
    run_terminal,
};
use portable_pty::{Child, CommandBuilder, ExitStatus, PtySize, native_pty_system};
use ratatui::{Terminal, backend::TestBackend};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const PYTHON_FIXTURE: &str = include_str!("fixtures/python-exported-turn.v1.json");
const PYTHON_AUTH_FIXTURE: &str =
    include_str!("../../heyfood-agent-runtime/tests/fixtures/python_backend_refresh.json");

struct QualificationCredentialCleanup {
    _store: Arc<QualificationCredentialStore>,
}

impl Drop for QualificationCredentialCleanup {
    fn drop(&mut self) {
        #[cfg(all(windows, feature = "native-credentials"))]
        self._store
            .delete()
            .expect("remove controlled Windows credential");
    }
}

struct FixedClock;

impl ClockPort for FixedClock {
    fn unix_timestamp(&self) -> i64 {
        2_000_000_000
    }
}

fn scratch(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock must be after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "heyfood-phase0-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn credentials(version: u64, access: &str, refresh: &str, expiry: i64) -> SessionCredentials {
    SessionCredentials::from_unix_expiry(
        AccountId::parse("account-fixture").expect("fixture account"),
        SensitiveString::new(access),
        SensitiveString::new(refresh),
        CredentialVersion::new(version),
        expiry,
    )
    .expect("fixture credential expiry")
}

fn config(base_url: &str) -> ClientConfig {
    ClientConfig {
        active_context: "qualification".into(),
        api_url: ServiceUrl::parse(base_url, NetworkPolicy::DEVELOPMENT).expect("loopback API URL"),
        auth_url: ServiceUrl::parse(base_url, NetworkPolicy::DEVELOPMENT)
            .expect("loopback auth URL"),
        revision: ConfigRevision::new(1),
    }
}

#[derive(Debug)]
struct Request {
    path: String,
    headers: String,
    body: String,
}

async fn read_request(stream: &mut TcpStream) -> Request {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 1024];
        let count = stream.read(&mut chunk).await.expect("read request");
        assert_ne!(count, 0, "peer closed before request headers");
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8(bytes[..header_end].to_vec()).expect("HTTP headers are UTF-8");
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().expect("content length"))
            })
        })
        .unwrap_or_default();
    while bytes.len() < header_end + content_length {
        let mut chunk = [0_u8; 1024];
        let count = stream.read(&mut chunk).await.expect("read request body");
        assert_ne!(count, 0, "peer closed before request body");
        bytes.extend_from_slice(&chunk[..count]);
    }
    let path = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .expect("request target")
        .to_owned();
    Request {
        path,
        headers,
        body: String::from_utf8(bytes[header_end..header_end + content_length].to_vec())
            .expect("HTTP body is UTF-8"),
    }
}

async fn write_response(stream: &mut TcpStream, content_type: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .expect("write controlled response");
}

fn assert_frozen_converse_request(request: &Request, fixture: &Value, operation_id: OperationId) {
    let request_line = request.headers.lines().next().expect("request line");
    assert!(request_line.starts_with(&format!(
        "{} {} ",
        fixture["method"].as_str().expect("fixture method"),
        fixture["path"].as_str().expect("fixture path")
    )));
    let actual_headers = request
        .headers
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .collect::<std::collections::HashMap<_, _>>();
    for (name, value) in fixture["headers"].as_object().expect("fixture headers") {
        let expected = if value == "$operation_id" {
            operation_id.as_uuid().to_string()
        } else {
            value.as_str().expect("fixture header value").to_owned()
        };
        assert_eq!(actual_headers.get(name), Some(&expected), "header {name}");
    }
    assert_eq!(
        serde_json::from_str::<Value>(&request.body).expect("converse JSON"),
        fixture["body"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg_attr(
    all(windows, not(feature = "native-credentials")),
    ignore = "Windows vertical requires the native-credentials feature"
)]
async fn python_fixture_drives_persistence_refresh_rustls_sse_run_turn_and_ratatui() {
    let fixture: Value = serde_json::from_str(PYTHON_FIXTURE).expect("valid Python export fixture");
    let auth_fixture: Value =
        serde_json::from_str(PYTHON_AUTH_FIXTURE).expect("valid Python auth fixture");
    assert_eq!(
        fixture["provenance"]["commit_sha"],
        "73494a57468dac83b4904ce6c390e36926f5c6fe"
    );
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind controlled service");
    let address = listener.local_addr().expect("controlled service address");
    let operation_id = OperationId::new();
    let frozen_request = fixture["request"].clone();
    let refresh_response = auth_fixture["refresh"]["response"].to_string();
    let sse = fixture["sse_lines"]
        .as_array()
        .expect("SSE lines")
        .iter()
        .map(|line| line.as_str().expect("SSE line"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let server = tokio::spawn(async move {
        let (mut refresh, _) = listener.accept().await.expect("accept refresh");
        let request = read_request(&mut refresh).await;
        assert_eq!(request.path, "/v1/auth/session/refresh");
        let headers = request.headers.to_ascii_lowercase();
        assert!(headers.contains("accept: application/json"));
        assert!(headers.contains("user-agent: heyfood-cli/0.4.0"));
        assert!(headers.contains("x-app-client-id: heyfood-cli"));
        assert!(headers.contains("x-device-id: hellofood-cli-fixture-device"));
        assert!(headers.contains("x-api-key: fixture-api-key"));
        assert!(headers.contains("x-request-id:"));
        assert_eq!(
            serde_json::from_str::<Value>(&request.body).expect("refresh JSON"),
            serde_json::json!({"refresh_token": "refresh-1"})
        );
        write_response(&mut refresh, "application/json", &refresh_response).await;

        let (mut converse, _) = listener.accept().await.expect("accept converse");
        let request = read_request(&mut converse).await;
        assert_frozen_converse_request(&request, &frozen_request, operation_id);
        write_response(&mut converse, "text/event-stream", &sse).await;
    });

    let root = scratch("vertical");
    let base_url = format!("http://{address}/");
    let initial = credentials(1, "access-1", "refresh-1", 1);
    let credential_store =
        Arc::new(QualificationCredentialStore::open(&root).expect("credential store"));
    let credential_cleanup = QualificationCredentialCleanup {
        _store: credential_store.clone(),
    };
    credential_store
        .initialize(&initial)
        .expect("initialize credentials");
    let config_store = Arc::new(
        NativeConfigStore::open(&root, config(&base_url), NetworkPolicy::DEVELOPMENT)
            .expect("config store"),
    );
    let writer = Arc::new(SerializedStateWriter::new(
        credential_store.clone(),
        config_store.clone(),
        GenerationId::INITIAL,
        Some(&initial),
    ));
    let service = Arc::new(
        HttpService::new(
            ServiceUrl::parse(&base_url, NetworkPolicy::DEVELOPMENT).expect("service URL"),
            NetworkPolicy::DEVELOPMENT,
            HttpDeadlines {
                connect: Duration::from_secs(1),
                request: Duration::from_secs(2),
                pool_idle: Duration::from_secs(1),
                sse_inactivity: Duration::from_secs(2),
            },
        )
        .expect("Rustls HTTP service")
        .with_cli_auth(
            CliAuthContext::new(
                "hellofood-cli-fixture-device",
                SensitiveString::new("channel-access-fixture"),
                Some(SensitiveString::new("fixture-api-key")),
            )
            .expect("Python-compatible CLI auth context"),
        ),
    );
    let run_turn = RunTurn::new(service, Arc::new(FixedClock), writer);
    let snapshot = OperationSnapshot {
        operation_id,
        generation: GenerationId::INITIAL,
        config: config_store.load().await.expect("load native config"),
        session: SessionSnapshot {
            credentials: initial,
            reconciliation_required: false,
        },
    };
    let (events_sender, mut events_receiver) = mpsc::channel(8);
    let outcome = run_turn
        .execute(
            TurnRequest {
                prompt: fixture["request"]["body"]["query"]
                    .as_str()
                    .expect("fixture prompt")
                    .to_owned(),
                conversation_id: fixture["request"]["body"]["conversation_id"]
                    .as_str()
                    .map(str::to_owned),
                context: TurnContext {
                    dietary: Some(fixture["request"]["body"]["dietary_context"].clone()),
                    device: Some(fixture["request"]["body"]["device_context"].clone()),
                    meal: Some(fixture["request"]["body"]["meal_context"].clone()),
                    latitude: fixture["request"]["body"]["lat"].as_f64(),
                    longitude: fixture["request"]["body"]["lng"].as_f64(),
                },
                refresh: RefreshPolicy::Required,
            },
            snapshot,
            CancellationToken::new(),
            events_sender,
        )
        .await
        .expect("qualified turn");
    assert_eq!(outcome, RunTurnOutcome::Completed);
    let event = events_receiver.recv().await.expect("terminal result event");
    assert!(events_receiver.try_recv().is_err());

    let rotated = credential_store
        .load()
        .await
        .expect("load rotated credentials")
        .expect("rotated credentials exist");
    assert_eq!(rotated.version, CredentialVersion::new(2));

    let mut model = AppModel::default();
    model.draft = fixture["request"]["body"]["query"]
        .as_str()
        .expect("fixture prompt")
        .to_owned();
    model.cursor = model.draft.chars().count();
    let effects = dispatch(&mut model, Action::Submit);
    assert_eq!(effects.len(), 1);
    let _ = dispatch(
        &mut model,
        Action::Runtime(RuntimeEvent::TurnEvent {
            operation_id: 1,
            event: event.event,
        }),
    );
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("test terminal");
    terminal
        .draw(|frame| render(frame, &model))
        .expect("render result");
    let rendered = terminal.backend().to_string();
    assert!(rendered.contains("done"));
    assert!(!rendered.contains("access-1"));
    assert!(!rendered.contains("access-2"));

    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("controlled service joined")
        .expect("controlled service task");
    drop(credential_cleanup);
    std::fs::remove_dir_all(root).expect("remove controlled native state");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg_attr(
    all(windows, not(feature = "native-credentials")),
    ignore = "Windows vertical requires the native-credentials feature"
)]
async fn cancellation_closes_sse_socket_and_every_owned_task_joins() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind controlled service");
    let address = listener.local_addr().expect("controlled service address");
    let (accepted_sender, accepted_receiver) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept converse");
        let request = read_request(&mut stream).await;
        assert_eq!(request.path, "/v1/agent/converse");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\nevent: partial\ndata: {\"text\":\"working\"}\n\n")
            .await
            .expect("write partial response");
        accepted_sender.send(()).expect("signal accepted SSE");
        let mut byte = [0_u8; 1];
        match tokio::time::timeout(Duration::from_secs(2), stream.read(&mut byte))
            .await
            .expect("client closes socket before deadline")
        {
            Ok(0) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
                ) => {}
            result => panic!("cancellation must close/reset the SSE peer socket: {result:?}"),
        }
    });

    let root = scratch("cancel");
    let base_url = format!("http://{address}/");
    let initial = credentials(1, "access-1", "refresh-1", 4_102_444_800);
    let credential_store =
        Arc::new(QualificationCredentialStore::open(&root).expect("credential store"));
    let credential_cleanup = QualificationCredentialCleanup {
        _store: credential_store.clone(),
    };
    credential_store
        .initialize(&initial)
        .expect("initialize credentials");
    let config_store = Arc::new(
        NativeConfigStore::open(&root, config(&base_url), NetworkPolicy::DEVELOPMENT)
            .expect("config store"),
    );
    let writer = Arc::new(SerializedStateWriter::new(
        credential_store.clone(),
        config_store.clone(),
        GenerationId::INITIAL,
        Some(&initial),
    ));
    let service = Arc::new(
        HttpService::new(
            ServiceUrl::parse(&base_url, NetworkPolicy::DEVELOPMENT).expect("service URL"),
            NetworkPolicy::DEVELOPMENT,
            HttpDeadlines::default(),
        )
        .expect("Rustls HTTP service")
        .with_cli_auth(
            CliAuthContext::new(
                "hellofood-cli-fixture-device",
                SensitiveString::new("channel-access-fixture"),
                None,
            )
            .expect("Python-compatible CLI auth context"),
        ),
    );
    let run_turn = RunTurn::new(service, Arc::new(FixedClock), writer);
    let cancellation = CancellationToken::new();
    let cancel_from_test = cancellation.clone();
    let (events_sender, mut events_receiver) = mpsc::channel(8);
    let turn = tokio::spawn(async move {
        run_turn
            .execute(
                TurnRequest {
                    prompt: "cancel fixture".into(),
                    conversation_id: None,
                    context: Default::default(),
                    refresh: RefreshPolicy::Never,
                },
                OperationSnapshot {
                    operation_id: OperationId::new(),
                    generation: GenerationId::INITIAL,
                    config: config_store.load().await.expect("load native config"),
                    session: SessionSnapshot {
                        credentials: initial,
                        reconciliation_required: false,
                    },
                },
                cancellation,
                events_sender,
            )
            .await
    });
    accepted_receiver.await.expect("SSE accepted");
    tokio::time::timeout(Duration::from_secs(2), events_receiver.recv())
        .await
        .expect("application observes accepted SSE before deadline")
        .expect("partial event establishes server acceptance");
    cancel_from_test.cancel();
    let outcome = tokio::time::timeout(Duration::from_secs(3), turn)
        .await
        .expect("turn joins before cancellation deadline")
        .expect("turn task")
        .expect("turn outcome");
    assert_eq!(outcome, RunTurnOutcome::CancelledAfterServerAcceptance);
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .expect("server joins after peer EOF")
        .expect("server task");
    drop(credential_cleanup);
    std::fs::remove_dir_all(root).expect("remove controlled native state");
}

struct OwnedTurn {
    operation_id: u64,
    cancellation: CancellationToken,
    task: JoinHandle<()>,
}

struct SupervisedQualificationDriver {
    runtime: tokio::runtime::Runtime,
    prepared: Option<(RunTurn, OperationSnapshot)>,
    turns: Vec<OwnedTurn>,
    controlled_server: Option<JoinHandle<()>>,
}

impl SupervisedQualificationDriver {
    fn join_controlled_server(&mut self, timeout: Duration) -> io::Result<()> {
        let Some(server) = self.controlled_server.take() else {
            return Ok(());
        };
        self.runtime.block_on(async move {
            tokio::time::timeout(timeout, server)
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "server did not join"))?
                .map_err(|error| io::Error::other(format!("server task failed: {error}")))
        })
    }
}

impl QualifiedTurnDriver for SupervisedQualificationDriver {
    fn start_turn(
        &mut self,
        operation_id: u64,
        prompt: String,
        runtime_events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        let (run_turn, snapshot) = self.prepared.take().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "qualification driver accepts one active turn",
            )
        })?;
        let cancellation = CancellationToken::new();
        let execute_cancellation = cancellation.clone();
        let supervisor_cancellation = cancellation.clone();
        let task = self.runtime.spawn(async move {
            let (turn_events, mut turn_receiver) = mpsc::channel(1);
            let execute = tokio::spawn(async move {
                run_turn
                    .execute(
                        TurnRequest {
                            prompt,
                            conversation_id: None,
                            context: Default::default(),
                            refresh: RefreshPolicy::Never,
                        },
                        snapshot,
                        execute_cancellation,
                        turn_events,
                    )
                    .await
            });

            while let Some(event) = turn_receiver.recv().await {
                if runtime_events
                    .send(RuntimeEvent::TurnEvent {
                        operation_id,
                        event: event.event,
                    })
                    .await
                    .is_err()
                {
                    supervisor_cancellation.cancel();
                    break;
                }
            }

            let runtime_event = match execute.await {
                Ok(Ok(outcome)) => RuntimeEvent::TurnFinished {
                    operation_id,
                    outcome,
                },
                Ok(Err(error)) => RuntimeEvent::TurnFailed {
                    operation_id,
                    message: error.to_string(),
                },
                Err(error) => RuntimeEvent::TurnFailed {
                    operation_id,
                    message: format!("turn task failed: {error}"),
                },
            };
            let _ = runtime_events.send(runtime_event).await;
        });
        self.turns.push(OwnedTurn {
            operation_id,
            cancellation,
            task,
        });
        Ok(())
    }

    fn cancel_turn(&mut self, operation_id: u64) -> io::Result<()> {
        let turn = self
            .turns
            .iter()
            .find(|turn| turn.operation_id == operation_id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "active turn is missing"))?;
        turn.cancellation.cancel();
        Ok(())
    }

    fn shutdown_and_join(&mut self, timeout: Duration) -> io::Result<()> {
        for turn in &self.turns {
            turn.cancellation.cancel();
        }
        let turns = std::mem::take(&mut self.turns);
        self.runtime.block_on(async move {
            tokio::time::timeout(timeout, async move {
                for turn in turns {
                    turn.task.await.map_err(|error| {
                        io::Error::other(format!("turn supervisor task failed: {error}"))
                    })?;
                }
                Ok(())
            })
            .await
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "turn supervisor exceeded its shutdown deadline",
                )
            })?
        })
    }
}

#[test]
#[ignore = "spawned under a PTY by supervised_tui_http_sse_cancel_and_join_vertical"]
fn qualification_active_turn_child() {
    let runtime = tokio::runtime::Runtime::new().expect("qualification runtime");
    let listener = runtime
        .block_on(TcpListener::bind("127.0.0.1:0"))
        .expect("bind controlled active-turn service");
    let address = listener.local_addr().expect("controlled service address");
    let server = runtime.spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept active turn");
        let request = read_request(&mut stream).await;
        assert_eq!(request.path, "/v1/agent/converse");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\nevent: partial\ndata: {\"text\":\"working until cancelled\"}\n\n")
            .await
            .expect("write active SSE response");
        eprintln!("QUALIFICATION_ACTIVE");
        let mut byte = [0_u8; 1];
        match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut byte))
            .await
            .expect("client closes socket before deadline")
        {
            Ok(0) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe
                ) => {}
            result => panic!("supervised cancellation must close the SSE socket: {result:?}"),
        }
        eprintln!("QUALIFICATION_SOCKET_CLOSED");
    });

    let root = scratch("supervised-active-turn");
    let base_url = format!("http://{address}/");
    let initial = credentials(1, "access-1", "refresh-1", 4_102_444_800);
    let credential_store =
        Arc::new(QualificationCredentialStore::open(&root).expect("credential store"));
    let credential_cleanup = QualificationCredentialCleanup {
        _store: credential_store.clone(),
    };
    credential_store
        .initialize(&initial)
        .expect("initialize credentials");
    let config_store = Arc::new(
        NativeConfigStore::open(&root, config(&base_url), NetworkPolicy::DEVELOPMENT)
            .expect("config store"),
    );
    let writer = Arc::new(SerializedStateWriter::new(
        credential_store,
        config_store.clone(),
        GenerationId::INITIAL,
        Some(&initial),
    ));
    let service = Arc::new(
        HttpService::new(
            ServiceUrl::parse(&base_url, NetworkPolicy::DEVELOPMENT).expect("service URL"),
            NetworkPolicy::DEVELOPMENT,
            HttpDeadlines {
                sse_inactivity: Duration::from_secs(5),
                ..HttpDeadlines::default()
            },
        )
        .expect("Rustls HTTP service")
        .with_cli_auth(
            CliAuthContext::new(
                "hellofood-cli-fixture-device",
                SensitiveString::new("channel-access-fixture"),
                None,
            )
            .expect("CLI auth context"),
        ),
    );
    let snapshot = OperationSnapshot {
        operation_id: OperationId::new(),
        generation: GenerationId::INITIAL,
        config: runtime
            .block_on(config_store.load())
            .expect("load native config"),
        session: SessionSnapshot {
            credentials: initial,
            reconciliation_required: false,
        },
    };
    let mut driver = SupervisedQualificationDriver {
        runtime,
        prepared: Some((
            RunTurn::new(service, Arc::new(FixedClock), writer),
            snapshot,
        )),
        turns: Vec::new(),
        controlled_server: Some(server),
    };

    eprintln!("QUALIFICATION_READY");
    let reason = run_qualified_session(&mut driver).expect("supervised terminal session");
    assert_eq!(reason, ExitReason::Requested);
    driver
        .join_controlled_server(Duration::from_secs(3))
        .expect("controlled server joined");
    eprintln!("QUALIFICATION_SUPERVISOR_JOINED");
    drop(credential_cleanup);
    std::fs::remove_dir_all(root).expect("remove controlled native state");
}

#[test]
#[cfg_attr(
    all(windows, not(feature = "native-credentials")),
    ignore = "Windows vertical requires the native-credentials feature"
)]
fn supervised_tui_http_sse_cancel_and_join_vertical() {
    run_active_turn_pty_child();
}

#[test]
#[ignore = "spawned under a PTY by qualification_pty_signal_and_restoration_matrix"]
fn qualification_pty_child() {
    let runtime = tokio::runtime::Runtime::new().expect("signal runtime");
    let (sender, mut receiver) = mpsc::channel(8);
    let signal_shutdown = CancellationToken::new();
    let signal_task_shutdown = signal_shutdown.clone();
    let signal_task = runtime.spawn(async move {
        let mut signals = NativeSignalSource::install().expect("install native signal source");
        let signal = tokio::select! {
            signal = signals.next() => signal,
            () = signal_task_shutdown.cancelled() => None,
        };
        if let Some(signal) = signal {
            let reason = match signal {
                SignalEvent::Interrupt => ExitReason::Interrupt,
                SignalEvent::Terminate | SignalEvent::ConsoleClose => ExitReason::Terminate,
                SignalEvent::Hangup => ExitReason::Hangup,
            };
            let _ = sender.send(RuntimeEvent::ExternalSignal(reason)).await;
        }
        signals
            .shutdown(Duration::from_secs(1))
            .await
            .expect("native signal listeners stopped and joined");
    });
    eprintln!("QUALIFICATION_READY");
    let reason = run_terminal(&mut receiver, |_| Ok(())).expect("qualified terminal session");
    signal_shutdown.cancel();
    runtime
        .block_on(async { tokio::time::timeout(Duration::from_secs(2), signal_task).await })
        .expect("native signal supervisor stopped within the qualification bound")
        .expect("native signal supervisor joined without panic");
    #[cfg(unix)]
    {
        let state = Command::new("stty")
            .arg("-a")
            .stdin(Stdio::inherit())
            .output()
            .expect("query restored PTY");
        assert!(
            state.status.success(),
            "stty must see a restored controlling PTY"
        );
        let state = String::from_utf8_lossy(&state.stdout);
        assert!(
            state.contains("icanon"),
            "terminal must return to canonical mode"
        );
    }
    eprintln!("QUALIFICATION_RESTORED:{reason:?}");
}

#[test]
#[cfg(windows)]
#[ignore = "spawned with a real Windows console by qualification_pty_signal_and_restoration_matrix"]
fn qualification_windows_console_signal_child() {
    let ready_path = std::env::var_os("HEYFOOD_QUALIFICATION_READY_FILE")
        .map(PathBuf::from)
        .expect("Windows signal readiness path");
    let result_path = std::env::var_os("HEYFOOD_QUALIFICATION_RESULT_FILE")
        .map(PathBuf::from)
        .expect("Windows signal result path");
    let runtime = tokio::runtime::Runtime::new().expect("Windows signal runtime");
    let mut signals = NativeSignalSource::install().expect("install Windows signal source");
    std::fs::write(&ready_path, b"ready").expect("publish Windows signal readiness");
    let signal = runtime
        .block_on(signals.next())
        .expect("receive Windows console control event");
    runtime
        .block_on(signals.shutdown(Duration::from_secs(1)))
        .expect("Windows signal listeners stopped and joined");
    std::fs::write(&result_path, format!("{signal:?}")).expect("publish Windows signal result");
    assert_eq!(signal, SignalEvent::Interrupt);
}

#[test]
fn qualification_pty_signal_and_restoration_matrix() {
    #[cfg(unix)]
    for (signal, expected) in [
        ("INT", "Interrupt"),
        ("TERM", "Terminate"),
        ("HUP", "Hangup"),
    ] {
        run_pty_child(Some(signal), expected);
    }

    #[cfg(windows)]
    {
        run_pty_child(None, "Requested");
        run_windows_console_control_child();
    }
}

#[test]
fn qualification_render_and_input_performance_budgets() {
    let mut warm_terminal = Terminal::new(TestBackend::new(80, 24)).expect("warm terminal");
    warm_terminal
        .draw(|frame| render(frame, &AppModel::default()))
        .expect("warm first frame");
    let mut first_frames = Vec::with_capacity(30);
    for _ in 0..30 {
        let started = Instant::now();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("test terminal");
        terminal
            .draw(|frame| render(frame, &AppModel::default()))
            .expect("first frame");
        first_frames.push(started.elapsed());
    }

    let mut model = AppModel::default();
    for index in 0..500 {
        model.scrollback.push(SemanticEntry {
            speaker: if index % 2 == 0 {
                Speaker::User
            } else {
                Speaker::Assistant
            },
            text: format!("controlled scrollback line {index}"),
            streaming: false,
        });
    }
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("input terminal");
    terminal
        .draw(|frame| render(frame, &model))
        .expect("warm input frame");
    let mut input_frames = Vec::with_capacity(2_000);
    for index in 0..2_000 {
        let started = Instant::now();
        let _ = dispatch(
            &mut model,
            Action::Insert(char::from(b'a' + (index % 26) as u8)),
        );
        terminal
            .draw(|frame| render(frame, &model))
            .expect("input frame");
        input_frames.push(started.elapsed());
    }
    let first_frame_p95 = percentile_95(&mut first_frames);
    let input_frame_p95 = percentile_95(&mut input_frames);
    assert!(
        first_frame_p95 < Duration::from_millis(100),
        "controlled first-frame p95 exceeded 100 ms: {first_frame_p95:?}"
    );
    assert!(
        input_frame_p95 < Duration::from_millis(25),
        "controlled input-to-frame p95 exceeded 25 ms: {input_frame_p95:?}"
    );
    println!(
        "QUALIFICATION_METRICS first_frame_samples=30 first_frame_p95_us={} input_frame_samples=2000 input_frame_p95_us={} scrollback_entries={}",
        first_frame_p95.as_micros(),
        input_frame_p95.as_micros(),
        model.scrollback.entries().len()
    );
}

fn percentile_95(samples: &mut [Duration]) -> Duration {
    samples.sort_unstable();
    samples[(samples.len() * 95).div_ceil(100) - 1]
}

fn run_pty_child(signal: Option<&str>, expected: &str) {
    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open native PTY/ConPTY");
    let mut command = CommandBuilder::new(std::env::current_exe().expect("test executable"));
    command.arg("--exact");
    command.arg("qualification_pty_child");
    command.arg("--ignored");
    command.arg("--nocapture");
    let mut child = pair
        .slave
        .spawn_command(command)
        .expect("spawn qualification test in PTY");
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().expect("clone PTY reader");
    let writer = Arc::new(Mutex::new(
        pair.master.take_writer().expect("take PTY writer"),
    ));
    let reply_writer = Arc::clone(&writer);
    let reader_task = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut replied_to_cursor_query = false;
        loop {
            let mut chunk = [0_u8; 1024];
            let count = reader.read(&mut chunk).expect("read PTY output");
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..count]);
            if !replied_to_cursor_query
                && bytes
                    .windows(b"\x1b[6n".len())
                    .any(|window| window == b"\x1b[6n")
            {
                let mut writer = reply_writer.lock().expect("lock PTY reply writer");
                writer
                    .write_all(b"\x1b[1;1R")
                    .expect("reply to cursor position query");
                writer.flush().expect("flush cursor position reply");
                replied_to_cursor_query = true;
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    });
    std::thread::sleep(Duration::from_millis(700));

    #[cfg(unix)]
    if let Some(signal) = signal {
        let pid = child.process_id().expect("PTY child process ID");
        let status = Command::new("kill")
            .args([format!("-{signal}"), pid.to_string()])
            .status()
            .expect("deliver catchable Unix signal");
        assert!(status.success(), "signal delivery must succeed");
    }
    #[cfg(unix)]
    if signal.is_none() {
        let mut writer = writer.lock().expect("lock ConPTY writer");
        writer.write_all(&[4]).expect("send Ctrl+D");
        writer.flush().expect("flush Ctrl+D");
    }
    #[cfg(windows)]
    {
        assert!(signal.is_none(), "ConPTY restoration uses a typed EOF");
        let mut writer = writer.lock().expect("lock ConPTY writer");
        writer.write_all(&[4]).expect("send Ctrl+D");
        writer.flush().expect("flush Ctrl+D");
    }

    let status = wait_for_pty_child(&mut child, Duration::from_secs(10));
    drop(pair.master);
    let output = reader_task.join().expect("join PTY reader");
    assert!(
        status.success(),
        "qualification child failed: {status:?}; output: {output:?}"
    );
    assert!(
        output.contains("QUALIFICATION_READY"),
        "child did not enter harness: {output:?}"
    );
    assert!(
        output.contains(&format!("QUALIFICATION_RESTORED:{expected}")),
        "child did not restore after {expected}: {output:?}"
    );
    assert!(
        output.contains("\u{1b}[?1049l"),
        "alternate screen was not left: {output:?}"
    );
    assert!(
        output.contains("\u{1b}[?2004l"),
        "bracketed paste was not disabled: {output:?}"
    );
}

#[cfg(windows)]
fn send_windows_console_control(pid: u32) {
    const SCRIPT: &str = r#"
param([UInt32]$TargetPid)
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class HeyfoodConsoleSignal {
    [DllImport("kernel32.dll", SetLastError=true)] public static extern bool FreeConsole();
    [DllImport("kernel32.dll", SetLastError=true)] public static extern bool AttachConsole(uint processId);
    [DllImport("kernel32.dll", SetLastError=true)] public static extern bool SetConsoleCtrlHandler(IntPtr handler, bool add);
    [DllImport("kernel32.dll", SetLastError=true)] public static extern bool GenerateConsoleCtrlEvent(uint signal, uint processGroupId);
}
'@
[HeyfoodConsoleSignal]::FreeConsole() | Out-Null
if (-not [HeyfoodConsoleSignal]::AttachConsole($TargetPid)) { exit 20 }
if (-not [HeyfoodConsoleSignal]::SetConsoleCtrlHandler([IntPtr]::Zero, $true)) { exit 21 }
if (-not [HeyfoodConsoleSignal]::GenerateConsoleCtrlEvent(0, $TargetPid)) { exit 22 }
Start-Sleep -Milliseconds 250
[HeyfoodConsoleSignal]::FreeConsole() | Out-Null
"#;
    let status = Command::new("pwsh")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            SCRIPT,
        ])
        .arg(pid.to_string())
        .status()
        .expect("launch Windows console-control sender");
    assert!(
        status.success(),
        "Windows console-control delivery failed: {status:?}"
    );
}

#[cfg(windows)]
fn run_windows_console_control_child() {
    use std::os::windows::process::CommandExt;

    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    let root = scratch("windows-console-control");
    let ready_path = root.join("ready");
    let result_path = root.join("result");
    let mut child = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "qualification_windows_console_signal_child",
            "--ignored",
            "--nocapture",
        ])
        .env("HEYFOOD_QUALIFICATION_READY_FILE", &ready_path)
        .env("HEYFOOD_QUALIFICATION_RESULT_FILE", &result_path)
        .creation_flags(CREATE_NEW_CONSOLE | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .expect("spawn isolated Windows console signal child");
    wait_for_file(&ready_path, Duration::from_secs(5), &mut child);
    send_windows_console_control(child.id());
    wait_for_file(&result_path, Duration::from_secs(5), &mut child);
    let status = wait_for_process_child(&mut child, Duration::from_secs(5));
    assert!(
        status.success(),
        "Windows console signal child failed: {status}"
    );
    assert_eq!(
        std::fs::read_to_string(&result_path).expect("read Windows signal result"),
        "Interrupt"
    );
    std::fs::remove_dir_all(root).expect("remove Windows signal fixture");
}

#[cfg(windows)]
fn wait_for_file(path: &std::path::Path, timeout: Duration, child: &mut std::process::Child) {
    let deadline = Instant::now() + timeout;
    while !path.exists() {
        if let Some(status) = child.try_wait().expect("poll Windows signal child") {
            panic!(
                "Windows signal child exited before {}: {status}",
                path.display()
            );
        }
        if Instant::now() >= deadline {
            child
                .kill()
                .expect("terminate stalled Windows signal child");
            let _ = child.wait();
            panic!("Windows signal child did not create {}", path.display());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(windows)]
fn wait_for_process_child(
    child: &mut std::process::Child,
    timeout: Duration,
) -> std::process::ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("poll Windows signal child") {
            return status;
        }
        if Instant::now() >= deadline {
            child
                .kill()
                .expect("terminate stalled Windows signal child");
            let _ = child.wait();
            panic!("Windows signal child did not exit within {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn run_active_turn_pty_child() {
    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open active-turn PTY/ConPTY");
    let mut command = CommandBuilder::new(std::env::current_exe().expect("test executable"));
    command.arg("--exact");
    command.arg("qualification_active_turn_child");
    command.arg("--ignored");
    command.arg("--nocapture");
    let mut child = pair
        .slave
        .spawn_command(command)
        .expect("spawn active-turn qualification test");
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().expect("clone PTY reader");
    let writer = Arc::new(Mutex::new(
        pair.master.take_writer().expect("take PTY writer"),
    ));
    let reply_writer = Arc::clone(&writer);
    let (active_sender, active_receiver) = std::sync::mpsc::sync_channel(1);
    let (closed_sender, closed_receiver) = std::sync::mpsc::sync_channel(1);
    let reader_task = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut replied_to_cursor_query = false;
        let mut active_reported = false;
        let mut closed_reported = false;
        loop {
            let mut chunk = [0_u8; 1024];
            let count = reader
                .read(&mut chunk)
                .expect("read active-turn PTY output");
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..count]);
            if !replied_to_cursor_query
                && bytes
                    .windows(b"\x1b[6n".len())
                    .any(|window| window == b"\x1b[6n")
            {
                let mut writer = reply_writer.lock().expect("lock PTY reply writer");
                writer
                    .write_all(b"\x1b[1;1R")
                    .expect("reply to cursor position query");
                writer.flush().expect("flush cursor position reply");
                replied_to_cursor_query = true;
            }
            if !active_reported
                && bytes
                    .windows(b"QUALIFICATION_ACTIVE".len())
                    .any(|window| window == b"QUALIFICATION_ACTIVE")
            {
                let _ = active_sender.send(());
                active_reported = true;
            }
            if !closed_reported
                && bytes
                    .windows(b"QUALIFICATION_SOCKET_CLOSED".len())
                    .any(|window| window == b"QUALIFICATION_SOCKET_CLOSED")
            {
                let _ = closed_sender.send(());
                closed_reported = true;
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    });

    std::thread::sleep(Duration::from_millis(700));
    {
        let mut writer = writer.lock().expect("lock active-turn PTY writer");
        writer
            .write_all(b"cancel this turn\r")
            .expect("submit active turn");
        writer.flush().expect("flush active turn");
    }
    active_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("TUI effect must reach HTTP/SSE runtime");
    {
        let mut writer = writer.lock().expect("lock active-turn cancel writer");
        writer.write_all(&[3]).expect("send active-turn Ctrl+C");
        writer.flush().expect("flush active-turn Ctrl+C");
    }
    closed_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("cancellation must close the SSE socket");
    std::thread::sleep(Duration::from_millis(300));
    for _ in 0..2 {
        let mut writer = writer.lock().expect("lock active-turn exit writer");
        writer.write_all(&[3]).expect("send exit Ctrl+C");
        writer.flush().expect("flush exit Ctrl+C");
        drop(writer);
        std::thread::sleep(Duration::from_millis(150));
    }

    let status = wait_for_pty_child(&mut child, Duration::from_secs(10));
    drop(pair.master);
    let output = reader_task.join().expect("join active-turn PTY reader");
    assert!(
        status.success(),
        "active-turn qualification child failed: {status:?}; output: {output:?}"
    );
    for marker in [
        "QUALIFICATION_READY",
        "QUALIFICATION_ACTIVE",
        "QUALIFICATION_SOCKET_CLOSED",
        "QUALIFICATION_SUPERVISOR_JOINED",
    ] {
        assert!(output.contains(marker), "missing {marker}: {output:?}");
    }
    assert!(
        output.contains("\u{1b}[?1049l"),
        "alternate screen was not left: {output:?}"
    );
    assert!(
        output.contains("\u{1b}[?2004l"),
        "bracketed paste was not disabled: {output:?}"
    );
}

fn wait_for_pty_child(child: &mut Box<dyn Child + Send + Sync>, timeout: Duration) -> ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("poll qualification child") {
            return status;
        }
        if Instant::now() >= deadline {
            child
                .kill()
                .expect("terminate timed-out qualification child");
            let _ = child.wait();
            panic!("qualification child did not exit within {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
