//! Dependency-light domain and wire contracts for heyfood.

#![forbid(unsafe_code)]

pub mod agent;
pub mod agent_household;
pub mod auth;
pub mod config;
pub mod error;
pub mod grocery;
pub mod health;
pub mod household_canonical;
pub mod household_effect;
pub mod household_evaluation;
pub mod household_state;
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
pub use agent_household::{
    AGENT_HOUSEHOLD_CONTRACT_VERSION, AGENT_HOUSEHOLD_MAX_MEMBERS_PER_PAGE,
    AGENT_HOUSEHOLD_REVIEW_MAXIMUM_WIDTH, AGENT_HOUSEHOLD_REVIEW_MINIMUM_WIDTH,
    AgentDisclosureDataClassV1, AgentDisclosureGrantSetV1, AgentDisclosureGrantStateV1,
    AgentDisclosureGrantSubjectV1, AgentDisclosureGrantV1, AgentDisclosureGrantingAuthorityV1,
    AgentDisclosurePurposeV1, AgentHouseholdChangeFieldV1, AgentHouseholdChangeV1,
    AgentHouseholdConsequenceV1, AgentHouseholdContextInputKindV1, AgentHouseholdContextInputV1,
    AgentHouseholdContractErrorV1, AgentHouseholdMemberInputKindV1, AgentHouseholdMemberInputV1,
    AgentHouseholdMemberProjectionV1, AgentHouseholdNextActionV1, AgentHouseholdOperationV1,
    AgentHouseholdOutcomeReceiptV1, AgentHouseholdPrepareRequestKindV1,
    AgentHouseholdPrepareRequestV1, AgentHouseholdProjectionV1, AgentHouseholdProposalIdV1,
    AgentHouseholdProposalPresentationV1, AgentHouseholdProposalRefInputKindV1,
    AgentHouseholdProposalRefInputV1, AgentHouseholdProposalStateV1,
    AgentHouseholdReadRequestKindV1, AgentHouseholdReadRequestV1, AgentHouseholdReadResultKindV1,
    AgentHouseholdReadSnapshotV1, AgentHouseholdRecoverabilityV1, AgentHouseholdRetryClassV1,
    AgentHouseholdSubjectV1, AgentMinimizedDeclaredProfileV1, AppliedHouseholdCommitProofV1,
    HouseholdCommitEvidenceBindingV1, LocalHouseholdAuthoritySnapshotV1,
    LocalHouseholdFrozenCandidateV1, LocalHouseholdProposalAuthorityV1,
    LocalHouseholdProposalBindingV1, LocalHouseholdProposalCasTokenV1,
    LocalHouseholdProposalJournalV1, UnappliedHouseholdCommitProofV1,
    household_review_safe_lines_v1, household_review_safe_text_v1,
};
pub use auth::{
    AccountId, AuthCapabilities, AuthCredentialBundle, AuthorizationCapability, ChannelCredentials,
    CredentialVersion, GROCERY_READ_SCOPE, GROCERY_WRITE_SCOPE, GroceryScopeAuthority,
    IdentityMethod, ProfileStatus, RefreshOutcome, RefreshRequest, RefreshResult,
    RegistrationStatus, SelfRegistrationCapability, SensitiveString, SessionCredentials,
    SessionSnapshot, negotiate_grocery_scopes,
};
pub use config::{
    CURRENT_CONFIG_SCHEMA, ClientConfig, ConfigRevision, ConfigSchemaVersion,
    NativeHouseholdRolloutV1,
};
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
pub use household_canonical::{
    CANONICAL_BYTES_V1_CONTRACT, CanonicalDigestV1, CanonicalJsonError, CanonicalJsonObjectV1,
    CanonicalJsonValueV1, CompatibilityJsonLimitsV1, MAX_SAFE_IJSON_INTEGER,
    MIN_SAFE_IJSON_INTEGER, canonical_sha256_v1, canonicalize_json_value_v1, decode_lower_hex_32,
    domain_hash_v1, encode_lower_hex, parse_bounded_json_object_v1, parse_bounded_json_v1,
    parse_bounded_typed_json_v1, to_canonical_bytes_v1,
};
pub use household_effect::{
    ExpectedHouseholdStateV1, HOUSEHOLD_EFFECT_FINGERPRINT_CONTRACT,
    HouseholdEffectFingerprintInputV1, HouseholdEffectFingerprintV1, HouseholdEffectV1,
    effect_fingerprint_v1,
};
pub use household_evaluation::{
    AnnotationDisposition, EvaluateMenuItem, EvaluateMenuResponse, EvaluationConfidence,
    EvaluationConsentState, EvaluationContextHash, EvaluationContextHashVersion,
    EvaluationMemberId, EvaluationProfileSource, EvaluationProfileVersion, EvaluationScope,
    HOUSEHOLD_EVALUATION_AGGREGATE_SHA256, HOUSEHOLD_EVALUATION_CONTRACT_SHA256,
    HOUSEHOLD_EVALUATION_CONTRACT_VERSION, HOUSEHOLD_EVALUATION_FIXTURE_SHA256,
    HOUSEHOLD_EVALUATION_SOURCE_COMMIT, HOUSEHOLD_EVALUATION_SOURCE_TREE, HouseholdContext,
    HouseholdEvaluationError, HouseholdMemberRef, HumanLabel, MealAttribution, MemberAnnotation,
    SafetyStatus,
};
pub use household_state::{
    AgeBandV1, AgeEvidenceSourceV1, AgeEvidenceV1, AppliedCommitOutcomeV1, AppliedCommitRecordV1,
    CanonicalDateV1, CanonicalTimestampV1, ConsentVersionV1, DateOfBirthV1,
    DietaryProfileProjectionV1, DisplayName, HOUSEHOLD_PROFILE_DOCUMENT_SCHEMA_VERSION,
    HOUSEHOLD_STATE_SCHEMA_VERSION, HouseholdDeclaredProfileV1, HouseholdLifecycleV1,
    HouseholdMemberV1, HouseholdOutboxId, HouseholdOutboxRecordV1, HouseholdOwnerV1,
    HouseholdProfileDocumentV1, HouseholdProfileOutboxEntryV1, HouseholdProfileRecordV1,
    HouseholdProfileStateV1, HouseholdRevision, HouseholdScope, HouseholdStateError,
    HouseholdStateV1, HouseholdSubjectId, ImportedCompatibilityFieldV1,
    ImportedCompatibilityStateV1, LastDefiniteOwnerSyncErrorV1, LegacyOutboxSourceKindV1,
    LegacyProfileOutboxEntryV1, LegacyPythonSnapshotProvenanceV1, LegacyRemoteProfileReferenceV1,
    LegacySourceIdentityV1, LegacyTimestampDispositionV1, LegacyTimestampProvenanceV1,
    LegacyTimestampRecordV1, MAX_APPLIED_COMMITS, MAX_CANONICAL_VAULT_PLAINTEXT_BYTES,
    MAX_COMPATIBILITY_ARRAY_ENTRIES, MAX_COMPATIBILITY_JSON_DEPTH, MAX_COMPATIBILITY_JSON_NODES,
    MAX_COMPATIBILITY_OBJECT_KEYS, MAX_HOUSEHOLD_MEMBERS, MAX_HOUSEHOLD_OUTBOX_ENTRIES,
    MAX_HOUSEHOLD_PROFILES, MAX_HOUSEHOLD_SUBJECTS, MAX_IMPORTED_COMPATIBILITY_FIELDS,
    MAX_LEGACY_APPLIED_MUTATION_IDS, MAX_LEGACY_REMOTE_PROFILE_REFERENCES,
    MAX_LEGACY_TIMESTAMP_PROVENANCE, MAX_MIGRATION_CANDIDATE_BYTES, MAX_MIGRATION_DISPOSITIONS,
    MAX_OWNER_SYNC_REQUEST_BODY_BYTES, MAX_PROFILE_DOCUMENT_BYTES, MemberId,
    MigrationDispositionKindV1, MigrationDispositionManifestV1, MigrationDispositionV1,
    MigrationProvenanceV1, MinorStatusV1, OWNER_SYNC_OUTBOX_PREFIX, OutboxPhaseV1, OutboxRevision,
    OwnerSyncIntentPhaseV1, OwnerSyncIntentV1, ProfileDocumentProvenanceV1, ProfileRevision,
    RelationshipSourceV1, RelationshipV1, RemoteProfileBaseV1, RemoteProfileExistenceV1,
    classify_legacy_outbox_v1, decode_canonical_household_state_v1, derive_minor_status_v1,
    normalize_legacy_timestamp_v1,
};
pub use menu_watch::{
    MENU_WATCH_SCOPE, MENU_WATCH_SOURCE_COMMIT, MENU_WATCH_SOURCE_SHA256, MenuWatchChangeEventWire,
    MenuWatchChangeSummaryWire, MenuWatchCreateRequestWire, MenuWatchId, MenuWatchListResponseWire,
    MenuWatchResponseWire, RestaurantId, WatchCadenceWire, WatchHour, WatchWeekday,
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
