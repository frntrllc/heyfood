use std::future::pending;
use std::sync::Mutex;

use heyfood_application::{
    AcceptedTurn, BoxEventStream, BoxFuture, EventStream, MAX_ONE_SHOT_EVENTS, PortError,
    RefreshPolicy, ServicePort, TurnContext, TurnRequest, execute_one_shot_turn,
};
use heyfood_core::{
    AccountId, AgentEvent, CredentialVersion, OperationId, RefreshOutcome, RefreshRequest,
    SensitiveString, SessionCredentials,
};
use tokio_util::sync::CancellationToken;

enum StreamBehavior {
    Pending,
    Eof,
    Error(Option<PortError>),
    Partials(usize),
}

struct FixtureStream(StreamBehavior);

impl EventStream for FixtureStream {
    fn next(&mut self) -> BoxFuture<'_, Result<Option<AgentEvent>, PortError>> {
        Box::pin(async move {
            match &mut self.0 {
                StreamBehavior::Pending => pending().await,
                StreamBehavior::Eof => Ok(None),
                StreamBehavior::Error(error) => Err(error.take().unwrap()),
                StreamBehavior::Partials(remaining) => {
                    if *remaining == 0 {
                        Ok(None)
                    } else {
                        *remaining -= 1;
                        Ok(Some(AgentEvent::Partial { text: "x".into() }))
                    }
                }
            }
        })
    }

    fn close(self: Box<Self>) -> BoxFuture<'static, Result<(), PortError>> {
        Box::pin(async { Ok(()) })
    }
}

struct FixtureService(Mutex<Option<BoxEventStream>>);

impl ServicePort for FixtureService {
    fn refresh_session(
        &self,
        _request: RefreshRequest,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<RefreshOutcome, PortError>> {
        Box::pin(async { Err(PortError::new("unused", "unused")) })
    }

    fn open_turn(
        &self,
        _request: TurnRequest,
        _credentials: SessionCredentials,
        _operation_id: OperationId,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<AcceptedTurn, PortError>> {
        Box::pin(async move {
            Ok(AcceptedTurn {
                events: self.0.lock().unwrap().take().unwrap(),
            })
        })
    }
}

fn credentials() -> SessionCredentials {
    SessionCredentials::from_unix_expiry(
        AccountId::parse("one-shot-uncertainty").unwrap(),
        SensitiveString::new("access"),
        SensitiveString::new("refresh"),
        CredentialVersion::new(1),
        4_102_444_800,
    )
    .unwrap()
}

async fn execute(behavior: StreamBehavior, cancellation: CancellationToken) -> PortError {
    let service = FixtureService(Mutex::new(Some(Box::new(FixtureStream(behavior)))));
    execute_one_shot_turn(
        &service,
        TurnRequest {
            prompt: "log lunch".into(),
            conversation_id: None,
            context: TurnContext::default(),
            refresh: RefreshPolicy::Never,
        },
        credentials(),
        OperationId::new(),
        cancellation,
    )
    .await
    .unwrap_err()
}

#[tokio::test]
async fn cancellation_after_acceptance_is_uncertain() {
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let error = execute(StreamBehavior::Pending, cancellation).await;
    assert_eq!(error.code, "turn_cancelled_after_acceptance");
    assert!(error.outcome_uncertain);
}

#[tokio::test]
async fn clean_eof_after_acceptance_is_uncertain() {
    let error = execute(StreamBehavior::Eof, CancellationToken::new()).await;
    assert_eq!(error.code, "stream_incomplete");
    assert!(error.outcome_uncertain);
}

#[tokio::test]
async fn inactivity_or_malformed_stream_after_acceptance_is_uncertain() {
    for code in ["sse_inactivity", "sse_utf8"] {
        let error = execute(
            StreamBehavior::Error(Some(PortError::new(code, "non-terminal stream failure"))),
            CancellationToken::new(),
        )
        .await;
        assert_eq!(error.code, code);
        assert!(error.outcome_uncertain);
    }
}

#[tokio::test]
async fn bounded_stream_exit_after_acceptance_is_uncertain() {
    let error = execute(
        StreamBehavior::Partials(MAX_ONE_SHOT_EVENTS + 1),
        CancellationToken::new(),
    )
    .await;
    assert_eq!(error.code, "stream_limit");
    assert!(error.outcome_uncertain);
}
