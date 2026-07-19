//! Renderer-neutral events from `/v1/agent/converse`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentChoice {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentFailure {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
}

/// Normalized SSE vocabulary shared by runtime and presentation surfaces.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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

impl AgentEvent {
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Result { .. } | Self::Error { .. })
    }
}
