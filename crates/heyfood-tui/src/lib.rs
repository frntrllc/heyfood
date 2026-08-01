//! Retained terminal presentation for the native heyfood client.
//!
//! The reducer in this crate is deliberately independent from Crossterm and
//! Ratatui. Runtime adapters feed [`RuntimeEvent`] values into it and execute
//! the returned [`Effect`] values; rendering is a read-only projection.

#![forbid(unsafe_code)]

mod input;
mod loop_driver;
mod model;
mod render;
mod terminal;

pub use input::action_from_key;
pub use loop_driver::{TuiError, run_terminal};
pub use model::{
    Action, AppModel, BoundedHouseholdMemberDraftV1, Effect, ExitReason,
    HouseholdAccountBindingDigestV1, HouseholdAgeEvidenceInputV1, HouseholdContextApplyFailureV1,
    HouseholdCounterExhaustedV1, HouseholdManagementFailureV1, HouseholdManagementLoadPurposeV1,
    HouseholdMemberPresentationV1, HouseholdModeGenerationV1, HouseholdMutationFailureV1,
    HouseholdMutationKindV1, HouseholdOperationBindingV1, HouseholdOperationIdV1,
    HouseholdPresentationModeV1, HouseholdPresentationValidationErrorV1,
    HouseholdReducerCorrelationV1, MAX_RENDERED_LINES, MAX_SCROLLBACK_BYTES,
    MAX_SCROLLBACK_ENTRIES, NativeOwnerProfileSaveStatusV1, OnboardingTargetV1, OperationState,
    OwnerProfileActionEligibilityV1, OwnerProfileActionLoadPurposeV1, OwnerProfileRetryActionV1,
    OwnerProfileRetryEligibilityV1, OwnerProfileRetryUnavailableReasonV1, OwnerSyncIntentHandleV1,
    PanelRequest, ProfileActionsLoadedV1, ProfileConsentFailureV1, ProfileConsentFinishedV1,
    ProfileConsentReview, ProfileCopyStateV1, ProfilePresentationModeV1,
    ProfileRetrySyncFinishedV1, RuntimeEvent, SLASH_COMMAND_REGISTRY, Scrollback, SemanticEntry,
    SlashCommandSpec, Speaker, VoiceAvailability, dispatch, slash_suggestions,
};
pub use render::{
    ResponsiveMode, composer_height, household_chrome_copy, household_panel_copy, profile_copy,
    render, responsive_mode,
};
pub use terminal::{
    CrosstermTerminalControl, GuardedError, TerminalControl, TerminalGuard, run_guarded,
};

/// The package version shared by the native workspace.
pub const VERSION: &str = heyfood_core::VERSION;
