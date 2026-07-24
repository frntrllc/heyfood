//! Dependency-light domain and wire contracts for heyfood.

#![forbid(unsafe_code)]

pub mod agent;
pub mod auth;
pub mod config;
pub mod error;
pub mod grocery;
pub mod health;
pub mod menu_watch;
pub mod migration;
pub mod network;
pub mod onboarding;
pub mod operation;
pub mod presentation;
pub mod transcription;
pub mod validation;
pub mod wire;

pub use agent::{AgentChoice, AgentEvent, AgentFailure};
pub use auth::{
    AccountId, AuthCapabilities, AuthCredentialBundle, AuthorizationCapability, ChannelCredentials,
    CredentialVersion, GROCERY_READ_SCOPE, GROCERY_WRITE_SCOPE, GroceryScopeAuthority,
    IdentityMethod, ProfileStatus, RefreshOutcome, RefreshRequest, RefreshResult,
    RegistrationStatus, SelfRegistrationCapability, SensitiveString, SessionCredentials,
    SessionSnapshot, negotiate_grocery_scopes,
};
pub use config::{CURRENT_CONFIG_SCHEMA, ClientConfig, ConfigRevision, ConfigSchemaVersion};
pub use error::{ClientError, ErrorCategory, ErrorCode};
pub use grocery::{
    ContextFingerprint, FrozenGroceryPreconditions, GroceryCapability, GroceryConfirmation,
    GroceryConfirmationCommand, GroceryConfirmationDecision, GroceryConfirmationId,
    GroceryConfirmationState, GroceryEditPatch, GroceryEntityId, GroceryError, GroceryErrorCode,
    GroceryIdempotencyKey, GroceryListVersion, GrocerySafetyStatus, HouseholdContextHashVersion,
};
pub use health::{
    HealthCapability, HealthConnectionStatus, HealthFreshness, HealthFreshnessStatus, HealthMetric,
    HealthProvider, HealthTrend, TrendDirection,
};
pub use menu_watch::{
    MENU_WATCH_SCOPE, MENU_WATCH_SOURCE_COMMIT, MENU_WATCH_SOURCE_SHA256,
    MenuWatchCreateRequestWire, MenuWatchId, MenuWatchListResponseWire, MenuWatchResponseWire,
    RestaurantId, WatchCadenceWire, WatchHour, WatchWeekday,
};
pub use migration::{
    ImportedPythonState, PythonFieldAction, PythonFieldDisposition, PythonImportOutcome,
    PythonImportReport,
};
pub use network::{BrowserUrl, NetworkPolicy, ProxyUrl, ServiceUrl, ServiceUrlError};
pub use onboarding::{
    OnboardingOption, OnboardingProfileInput, activity_options, allergy_options, condition_options,
    cuisine_options, diet_options,
};
pub use operation::{CommitId, GenerationId, OperationId};
pub use presentation::{
    NoticeLevel, PresentationBlock, PresentationDocument, PresentationError, PresentationText,
};
pub use transcription::{
    TRANSCRIPTION_CHANNELS, TRANSCRIPTION_CLIENT_ERROR_KINDS, TRANSCRIPTION_MAX_AUDIO_BYTES,
    TRANSCRIPTION_MAX_DURATION_SECONDS, TRANSCRIPTION_MAX_LANGUAGE_CHARACTERS,
    TRANSCRIPTION_MAX_MODEL_VERSION_CHARACTERS, TRANSCRIPTION_MAX_REQUEST_BYTES,
    TRANSCRIPTION_MAX_RESPONSE_DURATION_SECONDS, TRANSCRIPTION_MAX_TRANSCRIPT_CHARACTERS,
    TRANSCRIPTION_PREFERRED_SAMPLE_RATE_HZ, TRANSCRIPTION_SAMPLE_RATE_MAX_HZ,
    TRANSCRIPTION_SAMPLE_RATE_MIN_HZ, TRANSCRIPTION_SAMPLE_WIDTH_BYTES,
    TRANSCRIPTION_SCHEMA_SHA256, TRANSCRIPTION_SCHEMA_VERSION, TRANSCRIPTION_WAV_HEADER_BYTES,
    Transcription, TranscriptionContractError, TranscriptionPurpose, TranscriptionWire,
    transcription_sample_rate_supported,
};
pub use validation::{
    ValidationError, bounded_integer, bounded_number, choice, coordinates, iso_date, optional_text,
    required_text, terminal_safe_text, validate_identifier,
};
pub use wire::{
    ActionConfirmationEnvelopeWire, AddItemsRequestWire, AgentConfirmationCommandWire,
    ApplicationCapabilitiesWire, AuthorizationCapabilityWire, AuthorizationServerMetadataWire,
    ConfirmationDecisionWire, ExclusionListResponseWire, ExclusionMutationRequestWire,
    GROCERY_WIRE_CONTRACT_VERSION, GROCERY_WIRE_SCHEMA_SHA256, GroceryConfirmationToken,
    GroceryDecisionWire, GroceryItemInputWire, GroceryItemStateWire, GroceryItemWire,
    GroceryListWire, GroceryMutationConfirmRequestWire, GroceryMutationOperationWire,
    GroceryMutationProposalWire, GroceryMutationResultWire, GroceryMutationStatusWire,
    HEALTH_H1_H2_SOURCE_COMMIT, HealthContextWire, IdentityMethodWire,
    IntegrationAuthorizeRequestWire, IntegrationAuthorizeResponseWire,
    IntegrationDisconnectResponseWire, IntegrationListWire, IntegrationRedirectTargetWire,
    IntegrationStatusWire, IntegrationSyncResponseWire, ItemSourceWire, MemberFlagWire,
    ProposedItemWire, RemoveItemsRequestWire, SafetyAnnotationWire, SelfRegistrationCapabilityWire,
    SelfRegistrationStatusWire, SuggestedGoalWire, UpdateItemStateRequestWire,
    VersionConflictDetailWire,
};

/// The package version shared by the native workspace.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
