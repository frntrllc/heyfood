//! Minimal DG-R1 authenticated streaming turn use case.

use std::fmt;
use std::sync::Arc;

use heyfood_core::{AgentEvent, CommitId, RefreshRequest};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    ClockPort, CommitError, CommitOutcome, MutationProposal, OperationSnapshot, PortError,
    SerializedStateWriter, ServicePort,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshPolicy {
    Never,
    IfExpired,
    Required,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnRequest {
    pub prompt: String,
    pub conversation_id: Option<String>,
    pub refresh: RefreshPolicy,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TurnEvent {
    pub generation: heyfood_core::GenerationId,
    pub event: AgentEvent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunTurnOutcome {
    Completed,
    CancelledBeforeServerAcceptance,
    CancelledAfterServerAcceptance,
    StaleGeneration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunTurnError {
    InvalidRequest(&'static str),
    Service(PortError),
    State(CommitError),
    EventConsumerClosed,
    StreamEndedWithoutTerminalEvent,
}

impl fmt::Display for RunTurnError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest(message) => formatter.write_str(message),
            Self::Service(error) => write!(formatter, "service operation failed: {error}"),
            Self::State(error) => write!(formatter, "state operation failed: {error}"),
            Self::EventConsumerClosed => formatter.write_str("turn event consumer closed"),
            Self::StreamEndedWithoutTerminalEvent => {
                formatter.write_str("agent stream ended without a terminal event")
            }
        }
    }
}

impl std::error::Error for RunTurnError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Service(error) => Some(error),
            Self::State(error) => Some(error),
            _ => None,
        }
    }
}

pub struct RunTurn {
    service: Arc<dyn ServicePort>,
    clock: Arc<dyn ClockPort>,
    writer: Arc<SerializedStateWriter>,
}

impl RunTurn {
    #[must_use]
    pub fn new(
        service: Arc<dyn ServicePort>,
        clock: Arc<dyn ClockPort>,
        writer: Arc<SerializedStateWriter>,
    ) -> Self {
        Self {
            service,
            clock,
            writer,
        }
    }

    pub async fn execute(
        &self,
        request: TurnRequest,
        snapshot: OperationSnapshot,
        cancellation: CancellationToken,
        events: mpsc::Sender<TurnEvent>,
    ) -> Result<RunTurnOutcome, RunTurnError> {
        if request.prompt.trim().is_empty() {
            return Err(RunTurnError::InvalidRequest(
                "turn prompt must not be empty",
            ));
        }

        let needs_refresh = match request.refresh {
            RefreshPolicy::Never => false,
            RefreshPolicy::Required => true,
            RefreshPolicy::IfExpired => {
                snapshot.session.credentials.expires_at_unix() <= self.clock.unix_timestamp()
            }
        };
        let mut credentials = snapshot.session.credentials.clone();
        let mut server_accepted = false;

        if needs_refresh {
            let refresh = self.service.refresh_session(
                RefreshRequest::from(&credentials),
                cancellation.child_token(),
            );
            let Some(accepted) = cancellation.run_until_cancelled(refresh).await else {
                return Ok(RunTurnOutcome::CancelledBeforeServerAcceptance);
            };
            let accepted = accepted.map_err(RunTurnError::Service)?;

            // Successful authenticated response is the acceptance boundary.
            // Commit rotation without observing cancellation.
            credentials = accepted.into_rotated();
            server_accepted = true;
            let proposal = MutationProposal::credential_rotation(
                &snapshot,
                CommitId::new(),
                credentials.clone(),
            );
            self.writer
                .commit(proposal)
                .await
                .map_err(RunTurnError::State)?;

            if cancellation.is_cancelled() {
                return Ok(RunTurnOutcome::CancelledAfterServerAcceptance);
            }
        }

        let opening = self
            .service
            .open_turn(request, credentials, cancellation.child_token());
        let Some(accepted) = cancellation.run_until_cancelled(opening).await else {
            return Ok(if server_accepted {
                RunTurnOutcome::CancelledAfterServerAcceptance
            } else {
                RunTurnOutcome::CancelledBeforeServerAcceptance
            });
        };
        let mut accepted = accepted.map_err(RunTurnError::Service)?;

        if cancellation.is_cancelled() {
            accepted
                .events
                .close()
                .await
                .map_err(RunTurnError::Service)?;
            return Ok(RunTurnOutcome::CancelledAfterServerAcceptance);
        }

        loop {
            let next = accepted.events.next();
            let Some(event) = cancellation.run_until_cancelled(next).await else {
                accepted
                    .events
                    .close()
                    .await
                    .map_err(RunTurnError::Service)?;
                return Ok(RunTurnOutcome::CancelledAfterServerAcceptance);
            };
            let event = match event {
                Ok(event) => event,
                Err(error) => {
                    let _ = accepted.events.close().await;
                    return Err(RunTurnError::Service(error));
                }
            };
            let Some(event) = event else {
                accepted
                    .events
                    .close()
                    .await
                    .map_err(RunTurnError::Service)?;
                return Err(RunTurnError::StreamEndedWithoutTerminalEvent);
            };

            let terminal = event.is_terminal();
            let conversation_id = match &event {
                AgentEvent::Result {
                    conversation_id, ..
                } => conversation_id.clone(),
                _ => None,
            };
            let outcome = match self
                .writer
                .commit(MutationProposal::presentation(&snapshot, event.clone()))
                .await
            {
                Ok(outcome) => outcome,
                Err(error) => {
                    let _ = accepted.events.close().await;
                    return Err(RunTurnError::State(error));
                }
            };
            if outcome == CommitOutcome::RejectedStaleGeneration {
                accepted
                    .events
                    .close()
                    .await
                    .map_err(RunTurnError::Service)?;
                return Ok(RunTurnOutcome::StaleGeneration);
            }

            if events
                .send(TurnEvent {
                    generation: snapshot.generation,
                    event,
                })
                .await
                .is_err()
            {
                let _ = accepted.events.close().await;
                return Err(RunTurnError::EventConsumerClosed);
            }

            if terminal {
                if conversation_id.is_some() {
                    let pointer =
                        MutationProposal::conversation_pointer(&snapshot, conversation_id);
                    let pointer_outcome = match self.writer.commit(pointer).await {
                        Ok(outcome) => outcome,
                        Err(error) => {
                            let _ = accepted.events.close().await;
                            return Err(RunTurnError::State(error));
                        }
                    };
                    if pointer_outcome == CommitOutcome::RejectedStaleGeneration {
                        accepted
                            .events
                            .close()
                            .await
                            .map_err(RunTurnError::Service)?;
                        return Ok(RunTurnOutcome::StaleGeneration);
                    }
                }
                accepted
                    .events
                    .close()
                    .await
                    .map_err(RunTurnError::Service)?;
                return Ok(RunTurnOutcome::Completed);
            }
        }
    }
}
