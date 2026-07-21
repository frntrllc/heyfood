//! Renderer-neutral, bounded presentation documents.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::validation::without_terminal_controls;

pub const MAX_PRESENTATION_BLOCKS: usize = 256;
pub const MAX_PRESENTATION_TEXT_BYTES: usize = 16 * 1024;
pub const MAX_PRESENTATION_DOCUMENT_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PresentationError {
    EmptyText,
    TextTooLarge,
    TooManyBlocks,
    DocumentTooLarge,
}

impl fmt::Display for PresentationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyText => formatter.write_str("presentation text must not be empty"),
            Self::TextTooLarge => formatter.write_str("presentation text exceeds its limit"),
            Self::TooManyBlocks => formatter.write_str("presentation has too many blocks"),
            Self::DocumentTooLarge => formatter.write_str("presentation exceeds its byte limit"),
        }
    }
}

impl std::error::Error for PresentationError {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PresentationText(String);

impl PresentationText {
    pub fn from_untrusted(value: impl AsRef<str>) -> Result<Self, PresentationError> {
        let value = without_terminal_controls(value.as_ref());
        if value.is_empty() {
            return Err(PresentationError::EmptyText);
        }
        if value.len() > MAX_PRESENTATION_TEXT_BYTES {
            return Err(PresentationError::TextTooLarge);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeLevel {
    Information,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PresentationBlock {
    Heading {
        text: PresentationText,
    },
    Paragraph {
        text: PresentationText,
    },
    KeyValue {
        label: PresentationText,
        value: PresentationText,
    },
    Notice {
        level: NoticeLevel,
        text: PresentationText,
    },
    Progress {
        label: PresentationText,
        current: Option<u64>,
        total: Option<u64>,
    },
    Choices {
        choices: Vec<PresentationText>,
        allow_multiple: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PresentationDocument {
    pub schema_version: u16,
    pub title: Option<PresentationText>,
    pub blocks: Vec<PresentationBlock>,
}

impl PresentationDocument {
    pub fn new(
        title: Option<PresentationText>,
        blocks: Vec<PresentationBlock>,
    ) -> Result<Self, PresentationError> {
        if blocks.len() > MAX_PRESENTATION_BLOCKS {
            return Err(PresentationError::TooManyBlocks);
        }
        let document = Self {
            schema_version: 1,
            title,
            blocks,
        };
        let size = serde_json::to_vec(&document)
            .map_err(|_| PresentationError::DocumentTooLarge)?
            .len();
        if size > MAX_PRESENTATION_DOCUMENT_BYTES {
            return Err(PresentationError::DocumentTooLarge);
        }
        Ok(document)
    }
}
