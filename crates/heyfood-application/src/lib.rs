//! UI-independent use cases and outbound port contracts.

#![forbid(unsafe_code)]

pub mod capability;
pub mod ensure_session;
pub mod grocery;
pub mod health;
pub mod household_menu;
pub mod menu_watch;
pub mod one_shot_turn;
pub mod ports;
pub mod run_turn;
pub mod state_writer;
pub mod status;
pub mod supervisor;

pub use capability::{
    CapabilityPort, CapabilitySnapshot, DiscoverCapabilities, RegistrationAvailability,
};
pub use ensure_session::{EnsureSession, EnsureSessionError, EnsureSessionOutcome};
pub use grocery::{
    ConfirmGroceryMutation, DeployedGroceryMutationRequest, ExportGroceryList, GroceryCacheKey,
    GroceryDisplayItem, GroceryDisplayList, GroceryDisplayMemberFlag, GroceryDisplaySafety,
    GroceryDisplaySource, GroceryExclusions, GroceryExport, GroceryExportPort,
    GroceryItemReferenceCache, GroceryListSnapshot, GroceryMutationIntent, GroceryMutationPort,
    GroceryPort, GroceryReadPort, PrepareGroceryMutation, PreparedGroceryMutation,
    ReadActiveGroceryDisplay, ReadActiveGroceryList, ReadGroceryExclusions,
};
pub use health::{
    HealthAuthorization, HealthConnection, HealthContext, HealthManagementOutcome, HealthPort,
};
pub use household_menu::render_household_menu;
pub use menu_watch::{
    CreateMenuWatch, CreateMenuWatchRequest, ListMenuWatches, MenuWatchChangeEvent,
    MenuWatchChangeSummary, MenuWatchList, MenuWatchPort, MenuWatchReadPort, MenuWatchSnapshot,
    RemoveMenuWatch,
};
pub use one_shot_turn::{
    MAX_ONE_SHOT_EVENTS, MAX_ONE_SHOT_STREAM_BYTES, OneShotTurnResult, agent_result_text,
    execute_one_shot_turn,
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
pub use status::{
    OptionalCapabilityStatus, ProfileReadinessStatus, ReadStatus, StatusPort, StatusSnapshot,
    VoiceReadinessStatus,
};
pub use supervisor::{OperationSupervisor, SupervisorError, WorkflowLease};

/// The package version shared by the native workspace.
pub const VERSION: &str = heyfood_core::VERSION;
