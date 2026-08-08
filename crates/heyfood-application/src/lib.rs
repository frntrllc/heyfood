//! UI-independent use cases and outbound port contracts.

#![forbid(unsafe_code)]

pub mod capability;
pub mod diet;
pub mod ensure_session;
pub mod grocery;
pub mod health;
pub mod household_agent_phase0;
pub mod household_context;
pub mod household_evaluation;
pub mod household_menu;
pub mod household_profile_policy;
pub mod household_repository;
pub mod legacy_compatibility;
pub mod logout;
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
pub use diet::{DietPort, ReadDietCatalog, ReadDietDetail};
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
pub use household_agent_phase0::{
    AuthorizedAgentHouseholdPrepareV1, BoundAgentHouseholdDisclosureV1,
    BoundAgentHouseholdOutcomeReceiptV1, BoundAgentHouseholdProposalV1, BoundAgentHouseholdReadV1,
    BoundAgentHouseholdRosterAuthorityV1, FrozenAgentHouseholdDisclosureV1,
    HouseholdAgentPhase0Proof, PreparedAgentHouseholdDisclosureV1,
};
pub use household_context::{
    HouseholdContextErrorV1, HouseholdContextSnapshotV1, HouseholdSubjectContextV1,
    PreparedHouseholdTargetV1, resolve_personalized_context_v1, validate_scope_eligibility_v1,
};
pub use household_evaluation::{
    HouseholdEvaluationPresentationError, UNPRESENTABLE_HOUSEHOLD_EVALUATION_MESSAGE,
    household_evaluation_document, render_household_evaluation,
    render_household_evaluation_at_width,
};
pub use household_menu::{household_menu_document, is_full_household_menu, render_household_menu};
pub use household_profile_policy::{
    AuthoritativeConsentStateV1, HouseholdProfileEligibilityV1, HouseholdProfileIneligibilityV1,
    HouseholdProfileOperationV1, OwnerProfileActionEligibilityV1, OwnerProfileRetryActionV1,
    OwnerProfileRetryEligibilityV1, OwnerProfileRetryUnavailableReasonV1, OwnerSyncIntentHandleV1,
    household_profile_eligibility_v1, owner_profile_action_eligibility_v1,
    validate_d2_profile_policy_v1,
};
pub use household_repository::{
    AuthorizedHostedContextV1, AuthorizedOwnerHostedContextV1, CreateMemberWithDeclaredProfileV1,
    CreatedMemberWithDeclaredProfileV1, HouseholdCommit, HouseholdCommitOutcome, HouseholdErase,
    HouseholdEraseOutcome, HouseholdInitialize, HouseholdLoad, HouseholdOpenOutcomeV1,
    HouseholdReadLeaseV1, HouseholdRepositoryResolutionV1, HouseholdSession,
    NativeMemberAgeEvidenceV1, OpenHouseholdV1, OwnerSyncTransitionEventV1,
    SaveMemberDeclaredProfileV1, SaveOwnerProfileAndSyncIntentV1, SavedMemberDeclaredProfileV1,
    SavedOwnerProfileAndSyncIntentV1, SelectedHouseholdScopeV1, SelectedHouseholdTargetV1,
    SelfOnlyHouseholdInitializationV1, TransitionOwnerSyncIntentV1, resolve_household_commit_v1,
    resolve_household_initialize_v1,
};
pub use legacy_compatibility::{
    NativeHouseholdCompletionModeV1, NativeHouseholdEvidenceV1,
    NativeHouseholdInitializationPhaseV1, NativeHouseholdModeErrorV1, NativeHouseholdModeFactsV1,
    NativeHouseholdModeV1, NativeHouseholdPostLogoutPolicyV1, resolve_native_household_mode_v1,
};
pub use logout::{
    Logout, LogoutLocalPort, LogoutOutcome, LogoutRemotePort, LogoutStep, LogoutTeardown,
};
pub use menu_watch::{
    CreateMenuWatch, CreateMenuWatchRequest, ListMenuWatches, MenuWatchChangeEvent,
    MenuWatchChangeSummary, MenuWatchList, MenuWatchPort, MenuWatchReadPort, MenuWatchSnapshot,
    RemoveMenuWatch,
};
pub use one_shot_turn::{
    MAX_ONE_SHOT_EVENTS, MAX_ONE_SHOT_STREAM_BYTES, OneShotTurnResult,
    UNRENDERABLE_AGENT_RESULT_MESSAGE, agent_result_text, execute_one_shot_turn,
};

pub use ports::{
    AcceptedTurn, AudioCapture, AudioCapturePort, BoxEventStream, BoxFuture, BrowserPort,
    ClipboardPort, ClockPort, ConfigCommit, ConfigMutation, ConfigPort, CredentialCommit,
    CredentialPort, EventStream, HouseholdAgentDisclosureAccessV1,
    HouseholdAgentDisclosureControlPort, HouseholdAgentPhase0Port,
    HouseholdCommitEvidenceRepositoryPort, HouseholdMutationAuthorityPort,
    HouseholdMutationAuthorityV1, HouseholdMutationPurposeV1, HouseholdRepositoryPort, PortError,
    ServicePort,
};
pub use run_turn::{
    MAX_TURN_EVENTS, MAX_TURN_STREAM_BYTES, RefreshPolicy, RunTurn, RunTurnError, RunTurnOutcome,
    TurnContext, TurnEvent, TurnFailure, TurnFailureKind, TurnRequest,
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
