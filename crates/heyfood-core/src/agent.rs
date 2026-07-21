//! Renderer-neutral events from `/v1/agent/converse`.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{terminal_safe_text, validate_identifier};

pub const MAX_AGENT_METADATA_BYTES: usize = 16 * 1024;
pub const MAX_AGENT_CHOICES: usize = 256;

#[derive(Clone, Eq, PartialEq, Serialize)]
pub struct AgentChoice {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

impl AgentChoice {
    pub fn from_untrusted(label: String, value: Option<String>) -> Result<Self, &'static str> {
        let label = terminal_safe_text(&label);
        if label.is_empty() || label.len() > MAX_AGENT_METADATA_BYTES {
            return Err("agent choice label is invalid");
        }
        let value = value
            .map(|value| {
                let value = terminal_safe_text(&value);
                if value.is_empty() || value.len() > MAX_AGENT_METADATA_BYTES {
                    return Err("agent choice value is invalid");
                }
                Ok(value)
            })
            .transpose()?;
        Ok(Self { label, value })
    }
}

impl<'de> Deserialize<'de> for AgentChoice {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawAgentChoice {
            label: String,
            #[serde(default)]
            value: Option<String>,
        }

        let raw = RawAgentChoice::deserialize(deserializer)?;
        Self::from_untrusted(raw.label, raw.value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Debug for AgentChoice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentChoice")
            .field("label", &"[REDACTED]")
            .field("has_value", &self.value.is_some())
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq, Serialize)]
pub struct AgentFailure {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
}

impl AgentFailure {
    pub fn from_untrusted(
        code: String,
        message: String,
        retryable: bool,
    ) -> Result<Self, &'static str> {
        validate_identifier(&code, 64).map_err(|_| "agent failure code is invalid")?;
        let message = terminal_safe_text(&message);
        if message.is_empty() || message.len() > MAX_AGENT_METADATA_BYTES {
            return Err("agent failure message is invalid");
        }
        Ok(Self {
            code,
            message,
            retryable,
        })
    }
}

impl<'de> Deserialize<'de> for AgentFailure {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawAgentFailure {
            code: String,
            message: String,
            #[serde(default)]
            retryable: bool,
        }

        let raw = RawAgentFailure::deserialize(deserializer)?;
        Self::from_untrusted(raw.code, raw.message, raw.retryable).map_err(serde::de::Error::custom)
    }
}

impl fmt::Debug for AgentFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentFailure")
            .field("code", &"[REDACTED]")
            .field("message", &"[REDACTED]")
            .field("retryable", &self.retryable)
            .finish()
    }
}

/// Normalized SSE vocabulary shared by runtime and presentation surfaces.
#[derive(Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AgentEvent {
    Thinking {
        #[serde(skip_serializing_if = "Option::is_none")]
        stage: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    Progress {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        current: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        total: Option<u64>,
    },
    Partial {
        text: String,
    },
    Choices {
        choices: Vec<AgentChoice>,
        #[serde(default)]
        allow_multiple: bool,
    },
    Result {
        document: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        conversation_id: Option<String>,
    },
    Error {
        error: AgentFailure,
    },
}

impl fmt::Debug for AgentEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Thinking { stage, message } => formatter
                .debug_struct("Thinking")
                .field("has_stage", &stage.is_some())
                .field("has_message", &message.is_some())
                .finish(),
            Self::Progress { current, total, .. } => formatter
                .debug_struct("Progress")
                .field("message", &"[REDACTED]")
                .field("has_current", &current.is_some())
                .field("has_total", &total.is_some())
                .finish(),
            Self::Partial { .. } => formatter
                .debug_struct("Partial")
                .field("text", &"[REDACTED]")
                .finish(),
            Self::Choices {
                choices,
                allow_multiple,
            } => formatter
                .debug_struct("Choices")
                .field("choice_count", &choices.len())
                .field("allow_multiple", allow_multiple)
                .finish(),
            Self::Result {
                conversation_id, ..
            } => formatter
                .debug_struct("Result")
                .field("document", &"[REDACTED]")
                .field("has_conversation_id", &conversation_id.is_some())
                .finish(),
            Self::Error { error } => formatter.debug_tuple("Error").field(error).finish(),
        }
    }
}

impl AgentEvent {
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Result { .. } | Self::Error { .. })
    }
}
