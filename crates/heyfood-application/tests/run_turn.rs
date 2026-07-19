use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;
use std::time::Duration;

use heyfood_application::{
    AcceptedTurn, BoxFuture, ClockPort, CommitOutcome, ConfigCommit, ConfigPort, CredentialCommit,
    CredentialPort, EventStream, MutationProposal, PortError, RefreshPolicy, RunTurn, RunTurnError,
    RunTurnOutcome, SerializedStateWriter, ServicePort, TurnRequest,
};
use heyfood_core::{
    AccountId, AgentEvent, ClientConfig, ConfigRevision, CredentialVersion, GenerationId,
    NetworkPolicy, OperationId, RefreshOutcome, RefreshRequest, RefreshResult, SensitiveString,
    ServiceUrl, SessionCredentials, SessionSnapshot,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct ThreadWake(thread::Thread);

impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn block_on<T>(future: impl Future<Output = T>) -> T {
    let waker = Waker::from(Arc::new(ThreadWake(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    loop {
        match Future::poll(Pin::as_mut(&mut future), &mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => thread::park_timeout(Duration::from_millis(10)),
        }
    }
}

fn credentials(version: u64) -> SessionCredentials {
    SessionCredentials::from_unix_expiry(
        AccountId::parse("account-1").unwrap(),
        SensitiveString::new(format!("access-{version}")),
        SensitiveString::new(format!("refresh-{version}")),
        CredentialVersion::new(version),
        0,
    )
    .unwrap()
}

fn config() -> ClientConfig {
    ClientConfig {
        active_context: "test".into(),
        api_url: ServiceUrl::parse("https://api.hello.food", NetworkPolicy::HTTPS_ONLY).unwrap(),
        auth_url: ServiceUrl::parse(
            "https://auth.hello.food/authorize",
            NetworkPolicy::HTTPS_ONLY,
        )
        .unwrap(),
        revision: ConfigRevision::new(1),
    }
}

fn snapshot() -> heyfood_application::OperationSnapshot {
    heyfood_application::OperationSnapshot {
        operation_id: OperationId::new(),
        generation: GenerationId::INITIAL,
        config: config(),
        session: SessionSnapshot {
            credentials: credentials(1),
            reconciliation_required: false,
        },
    }
}

#[derive(Default)]
struct FakeCredentials {
    stored: Mutex<Option<SessionCredentials>>,
    commits: Mutex<Vec<CredentialCommit>>,
    reconciliation: Mutex<Vec<heyfood_core::CommitId>>,
    cancel_during_commit: Mutex<Option<CancellationToken>>,
    fail_commit: Mutex<bool>,
    fail_marker: Mutex<bool>,
    fail_clear: Mutex<bool>,
    clears: Mutex<Vec<heyfood_core::CommitId>>,
}

impl CredentialPort for FakeCredentials {
    fn load(&self) -> BoxFuture<'_, Result<Option<SessionCredentials>, PortError>> {
        Box::pin(async { Ok(self.stored.lock().unwrap().clone()) })
    }

    fn commit(&self, commit: CredentialCommit) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            if let Some(cancel) = self.cancel_during_commit.lock().unwrap().take() {
                cancel.cancel();
            }
            if *self.fail_commit.lock().unwrap() {
                return Err(PortError::uncertain(
                    "credential_commit_uncertain",
                    "credential adapter outcome is uncertain",
                ));
            }
            *self.stored.lock().unwrap() = Some(commit.credentials.clone());
            self.commits.lock().unwrap().push(commit);
            Ok(())
        })
    }

    fn mark_reconciliation_required(
        &self,
        commit_id: heyfood_core::CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            if *self.fail_marker.lock().unwrap() {
                return Err(PortError::new(
                    "marker_write",
                    "reconciliation marker write failed",
                ));
            }
            self.reconciliation.lock().unwrap().push(commit_id);
            Ok(())
        })
    }

    fn clear_reconciliation_required(
        &self,
        commit_id: heyfood_core::CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            if *self.fail_clear.lock().unwrap() {
                return Err(PortError::new(
                    "marker_clear",
                    "reconciliation marker clear failed",
                ));
            }
            self.reconciliation
                .lock()
                .unwrap()
                .retain(|value| *value != commit_id);
            self.clears.lock().unwrap().push(commit_id);
            Ok(())
        })
    }
}

#[derive(Default)]
struct FakeConfig {
    commits: Mutex<Vec<ConfigCommit>>,
}

impl ConfigPort for FakeConfig {
    fn load(&self) -> BoxFuture<'_, Result<ClientConfig, PortError>> {
        Box::pin(async { Ok(config()) })
    }

    fn commit(&self, commit: ConfigCommit) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            self.commits.lock().unwrap().push(commit);
            Ok(())
        })
    }
}

