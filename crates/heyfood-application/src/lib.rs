//! UI-independent use cases and outbound port contracts.

#![forbid(unsafe_code)]

pub mod ports;
pub mod run_turn;
pub mod state_writer;

pub use ports::{
    AcceptedTurn, AudioCapture, AudioCapturePort, BoxEventStream, BoxFuture, BrowserPort,
    ClipboardPort, ClockPort, ConfigCommit, ConfigMutation, ConfigPort, CredentialCommit,
    CredentialPort, EventStream, PortError, ServicePort,
};
pub use run_turn::{
    RefreshPolicy, RunTurn, RunTurnError, RunTurnOutcome, TurnContext, TurnEvent, TurnRequest,
};
pub use state_writer::{
    CommitError, CommitOutcome, Mutation, MutationClass, MutationMetadata, MutationProposal,
    OperationSnapshot, SerializedStateWriter,
};

/// The package version shared by the native workspace.
pub const VERSION: &str = heyfood_core::VERSION;
