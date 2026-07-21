//! Bounded, renderer-neutral one-shot conversation use case.

use std::fmt;

use heyfood_core::{AgentEvent, OperationId, SessionCredentials, terminal_safe_text};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{PortError, ServicePort, TurnRequest};

pub const MAX_ONE_SHOT_EVENTS: usize = 10_000;
pub const MAX_ONE_SHOT_STREAM_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, PartialEq)]
pub struct OneShotTurnResult {
    pub document: Value,
    pub conversation_id: Option<String>,
}

impl fmt::Debug for OneShotTurnResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OneShotTurnResult")
            .field("document", &"[REDACTED]")
            .field("has_conversation_id", &self.conversation_id.is_some())
            .finish()
    }
}

pub async fn execute_one_shot_turn(
    service: &dyn ServicePort,
    request: TurnRequest,
    credentials: SessionCredentials,
    operation_id: OperationId,
    cancellation: CancellationToken,
) -> Result<OneShotTurnResult, PortError> {
    if request.prompt.trim().is_empty() {
        return Err(PortError::new(
            "invalid_prompt",
            "turn prompt must not be empty",
        ));
    }

    let mut accepted = service
        .open_turn(
            request,
            credentials,
            operation_id,
            cancellation.child_token(),
        )
        .await?;
    let mut event_count = 0_usize;
    let mut stream_bytes = 0_usize;

    loop {
        let next = accepted.events.next();
        let event = tokio::select! {
            () = cancellation.cancelled() => {
                let _ = accepted.events.close().await;
                return Err(PortError::uncertain(
                    "turn_cancelled_after_acceptance",
                    "turn was cancelled after the service accepted it",
                ));
            }
            event = next => event.map_err(|error| {
                PortError::uncertain(error.code, terminal_safe_text(&error.message))
            })?,
        };
        let Some(event) = event else {
            let _ = accepted.events.close().await;
            return Err(PortError::uncertain(
                "stream_incomplete",
                "agent stream ended without a terminal event",
            ));
        };

        event_count = event_count.saturating_add(1);
        let event_bytes = serde_json::to_vec(&event)
            .map_or(MAX_ONE_SHOT_STREAM_BYTES.saturating_add(1), |bytes| {
                bytes.len()
            });
        stream_bytes = stream_bytes.saturating_add(event_bytes);
        if event_count > MAX_ONE_SHOT_EVENTS || stream_bytes > MAX_ONE_SHOT_STREAM_BYTES {
            let _ = accepted.events.close().await;
            return Err(PortError::uncertain(
                "stream_limit",
                "agent stream exceeded its bounded event or content budget",
            ));
        }

        match event {
            AgentEvent::Result {
                document,
                conversation_id,
            } => {
                accepted.events.close().await?;
                return Ok(OneShotTurnResult {
                    document,
                    conversation_id,
                });
            }
            AgentEvent::Error { error } => {
                accepted.events.close().await?;
                return Err(PortError::new(
                    "agent_error",
                    terminal_safe_text(&error.message),
                ));
            }
            AgentEvent::Thinking { .. }
            | AgentEvent::Progress { .. }
            | AgentEvent::Partial { .. }
            | AgentEvent::Choices { .. } => {}
        }
    }
}