struct FakeClock;

impl ClockPort for FakeClock {
    fn unix_timestamp(&self) -> i64 {
        1
    }
}

#[derive(Clone, Copy)]
enum RefreshBehavior {
    CancelledBeforeDispatch,
    Accepted,
    Uncertain,
}

struct FakeService {
    behavior: RefreshBehavior,
    cancellation: CancellationToken,
    events: Mutex<Vec<AgentEvent>>,
    opens: Mutex<usize>,
}

impl ServicePort for FakeService {
    fn refresh_session(
        &self,
        request: RefreshRequest,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<RefreshOutcome, PortError>> {
        Box::pin(async move {
            match self.behavior {
                RefreshBehavior::CancelledBeforeDispatch => {
                    self.cancellation.cancel();
                    Ok(RefreshOutcome::CancelledBeforeDispatch)
                }
                RefreshBehavior::Accepted => RefreshResult::validated(&request, credentials(2))
                    .map(RefreshOutcome::Refreshed)
                    .map_err(|error| PortError::new("fixture", error)),
                RefreshBehavior::Uncertain => Err(PortError::uncertain(
                    "refresh_transport",
                    "peer consumed the token but withheld its response",
                )),
            }
        })
    }

    fn open_turn(
        &self,
        _request: TurnRequest,
        _credentials: SessionCredentials,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<AcceptedTurn, PortError>> {
        Box::pin(async move {
            *self.opens.lock().unwrap() += 1;
            Ok(AcceptedTurn {
                events: Box::new(VecEvents {
                    events: self.events.lock().unwrap().drain(..).collect(),
                    closed: false,
                }),
            })
        })
    }
}

struct VecEvents {
    events: VecDeque<AgentEvent>,
    closed: bool,
}

impl EventStream for VecEvents {
    fn next(&mut self) -> BoxFuture<'_, Result<Option<AgentEvent>, PortError>> {
        Box::pin(async { Ok(self.events.pop_front()) })
    }

    fn close(mut self: Box<Self>) -> BoxFuture<'static, Result<(), PortError>> {
        self.closed = true;
        Box::pin(async { Ok(()) })
    }
}

fn harness(
    behavior: RefreshBehavior,
    cancellation: &CancellationToken,
    events: Vec<AgentEvent>,
) -> (
    RunTurn,
    Arc<FakeCredentials>,
    Arc<FakeConfig>,
    Arc<SerializedStateWriter>,
    Arc<FakeService>,
) {
    let credential_port = Arc::new(FakeCredentials::default());
    *credential_port.stored.lock().unwrap() = Some(credentials(1));
    let config_port = Arc::new(FakeConfig::default());
    let writer = Arc::new(SerializedStateWriter::new(
        credential_port.clone(),
        config_port.clone(),
        GenerationId::INITIAL,
        Some(&credentials(1)),
    ));
    let service = Arc::new(FakeService {
        behavior,
        cancellation: cancellation.clone(),
        events: Mutex::new(events),
        opens: Mutex::new(0),
    });
    let run_turn = RunTurn::new(service.clone(), Arc::new(FakeClock), writer.clone());
    (run_turn, credential_port, config_port, writer, service)
}

#[test]
fn cancellation_before_server_acceptance_does_not_mutate_credentials() {
    block_on(async {
        let cancellation = CancellationToken::new();
        let (run_turn, credentials_port, _, _, service) = harness(
            RefreshBehavior::CancelledBeforeDispatch,
            &cancellation,
            vec![],
        );
        let (sender, _receiver) = mpsc::channel(4);

        let outcome = run_turn
            .execute(
                TurnRequest {
                    prompt: "hello".into(),
                    conversation_id: None,
                    refresh: RefreshPolicy::Required,
                },
                snapshot(),
                cancellation,
                sender,
            )
            .await
            .unwrap();

        assert_eq!(outcome, RunTurnOutcome::CancelledBeforeServerAcceptance);
        assert!(credentials_port.commits.lock().unwrap().is_empty());
        assert_eq!(
            credentials_port
                .stored
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .version,
            CredentialVersion::new(1)
        );
        assert_eq!(*service.opens.lock().unwrap(), 0);
    });
}

