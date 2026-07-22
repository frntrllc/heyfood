//! Minimal DG-R1 authenticated streaming turn use case.

use std::fmt;
use std::sync::Arc;

use heyfood_core::{
    AgentConfirmationCommandWire, AgentEvent, CommitId, RefreshOutcome, RefreshRequest,
};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    ClockPort, CommitError, CommitOutcome, MutationProposal, OperationSnapshot, PortError,
    SerializedStateWriter, ServicePort,
};

pub const MAX_TURN_EVENTS: usize = 10_000;
pub const MAX_TURN_STREAM_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshPolicy {
    Never,
    IfExpired,
    Required,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TurnRequest {
    pub prompt: String,
    pub conversation_id: Option<String>,
    pub context: TurnContext,
    pub refresh: RefreshPolicy,
}

/// Optional context emitted for a normal text turn, plus the mutually
/// exclusive structured C3 confirmation input used by the interactive TUI.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TurnContext {
    pub dietary: Option<Value>,
    pub device: Option<Value>,
    pub meal: Option<Value>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    /// Structured C3 decision, mutually exclusive with a prompt.
    pub confirmation: Option<AgentConfirmationCommandWire>,
}

impl TurnRequest {
    #[must_use]
    pub fn has_exactly_one_input(&self) -> bool {
        let has_prompt = !self.prompt.trim().is_empty();
        has_prompt != self.context.confirmation.is_some()
    }
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
    /// The conversational POST was dispatched, but no response headers were
    /// observed. Retrying may duplicate a server-side conversational effect.
    CancelledAfterDispatchOutcomeUnknown,
    CancelledAfterServerAcceptance,
    StaleGeneration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunTurnError {
    InvalidRequest(&'static str),
    UnresolvedReconciliation,
    Service(PortError),
    ServiceReconciliationRequired(PortError),
    State(CommitError),
    EventConsumerClosed,
    StreamLimitExceeded,
    StreamEndedWithoutTerminalEvent,
}

impl fmt::Display for RunTurnError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest(message) => formatter.write_str(message),
            Self::UnresolvedReconciliation => formatter.write_str(
                "credentials have an unresolved refresh outcome; reconciliation is required",
            ),
            Self::Service(error) => write!(formatter, "service operation failed: {error}"),
            Self::ServiceReconciliationRequired(error) => write!(
                formatter,
                "service operation outcome is uncertain and requires reconciliation: {error}"
            ),
            Self::State(error) => write!(formatter, "state operation failed: {error}"),
            Self::EventConsumerClosed => formatter.write_str("turn event consumer closed"),
            Self::StreamLimitExceeded => {
                formatter.write_str("agent stream exceeded the bounded event or content budget")
            }
            Self::StreamEndedWithoutTerminalEvent => {
                formatter.write_str("agent stream ended without a terminal event")
            }
        }
    }
}

impl std::error::Error for RunTurnError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Service(error) | Self::ServiceReconciliationRequired(error) => Some(error),
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
        if !request.has_exactly_one_input() {
            return Err(RunTurnError::InvalidRequest(
                "turn requires exactly one prompt or confirmation",
            ));
        }
        if snapshot.session.reconciliation_required {
            return Err(RunTurnError::UnresolvedReconciliation);
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
            if cancellation.is_cancelled() {
                return Ok(RunTurnOutcome::CancelledBeforeServerAcceptance);
            }
            let reconciliation_id = CommitId::new();
            let accepted = match self
                .service
                .refresh_session(
                    RefreshRequest::from(&credentials),
                    cancellation.child_token(),
                )
                .await
            {
                Ok(RefreshOutcome::Refreshed(accepted)) => accepted,
                Ok(RefreshOutcome::CancelledBeforeDispatch) => {
                    return Ok(RunTurnOutcome::CancelledBeforeServerAcceptance);
                }
                Err(error) if error.outcome_uncertain => {
                    self.writer
                        .mark_reconciliation_required(reconciliation_id, error.clone())
                        .await
                        .map_err(RunTurnError::State)?;
                    return Err(RunTurnError::ServiceReconciliationRequired(error));
                }
                Err(error) => return Err(RunTurnError::Service(error)),
            };

            // Successful authenticated response is the acceptance boundary.
            // Commit rotation without observing cancellation.
            credentials = accepted.into_rotated();
            server_accepted = true;
            let proposal = MutationProposal::credential_rotation(
                &snapshot,
                reconciliation_id,
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

        let accepted = self
            .service
            .open_turn(
                request,
                credentials,
                snapshot.operation_id,
                cancellation.child_token(),
            )
            .await;
        let mut accepted = match accepted {
            Ok(accepted) => accepted,
            Err(error) if error.code == "converse_cancelled_before_dispatch" => {
                return Ok(if server_accepted {
                    RunTurnOutcome::CancelledAfterServerAcceptance
                } else {
                    RunTurnOutcome::CancelledBeforeServerAcceptance
                });
            }
            Err(error) if error.outcome_uncertain && cancellation.is_cancelled() => {
                return Ok(RunTurnOutcome::CancelledAfterDispatchOutcomeUnknown);
            }
            Err(error) => return Err(RunTurnError::Service(error)),
        };

        if cancellation.is_cancelled() {
            accepted
                .events
                .close()
                .await
                .map_err(RunTurnError::Service)?;
            return Ok(RunTurnOutcome::CancelledAfterServerAcceptance);
        }

        let mut event_count = 0_usize;
        let mut stream_bytes = 0_usize;
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

            event_count = event_count.saturating_add(1);
            let event_bytes = serde_json::to_vec(&event)
                .map_or(MAX_TURN_STREAM_BYTES.saturating_add(1), |bytes| bytes.len());
            stream_bytes = stream_bytes.saturating_add(event_bytes);
            if event_count > MAX_TURN_EVENTS || stream_bytes > MAX_TURN_STREAM_BYTES {
                let _ = accepted.events.close().await;
                return Err(RunTurnError::StreamLimitExceeded);
            }

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

            let delivery = tokio::select! {
                biased;
                () = cancellation.cancelled() => {
                    let _ = accepted.events.close().await;
                    return Ok(RunTurnOutcome::CancelledAfterServerAcceptance);
                }
                result = events.send(TurnEvent {
                    generation: snapshot.generation,
                    event,
                }) => result,
            };
            if delivery.is_err() {
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
