//! Object-safe outbound ports implemented by runtime and platform adapters.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use heyfood_core::{
    AgentEvent, BrowserUrl, ClientConfig, CommitId, CredentialVersion, OperationId, RefreshOutcome,
    RefreshRequest, SessionCredentials,
};
use tokio_util::sync::CancellationToken;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub type BoxEventStream = Box<dyn EventStream>;

#[derive(Clone, Eq, PartialEq)]
pub struct PortError {
    pub code: &'static str,
    pub message: String,
    pub outcome_uncertain: bool,
}

impl fmt::Debug for PortError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PortError")
            .field("code", &self.code)
            .field("message", &"[REDACTED]")
            .field("outcome_uncertain", &self.outcome_uncertain)
            .finish()
    }
}

impl PortError {
    #[must_use]
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            outcome_uncertain: false,
        }
    }

    #[must_use]
    pub fn uncertain(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            outcome_uncertain: true,
        }
    }
}

impl fmt::Display for PortError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PortError {}

/// Minimal object-safe stream contract; adapters may wrap SSE or fixtures.
pub trait EventStream: Send {
    fn next(&mut self) -> BoxFuture<'_, Result<Option<AgentEvent>, PortError>>;

    /// Close the underlying response/socket and join owned work.
    fn close(self: Box<Self>) -> BoxFuture<'static, Result<(), PortError>>;
}

pub struct AcceptedTurn {
    pub events: BoxEventStream,
}

/// Hosted service boundary. No method implicitly retries an uncertain POST.
pub trait ServicePort: Send + Sync {
    fn refresh_session(
        &self,
        request: RefreshRequest,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<RefreshOutcome, PortError>>;

    fn open_turn(
        &self,
        request: crate::TurnRequest,
        credentials: SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<AcceptedTurn, PortError>>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CredentialCommit {
    pub commit_id: CommitId,
    pub expected_version: CredentialVersion,
    pub credentials: SessionCredentials,
}

pub trait CredentialPort: Send + Sync {
    fn load(&self) -> BoxFuture<'_, Result<Option<SessionCredentials>, PortError>>;

    /// This adapter operation must be bounded, atomic, and idempotent by commit ID.
    fn commit(&self, commit: CredentialCommit) -> BoxFuture<'_, Result<(), PortError>>;

    fn mark_reconciliation_required(
        &self,
        commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>>;

    /// Clear only a marker written for this exact idempotent commit.
    fn clear_reconciliation_required(
        &self,
        commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigMutation {
    Replace(ClientConfig),
    ConversationPointer(Option<String>),
    LocalFirstRecord { kind: String, payload: Vec<u8> },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigCommit {
    pub commit_id: CommitId,
    pub mutation: ConfigMutation,
}

pub trait ConfigPort: Send + Sync {
    fn load(&self) -> BoxFuture<'_, Result<ClientConfig, PortError>>;

    /// This adapter operation must be bounded, atomic, and idempotent by commit ID.
    fn commit(&self, commit: ConfigCommit) -> BoxFuture<'_, Result<(), PortError>>;

    /// Persist an exact-commit repair marker when a durable config outcome is
    /// uncertain or a server-accepted config cannot be written locally.
    fn mark_reconciliation_required(
        &self,
        commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>>;

    /// Clear only the repair marker for this exact idempotent commit.
    fn clear_reconciliation_required(
        &self,
        commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>>;
}

pub trait ClockPort: Send + Sync {
    fn unix_timestamp(&self) -> i64;
}

pub trait BrowserPort: Send + Sync {
    fn open(&self, url: BrowserUrl) -> BoxFuture<'_, Result<(), PortError>>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioCapture {
    pub wav_bytes: Vec<u8>,
    pub sample_rate_hz: u32,
    pub duration_millis: u64,
    pub truncated: bool,
    pub overflowed: bool,
}

pub trait AudioCapturePort: Send + Sync {
    /// Report whether this adapter currently sees a compatible input device.
    /// This must not open a capture stream or request microphone permission.
    fn available(&self) -> bool;

    /// Capture in memory until `stop` requests a completed WAV, the hard
    /// duration limit is reached, or `cancellation` discards the recording.
    fn capture(
        &self,
        stop: CancellationToken,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<AudioCapture, PortError>>;
}

pub trait ClipboardPort: Send + Sync {
    fn read_text(&self, maximum_bytes: usize) -> BoxFuture<'_, Result<Option<String>, PortError>>;

    fn write_text(&self, text: String) -> BoxFuture<'_, Result<(), PortError>>;
}