#[test]
fn cancellation_during_post_acceptance_commit_cannot_lose_rotated_credentials() {
    block_on(async {
        let cancellation = CancellationToken::new();
        let (run_turn, credentials_port, _, _, service) =
            harness(RefreshBehavior::Accepted, &cancellation, vec![]);
        *credentials_port.cancel_during_commit.lock().unwrap() = Some(cancellation.clone());
        let (sender, _receiver) = mpsc::channel(4);

        let outcome = run_turn
            .execute(
                TurnRequest {
                    prompt: "hello".into(),
                    conversation_id: None,
                    refresh: RefreshPolicy::Required,
                },
                snapshot(),
                cancellation,
                sender,
            )
            .await
            .unwrap();

        assert_eq!(outcome, RunTurnOutcome::CancelledAfterServerAcceptance);
        assert_eq!(credentials_port.commits.lock().unwrap().len(), 1);
        let stored = credentials_port.stored.lock().unwrap().clone().unwrap();
        assert_eq!(stored.version, CredentialVersion::new(2));
        assert_eq!(stored.refresh_token.expose_secret(), "refresh-2");
        assert_eq!(*service.opens.lock().unwrap(), 0);
    });
}

#[test]
fn stale_generation_rejects_ui_but_not_server_accepted_rotation() {
    block_on(async {
        let cancellation = CancellationToken::new();
        let event = AgentEvent::Partial {
            text: "stale".into(),
        };
        let (run_turn, credentials_port, _, writer, _) =
            harness(RefreshBehavior::Accepted, &cancellation, vec![event]);
        writer.advance_generation().await;
        let (sender, mut receiver) = mpsc::channel(4);

        let outcome = run_turn
            .execute(
                TurnRequest {
                    prompt: "hello".into(),
                    conversation_id: None,
                    refresh: RefreshPolicy::Required,
                },
                snapshot(),
                cancellation,
                sender,
            )
            .await
            .unwrap();

        assert_eq!(outcome, RunTurnOutcome::StaleGeneration);
        assert_eq!(
            credentials_port
                .stored
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .version,
            CredentialVersion::new(2)
        );
        assert!(receiver.try_recv().is_err());
    });
}

#[test]
fn terminal_result_streams_and_persists_current_conversation_pointer() {
    block_on(async {
        let cancellation = CancellationToken::new();
        let events = vec![
            AgentEvent::Partial {
                text: "hello".into(),
            },
            AgentEvent::Result {
                document: Default::default(),
                conversation_id: Some("conversation-2".into()),
            },
        ];
        let (run_turn, _, config_port, _, _) =
            harness(RefreshBehavior::Accepted, &cancellation, events);
        let (sender, mut receiver) = mpsc::channel(4);

        let outcome = run_turn
            .execute(
                TurnRequest {
                    prompt: "hello".into(),
                    conversation_id: None,
                    refresh: RefreshPolicy::Required,
                },
                snapshot(),
                cancellation,
                sender,
            )
            .await
            .unwrap();

        assert_eq!(outcome, RunTurnOutcome::Completed);
        assert!(matches!(
            receiver.try_recv().unwrap().event,
            AgentEvent::Partial { .. }
        ));
        assert!(matches!(
            receiver.try_recv().unwrap().event,
            AgentEvent::Result { .. }
        ));
        assert_eq!(config_port.commits.lock().unwrap().len(), 1);
    });
}

#[test]
fn duplicate_durable_proposal_is_idempotent() {
    block_on(async {
        let cancellation = CancellationToken::new();
        let (_, credentials_port, _, writer, _) =
            harness(RefreshBehavior::Accepted, &cancellation, vec![]);
        let proposal = MutationProposal::credential_rotation(
            &snapshot(),
            heyfood_core::CommitId::new(),
            credentials(2),
        );

        assert_eq!(
            writer.commit(proposal.clone()).await.unwrap(),
            CommitOutcome::Applied
        );
        credentials_port
            .reconciliation
            .lock()
            .unwrap()
            .push(proposal.metadata.commit_id);
        assert_eq!(
            writer.commit(proposal).await.unwrap(),
            CommitOutcome::Duplicate
        );
        assert_eq!(credentials_port.commits.lock().unwrap().len(), 1);
        assert!(credentials_port.reconciliation.lock().unwrap().is_empty());
        assert_eq!(credentials_port.clears.lock().unwrap().len(), 2);
    });
}

