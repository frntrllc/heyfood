//! UI-independent use cases and outbound port contracts.

#![forbid(unsafe_code)]

pub mod ensure_session;
pub mod grocery;
pub mod health;
pub mod ports;
pub mod run_turn;
pub mod state_writer;
pub mod supervisor;

pub use ensure_session::{EnsureSession, EnsureSessionError, EnsureSessionOutcome};
pub use grocery::{
    GroceryCacheKey, GroceryItemReferenceCache, GroceryListSnapshot, GroceryMutationIntent,
    GroceryPort, PreparedGroceryMutation,
};
pub use health::{
    HealthAuthorization, HealthConnection, HealthContext, HealthManagementOutcome, HealthPort,
};

pub use ports::{
    AcceptedTurn, AudioCapture, AudioCapturePort, BoxEventStream, BoxFuture, BrowserPort,
    ClipboardPort, ClockPort, ConfigCommit, ConfigMutation, ConfigPort, CredentialCommit,
    CredentialPort, EventStream, PortError, ServicePort,
};
pub use run_turn::{
    MAX_TURN_EVENTS, MAX_TURN_STREAM_BYTES, RefreshPolicy, RunTurn, RunTurnError, RunTurnOutcome,
    TurnContext, TurnEvent, TurnRequest,
};
pub use state_writer::{
    CommitError, CommitOutcome, Mutation, MutationClass, MutationMetadata, MutationProposal,
    OperationSnapshot, SerializedStateWriter,
};
pub use supervisor::{OperationSupervisor, SupervisorError, WorkflowLease};

/// The package version shared by the native workspace.
pub const VERSION: &str = heyfood_core::VERSION;
