use std::collections::VecDeque;
use std::time::Duration;

use heyfood_application::{BoxFuture, EventStream, PortError};
use heyfood_core::{AgentChoice, AgentEvent, AgentFailure};
use reqwest::Response;
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub struct SseEventStream {
    response: Option<Response>,
    cancellation: CancellationToken,
    inactivity: Duration,
    parser: RawSseParser,
    normalized: VecDeque<AgentEvent>,
}

impl SseEventStream {
    pub(crate) fn new(
        response: Response,
        cancellation: CancellationToken,
        inactivity: Duration,
    ) -> Self {
        Self {
            response: Some(response),
            cancellation,
            inactivity,
            parser: RawSseParser::default(),
            normalized: VecDeque::new(),
        }
    }
}

impl EventStream for SseEventStream {
    fn next(&mut self) -> BoxFuture<'_, Result<Option<AgentEvent>, PortError>> {
        Box::pin(async move {
            loop {
                if let Some(event) = self.normalized.pop_front() {
                    return Ok(Some(event));
                }
                let Some(response) = self.response.as_mut() else {
                    return Ok(None);
                };
                let chunk = tokio::select! {
                    () = self.cancellation.cancelled() => {
                        self.response.take();
                        // `RunTurn` owns the terminal cancellation outcome. Drop
                        // the socket here, then let its outer cancellation race
                        // win instead of surfacing cancellation as a service
                        // failure.
                        std::future::pending::<()>().await;
                        unreachable!("a pending cancellation branch cannot complete");
                    }
                    result = tokio::time::timeout(self.inactivity, response.chunk()) => {
                        match result {
                            Ok(Ok(value)) => value,
                            Ok(Err(_)) => {
                                self.response.take();
                                return Err(PortError::uncertain("sse_transport", "event stream transport failed"));
                            }
                            Err(_) => {
                                self.response.take();
                                return Err(PortError::new("sse_inactivity", "event stream inactivity deadline expired"));
                            }
                        }
                    }
                };
                let Some(chunk) = chunk else {
                    self.response.take();
                    return Ok(None);
                };
                let raw_events = self.parser.push(&chunk)?;
                for raw in raw_events {
                    self.normalized.push_back(normalize(raw)?);
                }
            }
        })
    }

    fn close(mut self: Box<Self>) -> BoxFuture<'static, Result<(), PortError>> {
        self.cancellation.cancel();
        self.response.take();
        Box::pin(async { Ok(()) })
    }
}

#[derive(Default)]
struct RawSseParser {
    bytes: Vec<u8>,
    event_type: String,
    data: Vec<String>,
    first_line: bool,
}

struct RawEvent {
    event_type: String,
    data: String,
}

impl RawSseParser {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<RawEvent>, PortError> {
        self.bytes.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some(index) = self.bytes.iter().position(|byte| *byte == b'\n') {
            let mut line = self.bytes.drain(..=index).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if !self.first_line {
                self.first_line = true;
                if line.starts_with(&[0xef, 0xbb, 0xbf]) {
                    line.drain(..3);
                }
            }
            let line = String::from_utf8(line)
                .map_err(|_| PortError::new("sse_utf8", "event stream is not valid UTF-8"))?;
            if line.is_empty() {
                if !self.data.is_empty() {
                    events.push(RawEvent {
                        event_type: if self.event_type.is_empty() {
                            "message".into()
                        } else {
                            std::mem::take(&mut self.event_type)
                        },
                        data: self.data.join("\n"),
                    });
                    self.data.clear();
                } else {
                    self.event_type.clear();
                }
                continue;
            }
            if line.starts_with(':') {
                continue;
            }
            let (field, mut value) = line
                .split_once(':')
                .map_or((line.as_str(), ""), |(field, value)| (field, value));
            if let Some(stripped) = value.strip_prefix(' ') {
                value = stripped;
            }
            match field {
                "event" => self.event_type = value.into(),
                "data" => self.data.push(value.into()),
                _ => {}
            }
        }
        Ok(events)
    }
}

#[derive(Deserialize)]
struct ThinkingData {
    #[serde(default)]
    stage: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Deserialize)]
