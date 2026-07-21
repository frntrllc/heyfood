use std::collections::VecDeque;
use std::time::Duration;

use heyfood_application::{BoxFuture, EventStream, PortError};
use heyfood_core::{AgentChoice, AgentEvent, AgentFailure, terminal_safe_text};
use reqwest::Response;
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

const MAX_SSE_LINE_BYTES: usize = 64 * 1024;
const MAX_SSE_EVENT_BYTES: usize = 1024 * 1024;
const MAX_SSE_BUFFERED_BYTES: usize = MAX_SSE_EVENT_BYTES + (2 * MAX_SSE_LINE_BYTES);
const MAX_STREAM_METADATA_BYTES: usize = heyfood_core::agent::MAX_AGENT_METADATA_BYTES;
const MAX_CONVERSATION_ID_BYTES: usize = 4 * 1024;

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
                let raw_events = match self.parser.push(&chunk) {
                    Ok(events) => events,
                    Err(error) => {
                        self.response.take();
                        return Err(error);
                    }
                };
                for raw in raw_events {
                    if consume_done_marker(&raw)? {
                        continue;
                    }
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
    data: String,
    data_lines: usize,
    first_line: bool,
}

#[derive(Debug)]
struct RawEvent {
    event_type: String,
    data: String,
}

impl RawSseParser {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<RawEvent>, PortError> {
        if self
            .bytes
            .len()
            .saturating_add(self.data.len())
            .saturating_add(bytes.len())
            > MAX_SSE_BUFFERED_BYTES
        {
            return Err(PortError::new(
                "sse_buffer_too_large",
                "event stream exceeded the aggregate buffer limit",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some(index) = self.bytes.iter().position(|byte| *byte == b'\n') {
            if index > MAX_SSE_LINE_BYTES {
                return Err(PortError::new(
                    "sse_line_too_large",
                    "event stream line exceeded the size limit",
                ));
            }
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
                if self.data_lines != 0 {
                    events.push(RawEvent {
                        event_type: if self.event_type.is_empty() {
                            "message".into()
                        } else {
                            std::mem::take(&mut self.event_type)
                        },
                        data: std::mem::take(&mut self.data),
                    });
                    self.data_lines = 0;
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
                "data" => {
                    let separator = usize::from(self.data_lines != 0);
                    if self
                        .data
                        .len()
                        .saturating_add(separator)
                        .saturating_add(value.len())
                        > MAX_SSE_EVENT_BYTES
                    {
                        return Err(PortError::new(
                            "sse_event_too_large",
                            "event stream event exceeded the size limit",
                        ));
                    }
                    if self.data_lines != 0 {
                        self.data.push('\n');
                    }
                    self.data.push_str(value);
                    self.data_lines += 1;
                }
                _ => {}
            }
        }
        if self.bytes.len() > MAX_SSE_LINE_BYTES {
            return Err(PortError::new(
                "sse_line_too_large",
                "event stream line exceeded the size limit",
            ));
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

fn consume_done_marker(raw: &RawEvent) -> Result<bool, PortError> {
    if raw.event_type != "done" {
        return Ok(false);
    }
    let document: Value = serde_json::from_str(&raw.data)
        .map_err(|_| PortError::new("sse_payload", "invalid done event payload"))?;
    if !matches!(document, Value::Object(ref fields) if fields.is_empty()) {
        return Err(PortError::new("sse_payload", "invalid done event payload"));
    }
    // Production sends this transport-level marker immediately after the
    // domain-terminal `result` or `error`. It must not become a second
    // terminal event or alter machine-readable output.
    Ok(true)
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
                stage: normalize_optional_text(data.stage, MAX_STREAM_METADATA_BYTES)?,
                message: normalize_optional_text(data.message, MAX_STREAM_METADATA_BYTES)?,
            })
        }
        "progress" => {
            let data: ProgressData = serde_json::from_str(&raw.data).map_err(|_| parse_error())?;
            Ok(AgentEvent::Progress {
                message: normalize_text(data.message, MAX_STREAM_METADATA_BYTES)?,
                current: data.current,
                total: data.total,
            })
        }
        "partial" => {
            let data: PartialData = serde_json::from_str(&raw.data).map_err(|_| parse_error())?;
            Ok(AgentEvent::Partial {
                text: normalize_text(data.text, MAX_SSE_EVENT_BYTES)?,
            })
        }
        "choices" => {
            let data: ChoicesData = serde_json::from_str(&raw.data).map_err(|_| parse_error())?;
            let choices = data
                .choices
                .into_iter()
                .map(|choice| match choice {
                    ChoiceWire::Label(label) => AgentChoice::from_untrusted(label, None),
                    ChoiceWire::Detailed(choice) => Ok(choice),
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| PortError::new("sse_payload", error))?;
            if choices.len() > heyfood_core::agent::MAX_AGENT_CHOICES {
                return Err(PortError::new(
                    "sse_payload",
                    "agent choice count exceeds its limit",
                ));
            }
            Ok(AgentEvent::Choices {
                choices,
                allow_multiple: data.allow_multiple,
            })
        }
        "result" => {
            let mut document: Value = serde_json::from_str(&raw.data).map_err(|_| parse_error())?;
            sanitize_json_strings(&mut document);
            let conversation_id = document
                .get("conversation_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            if conversation_id
                .as_ref()
                .is_some_and(|value| value.is_empty() || value.len() > MAX_CONVERSATION_ID_BYTES)
            {
                return Err(PortError::new("sse_payload", "conversation ID is invalid"));
            }
            Ok(AgentEvent::Result {
                document,
                conversation_id,
            })
        }
        "error" => {
            let data: ErrorData = serde_json::from_str(&raw.data).map_err(|_| parse_error())?;
            let error = data.error.map_or_else(
                || {
                    AgentFailure::from_untrusted(
                        data.code.unwrap_or_else(|| "service_error".into()),
                        data.message.unwrap_or_else(|| "service error".into()),
                        data.retryable,
                    )
                    .map_err(|error| PortError::new("sse_payload", error))
                },
                Ok,
            )?;
            Ok(AgentEvent::Error { error })
        }
        _ => Err(PortError::new(
            "sse_event",
            format!("unsupported event type {}", raw.event_type),
        )),
    }
}

fn normalize_text(value: String, maximum_bytes: usize) -> Result<String, PortError> {
    let value = terminal_safe_text(&value);
    if value.len() > maximum_bytes {
        return Err(PortError::new(
            "sse_payload",
            "agent presentation text exceeds its limit",
        ));
    }
    Ok(value)
}

fn normalize_optional_text(
    value: Option<String>,
    maximum_bytes: usize,
) -> Result<Option<String>, PortError> {
    value
        .map(|value| normalize_text(value, maximum_bytes))
        .transpose()
}

fn sanitize_json_strings(value: &mut Value) {
    match value {
        Value::String(text) => *text = terminal_safe_text(text),
        Value::Array(values) => values.iter_mut().for_each(sanitize_json_strings),
        Value::Object(values) => values.values_mut().for_each(sanitize_json_strings),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_SSE_BUFFERED_BYTES, MAX_SSE_LINE_BYTES, RawEvent, RawSseParser, consume_done_marker,
        normalize,
    };
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

    #[test]
    fn normalized_service_text_cannot_emit_terminal_controls() {
        let partial = normalize(RawEvent {
            event_type: "partial".into(),
            data: r#"{"text":"safe\u001b]52;clipboard\u0007"}"#.into(),
        })
        .unwrap();
        assert_eq!(
            partial,
            AgentEvent::Partial {
                text: "safe]52;clipboard".into()
            }
        );

        let result = normalize(RawEvent {
            event_type: "result".into(),
            data: r#"{"text":"safe\u001b[31m","nested":["\u0007value"]}"#.into(),
        })
        .unwrap();
        let AgentEvent::Result { document, .. } = result else {
            panic!("expected result event");
        };
        assert_eq!(document["text"], "safe[31m");
        assert_eq!(document["nested"][0], "value");
    }

    #[test]
    fn choice_count_is_bounded_before_reaching_the_ui() {
        let choices = (0..=heyfood_core::agent::MAX_AGENT_CHOICES)
            .map(|index| format!("choice-{index}"))
            .collect::<Vec<_>>();
        let event = RawEvent {
            event_type: "choices".into(),
            data: serde_json::json!({"choices": choices}).to_string(),
        };
        assert_eq!(normalize(event).unwrap_err().code, "sse_payload");
    }

    #[test]
    fn empty_done_marker_is_consumed_without_a_domain_event() {
        assert!(
            consume_done_marker(&RawEvent {
                event_type: "done".into(),
                data: "{}".into(),
            })
            .unwrap()
        );
    }

    #[test]
    fn malformed_or_extended_done_markers_fail_closed() {
        for data in ["", "not-json", "null", "[]", r#"{"unexpected":true}"#] {
            let error = consume_done_marker(&RawEvent {
                event_type: "done".into(),
                data: data.into(),
            })
            .unwrap_err();
            assert_eq!(error.code, "sse_payload", "payload: {data}");
        }
    }

    #[test]
    fn unknown_event_type_remains_rejected() {
        let raw = RawEvent {
            event_type: "future_terminal".into(),
            data: "{}".into(),
        };
        assert!(!consume_done_marker(&raw).unwrap());
        let error = normalize(RawEvent {
            event_type: "future_terminal".into(),
            data: "{}".into(),
        })
        .unwrap_err();
        assert_eq!(error.code, "sse_event");
    }

    #[test]
    fn continuous_line_is_rejected_at_a_typed_bound() {
        let mut parser = RawSseParser::default();
        parser.push(&vec![b'x'; MAX_SSE_LINE_BYTES]).unwrap();
        let error = parser.push(b"x").unwrap_err();
        assert_eq!(error.code, "sse_line_too_large");
    }

    #[test]
    fn multiline_event_is_rejected_at_a_typed_bound() {
        let mut parser = RawSseParser::default();
        let first = format!("data: {}\n", "x".repeat(MAX_SSE_LINE_BYTES - 6));
        for _ in 0..16 {
            parser.push(first.as_bytes()).unwrap();
        }
        let overflow = format!("data: {}\n", "y".repeat(100));
        let error = parser.push(overflow.as_bytes()).unwrap_err();
        assert_eq!(error.code, "sse_event_too_large");
    }

    #[test]
    fn aggregate_chunk_is_rejected_before_allocation() {
        let mut parser = RawSseParser::default();
        let error = parser
            .push(&vec![b'x'; MAX_SSE_BUFFERED_BYTES + 1])
            .unwrap_err();
        assert_eq!(error.code, "sse_buffer_too_large");
    }
}