#[test]
fn uncertain_post_dispatch_refresh_is_marked_and_blocks_restart() {
    block_on(async {
        let cancellation = CancellationToken::new();
        let (run_turn, credentials_port, _, _, service) =
            harness(RefreshBehavior::Uncertain, &cancellation, vec![]);
        let (sender, _receiver) = mpsc::channel(4);

        let error = run_turn
            .execute(
                TurnRequest {
                    prompt: "hello".into(),
                    conversation_id: None,
                    refresh: RefreshPolicy::Required,
                },
                snapshot(),
                cancellation,
                sender,
            )
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            RunTurnError::ServiceReconciliationRequired(ref cause)
                if cause.outcome_uncertain
        ));
        assert_eq!(credentials_port.reconciliation.lock().unwrap().len(), 1);
        assert_eq!(*service.opens.lock().unwrap(), 0);

        // A restarted application reconstructs this bit from the durable
        // marker and must fail closed before dispatching another request.
        let restart_cancellation = CancellationToken::new();
        let restarted_writer = Arc::new(SerializedStateWriter::new(
            credentials_port.clone(),
            Arc::new(FakeConfig::default()),
            GenerationId::INITIAL,
            Some(&credentials(1)),
        ));
        let restarted_service = Arc::new(FakeService {
            behavior: RefreshBehavior::Accepted,
            cancellation: restart_cancellation.clone(),
            events: Mutex::new(vec![]),
            opens: Mutex::new(0),
        });
        let restarted = RunTurn::new(
            restarted_service.clone(),
            Arc::new(FakeClock),
            restarted_writer,
        );
        let mut restart_snapshot = snapshot();
        restart_snapshot.session.reconciliation_required =
            !credentials_port.reconciliation.lock().unwrap().is_empty();
        let (sender, _receiver) = mpsc::channel(4);
        let error = restarted
            .execute(
                TurnRequest {
                    prompt: "do not dispatch".into(),
                    conversation_id: None,
                    refresh: RefreshPolicy::Required,
                },
                restart_snapshot,
                restart_cancellation,
                sender,
            )
            .await
            .unwrap_err();
        assert_eq!(error, RunTurnError::UnresolvedReconciliation);
        assert_eq!(*restarted_service.opens.lock().unwrap(), 0);
    });
}

#[test]
fn reconciliation_marker_write_failure_is_surfaced_fail_closed() {
    block_on(async {
        let cancellation = CancellationToken::new();
        let (run_turn, credentials_port, _, _, service) =
            harness(RefreshBehavior::Uncertain, &cancellation, vec![]);
        *credentials_port.fail_marker.lock().unwrap() = true;
        let (sender, _receiver) = mpsc::channel(4);

        let error = run_turn
            .execute(
                TurnRequest {
                    prompt: "hello".into(),
                    conversation_id: None,
                    refresh: RefreshPolicy::Required,
                },
                snapshot(),
                cancellation,
                sender,
            )
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            RunTurnError::State(heyfood_application::CommitError::ReconciliationMarkerWrite { .. })
        ));
        assert!(credentials_port.commits.lock().unwrap().is_empty());
        assert!(credentials_port.reconciliation.lock().unwrap().is_empty());
        assert_eq!(*service.opens.lock().unwrap(), 0);
    });
}

#[test]
fn uncertain_rotation_commit_marks_reconciliation_required() {
    block_on(async {
        let cancellation = CancellationToken::new();
        let (run_turn, credentials_port, _, _, _) =
            harness(RefreshBehavior::Accepted, &cancellation, vec![]);
        *credentials_port.fail_commit.lock().unwrap() = true;
        let (sender, _receiver) = mpsc::channel(4);

        let error = run_turn
            .execute(
                TurnRequest {
                    prompt: "hello".into(),
                    conversation_id: None,
                    refresh: RefreshPolicy::Required,
                },
                snapshot(),
                cancellation,
                sender,
            )
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            RunTurnError::State(heyfood_application::CommitError::ReconciliationRequired(_))
        ));
        assert_eq!(credentials_port.reconciliation.lock().unwrap().len(), 1);
        assert_eq!(
            credentials_port
                .stored
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .version,
            CredentialVersion::new(1)
        );
    });
}

#[test]
fn stale_generation_cannot_discard_a_local_first_durable_record() {
    block_on(async {
        let cancellation = CancellationToken::new();
        let (_, _, config_port, writer, _) =
            harness(RefreshBehavior::Accepted, &cancellation, vec![]);
        let operation = snapshot();
        writer.advance_generation().await;

        let outcome = writer
            .commit(MutationProposal::local_first(
                &operation,
                heyfood_core::CommitId::new(),
                "household_effect",
                b"repairable".to_vec(),
            ))
            .await
            .unwrap();

        assert_eq!(outcome, CommitOutcome::Applied);
        let commits = config_port.commits.lock().unwrap();
        assert_eq!(commits.len(), 1);
        assert!(matches!(
            &commits[0].mutation,
            heyfood_application::ConfigMutation::LocalFirstRecord { kind, payload }
                if kind == "household_effect" && payload == b"repairable"
        ));
    });
}