struct ProgressData {
    message: String,
    #[serde(default)]
    current: Option<u64>,
    #[serde(default)]
    total: Option<u64>,
}

#[derive(Deserialize)]
struct PartialData {
    #[serde(default, alias = "delta")]
    text: String,
}

#[derive(Deserialize)]
struct ChoicesData {
    choices: Vec<ChoiceWire>,
    #[serde(default)]
    allow_multiple: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ChoiceWire {
    Label(String),
    Detailed(AgentChoice),
}

#[derive(Deserialize)]
struct ErrorData {
    #[serde(default)]
    error: Option<AgentFailure>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    retryable: bool,
}

fn normalize(raw: RawEvent) -> Result<AgentEvent, PortError> {
    let parse_error = || {
        PortError::new(
            "sse_payload",
            format!("invalid {} event payload", raw.event_type),
        )
    };
    match raw.event_type.as_str() {
        "thinking" => {
            let data: ThinkingData = serde_json::from_str(&raw.data).map_err(|_| parse_error())?;
            Ok(AgentEvent::Thinking {
                stage: data.stage,
                message: data.message,
            })
        }
        "progress" => {
            let data: ProgressData = serde_json::from_str(&raw.data).map_err(|_| parse_error())?;
            Ok(AgentEvent::Progress {
                message: data.message,
                current: data.current,
                total: data.total,
            })
        }
        "partial" => {
            let data: PartialData = serde_json::from_str(&raw.data).map_err(|_| parse_error())?;
            Ok(AgentEvent::Partial { text: data.text })
        }
        "choices" => {
            let data: ChoicesData = serde_json::from_str(&raw.data).map_err(|_| parse_error())?;
            let choices = data
                .choices
                .into_iter()
                .map(|choice| match choice {
                    ChoiceWire::Label(label) => AgentChoice { label, value: None },
                    ChoiceWire::Detailed(choice) => choice,
                })
                .collect();
            Ok(AgentEvent::Choices {
                choices,
                allow_multiple: data.allow_multiple,
            })
        }
        "result" => {
            let document: Value = serde_json::from_str(&raw.data).map_err(|_| parse_error())?;
            let conversation_id = document
                .get("conversation_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            Ok(AgentEvent::Result {
                document,
                conversation_id,
            })
        }
        "error" => {
            let data: ErrorData = serde_json::from_str(&raw.data).map_err(|_| parse_error())?;
            let error = data.error.unwrap_or_else(|| AgentFailure {
                code: data.code.unwrap_or_else(|| "service_error".into()),
                message: data.message.unwrap_or_else(|| "service error".into()),
                retryable: data.retryable,
            });
            Ok(AgentEvent::Error { error })
        }
        _ => Err(PortError::new(
            "sse_event",
            format!("unsupported event type {}", raw.event_type),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{RawSseParser, normalize};
    use heyfood_core::AgentEvent;

    #[test]
    fn comments_crlf_chunking_and_multiline_data_are_normalized() {
        let mut parser = RawSseParser::default();
        assert!(parser.push(b": heart").unwrap().is_empty());
        let raw = parser
            .push(
                b"beat\r\nevent: partial\r\ndata: {\"text\":\"hel\"\r\ndata: ,\"ignored\":true}\r\n\r\n",
            )
            .unwrap();
        assert_eq!(raw.len(), 1);
        assert_eq!(
            normalize(raw.into_iter().next().unwrap()).unwrap(),
            AgentEvent::Partial { text: "hel".into() }
        );
    }

    #[test]
    fn string_and_detailed_choices_share_the_domain_shape() {
        let mut parser = RawSseParser::default();
        let raw = parser
            .push(
                b"event: choices\ndata: {\"choices\":[\"One\",{\"label\":\"Two\",\"value\":\"2\"}],\"allow_multiple\":true}\n\n",
            )
            .unwrap();
        let event = normalize(raw.into_iter().next().unwrap()).unwrap();
        assert!(matches!(
            event,
            AgentEvent::Choices {
                allow_multiple: true,
                ..
            }
        ));
    }
}
