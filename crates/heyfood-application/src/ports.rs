//! Object-safe outbound ports implemented by runtime and platform adapters.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use heyfood_core::{
    AccountId, AgentEvent, BrowserUrl, CanonicalDateV1, CanonicalTimestampV1, ClientConfig,
    CommitId, CredentialVersion, MemberId, OperationId, RefreshOutcome, RefreshRequest,
    SessionCredentials,
};
use tokio_util::sync::CancellationToken;

use crate::household_repository::{
    HouseholdCommit, HouseholdCommitOutcome, HouseholdErase, HouseholdEraseOutcome,
    HouseholdInitialize, HouseholdLoad, HouseholdReadLeaseV1,
};

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

#[derive(Clone, Eq, PartialEq)]
pub struct AudioCapture {
    pub wav_bytes: Vec<u8>,
    pub sample_rate_hz: u32,
    pub duration_millis: u64,
    pub truncated: bool,
    pub overflowed: bool,
}

impl fmt::Debug for AudioCapture {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AudioCapture")
            .field("wav_bytes", &"[REDACTED]")
            .field("byte_length", &self.wav_bytes.len())
            .field("sample_rate_hz", &self.sample_rate_hz)
            .field("duration_millis", &self.duration_millis)
            .field("truncated", &self.truncated)
            .field("overflowed", &self.overflowed)
            .finish()
    }
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

/// Account-bound native household persistence.
///
/// Every method is explicitly cancellable and returns the exact object-safe
/// boxed future shape required by D2. Platform adapters must check
/// cancellation before lock acquisition, between bounded I/O/CAS steps, and
/// before child dispatch while still completing an already-reached atomic
/// commit or kill/reap obligation.
pub trait HouseholdRepositoryPort: Send + Sync {
    fn load<'a>(
        &'a self,
        account: &'a AccountId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<Option<HouseholdLoad>, PortError>>;

    /// Load one committed generation while retaining the adapter's
    /// cross-process lifecycle and vault locks. Hosted consumers keep the
    /// returned lease alive through credential preparation and first network
    /// dispatch so another process cannot change the active scope in between.
    fn acquire_read_lease<'a>(
        &'a self,
        account: &'a AccountId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdReadLeaseV1, PortError>>;

    fn initialize<'a>(
        &'a self,
        command: HouseholdInitialize,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdCommitOutcome, PortError>>;

    fn commit<'a>(
        &'a self,
        command: HouseholdCommit,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdCommitOutcome, PortError>>;

    fn erase_account<'a>(
        &'a self,
        command: HouseholdErase,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdEraseOutcome, PortError>>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HouseholdMutationPurposeV1 {
    CreateMember,
    SaveMemberProfile,
    SelectScope,
}

/// One closed, locally allocated authority bundle for an exact household
/// mutation. `Debug` intentionally exposes neither durable identities nor
/// frozen time.
#[derive(Clone, Eq, PartialEq)]
pub struct HouseholdMutationAuthorityV1 {
    pub commit_id: CommitId,
    pub frozen_commit_timestamp: CanonicalTimestampV1,
    pub frozen_evaluation_date: CanonicalDateV1,
    pub member_id: Option<MemberId>,
}

impl fmt::Debug for HouseholdMutationAuthorityV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdMutationAuthorityV1")
            .field("has_member_id", &self.member_id.is_some())
            .finish_non_exhaustive()
    }
}

/// Synchronous, bounded local authority source. Implementations perform no
/// repository, credential, terminal, or network work.
pub trait HouseholdMutationAuthorityPort: Send + Sync {
    fn allocate(
        &self,
        purpose: HouseholdMutationPurposeV1,
    ) -> Result<HouseholdMutationAuthorityV1, PortError>;
}

#[cfg(test)]
mod tests {
    use super::AudioCapture;

    #[test]
    fn captured_audio_debug_is_redacted() {
        let capture = AudioCapture {
            wav_bytes: b"RIFF-sentinel-sensitive-audio".to_vec(),
            sample_rate_hz: 16_000,
            duration_millis: 250,
            truncated: false,
            overflowed: false,
        };
        let debug = format!("{capture:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(debug.contains("byte_length"));
        assert!(!debug.contains("sentinel"));
        assert!(!debug.contains("82, 73, 70, 70"));
    }
}
