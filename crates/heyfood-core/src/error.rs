//! Normalized client failures and stable renderer/exit categories.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::validate_identifier;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    Usage,
    Authentication,
    Authorization,
    Network,
    Service,
    Conflict,
    Safety,
    Storage,
    Cancelled,
    Unsupported,
    Internal,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ErrorCode(String);

impl ErrorCode {
    pub fn parse(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        validate_identifier(&value, 64).map_err(|_| "error code is invalid")?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A client-safe error envelope. The message is explicitly public and must not
/// be constructed from credentials, dietary values, health values, or raw
/// provider responses.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClientError {
    pub code: ErrorCode,
    pub category: ErrorCategory,
    pub public_message: String,
    pub retryable: bool,
    pub outcome_uncertain: bool,
}

impl ClientError {
    pub fn new(
        code: ErrorCode,
        category: ErrorCategory,
        public_message: impl Into<String>,
    ) -> Result<Self, &'static str> {
        let public_message = public_message.into();
        if public_message.is_empty() || public_message.len() > 1_024 {
            return Err("public error message must contain 1 to 1024 bytes");
        }
        if public_message.chars().any(char::is_control) {
            return Err("public error message contains control characters");
        }
        Ok(Self {
            code,
            category,
            public_message,
            retryable: false,
            outcome_uncertain: false,
        })
    }

    #[must_use]
    pub const fn retryable(mut self, value: bool) -> Self {
        self.retryable = value;
        self
    }

    #[must_use]
    pub const fn outcome_uncertain(mut self, value: bool) -> Self {
        self.outcome_uncertain = value;
        self
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.public_message)
    }
}

impl std::error::Error for ClientError {}
