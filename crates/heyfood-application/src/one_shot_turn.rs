//! Bounded, renderer-neutral one-shot conversation use case.

use std::fmt;

use heyfood_core::{AgentChoice, AgentEvent, OperationId, SessionCredentials, terminal_safe_text};
use serde_json::{Map, Value};
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
    let mut partial_text = String::new();
    let mut choices: Option<(Vec<AgentChoice>, bool)> = None;

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
                mut document,
                conversation_id,
            } => {
                merge_stream_content(&mut document, &partial_text, choices.take());
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
            AgentEvent::Partial { text } => partial_text.push_str(&text),
            AgentEvent::Choices {
                choices: streamed_choices,
                allow_multiple,
            } => choices = Some((streamed_choices, allow_multiple)),
            AgentEvent::Thinking { .. } | AgentEvent::Progress { .. } => {}
        }
    }
}

fn merge_stream_content(
    document: &mut Value,
    partial_text: &str,
    choices: Option<(Vec<AgentChoice>, bool)>,
) {
    let Some(fields) = document.as_object_mut() else {
        return;
    };
    let has_final_text = ["message", "text", "response"].iter().any(|key| {
        fields
            .get(*key)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
    });
    if !partial_text.is_empty() && !has_final_text {
        fields.insert("text".into(), Value::String(partial_text.to_owned()));
    }
    if let Some((choices, allow_multiple)) = choices {
        let mut choice_document = Map::new();
        let detailed = choices
            .iter()
            .filter_map(|choice| {
                choice.value.as_ref().map(|value| {
                    serde_json::json!({
                        "label": &choice.label,
                        "value": value
                    })
                })
            })
            .collect::<Vec<_>>();
        choice_document.insert(
            "choices".into(),
            Value::Array(
                choices
                    .into_iter()
                    .map(|choice| Value::String(choice.label))
                    .collect(),
            ),
        );
        if !detailed.is_empty() {
            choice_document.insert("choice_details".into(), Value::Array(detailed));
        }
        choice_document.insert("allow_multiple".into(), Value::Bool(allow_multiple));
        fields.insert("choices".into(), Value::Object(choice_document));
    }
}
