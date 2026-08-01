use std::{
    collections::{HashSet, VecDeque},
    fmt::{self, Write as _},
};

use heyfood_application::household_evaluation::contains_private_household_identifier;
pub use heyfood_application::{
    OwnerProfileActionEligibilityV1, OwnerProfileRetryActionV1, OwnerProfileRetryEligibilityV1,
    OwnerProfileRetryUnavailableReasonV1, OwnerSyncIntentHandleV1,
};
use heyfood_application::{
    RunTurnOutcome, TurnFailure, TurnFailureKind, UNRENDERABLE_AGENT_RESULT_MESSAGE,
    agent_result_text, household_evaluation_document, is_full_household_menu,
    render_household_evaluation, render_household_menu,
};
use heyfood_core::{
    ActionConfirmationEnvelopeWire, AgentConfirmationCommandWire, AgentEvent,
    ConfirmationDecisionWire, ConsentVersionV1, GroceryEditPatch, HouseholdLifecycleV1,
    HouseholdProfileStateV1, HouseholdRevision, HouseholdScope, HouseholdSubjectId,
    OnboardingOption, OnboardingProfileInput, ProfileRevision, RelationshipV1,
    TRANSCRIPTION_MAX_TRANSCRIPT_CHARACTERS, activity_options, allergy_options, condition_options,
    cuisine_options, diet_options, required_text, terminal_safe_text,
};

pub const MAX_SCROLLBACK_ENTRIES: usize = 1_000;
pub const MAX_RENDERED_LINES: usize = 20_000;
pub const MAX_SCROLLBACK_BYTES: usize = 4 * 1024 * 1024;
const TRUNCATION_NOTICE: &str = "[… earlier content truncated …]\n";
const MAX_PROMPT_HISTORY: usize = 100;
const MAX_CONFIRMATION_SOURCES_PER_ITEM: usize = 8;
const UNPRESENTABLE_AGENT_CHOICES_MESSAGE: &str = "hey.food returned choices this version can’t display safely. Ask the question again without selecting one of these options.";

const PROFILE_USAGE: &str = "/profile | /profile consent | /profile retry-sync";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileCopyStateV1 {
    OnboardingSaveReview,
    OnboardingSaveCancelled,
    SavedWithAbsentConsent,
    ConsentReview,
    ConsentReviewPrompt,
    ConsentCancelled,
    ConsentGranted { consent_version: ConsentVersionV1 },
    RetryOffered { consent_version: ConsentVersionV1 },
    InterruptedRetry,
    ConsentVersionChanged,
    ConsentRevoked,
    SyncPending,
    RetryUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeOwnerProfileSaveStatusV1 {
    SavedWithAbsentConsent,
    SyncPending,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfilePresentationModeV1 {
    LegacyCompatibility,
    NativeEnabled,
    NativeRollbackReadOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileConsentReview {
    Reviewing,
    Granting { operation_id: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProfileConsentFinishedV1 {
    pub consent_version: ConsentVersionV1,
    pub retry_offered: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileConsentFailureV1 {
    Cancelled,
    Unavailable,
    Uncertain,
    MalformedResponse,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileRetrySyncFinishedV1 {
    SyncPending,
    Interrupted,
    ConsentVersionChangedRequiresNewSave,
    ConsentRevokedRegrantRequired,
    Unavailable {
        reason: OwnerProfileRetryUnavailableReasonV1,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerProfileActionLoadPurposeV1 {
    View,
    ExplicitRetry,
}

#[derive(Clone, Eq, PartialEq)]
pub enum ProfileActionsLoadedV1 {
    /// Byte-compatible body produced by the released compatibility panel.
    LegacyPanel { body: String },
    /// Native, content-free owner action state.
    NativeActions(OwnerProfileActionEligibilityV1),
}

impl fmt::Debug for ProfileActionsLoadedV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::LegacyPanel { .. } => "ProfileActionsLoadedV1::LegacyPanel([REDACTED])",
            Self::NativeActions(_) => "ProfileActionsLoadedV1::NativeActions([REDACTED])",
        })
    }
}

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HouseholdOperationIdV1(u64);

impl HouseholdOperationIdV1 {
    pub fn new(value: u64) -> Result<Self, HouseholdCounterExhaustedV1> {
        (value != 0)
            .then_some(Self(value))
            .ok_or(HouseholdCounterExhaustedV1)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn checked_next(self) -> Result<Self, HouseholdCounterExhaustedV1> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(HouseholdCounterExhaustedV1)
    }
}

impl fmt::Debug for HouseholdOperationIdV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("HouseholdOperationIdV1")
            .field(&self.0)
            .finish()
    }
}

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HouseholdModeGenerationV1(u64);

impl HouseholdModeGenerationV1 {
    pub fn new(value: u64) -> Result<Self, HouseholdCounterExhaustedV1> {
        (value != 0)
            .then_some(Self(value))
            .ok_or(HouseholdCounterExhaustedV1)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn checked_next(self) -> Result<Self, HouseholdCounterExhaustedV1> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(HouseholdCounterExhaustedV1)
    }
}

impl fmt::Debug for HouseholdModeGenerationV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("HouseholdModeGenerationV1")
            .field(&self.0)
            .finish()
    }
}

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HouseholdReducerCorrelationV1(u64);

impl HouseholdReducerCorrelationV1 {
    pub fn new(value: u64) -> Result<Self, HouseholdCounterExhaustedV1> {
        (value != 0)
            .then_some(Self(value))
            .ok_or(HouseholdCounterExhaustedV1)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn checked_next(self) -> Result<Self, HouseholdCounterExhaustedV1> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(HouseholdCounterExhaustedV1)
    }
}

impl fmt::Debug for HouseholdReducerCorrelationV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HouseholdReducerCorrelationV1([REDACTED])")
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct HouseholdAccountBindingDigestV1([u8; 32]);

impl HouseholdAccountBindingDigestV1 {
    #[must_use]
    pub const fn from_bytes(value: [u8; 32]) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for HouseholdAccountBindingDigestV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HouseholdAccountBindingDigestV1([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HouseholdCounterExhaustedV1;

impl fmt::Display for HouseholdCounterExhaustedV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("household reducer counter exhausted")
    }
}

impl std::error::Error for HouseholdCounterExhaustedV1 {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HouseholdPresentationModeV1 {
    NativeEnabled,
    NativeRollbackReadOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HouseholdManagementLoadPurposeV1 {
    Bootstrap,
    Panel,
    AddMember,
    OnboardMember,
    SelectScope,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HouseholdMutationKindV1 {
    CreateMember,
    SaveMemberProfile,
    SelectScope,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HouseholdManagementFailureV1 {
    AccountChanged,
    ModeChanged,
    StateChanged,
    Unavailable,
    MalformedPresentation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HouseholdMutationFailureV1 {
    BeforeCommitCancelled,
    StaleRevision,
    Ineligible,
    ConflictResolutionRequired,
    OutcomeUncertain,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HouseholdContextApplyFailureV1 {
    AccountChanged,
    ModeChanged,
    StateChanged,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HouseholdPresentationValidationErrorV1 {
    InvalidLabel,
    InvalidSubject,
    InvalidRelationship,
    InvalidLifecycle,
    InvalidProfileState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HouseholdAgeEvidenceInputV1 {
    Under13,
    Age13To17,
    Age18Plus,
    Unknown,
}

#[derive(Clone, Eq, PartialEq)]
pub struct HouseholdMemberPresentationV1 {
    subject: HouseholdSubjectId,
    display_label: String,
    relationship: RelationshipV1,
    lifecycle: HouseholdLifecycleV1,
    profile_readiness: HouseholdProfileStateV1,
    profile_revision: Option<ProfileRevision>,
}

impl HouseholdMemberPresentationV1 {
    pub fn new(
        subject: HouseholdSubjectId,
        display_label: impl Into<String>,
        relationship: RelationshipV1,
        lifecycle: HouseholdLifecycleV1,
        profile_readiness: HouseholdProfileStateV1,
        profile_revision: Option<ProfileRevision>,
    ) -> Result<Self, HouseholdPresentationValidationErrorV1> {
        let display_label = display_label.into();
        let bounded = required_text(&display_label, 80)
            .map_err(|_| HouseholdPresentationValidationErrorV1::InvalidLabel)?;
        if display_label != bounded {
            return Err(HouseholdPresentationValidationErrorV1::InvalidLabel);
        }
        match &subject {
            HouseholdSubjectId::Self_
                if relationship != RelationshipV1::Self_
                    || lifecycle != HouseholdLifecycleV1::Active =>
            {
                return Err(HouseholdPresentationValidationErrorV1::InvalidRelationship);
            }
            HouseholdSubjectId::Member(_) if relationship == RelationshipV1::Self_ => {
                return Err(HouseholdPresentationValidationErrorV1::InvalidRelationship);
            }
            HouseholdSubjectId::Self_ => {}
            HouseholdSubjectId::Member(_) => {}
        }
        if matches!(subject, HouseholdSubjectId::Member(_))
            && matches!(
                profile_readiness,
                HouseholdProfileStateV1::PendingSync | HouseholdProfileStateV1::Synced
            )
        {
            return Err(HouseholdPresentationValidationErrorV1::InvalidProfileState);
        }
        Ok(Self {
            subject,
            display_label,
            relationship,
            lifecycle,
            profile_readiness,
            profile_revision,
        })
    }

    #[must_use]
    pub const fn subject(&self) -> &HouseholdSubjectId {
        &self.subject
    }

    #[must_use]
    pub fn display_label(&self) -> &str {
        &self.display_label
    }

    #[must_use]
    pub const fn relationship(&self) -> RelationshipV1 {
        self.relationship
    }

    #[must_use]
    pub const fn lifecycle(&self) -> HouseholdLifecycleV1 {
        self.lifecycle
    }

    #[must_use]
    pub const fn profile_readiness(&self) -> HouseholdProfileStateV1 {
        self.profile_readiness
    }

    #[must_use]
    pub const fn profile_revision(&self) -> Option<ProfileRevision> {
        self.profile_revision
    }
}

impl fmt::Debug for HouseholdMemberPresentationV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdMemberPresentationV1")
            .field(
                "subject_kind",
                &match self.subject {
                    HouseholdSubjectId::Self_ => "self",
                    HouseholdSubjectId::Member(_) => "member",
                },
            )
            .field("lifecycle", &self.lifecycle)
            .field("profile_readiness", &self.profile_readiness)
            .field(
                "profile_revision",
                &self.profile_revision.map(ProfileRevision::get),
            )
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct BoundedHouseholdMemberDraftV1 {
    display_name: String,
    relationship: RelationshipV1,
    age_evidence: HouseholdAgeEvidenceInputV1,
}

impl BoundedHouseholdMemberDraftV1 {
    pub fn new(
        display_name: impl Into<String>,
        relationship: RelationshipV1,
        age_evidence: HouseholdAgeEvidenceInputV1,
    ) -> Result<Self, HouseholdPresentationValidationErrorV1> {
        let display_name = display_name.into();
        let bounded = required_text(&display_name, 80)
            .map_err(|_| HouseholdPresentationValidationErrorV1::InvalidLabel)?;
        if display_name != bounded {
            return Err(HouseholdPresentationValidationErrorV1::InvalidLabel);
        }
        if relationship == RelationshipV1::Self_ {
            return Err(HouseholdPresentationValidationErrorV1::InvalidRelationship);
        }
        Ok(Self {
            display_name,
            relationship,
            age_evidence,
        })
    }

    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    #[must_use]
    pub const fn relationship(&self) -> RelationshipV1 {
        self.relationship
    }

    #[must_use]
    pub const fn age_evidence(&self) -> HouseholdAgeEvidenceInputV1 {
        self.age_evidence
    }
}

impl fmt::Debug for BoundedHouseholdMemberDraftV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BoundedHouseholdMemberDraftV1([REDACTED])")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct HouseholdOperationBindingV1 {
    operation_id: HouseholdOperationIdV1,
    session_mode_generation: HouseholdModeGenerationV1,
    account_binding_digest: HouseholdAccountBindingDigestV1,
    expected_household_revision: HouseholdRevision,
    reducer_correlation: HouseholdReducerCorrelationV1,
}

impl HouseholdOperationBindingV1 {
    #[must_use]
    pub const fn new(
        operation_id: HouseholdOperationIdV1,
        session_mode_generation: HouseholdModeGenerationV1,
        account_binding_digest: HouseholdAccountBindingDigestV1,
        expected_household_revision: HouseholdRevision,
        reducer_correlation: HouseholdReducerCorrelationV1,
    ) -> Self {
        Self {
            operation_id,
            session_mode_generation,
            account_binding_digest,
            expected_household_revision,
            reducer_correlation,
        }
    }

    #[must_use]
    pub const fn operation_id(&self) -> HouseholdOperationIdV1 {
        self.operation_id
    }

    #[must_use]
    pub const fn session_mode_generation(&self) -> HouseholdModeGenerationV1 {
        self.session_mode_generation
    }

    #[must_use]
    pub const fn account_binding_digest(&self) -> HouseholdAccountBindingDigestV1 {
        self.account_binding_digest
    }

    #[must_use]
    pub const fn expected_household_revision(&self) -> HouseholdRevision {
        self.expected_household_revision
    }

    #[must_use]
    pub const fn reducer_correlation(&self) -> HouseholdReducerCorrelationV1 {
        self.reducer_correlation
    }
}

impl fmt::Debug for HouseholdOperationBindingV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdOperationBindingV1")
            .field("operation_id", &self.operation_id)
            .field("session_mode_generation", &self.session_mode_generation)
            .field(
                "expected_household_revision",
                &self.expected_household_revision.get(),
            )
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub enum OnboardingTargetV1 {
    Owner,
    ExistingMember {
        member_id: HouseholdSubjectId,
        expected_household_revision: HouseholdRevision,
        expected_profile_revision: Option<ProfileRevision>,
        display_label: String,
    },
    NewMember {
        bounded_draft: Option<BoundedHouseholdMemberDraftV1>,
        expected_household_revision: HouseholdRevision,
        reducer_correlation: HouseholdReducerCorrelationV1,
    },
}

impl fmt::Debug for OnboardingTargetV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Owner => formatter.write_str("OnboardingTargetV1::Owner"),
            Self::ExistingMember {
                expected_household_revision,
                expected_profile_revision,
                ..
            } => formatter
                .debug_struct("OnboardingTargetV1::ExistingMember")
                .field(
                    "expected_household_revision",
                    &expected_household_revision.get(),
                )
                .field(
                    "expected_profile_revision",
                    &expected_profile_revision.map(ProfileRevision::get),
                )
                .finish_non_exhaustive(),
            Self::NewMember {
                bounded_draft,
                expected_household_revision,
                ..
            } => formatter
                .debug_struct("OnboardingTargetV1::NewMember")
                .field("has_bounded_draft", &bounded_draft.is_some())
                .field(
                    "expected_household_revision",
                    &expected_household_revision.get(),
                )
                .finish_non_exhaustive(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SlashCommandKind {
    Help,
    New,
    Grocery,
    Watch,
    Household,
    For,
    Profile,
    Onboard,
    Location,
    Voice,
    Status,
    Clear,
    Exit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanelRequest {
    Status,
    Grocery,
    Watch,
    Health,
    Household,
    Profile,
    Location,
}

impl PanelRequest {
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Status => "Status",
            Self::Grocery => "Grocery",
            Self::Watch => "Menu Watch",
            Self::Health => "Health",
            Self::Household => "Household",
            Self::Profile => "Dietary profile",
            Self::Location => "Location",
        }
    }

    #[must_use]
    pub const fn command(self) -> &'static str {
        match self {
            Self::Status => "/status",
            Self::Grocery => "/grocery",
            Self::Watch => "/watch",
            Self::Health => "/health",
            Self::Household => "/household",
            Self::Profile => "/profile",
            Self::Location => "/location",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SlashCommandSpec {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub usage: &'static str,
    pub description: &'static str,
    kind: SlashCommandKind,
}

pub const SLASH_COMMAND_REGISTRY: &[SlashCommandSpec] = &[
    SlashCommandSpec {
        name: "/help",
        aliases: &["/?"],
        usage: "/help",
        description: "Show commands and keyboard help",
        kind: SlashCommandKind::Help,
    },
    SlashCommandSpec {
        name: "/new",
        aliases: &[],
        usage: "/new",
        description: "Start a fresh conversation",
        kind: SlashCommandKind::New,
    },
    SlashCommandSpec {
        name: "/grocery",
        aliases: &[],
        usage: "/grocery",
        description: "Open the screened active Grocery list",
        kind: SlashCommandKind::Grocery,
    },
    SlashCommandSpec {
        name: "/watch",
        aliases: &[],
        usage: "/watch",
        description: "Open recurring Menu Watch subscriptions",
        kind: SlashCommandKind::Watch,
    },
    SlashCommandSpec {
        name: "/household",
        aliases: &[],
        usage: "/household [add]",
        description: "Open or add to the native household",
        kind: SlashCommandKind::Household,
    },
    SlashCommandSpec {
        name: "/for",
        aliases: &[],
        usage: "/for me|MEMBER|everyone",
        description: "Target future turns to a household scope",
        kind: SlashCommandKind::For,
    },
    SlashCommandSpec {
        name: "/profile",
        aliases: &[],
        usage: "/profile",
        description: "Open dietary profile readiness",
        kind: SlashCommandKind::Profile,
    },
    SlashCommandSpec {
        name: "/onboard",
        aliases: &[],
        usage: "/onboard [--for MEMBER]",
        description: "Build a declared dietary profile",
        kind: SlashCommandKind::Onboard,
    },
    SlashCommandSpec {
        name: "/location",
        aliases: &[],
        usage: "/location",
        description: "Open active location context",
        kind: SlashCommandKind::Location,
    },
    SlashCommandSpec {
        name: "/voice",
        aliases: &[],
        usage: "/voice",
        description: "Start or stop native microphone capture",
        kind: SlashCommandKind::Voice,
    },
    SlashCommandSpec {
        name: "/status",
        aliases: &[],
        usage: "/status",
        description: "Show session readiness",
        kind: SlashCommandKind::Status,
    },
    SlashCommandSpec {
        name: "/clear",
        aliases: &[],
        usage: "/clear",
        description: "Clear visible scrollback",
        kind: SlashCommandKind::Clear,
    },
    SlashCommandSpec {
        name: "/exit",
        aliases: &["/quit"],
        usage: "/exit",
        description: "Close hey.food",
        kind: SlashCommandKind::Exit,
    },
];

#[must_use]
pub fn slash_suggestions(model: &AppModel, limit: usize) -> Vec<&'static SlashCommandSpec> {
    let query = model.draft.trim();
    if !query.starts_with('/') || query.contains(char::is_whitespace) {
        return Vec::new();
    }
    SLASH_COMMAND_REGISTRY
        .iter()
        .filter(|spec| {
            spec.name.starts_with(query)
                || spec.aliases.iter().any(|alias| alias.starts_with(query))
        })
        .take(limit)
        .collect()
}

fn resolve_slash_command(name: &str) -> Option<&'static SlashCommandSpec> {
    SLASH_COMMAND_REGISTRY
        .iter()
        .find(|spec| spec.name == name || spec.aliases.contains(&name))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Speaker {
    User,
    Assistant,
    Notice,
}

#[derive(Clone, Eq, PartialEq)]
pub struct SemanticEntry {
    pub speaker: Speaker,
    pub text: String,
    pub streaming: bool,
}

impl SemanticEntry {
    fn line_count(&self) -> usize {
        self.text.lines().count().max(1)
    }
}

impl fmt::Debug for SemanticEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SemanticEntry")
            .field("speaker", &self.speaker)
            .field("text_bytes", &self.text.len())
            .field("streaming", &self.streaming)
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct Scrollback {
    entries: VecDeque<SemanticEntry>,
    rendered_lines: usize,
    rendered_bytes: usize,
    maximum_entries: usize,
    maximum_lines: usize,
    maximum_bytes: usize,
}

impl fmt::Debug for Scrollback {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Scrollback")
            .field("entry_count", &self.entries.len())
            .field("rendered_lines", &self.rendered_lines)
            .field("rendered_bytes", &self.rendered_bytes)
            .field("maximum_entries", &self.maximum_entries)
            .field("maximum_lines", &self.maximum_lines)
            .field("maximum_bytes", &self.maximum_bytes)
            .finish()
    }
}

impl Default for Scrollback {
    fn default() -> Self {
        Self::bounded(
            MAX_SCROLLBACK_ENTRIES,
            MAX_RENDERED_LINES,
            MAX_SCROLLBACK_BYTES,
        )
    }
}

impl Scrollback {
    #[must_use]
    pub fn bounded(maximum_entries: usize, maximum_lines: usize, maximum_bytes: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            rendered_lines: 0,
            rendered_bytes: 0,
            maximum_entries: maximum_entries.max(1),
            maximum_lines: maximum_lines.max(1),
            maximum_bytes: maximum_bytes.max(1),
        }
    }

    pub fn push(&mut self, entry: SemanticEntry) {
        self.rendered_lines = self.rendered_lines.saturating_add(entry.line_count());
        self.rendered_bytes = self.rendered_bytes.saturating_add(entry.text.len());
        self.entries.push_back(entry);
        self.enforce_bounds();
    }

    #[must_use]
    pub fn entries(&self) -> &VecDeque<SemanticEntry> {
        &self.entries
    }

    #[must_use]
    pub const fn rendered_lines(&self) -> usize {
        self.rendered_lines
    }

    #[must_use]
    pub const fn rendered_bytes(&self) -> usize {
        self.rendered_bytes
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.rendered_lines = 0;
        self.rendered_bytes = 0;
    }

    fn mutate_last_assistant(&mut self, mutate: impl FnOnce(&mut SemanticEntry)) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .rev()
            .find(|entry| entry.speaker == Speaker::Assistant && entry.streaming)
        {
            let before = entry.line_count();
            let before_bytes = entry.text.len();
            mutate(entry);
            let after = entry.line_count();
            let after_bytes = entry.text.len();
            self.rendered_lines = self
                .rendered_lines
                .saturating_sub(before)
                .saturating_add(after);
            self.rendered_bytes = self
                .rendered_bytes
                .saturating_sub(before_bytes)
                .saturating_add(after_bytes);
        }
        self.enforce_bounds();
    }

    fn enforce_bounds(&mut self) {
        while self.entries.len() > self.maximum_entries
            || (self.rendered_lines > self.maximum_lines && self.entries.len() > 1)
            || (self.rendered_bytes > self.maximum_bytes && self.entries.len() > 1)
        {
            if let Some(removed) = self.entries.pop_front() {
                self.rendered_lines = self.rendered_lines.saturating_sub(removed.line_count());
                self.rendered_bytes = self.rendered_bytes.saturating_sub(removed.text.len());
            }
        }
        if let Some(entry) = self.entries.back_mut() {
            if self.rendered_lines > self.maximum_lines {
                let mut retained = entry
                    .text
                    .lines()
                    .rev()
                    .take(self.maximum_lines)
                    .collect::<Vec<_>>();
                retained.reverse();
                entry.text = retained.join("\n");
            }
            retain_utf8_tail(&mut entry.text, self.maximum_bytes);
            self.rendered_lines = self.entries.iter().map(SemanticEntry::line_count).sum();
            self.rendered_bytes = self.entries.iter().map(|entry| entry.text.len()).sum();
        }
    }
}

fn retain_utf8_tail(text: &mut String, maximum_bytes: usize) {
    if text.len() <= maximum_bytes {
        return;
    }
    if maximum_bytes <= TRUNCATION_NOTICE.len() {
        let mut end = maximum_bytes;
        while !text.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        text.truncate(end);
        return;
    }
    let tail_bytes = maximum_bytes - TRUNCATION_NOTICE.len();
    let mut start = text.len().saturating_sub(tail_bytes);
    while !text.is_char_boundary(start) {
        start = start.saturating_add(1);
    }
    let tail = text[start..].to_owned();
    text.clear();
    text.push_str(TRUNCATION_NOTICE);
    text.push_str(&tail);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExitReason {
    Requested,
    Interrupt,
    Terminate,
    Hangup,
}

impl ExitReason {
    #[must_use]
    pub const fn exit_code(self) -> i32 {
        match self {
            Self::Requested => 0,
            Self::Interrupt => 130,
            Self::Terminate => 143,
            Self::Hangup => 129,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationState {
    Idle,
    Running(u64),
    Cancelling(u64),
    Finishing(u64),
    Exiting(ExitReason),
}

impl OperationState {
    #[must_use]
    pub const fn operation_id(self) -> Option<u64> {
        match self {
            Self::Running(id) | Self::Cancelling(id) | Self::Finishing(id) => Some(id),
            Self::Idle | Self::Exiting(_) => None,
        }
    }

    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(
            self,
            Self::Running(_) | Self::Cancelling(_) | Self::Finishing(_)
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OnboardingStep {
    MemberRelationship,
    MemberName,
    MemberAgeEvidence,
    Diets,
    Allergies,
    Conditions,
    Severity,
    AvoidIngredients,
    Activity,
    Cuisines,
    Notes,
    Review,
    Saving,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VoiceAvailability {
    Unavailable,
    AuthorizationRequired,
    Ready,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VoicePhase {
    Idle,
    Recording { operation_id: u64 },
    Transcribing { operation_id: u64 },
    Review,
}

#[derive(Clone, Eq, PartialEq)]
struct OnboardingFlow {
    step: OnboardingStep,
    profile: OnboardingProfileInput,
    copy_mode: OnboardingCopyMode,
    target: OnboardingTargetV1,
    member_relationship: Option<RelationshipV1>,
    member_name: Option<String>,
    member_age_evidence: Option<HouseholdAgeEvidenceInputV1>,
    household_correlation: Option<HouseholdReducerCorrelationV1>,
}

impl fmt::Debug for OnboardingFlow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OnboardingFlow")
            .field("step", &self.step)
            .field("target_kind", &self.target_kind())
            .field("profile", &self.profile)
            .finish()
    }
}

impl OnboardingFlow {
    fn target_kind(&self) -> &'static str {
        match self.target {
            OnboardingTargetV1::Owner => "owner",
            OnboardingTargetV1::ExistingMember { .. } => "existing_member",
            OnboardingTargetV1::NewMember { .. } => "new_member",
        }
    }

    fn display_label(&self) -> Option<&str> {
        match &self.target {
            OnboardingTargetV1::Owner => Some("Me"),
            OnboardingTargetV1::ExistingMember { display_label, .. } => Some(display_label),
            OnboardingTargetV1::NewMember {
                bounded_draft: Some(draft),
                ..
            } => Some(draft.display_name()),
            OnboardingTargetV1::NewMember {
                bounded_draft: None,
                ..
            } => None,
        }
    }
}

struct MultiSelection {
    ids: Vec<String>,
    custom: Vec<String>,
}

#[derive(Clone, Eq, PartialEq)]
struct PendingActionConfirmation {
    confirmation_id: heyfood_core::GroceryConfirmationId,
    idempotency_key: heyfood_core::GroceryIdempotencyKey,
    editable_items: Option<Vec<serde_json::Map<String, serde_json::Value>>>,
}

impl fmt::Debug for PendingActionConfirmation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingActionConfirmation")
            .field("has_editable_items", &self.editable_items.is_some())
            .finish_non_exhaustive()
    }
}

impl PendingActionConfirmation {
    fn command(
        &self,
        decision: ConfirmationDecisionWire,
        edits: Option<GroceryEditPatch>,
    ) -> AgentConfirmationCommandWire {
        AgentConfirmationCommandWire {
            confirmation_id: self.confirmation_id,
            idempotency_key: self.idempotency_key,
            decision,
            edits,
        }
    }
}

impl Default for OnboardingFlow {
    fn default() -> Self {
        Self {
            step: OnboardingStep::Diets,
            profile: OnboardingProfileInput::default(),
            copy_mode: OnboardingCopyMode::LegacyCompatibility,
            target: OnboardingTargetV1::Owner,
            member_relationship: None,
            member_name: None,
            member_age_evidence: None,
            household_correlation: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OnboardingCopyMode {
    LegacyCompatibility,
    NativeLocalFirst,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingProfileActionV1 {
    Loading {
        operation_id: u64,
        purpose: OwnerProfileActionLoadPurposeV1,
        mode: ProfilePresentationModeV1,
    },
    Retrying {
        operation_id: u64,
        mode: ProfilePresentationModeV1,
    },
}

#[derive(Clone, Eq, PartialEq)]
struct HouseholdManagementSnapshotV1 {
    household_revision: HouseholdRevision,
    active_scope: HouseholdScope,
    members: Vec<HouseholdMemberPresentationV1>,
}

impl fmt::Debug for HouseholdManagementSnapshotV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdManagementSnapshotV1")
            .field("household_revision", &self.household_revision.get())
            .field(
                "active_scope_kind",
                &household_scope_kind(&self.active_scope),
            )
            .field("member_count", &self.members.len())
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
enum HouseholdLoadIntentV1 {
    Bootstrap,
    Panel,
    AddMember,
    OnboardMember { selector: String },
    SelectScope { selector: HouseholdSelectorV1 },
}

impl fmt::Debug for HouseholdLoadIntentV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Bootstrap => "HouseholdLoadIntentV1::Bootstrap",
            Self::Panel => "HouseholdLoadIntentV1::Panel",
            Self::AddMember => "HouseholdLoadIntentV1::AddMember",
            Self::OnboardMember { .. } => "HouseholdLoadIntentV1::OnboardMember([REDACTED])",
            Self::SelectScope { selector } => match selector {
                HouseholdSelectorV1::Me => "HouseholdLoadIntentV1::SelectScope(Me)",
                HouseholdSelectorV1::Everyone => "HouseholdLoadIntentV1::SelectScope(Everyone)",
                HouseholdSelectorV1::Member(_) => {
                    "HouseholdLoadIntentV1::SelectScope(Member([REDACTED]))"
                }
            },
        })
    }
}

impl HouseholdLoadIntentV1 {
    const fn purpose(&self) -> HouseholdManagementLoadPurposeV1 {
        match self {
            Self::Bootstrap => HouseholdManagementLoadPurposeV1::Bootstrap,
            Self::Panel => HouseholdManagementLoadPurposeV1::Panel,
            Self::AddMember => HouseholdManagementLoadPurposeV1::AddMember,
            Self::OnboardMember { .. } => HouseholdManagementLoadPurposeV1::OnboardMember,
            Self::SelectScope { .. } => HouseholdManagementLoadPurposeV1::SelectScope,
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
enum HouseholdSelectorV1 {
    Me,
    Everyone,
    Member(String),
}

#[derive(Clone, Eq, PartialEq)]
struct PendingHouseholdLoadV1 {
    operation_id: HouseholdOperationIdV1,
    session_mode_generation: HouseholdModeGenerationV1,
    expected_account_binding_digest: HouseholdAccountBindingDigestV1,
    reducer_correlation: HouseholdReducerCorrelationV1,
    intent: HouseholdLoadIntentV1,
    cancel_requested: bool,
}

struct HouseholdManagementLoadedInputV1 {
    operation_id: HouseholdOperationIdV1,
    session_mode_generation: HouseholdModeGenerationV1,
    reducer_correlation: HouseholdReducerCorrelationV1,
    purpose: HouseholdManagementLoadPurposeV1,
    account_binding_digest: HouseholdAccountBindingDigestV1,
    household_revision: HouseholdRevision,
    active_scope: HouseholdScope,
    members: Vec<HouseholdMemberPresentationV1>,
}

struct HouseholdManagementFailedInputV1 {
    operation_id: HouseholdOperationIdV1,
    session_mode_generation: HouseholdModeGenerationV1,
    reducer_correlation: HouseholdReducerCorrelationV1,
    purpose: HouseholdManagementLoadPurposeV1,
    account_binding_digest: HouseholdAccountBindingDigestV1,
    observed_household_revision: Option<HouseholdRevision>,
    reason: HouseholdManagementFailureV1,
}

impl fmt::Debug for PendingHouseholdLoadV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingHouseholdLoadV1")
            .field("operation_id", &self.operation_id)
            .field("session_mode_generation", &self.session_mode_generation)
            .field("purpose", &self.intent.purpose())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingHouseholdMutationPhaseV1 {
    Dispatched,
    Cancelling,
    Finishing {
        resulting_household_revision: HouseholdRevision,
    },
}

#[derive(Clone, Eq, PartialEq)]
struct PendingHouseholdMutationV1 {
    binding: HouseholdOperationBindingV1,
    kind: HouseholdMutationKindV1,
    affected_subject: Option<HouseholdSubjectId>,
    expected_active_scope: Option<HouseholdScope>,
    bounded_active_label: String,
    affected_display_label: Option<String>,
    phase: PendingHouseholdMutationPhaseV1,
}

impl fmt::Debug for PendingHouseholdMutationV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingHouseholdMutationV1")
            .field("binding", &self.binding)
            .field("kind", &self.kind)
            .field(
                "affected_subject_kind",
                &self.affected_subject.as_ref().map(household_subject_kind),
            )
            .field(
                "expected_active_scope_kind",
                &self
                    .expected_active_scope
                    .as_ref()
                    .map(household_scope_kind),
            )
            .field("phase", &self.phase)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HouseholdChoicePurposeV1 {
    OnboardMember,
    SelectScope,
}

#[derive(Clone, Eq, PartialEq)]
struct PendingHouseholdChoiceV1 {
    purpose: HouseholdChoicePurposeV1,
    household_revision: HouseholdRevision,
    reducer_correlation: HouseholdReducerCorrelationV1,
    candidates: Vec<HouseholdMemberPresentationV1>,
}

impl fmt::Debug for PendingHouseholdChoiceV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingHouseholdChoiceV1")
            .field("purpose", &self.purpose)
            .field("household_revision", &self.household_revision.get())
            .field("candidate_count", &self.candidates.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq)]
struct HouseholdGenerationStateV1 {
    session_mode_generation: HouseholdModeGenerationV1,
    mode: HouseholdPresentationModeV1,
    account_binding_digest: HouseholdAccountBindingDigestV1,
}

impl fmt::Debug for HouseholdGenerationStateV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdGenerationStateV1")
            .field("session_mode_generation", &self.session_mode_generation)
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum HouseholdTurnGateV1 {
    Legacy,
    Loading,
    HostedReady,
    ReconciliationRequired,
    CounterExhausted,
}

#[derive(Clone, Eq, PartialEq)]
pub struct AppModel {
    pub scrollback: Scrollback,
    pub draft: String,
    /// Character index, not byte index.
    pub cursor: usize,
    pub width: u16,
    pub height: u16,
    pub operation: OperationState,
    pub activity: Option<String>,
    pub follow_tail: bool,
    pub scroll_from_tail: usize,
    pub unseen_lines: usize,
    pub idle_exit_armed: bool,
    prompt_history: VecDeque<String>,
    history_index: Option<usize>,
    history_draft: String,
    pending_choice_labels: Vec<String>,
    pending_agent_partial: String,
    pending_confirmation: Option<PendingActionConfirmation>,
    onboarding: Option<OnboardingFlow>,
    profile_consent_review: Option<ProfileConsentReview>,
    owner_profile_actions: Option<OwnerProfileActionEligibilityV1>,
    pending_profile_action: Option<PendingProfileActionV1>,
    pending_native_startup_onboarding: Option<String>,
    profile_presentation_mode: ProfilePresentationModeV1,
    voice_availability: VoiceAvailability,
    voice_phase: VoicePhase,
    draft_before_voice: String,
    next_operation_id: u64,
    household_generation: Option<HouseholdGenerationStateV1>,
    highest_household_generation: Option<HouseholdModeGenerationV1>,
    household_snapshot: Option<HouseholdManagementSnapshotV1>,
    pending_household_load: Option<PendingHouseholdLoadV1>,
    pending_household_mutation: Option<PendingHouseholdMutationV1>,
    pending_household_choice: Option<PendingHouseholdChoiceV1>,
    household_chrome_label: Option<String>,
    household_turn_gate: HouseholdTurnGateV1,
    next_household_operation_id: Option<HouseholdOperationIdV1>,
    next_household_correlation: Option<HouseholdReducerCorrelationV1>,
    focus_latest_result_on_finish: bool,
    pub(crate) focus_latest_result_start: bool,
    pub(crate) latest_result_start_offset: usize,
}

impl fmt::Debug for AppModel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppModel")
            .field("scrollback", &self.scrollback)
            .field("draft_bytes", &self.draft.len())
            .field("cursor", &self.cursor)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("operation", &self.operation)
            .field("has_activity", &self.activity.is_some())
            .field("follow_tail", &self.follow_tail)
            .field("scroll_from_tail", &self.scroll_from_tail)
            .field("unseen_lines", &self.unseen_lines)
            .field("idle_exit_armed", &self.idle_exit_armed)
            .field("prompt_history_count", &self.prompt_history.len())
            .field("pending_choice_count", &self.pending_choice_labels.len())
            .field(
                "pending_agent_partial_bytes",
                &self.pending_agent_partial.len(),
            )
            .field(
                "has_pending_confirmation",
                &self.pending_confirmation.is_some(),
            )
            .field("onboarding", &self.onboarding)
            .field(
                "has_profile_consent_review",
                &self.profile_consent_review.is_some(),
            )
            .field(
                "has_pending_profile_action",
                &self.pending_profile_action.is_some(),
            )
            .field(
                "has_pending_native_startup_onboarding",
                &self.pending_native_startup_onboarding.is_some(),
            )
            .field("voice_availability", &self.voice_availability)
            .field("voice_phase", &self.voice_phase)
            .field("household_generation", &self.household_generation)
            .field("has_household_snapshot", &self.household_snapshot.is_some())
            .field("pending_household_load", &self.pending_household_load)
            .field(
                "pending_household_mutation",
                &self.pending_household_mutation,
            )
            .field("pending_household_choice", &self.pending_household_choice)
            .field("household_turn_gate", &self.household_turn_gate)
            .finish()
    }
}

impl Default for AppModel {
    fn default() -> Self {
        Self {
            scrollback: Scrollback::default(),
            draft: String::new(),
            cursor: 0,
            width: 80,
            height: 24,
            operation: OperationState::Idle,
            activity: None,
            follow_tail: true,
            scroll_from_tail: 0,
            unseen_lines: 0,
            idle_exit_armed: false,
            prompt_history: VecDeque::new(),
            history_index: None,
            history_draft: String::new(),
            pending_choice_labels: Vec::new(),
            pending_agent_partial: String::new(),
            pending_confirmation: None,
            onboarding: None,
            profile_consent_review: None,
            owner_profile_actions: None,
            pending_profile_action: None,
            pending_native_startup_onboarding: None,
            profile_presentation_mode: ProfilePresentationModeV1::LegacyCompatibility,
            voice_availability: VoiceAvailability::Unavailable,
            voice_phase: VoicePhase::Idle,
            draft_before_voice: String::new(),
            next_operation_id: 1,
            household_generation: None,
            highest_household_generation: None,
            household_snapshot: None,
            pending_household_load: None,
            pending_household_mutation: None,
            pending_household_choice: None,
            household_chrome_label: None,
            household_turn_gate: HouseholdTurnGateV1::Legacy,
            next_household_operation_id: HouseholdOperationIdV1::new(1).ok(),
            next_household_correlation: HouseholdReducerCorrelationV1::new(1).ok(),
            focus_latest_result_on_finish: false,
            focus_latest_result_start: false,
            latest_result_start_offset: 0,
        }
    }
}

impl AppModel {
    #[must_use]
    pub fn household_chrome_label(&self) -> Option<&str> {
        self.household_chrome_label.as_deref()
    }

    #[must_use]
    pub fn household_management_ready(&self) -> bool {
        matches!(self.household_turn_gate, HouseholdTurnGateV1::HostedReady)
            && self.household_generation.is_some()
            && self.household_snapshot.is_some()
    }
}

fn household_turn_gate_for_scope(_scope: &HouseholdScope) -> HouseholdTurnGateV1 {
    HouseholdTurnGateV1::HostedReady
}

fn household_subject_kind(subject: &HouseholdSubjectId) -> &'static str {
    match subject {
        HouseholdSubjectId::Self_ => "self",
        HouseholdSubjectId::Member(_) => "member",
    }
}

fn household_scope_kind(scope: &HouseholdScope) -> &'static str {
    match scope {
        HouseholdScope::Subject(subject) => household_subject_kind(subject),
        HouseholdScope::Everyone => "everyone",
    }
}

#[derive(Clone, PartialEq)]
pub enum RuntimeEvent {
    HouseholdGenerationReadyV1 {
        session_mode_generation: HouseholdModeGenerationV1,
        mode: HouseholdPresentationModeV1,
        account_binding_digest: HouseholdAccountBindingDigestV1,
    },
    HouseholdGenerationInvalidatedV1 {
        session_mode_generation: HouseholdModeGenerationV1,
    },
    HouseholdManagementLoadedV1 {
        operation_id: HouseholdOperationIdV1,
        session_mode_generation: HouseholdModeGenerationV1,
        reducer_correlation: HouseholdReducerCorrelationV1,
        purpose: HouseholdManagementLoadPurposeV1,
        account_binding_digest: HouseholdAccountBindingDigestV1,
        household_revision: HouseholdRevision,
        active_scope: HouseholdScope,
        members: Vec<HouseholdMemberPresentationV1>,
    },
    HouseholdManagementLoadFailedV1 {
        operation_id: HouseholdOperationIdV1,
        session_mode_generation: HouseholdModeGenerationV1,
        reducer_correlation: HouseholdReducerCorrelationV1,
        purpose: HouseholdManagementLoadPurposeV1,
        account_binding_digest: HouseholdAccountBindingDigestV1,
        observed_household_revision: Option<HouseholdRevision>,
        reason: HouseholdManagementFailureV1,
    },
    HouseholdMutationCommittedV1 {
        binding: HouseholdOperationBindingV1,
        kind: HouseholdMutationKindV1,
        resulting_household_revision: HouseholdRevision,
        affected_subject: Option<HouseholdSubjectId>,
        active_scope: HouseholdScope,
        bounded_active_label: String,
    },
    HouseholdMutationFailedV1 {
        binding: HouseholdOperationBindingV1,
        kind: HouseholdMutationKindV1,
        affected_subject: Option<HouseholdSubjectId>,
        observed_household_revision: Option<HouseholdRevision>,
        reason: HouseholdMutationFailureV1,
    },
    HouseholdContextAppliedV1 {
        binding: HouseholdOperationBindingV1,
        resulting_household_revision: HouseholdRevision,
        active_scope: HouseholdScope,
        bounded_active_label: String,
    },
    HouseholdContextApplyFailedV1 {
        binding: HouseholdOperationBindingV1,
        resulting_household_revision: HouseholdRevision,
        reason: HouseholdContextApplyFailureV1,
    },
    BeginOnboarding {
        message: String,
    },
    BeginNativeOwnerOnboarding {
        message: String,
    },
    OnboardingSaved {
        operation_id: u64,
    },
    NativeOwnerOnboardingSaved {
        operation_id: u64,
        status: NativeOwnerProfileSaveStatusV1,
    },
    OnboardingFailed {
        operation_id: u64,
        message: String,
    },
    OnboardingCancelled {
        operation_id: u64,
        outcome: RunTurnOutcome,
    },
    TurnEvent {
        operation_id: u64,
        event: AgentEvent,
    },
    TurnFinished {
        operation_id: u64,
        outcome: RunTurnOutcome,
    },
    TurnFailed {
        operation_id: u64,
        failure: TurnFailure,
    },
    PanelReady {
        operation_id: u64,
        panel: PanelRequest,
        body: String,
    },
    PanelFailed {
        operation_id: u64,
        panel: PanelRequest,
        message: String,
    },
    HouseholdScopeReady {
        operation_id: u64,
        label: String,
    },
    HouseholdScopeFailed {
        operation_id: u64,
        message: String,
    },
    ProfileActionsLoaded {
        operation_id: u64,
        loaded: ProfileActionsLoadedV1,
    },
    ProfileConsentRequested,
    ProfileConsentConfirmed,
    ProfileConsentCancelled,
    ProfileConsentFinished {
        operation_id: u64,
        result: Result<ProfileConsentFinishedV1, ProfileConsentFailureV1>,
    },
    ProfileRetrySyncRequested,
    ProfileRetrySyncFinished {
        operation_id: u64,
        outcome: ProfileRetrySyncFinishedV1,
    },
    ProfilePresentationMode(ProfilePresentationModeV1),
    VoiceAvailability(VoiceAvailability),
    VoiceRecordingElapsed {
        operation_id: u64,
        seconds: u64,
    },
    VoiceTranscriptReady {
        operation_id: u64,
        transcript: String,
    },
    VoiceFailed {
        operation_id: u64,
        message: String,
    },
    VoiceCancelled {
        operation_id: u64,
    },
    Notice {
        message: String,
    },
    ExternalSignal(ExitReason),
}

impl fmt::Debug for RuntimeEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::HouseholdGenerationReadyV1 { .. } => "RuntimeEvent::HouseholdGenerationReadyV1",
            Self::HouseholdGenerationInvalidatedV1 { .. } => {
                "RuntimeEvent::HouseholdGenerationInvalidatedV1"
            }
            Self::HouseholdManagementLoadedV1 { .. } => "RuntimeEvent::HouseholdManagementLoadedV1",
            Self::HouseholdManagementLoadFailedV1 { .. } => {
                "RuntimeEvent::HouseholdManagementLoadFailedV1"
            }
            Self::HouseholdMutationCommittedV1 { .. } => {
                "RuntimeEvent::HouseholdMutationCommittedV1"
            }
            Self::HouseholdMutationFailedV1 { .. } => "RuntimeEvent::HouseholdMutationFailedV1",
            Self::HouseholdContextAppliedV1 { .. } => "RuntimeEvent::HouseholdContextAppliedV1",
            Self::HouseholdContextApplyFailedV1 { .. } => {
                "RuntimeEvent::HouseholdContextApplyFailedV1"
            }
            Self::BeginOnboarding { .. } => "RuntimeEvent::BeginOnboarding",
            Self::BeginNativeOwnerOnboarding { .. } => "RuntimeEvent::BeginNativeOwnerOnboarding",
            Self::OnboardingSaved { .. } => "RuntimeEvent::OnboardingSaved",
            Self::NativeOwnerOnboardingSaved { .. } => "RuntimeEvent::NativeOwnerOnboardingSaved",
            Self::OnboardingFailed { .. } => "RuntimeEvent::OnboardingFailed",
            Self::OnboardingCancelled { .. } => "RuntimeEvent::OnboardingCancelled",
            Self::TurnEvent { .. } => "RuntimeEvent::TurnEvent",
            Self::TurnFinished { .. } => "RuntimeEvent::TurnFinished",
            Self::TurnFailed { .. } => "RuntimeEvent::TurnFailed",
            Self::PanelReady { .. } => "RuntimeEvent::PanelReady",
            Self::PanelFailed { .. } => "RuntimeEvent::PanelFailed",
            Self::HouseholdScopeReady { .. } => "RuntimeEvent::HouseholdScopeReady",
            Self::HouseholdScopeFailed { .. } => "RuntimeEvent::HouseholdScopeFailed",
            Self::ProfileActionsLoaded { .. } => "RuntimeEvent::ProfileActionsLoaded",
            Self::ProfileConsentRequested => "RuntimeEvent::ProfileConsentRequested",
            Self::ProfileConsentConfirmed => "RuntimeEvent::ProfileConsentConfirmed",
            Self::ProfileConsentCancelled => "RuntimeEvent::ProfileConsentCancelled",
            Self::ProfileConsentFinished { .. } => "RuntimeEvent::ProfileConsentFinished",
            Self::ProfileRetrySyncRequested => "RuntimeEvent::ProfileRetrySyncRequested",
            Self::ProfileRetrySyncFinished { .. } => "RuntimeEvent::ProfileRetrySyncFinished",
            Self::ProfilePresentationMode(_) => "RuntimeEvent::ProfilePresentationMode",
            Self::VoiceAvailability(_) => "RuntimeEvent::VoiceAvailability",
            Self::VoiceRecordingElapsed { .. } => "RuntimeEvent::VoiceRecordingElapsed",
            Self::VoiceTranscriptReady { .. } => "RuntimeEvent::VoiceTranscriptReady",
            Self::VoiceFailed { .. } => "RuntimeEvent::VoiceFailed",
            Self::VoiceCancelled { .. } => "RuntimeEvent::VoiceCancelled",
            Self::Notice { .. } => "RuntimeEvent::Notice",
            Self::ExternalSignal(_) => "RuntimeEvent::ExternalSignal",
        })
    }
}

#[derive(Clone, PartialEq)]
pub enum Action {
    Insert(char),
    InsertText(String),
    Backspace,
    Delete,
    MoveLeft,
    MoveRight,
    HistoryPrevious,
    HistoryNext,
    CompleteSlash,
    InsertNewline,
    Submit,
    VoiceToggle,
    CancelVoice,
    CancelOrExit,
    Exit,
    ScrollUp(usize),
    ScrollDown(usize),
    ScrollTop,
    FollowTail,
    Resize { width: u16, height: u16 },
    Runtime(RuntimeEvent),
}

impl fmt::Debug for Action {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Insert(_) => "Action::Insert([REDACTED])",
            Self::InsertText(_) => "Action::InsertText([REDACTED])",
            Self::Backspace => "Action::Backspace",
            Self::Delete => "Action::Delete",
            Self::MoveLeft => "Action::MoveLeft",
            Self::MoveRight => "Action::MoveRight",
            Self::HistoryPrevious => "Action::HistoryPrevious",
            Self::HistoryNext => "Action::HistoryNext",
            Self::CompleteSlash => "Action::CompleteSlash",
            Self::InsertNewline => "Action::InsertNewline",
            Self::Submit => "Action::Submit",
            Self::VoiceToggle => "Action::VoiceToggle",
            Self::CancelVoice => "Action::CancelVoice",
            Self::CancelOrExit => "Action::CancelOrExit",
            Self::Exit => "Action::Exit",
            Self::ScrollUp(_) => "Action::ScrollUp",
            Self::ScrollDown(_) => "Action::ScrollDown",
            Self::ScrollTop => "Action::ScrollTop",
            Self::FollowTail => "Action::FollowTail",
            Self::Resize { .. } => "Action::Resize",
            Self::Runtime(_) => "Action::Runtime([REDACTED])",
        })
    }
}

#[derive(Clone, Eq, PartialEq)]
pub enum Effect {
    LoadHouseholdManagementV1 {
        operation_id: HouseholdOperationIdV1,
        session_mode_generation: HouseholdModeGenerationV1,
        expected_account_binding_digest: HouseholdAccountBindingDigestV1,
        reducer_correlation: HouseholdReducerCorrelationV1,
        purpose: HouseholdManagementLoadPurposeV1,
    },
    CreateMemberWithDeclaredProfileV1 {
        binding: HouseholdOperationBindingV1,
        bounded_member_draft: BoundedHouseholdMemberDraftV1,
        onboarding_profile_input: Box<OnboardingProfileInput>,
    },
    SaveMemberDeclaredProfileV1 {
        binding: HouseholdOperationBindingV1,
        subject: HouseholdSubjectId,
        expected_profile_revision: Option<ProfileRevision>,
        onboarding_profile_input: Box<OnboardingProfileInput>,
    },
    SelectHouseholdScopeV1 {
        binding: HouseholdOperationBindingV1,
        selected_scope: HouseholdScope,
    },
    ApplyCommittedHouseholdContextV1 {
        binding: HouseholdOperationBindingV1,
        resulting_household_revision: HouseholdRevision,
        affected_subject: Option<HouseholdSubjectId>,
        active_scope: HouseholdScope,
        bounded_active_label: String,
    },
    CancelHouseholdOperationV1 {
        binding: HouseholdOperationBindingV1,
    },
    SaveOnboarding {
        operation_id: u64,
        profile: Box<OnboardingProfileInput>,
    },
    SubmitTurn {
        operation_id: u64,
        prompt: String,
    },
    ConfirmAction {
        operation_id: u64,
        command: AgentConfirmationCommandWire,
    },
    OpenPanel {
        operation_id: u64,
        panel: PanelRequest,
    },
    LoadOwnerProfileActionsV1 {
        operation_id: u64,
        purpose: OwnerProfileActionLoadPurposeV1,
    },
    GrantOwnerProfileConsentV1 {
        operation_id: u64,
    },
    RetryOwnerProfileSyncV1 {
        operation_id: u64,
        action: OwnerProfileRetryActionV1,
        intent: OwnerSyncIntentHandleV1,
    },
    SelectHousehold {
        operation_id: u64,
        selector: String,
    },
    StartVoice {
        operation_id: u64,
    },
    StopVoice {
        operation_id: u64,
    },
    CancelVoice {
        operation_id: u64,
    },
    CancelTurn {
        operation_id: u64,
    },
    ResetConversation,
    Exit(ExitReason),
}

impl fmt::Debug for Effect {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::LoadHouseholdManagementV1 { .. } => "Effect::LoadHouseholdManagementV1",
            Self::CreateMemberWithDeclaredProfileV1 { .. } => {
                "Effect::CreateMemberWithDeclaredProfileV1"
            }
            Self::SaveMemberDeclaredProfileV1 { .. } => "Effect::SaveMemberDeclaredProfileV1",
            Self::SelectHouseholdScopeV1 { .. } => "Effect::SelectHouseholdScopeV1",
            Self::ApplyCommittedHouseholdContextV1 { .. } => {
                "Effect::ApplyCommittedHouseholdContextV1"
            }
            Self::CancelHouseholdOperationV1 { .. } => "Effect::CancelHouseholdOperationV1",
            Self::SaveOnboarding { .. } => "Effect::SaveOnboarding([REDACTED])",
            Self::SubmitTurn { .. } => "Effect::SubmitTurn([REDACTED])",
            Self::ConfirmAction { .. } => "Effect::ConfirmAction([REDACTED])",
            Self::OpenPanel { .. } => "Effect::OpenPanel",
            Self::LoadOwnerProfileActionsV1 { .. } => "Effect::LoadOwnerProfileActionsV1",
            Self::GrantOwnerProfileConsentV1 { .. } => "Effect::GrantOwnerProfileConsentV1",
            Self::RetryOwnerProfileSyncV1 { .. } => "Effect::RetryOwnerProfileSyncV1([REDACTED])",
            Self::SelectHousehold { .. } => "Effect::SelectHousehold([REDACTED])",
            Self::StartVoice { .. } => "Effect::StartVoice",
            Self::StopVoice { .. } => "Effect::StopVoice",
            Self::CancelVoice { .. } => "Effect::CancelVoice",
            Self::CancelTurn { .. } => "Effect::CancelTurn",
            Self::ResetConversation => "Effect::ResetConversation",
            Self::Exit(_) => "Effect::Exit",
        })
    }
}

#[must_use]
pub fn dispatch(model: &mut AppModel, action: Action) -> Vec<Effect> {
    if matches!(
        model.voice_phase,
        VoicePhase::Recording { .. } | VoicePhase::Transcribing { .. }
    ) && matches!(
        &action,
        Action::Insert(_)
            | Action::InsertText(_)
            | Action::Backspace
            | Action::Delete
            | Action::MoveLeft
            | Action::MoveRight
            | Action::HistoryPrevious
            | Action::HistoryNext
            | Action::CompleteSlash
            | Action::InsertNewline
    ) {
        model.activity = Some(match model.voice_phase {
            VoicePhase::Recording { .. } => {
                "Recording · composer editing is paused · stop or cancel voice first".into()
            }
            VoicePhase::Transcribing { .. } => {
                "Transcribing securely… · composer editing is paused · Esc to cancel".into()
            }
            VoicePhase::Idle | VoicePhase::Review => unreachable!(),
        });
        model.idle_exit_armed = false;
        return Vec::new();
    }
    match action {
        Action::Insert('?') if model.draft.is_empty() && model.onboarding.is_none() => {
            show_help(model)
        }
        Action::Insert(character) => {
            reset_history_navigation(model);
            insert_at_cursor(model, &character.to_string());
            model.idle_exit_armed = false;
        }
        Action::InsertText(text) => {
            reset_history_navigation(model);
            insert_at_cursor(model, &text);
            model.idle_exit_armed = false;
        }
        Action::Backspace => {
            reset_history_navigation(model);
            backspace(model);
        }
        Action::Delete => {
            reset_history_navigation(model);
            delete(model);
        }
        Action::MoveLeft => model.cursor = model.cursor.saturating_sub(1),
        Action::MoveRight => model.cursor = (model.cursor + 1).min(model.draft.chars().count()),
        Action::HistoryPrevious => history_previous(model),
        Action::HistoryNext => history_next(model),
        Action::CompleteSlash => complete_slash(model),
        Action::InsertNewline => {
            reset_history_navigation(model);
            insert_at_cursor(model, "\n");
        }
        Action::Submit => return submit(model),
        Action::VoiceToggle => return toggle_voice(model),
        Action::CancelVoice if has_active_household_work(model) => {
            return cancel_household_draft(model);
        }
        Action::CancelVoice
            if matches!(
                model.profile_consent_review,
                Some(ProfileConsentReview::Reviewing)
            ) =>
        {
            return profile_event(model, RuntimeEvent::ProfileConsentCancelled);
        }
        Action::CancelVoice => return cancel_voice(model),
        Action::CancelOrExit => return cancel_or_exit(model),
        Action::Exit if has_active_household_work(model) => {
            return begin_exit(model, ExitReason::Requested);
        }
        Action::Exit
            if matches!(
                model.profile_consent_review,
                Some(ProfileConsentReview::Reviewing)
            ) =>
        {
            return profile_event(model, RuntimeEvent::ProfileConsentCancelled);
        }
        Action::Exit if model.draft.is_empty() => {
            return begin_exit(model, ExitReason::Requested);
        }
        Action::Exit => {}
        Action::ScrollUp(lines) => {
            if model.focus_latest_result_start {
                model.latest_result_start_offset = model
                    .latest_result_start_offset
                    .saturating_sub(lines.max(1));
            } else {
                model.follow_tail = false;
                model.scroll_from_tail = model.scroll_from_tail.saturating_add(lines.max(1));
            }
        }
        Action::ScrollDown(lines) => {
            if model.focus_latest_result_start {
                model.latest_result_start_offset = model
                    .latest_result_start_offset
                    .saturating_add(lines.max(1));
            } else {
                model.scroll_from_tail = model.scroll_from_tail.saturating_sub(lines.max(1));
                if model.scroll_from_tail == 0 {
                    follow_tail(model);
                }
            }
        }
        Action::ScrollTop => {
            model.focus_latest_result_start = false;
            model.latest_result_start_offset = 0;
            model.follow_tail = false;
            model.scroll_from_tail = usize::MAX / 2;
        }
        Action::FollowTail => follow_tail(model),
        Action::Resize { width, height } => {
            model.width = width;
            model.height = height;
        }
        Action::Runtime(event) => return runtime_event(model, event),
    }
    Vec::new()
}

fn submit(model: &mut AppModel) -> Vec<Effect> {
    if matches!(model.voice_phase, VoicePhase::Recording { .. }) {
        return stop_voice(model);
    }
    if model.draft.trim().is_empty() {
        return Vec::new();
    }
    if model.draft.trim_start().starts_with('/') {
        if model.draft.trim().eq_ignore_ascii_case("/cancel")
            && (model.pending_household_choice.is_some()
                || model
                    .onboarding
                    .as_ref()
                    .is_some_and(|flow| !matches!(flow.target, OnboardingTargetV1::Owner)))
        {
            return cancel_household_draft(model);
        }
        return submit_slash_command(model);
    }
    if model.profile_consent_review.is_some() {
        return submit_profile_consent_review(model);
    }
    if model.pending_household_choice.is_some() {
        return submit_household_choice(model);
    }
    if model.onboarding.is_some() {
        return submit_onboarding(model);
    }
    if model.pending_confirmation.is_some() {
        return submit_confirmation_answer(model);
    }
    if model.operation.is_active() {
        return Vec::new();
    }
    if !household_turn_is_authorized(model) {
        return Vec::new();
    }
    model.focus_latest_result_on_finish = false;
    model.voice_phase = VoicePhase::Idle;
    model.draft_before_voice.clear();
    let prompt = std::mem::take(&mut model.draft);
    model.pending_choice_labels.clear();
    model.pending_agent_partial.clear();
    remember_prompt(model, &prompt);
    model.cursor = 0;
    let operation_id = model.next_operation_id;
    model.next_operation_id = model.next_operation_id.saturating_add(1);
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::User,
        text: prompt.clone(),
        streaming: false,
    });
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::Assistant,
        text: String::new(),
        streaming: true,
    });
    model.operation = OperationState::Running(operation_id);
    model.activity = Some("Connecting…".into());
    follow_tail(model);
    vec![Effect::SubmitTurn {
        operation_id,
        prompt,
    }]
}

fn toggle_voice(model: &mut AppModel) -> Vec<Effect> {
    match model.voice_phase {
        VoicePhase::Recording { .. } => return stop_voice(model),
        VoicePhase::Transcribing { .. } => {
            push_notice(
                model,
                "The recording is already being transcribed. Esc cancels this voice operation.",
            );
            return Vec::new();
        }
        VoicePhase::Idle | VoicePhase::Review => {}
    }
    match model.voice_availability {
        VoiceAvailability::Unavailable => {
            push_notice(
                model,
                "Native microphone capture is unavailable in this artifact. The composer remains available for typed input.",
            );
            return Vec::new();
        }
        VoiceAvailability::AuthorizationRequired => {
            // Native authority may have been safely upgraded since launch.
            // The driver checks the freshly loaded scope before opening the
            // microphone and reports the current availability back here.
        }
        VoiceAvailability::Ready => {}
    }
    if model.operation.is_active()
        || model.onboarding.is_some()
        || model.pending_confirmation.is_some()
    {
        push_notice(
            model,
            "Finish or stop the active work before starting voice capture.",
        );
        return Vec::new();
    }
    model.draft_before_voice = model.draft.clone();
    model.draft.clear();
    model.cursor = 0;
    let operation_id = model.next_operation_id;
    model.next_operation_id = model.next_operation_id.saturating_add(1);
    model.voice_phase = VoicePhase::Recording { operation_id };
    model.operation = OperationState::Running(operation_id);
    model.activity =
        Some("Opening microphone… · Enter, Ctrl+Space, or F8 to stop · Esc to cancel".into());
    model.idle_exit_armed = false;
    vec![Effect::StartVoice { operation_id }]
}

fn stop_voice(model: &mut AppModel) -> Vec<Effect> {
    let VoicePhase::Recording { operation_id } = model.voice_phase else {
        return Vec::new();
    };
    model.voice_phase = VoicePhase::Transcribing { operation_id };
    model.operation = OperationState::Finishing(operation_id);
    model.activity = Some("Transcribing securely… · Esc to cancel".into());
    vec![Effect::StopVoice { operation_id }]
}

fn cancel_voice(model: &mut AppModel) -> Vec<Effect> {
    match model.voice_phase {
        VoicePhase::Recording { operation_id } | VoicePhase::Transcribing { operation_id } => {
            model.operation = OperationState::Cancelling(operation_id);
            model.activity = Some("Cancelling voice capture…".into());
            vec![Effect::CancelVoice { operation_id }]
        }
        VoicePhase::Review => {
            model.draft = std::mem::take(&mut model.draft_before_voice);
            model.cursor = model.draft.chars().count();
            model.voice_phase = VoicePhase::Idle;
            model.activity = None;
            model.idle_exit_armed = false;
            push_notice(model, "Voice transcript discarded. Nothing was submitted.");
            Vec::new()
        }
        VoicePhase::Idle => Vec::new(),
    }
}

fn voice_operation_id(phase: VoicePhase) -> Option<u64> {
    match phase {
        VoicePhase::Recording { operation_id } | VoicePhase::Transcribing { operation_id } => {
            Some(operation_id)
        }
        VoicePhase::Idle | VoicePhase::Review => None,
    }
}

fn finish_voice_transcription(model: &mut AppModel, result: Result<String, String>) {
    model.operation = OperationState::Idle;
    model.idle_exit_armed = false;
    match result {
        Ok(transcript) => {
            let transcript = terminal_safe_text(&transcript);
            if transcript.trim().is_empty()
                || transcript.chars().count() > TRANSCRIPTION_MAX_TRANSCRIPT_CHARACTERS
            {
                finish_voice_transcription(
                    model,
                    Err("The transcription response was empty or too long.".into()),
                );
                return;
            }
            model.draft = transcript.trim().to_owned();
            model.cursor = model.draft.chars().count();
            model.voice_phase = VoicePhase::Review;
            model.activity = Some(
                "Review voice transcript · edit and press Enter to submit · Esc to discard".into(),
            );
            push_notice(
                model,
                "Voice transcript ready in the composer. Edit it if needed, then press Enter to submit through the same agent path. Use `/voice` to record again or Esc to discard it.",
            );
        }
        Err(message) => {
            model.draft = std::mem::take(&mut model.draft_before_voice);
            model.cursor = model.draft.chars().count();
            model.voice_phase = VoicePhase::Idle;
            model.activity = None;
            push_notice(
                model,
                &format!(
                    "Voice input was not submitted: {} Continue with typed input or try `/voice` again.",
                    terminal_safe_text(&message)
                ),
            );
        }
    }
}

fn finish_voice_cancel(model: &mut AppModel) {
    model.draft = std::mem::take(&mut model.draft_before_voice);
    model.cursor = model.draft.chars().count();
    model.voice_phase = VoicePhase::Idle;
    model.operation = OperationState::Idle;
    model.activity = None;
    model.idle_exit_armed = false;
    push_notice(
        model,
        "Voice capture cancelled. Audio and transcript were discarded; nothing was submitted.",
    );
}

fn submit_confirmation_answer(model: &mut AppModel) -> Vec<Effect> {
    if model.operation.is_active() {
        return Vec::new();
    }
    let answer = model.draft.trim().to_owned();
    let normalized = answer.to_ascii_lowercase();
    let decision = match normalized.as_str() {
        "y" | "yes" | "confirm" | "accept" => ConfirmationDecisionWire::Accept,
        "n" | "no" | "cancel" => ConfirmationDecisionWire::Cancel,
        value if value.starts_with("edit ") => return submit_confirmation_edit(model, &answer),
        _ => {
            push_notice(
                model,
                "A write is awaiting your decision. Type `y` to confirm, `n` to cancel, or use the edit instruction shown on the card.",
            );
            return Vec::new();
        }
    };
    submit_confirmation(model, decision, None)
}

fn submit_confirmation_edit(model: &mut AppModel, answer: &str) -> Vec<Effect> {
    let Some(pending) = model.pending_confirmation.as_ref() else {
        return Vec::new();
    };
    let Some(editable_items) = pending.editable_items.as_ref() else {
        push_notice(
            model,
            "This proposal does not expose a contract-backed item edit.",
        );
        return Vec::new();
    };
    let mut words = answer.split_whitespace();
    let command = words.next();
    let reference = words.next();
    let replacement = words.collect::<Vec<_>>().join(" ");
    let index = command
        .filter(|value| value.eq_ignore_ascii_case("edit"))
        .and(reference)
        .and_then(|value| value.strip_prefix('#'))
        .and_then(|value| value.parse::<usize>().ok());
    let replacement = required_text(&replacement, 255).ok();
    let (Some(index), Some(replacement)) = (index, replacement) else {
        push_notice(model, "Use `edit #N <replacement item name>`.");
        return Vec::new();
    };
    if index == 0 || index > editable_items.len() {
        push_notice(model, "That item number is outside the pending proposal.");
        return Vec::new();
    }
    let mut items = editable_items.clone();
    items[index - 1].insert("name".into(), serde_json::Value::String(replacement));
    let edits = GroceryEditPatch::new(serde_json::Map::from_iter([(
        "items".into(),
        serde_json::Value::Array(items.into_iter().map(serde_json::Value::Object).collect()),
    )]));
    let Ok(edits) = edits else {
        push_notice(model, "The corrected proposal is too large or invalid.");
        return Vec::new();
    };
    submit_confirmation(model, ConfirmationDecisionWire::Accept, Some(edits))
}

fn submit_confirmation(
    model: &mut AppModel,
    decision: ConfirmationDecisionWire,
    edits: Option<GroceryEditPatch>,
) -> Vec<Effect> {
    if model.operation.is_active() {
        return Vec::new();
    }
    let Some(pending) = model.pending_confirmation.as_ref() else {
        return Vec::new();
    };
    let editing = edits.is_some();
    let command = pending.command(decision, edits);
    model.pending_agent_partial.clear();
    model.draft.clear();
    model.cursor = 0;
    let operation_id = model.next_operation_id;
    model.next_operation_id = model.next_operation_id.saturating_add(1);
    let label = match (decision, editing) {
        (ConfirmationDecisionWire::Accept, true) => "Edit and confirm",
        (ConfirmationDecisionWire::Accept, false) => "Confirm",
        (ConfirmationDecisionWire::Cancel, _) => "Cancel",
    };
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::User,
        text: label.into(),
        streaming: false,
    });
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::Assistant,
        text: String::new(),
        streaming: true,
    });
    model.operation = OperationState::Running(operation_id);
    model.activity = Some(match (decision, editing) {
        (ConfirmationDecisionWire::Accept, true) => "Applying correction…".into(),
        (ConfirmationDecisionWire::Accept, false) => "Confirming…".into(),
        (ConfirmationDecisionWire::Cancel, _) => "Cancelling proposal…".into(),
    });
    model.idle_exit_armed = false;
    follow_tail(model);
    vec![Effect::ConfirmAction {
        operation_id,
        command,
    }]
}

fn begin_onboarding(model: &mut AppModel, message: &str) {
    begin_onboarding_with_mode(model, message, OnboardingCopyMode::LegacyCompatibility);
}

fn begin_native_owner_onboarding(model: &mut AppModel, message: &str) {
    begin_onboarding_with_mode(model, message, OnboardingCopyMode::NativeLocalFirst);
}

fn begin_onboarding_with_mode(model: &mut AppModel, message: &str, copy_mode: OnboardingCopyMode) {
    if model.onboarding.is_some() {
        push_notice(model, "Dietary onboarding is already in progress.");
        return;
    }
    model.onboarding = Some(OnboardingFlow {
        copy_mode,
        ..OnboardingFlow::default()
    });
    model.idle_exit_armed = false;
    push_notice(model, message);
    push_onboarding_prompt(model);
}

fn submit_onboarding(model: &mut AppModel) -> Vec<Effect> {
    if model.operation.is_active() {
        return Vec::new();
    }
    let answer = std::mem::take(&mut model.draft);
    model.cursor = 0;
    let answer = answer.trim();
    if matches!(answer.to_ascii_lowercase().as_str(), "cancel" | "/cancel") {
        let household_member = model
            .onboarding
            .as_ref()
            .is_some_and(|flow| !matches!(flow.target, OnboardingTargetV1::Owner));
        let native_local_first = model.onboarding.as_ref().is_some_and(|flow| {
            matches!(flow.copy_mode, OnboardingCopyMode::NativeLocalFirst)
                && matches!(flow.target, OnboardingTargetV1::Owner)
        });
        model.onboarding = None;
        if household_member {
            push_notice(
                model,
                "Household member setup cancelled. No member or member profile was changed.",
            );
        } else if native_local_first {
            push_notice(
                model,
                &crate::render::profile_copy(ProfileCopyStateV1::OnboardingSaveCancelled),
            );
        } else {
            push_notice(
                model,
                "Dietary onboarding cancelled. Nothing was sent or saved.",
            );
        }
        return Vec::new();
    }

    let mut flow = model
        .onboarding
        .take()
        .expect("onboarding submission requires an active flow");
    if answer.eq_ignore_ascii_case("back") {
        flow.step = previous_onboarding_step(flow.step, &flow.profile, &flow.target);
        model.onboarding = Some(flow);
        push_onboarding_prompt(model);
        return Vec::new();
    }

    let result = apply_onboarding_answer(&mut flow, answer);
    if let Err(message) = result {
        model.onboarding = Some(flow);
        push_notice(model, &message);
        push_onboarding_prompt(model);
        return Vec::new();
    }

    if flow.step == OnboardingStep::Saving {
        let profile = flow.profile.clone();
        if let Err(message) = profile.profile_data() {
            flow.step = OnboardingStep::Review;
            model.onboarding = Some(flow);
            push_notice(model, &format!("Unable to review this profile: {message}"));
            return Vec::new();
        }
        if !matches!(flow.target, OnboardingTargetV1::Owner) {
            model.onboarding = Some(flow);
            return dispatch_member_onboarding_save(model, profile);
        }
        let operation_id = model.next_operation_id;
        model.next_operation_id = model.next_operation_id.saturating_add(1);
        model.scrollback.push(SemanticEntry {
            speaker: Speaker::User,
            text: "Save dietary profile".into(),
            streaming: false,
        });
        model.scrollback.push(SemanticEntry {
            speaker: Speaker::Assistant,
            text: String::new(),
            streaming: true,
        });
        model.onboarding = Some(flow);
        model.operation = OperationState::Running(operation_id);
        model.activity = Some("Saving dietary profile…".into());
        follow_tail(model);
        return vec![Effect::SaveOnboarding {
            operation_id,
            profile: Box::new(profile),
        }];
    }

    model.scrollback.push(SemanticEntry {
        speaker: Speaker::User,
        text: terminal_safe_text(answer),
        streaming: false,
    });
    model.onboarding = Some(flow);
    push_onboarding_prompt(model);
    Vec::new()
}

fn apply_onboarding_answer(flow: &mut OnboardingFlow, answer: &str) -> Result<(), String> {
    match flow.step {
        OnboardingStep::MemberRelationship => {
            flow.member_relationship = Some(parse_household_relationship(answer)?);
            flow.step = OnboardingStep::MemberName;
        }
        OnboardingStep::MemberName => {
            let bounded = required_text(answer, 80)
                .map_err(|_| "Enter a display name from 1 to 80 characters.".to_owned())?;
            if bounded != answer {
                return Err(
                    "Enter the display name without leading or trailing whitespace.".into(),
                );
            }
            flow.member_name = Some(bounded);
            flow.step = OnboardingStep::MemberAgeEvidence;
        }
        OnboardingStep::MemberAgeEvidence => {
            let age_evidence = parse_household_age_evidence(answer)?;
            let relationship = flow
                .member_relationship
                .ok_or_else(|| "Restart household member setup.".to_owned())?;
            let display_name = flow
                .member_name
                .clone()
                .ok_or_else(|| "Restart household member setup.".to_owned())?;
            let draft =
                BoundedHouseholdMemberDraftV1::new(display_name, relationship, age_evidence)
                    .map_err(|_| "Restart household member setup.".to_owned())?;
            let OnboardingTargetV1::NewMember { bounded_draft, .. } = &mut flow.target else {
                return Err("Restart household member setup.".into());
            };
            *bounded_draft = Some(draft);
            flow.member_age_evidence = Some(age_evidence);
            flow.step = OnboardingStep::Diets;
        }
        OnboardingStep::Diets => {
            let selected = parse_multi_options(answer, diet_options(), 10, 40)?;
            flow.profile.diet_style_ids = selected.ids;
            flow.profile.custom_diet_styles = selected.custom;
            flow.step = OnboardingStep::Allergies;
        }
        OnboardingStep::Allergies => {
            let selected = parse_multi_options(answer, allergy_options(), 10, 60)?;
            flow.profile.allergy_ids = selected.ids;
            flow.profile.custom_restrictions = selected.custom;
            flow.step = OnboardingStep::Conditions;
        }
        OnboardingStep::Conditions => {
            let selected = parse_multi_options(answer, condition_options(), 10, 60)?;
            flow.profile.health_condition_ids = selected.ids;
            flow.profile.custom_health_conditions = selected.custom;
            flow.step = if flow.profile.health_condition_ids.is_empty() {
                flow.profile.severity_level = None;
                OnboardingStep::AvoidIngredients
            } else {
                OnboardingStep::Severity
            };
        }
        OnboardingStep::Severity => {
            let severity = answer
                .parse::<u8>()
                .ok()
                .filter(|value| (1..=5).contains(value))
                .ok_or_else(|| "Enter a condition severity from 1 to 5.".to_owned())?;
            flow.profile.severity_level = Some(severity);
            flow.step = OnboardingStep::AvoidIngredients;
        }
        OnboardingStep::AvoidIngredients => {
            flow.profile.avoid_ingredients = parse_free_text_list(answer, 20, 40)?;
            flow.step = OnboardingStep::Activity;
        }
        OnboardingStep::Activity => {
            flow.profile.activity_level = parse_single_option(answer, activity_options())?;
            flow.step = OnboardingStep::Cuisines;
        }
        OnboardingStep::Cuisines => {
            let selected = parse_multi_options(answer, cuisine_options(), 10, 40)?;
            flow.profile.cuisine_preferences = selected.ids;
            flow.profile.custom_cuisines = selected.custom;
            flow.step = OnboardingStep::Notes;
        }
        OnboardingStep::Notes => {
            flow.profile.notes = parse_optional_text(answer, 280)?;
            flow.step = OnboardingStep::Review;
        }
        OnboardingStep::Review if answer.eq_ignore_ascii_case("save") => {
            flow.step = OnboardingStep::Saving;
        }
        OnboardingStep::Review => {
            return Err(
                "Type `save` to confirm, `back` to edit, or `cancel` to discard it.".into(),
            );
        }
        OnboardingStep::Saving => return Err("The dietary profile is already being saved.".into()),
    }
    Ok(())
}

fn parse_household_relationship(answer: &str) -> Result<RelationshipV1, String> {
    let normalized = normalize_choice(answer);
    let relationship = match normalized.as_str() {
        "1" | "spouse" => RelationshipV1::Spouse,
        "2" | "partner" => RelationshipV1::Partner,
        "3" | "parent" => RelationshipV1::Parent,
        "4" | "child" => RelationshipV1::Child,
        "5" | "sibling" => RelationshipV1::Sibling,
        "6" | "grandparent" => RelationshipV1::Grandparent,
        "7" | "friend" => RelationshipV1::Friend,
        "8" | "other" => RelationshipV1::Other,
        _ => {
            return Err(
                "Choose a relationship by number or exact label from the listed options.".into(),
            );
        }
    };
    Ok(relationship)
}

fn parse_household_age_evidence(answer: &str) -> Result<HouseholdAgeEvidenceInputV1, String> {
    match answer.trim().to_ascii_lowercase().as_str() {
        "1" | "under_13" => Ok(HouseholdAgeEvidenceInputV1::Under13),
        "2" | "age_13_17" => Ok(HouseholdAgeEvidenceInputV1::Age13To17),
        "3" | "age_18_plus" => Ok(HouseholdAgeEvidenceInputV1::Age18Plus),
        "4" | "unknown" => Ok(HouseholdAgeEvidenceInputV1::Unknown),
        _ => Err(
            "Choose age evidence by number or one of under_13, age_13_17, age_18_plus, unknown."
                .into(),
        ),
    }
}

fn previous_onboarding_step(
    step: OnboardingStep,
    profile: &OnboardingProfileInput,
    target: &OnboardingTargetV1,
) -> OnboardingStep {
    match step {
        OnboardingStep::MemberRelationship => OnboardingStep::MemberRelationship,
        OnboardingStep::MemberName => OnboardingStep::MemberRelationship,
        OnboardingStep::MemberAgeEvidence => OnboardingStep::MemberName,
        OnboardingStep::Diets if matches!(target, OnboardingTargetV1::NewMember { .. }) => {
            OnboardingStep::MemberAgeEvidence
        }
        OnboardingStep::Diets => OnboardingStep::Diets,
        OnboardingStep::Allergies => OnboardingStep::Diets,
        OnboardingStep::Conditions => OnboardingStep::Allergies,
        OnboardingStep::Severity => OnboardingStep::Conditions,
        OnboardingStep::AvoidIngredients if profile.health_condition_ids.is_empty() => {
            OnboardingStep::Conditions
        }
        OnboardingStep::AvoidIngredients => OnboardingStep::Severity,
        OnboardingStep::Activity => OnboardingStep::AvoidIngredients,
        OnboardingStep::Cuisines => OnboardingStep::Activity,
        OnboardingStep::Notes => OnboardingStep::Cuisines,
        OnboardingStep::Review | OnboardingStep::Saving => OnboardingStep::Notes,
    }
}

fn parse_multi_options(
    answer: &str,
    options: &[OnboardingOption],
    custom_maximum: usize,
    custom_max_length: usize,
) -> Result<MultiSelection, String> {
    if is_none_answer(answer) {
        return Ok(MultiSelection {
            ids: Vec::new(),
            custom: Vec::new(),
        });
    }
    if let Some(option) = resolve_onboarding_option(answer.trim(), options) {
        return Ok(MultiSelection {
            ids: vec![option.id.clone()],
            custom: Vec::new(),
        });
    }
    let mut selected = MultiSelection {
        ids: Vec::new(),
        custom: Vec::new(),
    };
    for token in answer
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if is_none_answer(token) {
            return Err("Use `none` by itself to clear this section.".into());
        }
        if let Some((start, end)) = numeric_range(token)? {
            if start == 0 || start > end || end > options.len() {
                return Err(
                    "A numeric range must refer to listed options in ascending order.".into(),
                );
            }
            for index in start..=end {
                let id = &options[index - 1].id;
                if !selected.ids.contains(id) {
                    selected.ids.push(id.clone());
                }
            }
            continue;
        }
        if let Some(option) = resolve_onboarding_option(token, options) {
            if !selected.ids.contains(&option.id) {
                selected.ids.push(option.id.clone());
            }
            continue;
        }
        if token.parse::<usize>().is_ok() {
            return Err("A numeric choice must refer to one of the listed options.".into());
        }
        if token.chars().count() > custom_max_length || token.chars().any(char::is_control) {
            return Err(format!(
                "Custom entries must be at most {custom_max_length} characters."
            ));
        }
        if !selected.custom.iter().any(|value| value == token) {
            selected.custom.push(token.to_owned());
        }
    }
    if selected.ids.is_empty() && selected.custom.is_empty() {
        return Err("Choose at least one option, or type `none`.".into());
    }
    if selected.custom.len() > custom_maximum {
        return Err(format!("Enter at most {custom_maximum} custom selections."));
    }
    Ok(selected)
}

fn numeric_range(token: &str) -> Result<Option<(usize, usize)>, String> {
    let Some((start, end)) = token.split_once('-') else {
        return Ok(None);
    };
    if start.trim().chars().all(|value| value.is_ascii_digit())
        && end.trim().chars().all(|value| value.is_ascii_digit())
    {
        let start = start
            .trim()
            .parse()
            .map_err(|_| "The numeric range is too large.".to_owned())?;
        let end = end
            .trim()
            .parse()
            .map_err(|_| "The numeric range is too large.".to_owned())?;
        Ok(Some((start, end)))
    } else {
        Ok(None)
    }
}

fn parse_single_option(
    answer: &str,
    options: &[OnboardingOption],
) -> Result<Option<String>, String> {
    if is_none_answer(answer) {
        return Ok(None);
    }
    if answer.contains(',') {
        return Err("Choose one activity level, or type `none`.".into());
    }
    resolve_onboarding_option(answer.trim(), options)
        .map(|option| Some(option.id.clone()))
        .ok_or_else(|| "Choose an activity by number, exact label, or canonical ID.".into())
}

fn resolve_onboarding_option<'a>(
    token: &str,
    options: &'a [OnboardingOption],
) -> Option<&'a OnboardingOption> {
    if let Ok(number) = token.parse::<usize>() {
        return number.checked_sub(1).and_then(|index| options.get(index));
    }
    let normalized = normalize_choice(token);
    options.iter().find(|option| {
        normalize_choice(&option.id) == normalized || normalize_choice(&option.label) == normalized
    })
}

fn normalize_choice(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn parse_free_text_list(
    answer: &str,
    maximum: usize,
    max_length: usize,
) -> Result<Vec<String>, String> {
    if is_none_answer(answer) {
        return Ok(Vec::new());
    }
    let mut values = Vec::new();
    for value in answer
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if value.chars().count() > max_length || value.chars().any(char::is_control) {
            return Err(format!(
                "Each entry must be at most {max_length} characters."
            ));
        }
        if !values.iter().any(|current| current == value) {
            values.push(value.to_owned());
        }
    }
    if values.is_empty() {
        return Err("Enter comma-separated ingredients, or type `none`.".into());
    }
    if values.len() > maximum {
        return Err(format!("Enter at most {maximum} ingredients."));
    }
    Ok(values)
}

fn parse_optional_text(answer: &str, maximum: usize) -> Result<Option<String>, String> {
    if is_none_answer(answer) {
        return Ok(None);
    }
    if answer.chars().count() > maximum || answer.chars().any(char::is_control) {
        return Err(format!("Notes must be at most {maximum} characters."));
    }
    Ok(Some(answer.to_owned()))
}

fn is_none_answer(answer: &str) -> bool {
    matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "none" | "0" | "skip"
    )
}

fn push_onboarding_prompt(model: &mut AppModel) {
    let Some(flow) = model.onboarding.as_ref() else {
        return;
    };
    let prompt = onboarding_prompt(flow);
    push_notice(model, &prompt);
}

fn onboarding_prompt(flow: &OnboardingFlow) -> String {
    let prompt = match flow.step {
        OnboardingStep::MemberRelationship => "New household member · relationship\nChoose how this person is related to you.\n\n 1. Spouse\n 2. Partner\n 3. Parent\n 4. Child\n 5. Sibling\n 6. Grandparent\n 7. Friend\n 8. Other\n\nType `cancel` to discard household member setup.".into(),
        OnboardingStep::MemberName => "New household member · display name\nEnter the name you want shown in this household (80 characters maximum).\n\nType `back` to revisit relationship or `cancel` to discard household member setup.".into(),
        OnboardingStep::MemberAgeEvidence => format!(
            "{} · age evidence\nChoose the one available age band. A date of birth is not requested.\n\n 1. under_13\n 2. age_13_17\n 3. age_18_plus\n 4. unknown\n\nType `back` to edit the display name or `cancel` to discard household member setup.",
            flow.display_label().unwrap_or("New household member")
        ),
        OnboardingStep::Diets => option_prompt(
            "Diet styles · 1/8",
            "Choose any that apply by number, range, ID, label, or custom text. Separate choices with commas; type `none` for no restrictions.",
            diet_options(),
        ),
        OnboardingStep::Allergies => option_prompt(
            "Allergies & restrictions · 2/8",
            "Choose every option that must be avoided by number, range, ID, label, or custom text; type `none` if there are none.",
            allergy_options(),
        ),
        OnboardingStep::Conditions => option_prompt(
            "Health conditions · 3/8",
            "Choose conditions by number, range, ID, label, or custom text; type `none` if there are none.",
            condition_options(),
        ),
        OnboardingStep::Severity => {
            "Condition severity · 4/8\nChoose a shared severity from 1 (mild) to 5 (critical).".into()
        }
        OnboardingStep::AvoidIngredients => "Ingredients to avoid · 5/8\nEnter up to 20 ingredients separated by commas, or type `none`.".into(),
        OnboardingStep::Activity => option_prompt(
            "Activity level · 6/8",
            "Choose one option by number, ID, or label; type `none` to leave it unset.",
            activity_options(),
        ),
        OnboardingStep::Cuisines => option_prompt(
            "Cuisines you love · 7/8",
            "Choose favorites by number, range, ID, label, or custom text; type `none` to skip.",
            cuisine_options(),
        ),
        OnboardingStep::Notes => "Additional notes · 8/8\nAdd anything else the food guide should know (280 characters maximum), or type `none`.".into(),
        OnboardingStep::Review => onboarding_review(flow),
        OnboardingStep::Saving => "Saving the declared dietary profile…".into(),
    };
    if matches!(
        flow.step,
        OnboardingStep::Diets
            | OnboardingStep::Allergies
            | OnboardingStep::Conditions
            | OnboardingStep::Severity
            | OnboardingStep::AvoidIngredients
            | OnboardingStep::Activity
            | OnboardingStep::Cuisines
            | OnboardingStep::Notes
    ) && !matches!(flow.target, OnboardingTargetV1::Owner)
    {
        format!(
            "Declared dietary profile for {}\n\n{prompt}",
            flow.display_label().unwrap_or("household member")
        )
    } else {
        prompt
    }
}

fn option_prompt(title: &str, instructions: &str, options: &[OnboardingOption]) -> String {
    let mut output = format!("{title}\n{instructions}\n\n");
    for (index, option) in options.iter().enumerate() {
        let _ = writeln!(output, "{:>2}. {}", index + 1, option.label);
    }
    output
        .push_str("\nType `back` to revisit the previous step or `cancel` to discard onboarding.");
    output
}

fn onboarding_review(flow: &OnboardingFlow) -> String {
    let profile = &flow.profile;
    let (title, review_action) = match &flow.target {
        OnboardingTargetV1::Owner => {
            let review_action = match flow.copy_mode {
                OnboardingCopyMode::LegacyCompatibility => "No profile data has been sent yet. Type `save` to grant profile-sync consent and replace the synced profile, `back` to edit, or `cancel` to discard it.".into(),
                OnboardingCopyMode::NativeLocalFirst => format!(
                    "{}\n\nType `save` to continue, `back` to edit, or `cancel` to discard it.",
                    crate::render::profile_copy(ProfileCopyStateV1::OnboardingSaveReview)
                ),
            };
            ("Review dietary profile".to_owned(), review_action)
        }
        OnboardingTargetV1::NewMember {
            bounded_draft: Some(draft),
            ..
        } => (
            format!(
                "Review declared dietary profile for {}",
                draft.display_name()
            ),
            format!(
                "Add {} to this household and save this declared dietary profile on this device? No profile-sync consent or remote member sync will be created.\n\nType `save` to continue, `back` to edit, or `cancel` to discard it.",
                draft.display_name()
            ),
        ),
        OnboardingTargetV1::ExistingMember { display_label, .. } => (
            format!("Review declared dietary profile for {display_label}"),
            format!(
                "Save {display_label}'s declared dietary profile on this device? No profile-sync consent or remote member sync will be created.\n\nType `save` to continue, `back` to edit, or `cancel` to discard it."
            ),
        ),
        OnboardingTargetV1::NewMember {
            bounded_draft: None,
            ..
        } => (
            "Review declared dietary profile for household member".into(),
            "Household member setup is incomplete. Type `back` to finish it or `cancel` to discard it.".into(),
        ),
    };
    format!(
        "{title}\n\nDiet styles: {}\nAllergies: {}\nHealth conditions: {}\nCondition severity: {}\nAvoid ingredients: {}\nActivity: {}\nCuisines: {}\nNotes: {}\n\n{}",
        labels_and_custom(
            &profile.diet_style_ids,
            &profile.custom_diet_styles,
            diet_options()
        ),
        labels_and_custom(
            &profile.allergy_ids,
            &profile.custom_restrictions,
            allergy_options()
        ),
        labels_and_custom(
            &profile.health_condition_ids,
            &profile.custom_health_conditions,
            condition_options()
        ),
        profile
            .severity_level
            .map_or_else(|| "None".into(), |value| value.to_string()),
        display_values(&profile.avoid_ingredients),
        profile.activity_level.as_deref().map_or_else(
            || "None".into(),
            |value| labels_for(&[value.to_owned()], activity_options())
        ),
        labels_and_custom(
            &profile.cuisine_preferences,
            &profile.custom_cuisines,
            cuisine_options()
        ),
        profile.notes.clone().unwrap_or_else(|| "None".into()),
        review_action,
    )
}

fn labels_for(values: &[String], options: &[OnboardingOption]) -> String {
    if values.is_empty() {
        return "None".into();
    }
    values
        .iter()
        .map(|value| {
            options
                .iter()
                .find(|option| option.id == *value)
                .map_or(value.as_str(), |option| option.label.as_str())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn labels_and_custom(values: &[String], custom: &[String], options: &[OnboardingOption]) -> String {
    let canonical = labels_for(values, options);
    match (canonical.as_str(), custom.is_empty()) {
        ("None", true) => canonical,
        ("None", false) => custom.join(", "),
        (_, true) => canonical,
        (_, false) => format!("{canonical}, {}", custom.join(", ")),
    }
}

fn display_values(values: &[String]) -> String {
    if values.is_empty() {
        "None".into()
    } else {
        values.join(", ")
    }
}

fn submit_slash_command(model: &mut AppModel) -> Vec<Effect> {
    let command = model.draft.trim().to_owned();
    remember_prompt(model, &command);
    model.draft.clear();
    model.cursor = 0;
    let (name, arguments) = command
        .split_once(char::is_whitespace)
        .map_or((command.as_str(), ""), |(name, arguments)| {
            (name, arguments.trim())
        });
    if name == "/health" {
        push_notice(
            model,
            "Health integrations are deferred from the supported heyfood v0.6.3 contract.",
        );
        return Vec::new();
    }
    let Some(spec) = resolve_slash_command(name) else {
        push_notice(
            model,
            "Unknown command. Use /help to see the interactive command registry.",
        );
        return Vec::new();
    };
    match spec.kind {
        SlashCommandKind::Help => show_help(model),
        SlashCommandKind::Clear if model.operation.is_active() => push_notice(
            model,
            "Finish or stop the active turn before clearing the visible transcript.",
        ),
        SlashCommandKind::Clear => {
            model.scrollback.clear();
            model.activity = None;
            follow_tail(model);
        }
        SlashCommandKind::New if !arguments.is_empty() => {
            push_notice(model, &format!("Usage: {}", spec.usage));
        }
        SlashCommandKind::New if model.operation.is_active() => push_notice(
            model,
            "Stop the active turn with Ctrl+C, then run /new again.",
        ),
        SlashCommandKind::New => {
            push_notice(model, "Started a fresh conversation.");
            return vec![Effect::ResetConversation];
        }
        SlashCommandKind::Status
        | SlashCommandKind::Grocery
        | SlashCommandKind::Watch
        | SlashCommandKind::Location
        | SlashCommandKind::Voice
            if !arguments.is_empty() =>
        {
            push_notice(model, &format!("Usage: {}", spec.usage));
        }
        SlashCommandKind::Status => return open_panel(model, PanelRequest::Status),
        SlashCommandKind::Grocery => return open_panel(model, PanelRequest::Grocery),
        SlashCommandKind::Watch => return open_panel(model, PanelRequest::Watch),
        SlashCommandKind::Household if arguments.is_empty() => {
            if model.household_generation.is_some() {
                return begin_household_load(model, HouseholdLoadIntentV1::Panel);
            }
            return open_panel(model, PanelRequest::Household);
        }
        SlashCommandKind::Household if arguments.eq_ignore_ascii_case("add") => {
            return begin_household_load(model, HouseholdLoadIntentV1::AddMember);
        }
        SlashCommandKind::Household => {
            push_notice(model, "Usage: /household | /household add");
        }
        SlashCommandKind::For if arguments.is_empty() => {
            push_notice(model, &format!("Usage: {}", spec.usage));
        }
        SlashCommandKind::For => return select_household(model, arguments),
        SlashCommandKind::Profile if arguments.is_empty() => {
            return load_owner_profile_actions(model, OwnerProfileActionLoadPurposeV1::View);
        }
        SlashCommandKind::Profile if arguments == "consent" => {
            return profile_event(model, RuntimeEvent::ProfileConsentRequested);
        }
        SlashCommandKind::Profile if arguments == "retry-sync" => {
            return profile_event(model, RuntimeEvent::ProfileRetrySyncRequested);
        }
        SlashCommandKind::Profile => push_notice(model, &format!("Usage: {PROFILE_USAGE}")),
        SlashCommandKind::Onboard
            if matches!(
                model.profile_presentation_mode,
                ProfilePresentationModeV1::NativeRollbackReadOnly
            ) =>
        {
            push_notice(
                model,
                "Dietary onboarding is unavailable in native rollback read-only mode.",
            );
        }
        SlashCommandKind::Onboard if model.operation.is_active() => push_notice(
            model,
            "Finish or stop the active work before starting dietary onboarding.",
        ),
        SlashCommandKind::Onboard if arguments.is_empty() => {
            let message = "Dietary onboarding replaces your synced profile only after you review and save it.";
            if matches!(
                model.profile_presentation_mode,
                ProfilePresentationModeV1::NativeEnabled
            ) {
                begin_native_owner_onboarding(model, message);
            } else {
                begin_onboarding(model, message);
            }
        }
        SlashCommandKind::Onboard
            if arguments
                .strip_prefix("--for ")
                .is_some_and(|selector| !selector.trim().is_empty()) =>
        {
            let selector = arguments
                .strip_prefix("--for ")
                .expect("guarded --for argument")
                .trim()
                .to_owned();
            return begin_household_load(model, HouseholdLoadIntentV1::OnboardMember { selector });
        }
        SlashCommandKind::Onboard => {
            push_notice(model, "Usage: /onboard | /onboard --for <member>");
        }
        SlashCommandKind::Location => return open_panel(model, PanelRequest::Location),
        SlashCommandKind::Voice => return toggle_voice(model),
        SlashCommandKind::Exit => return begin_exit(model, ExitReason::Requested),
    }
    Vec::new()
}

fn select_household(model: &mut AppModel, selector: &str) -> Vec<Effect> {
    if model.household_generation.is_some() {
        let selector = match selector.trim().to_ascii_lowercase().as_str() {
            "me" => HouseholdSelectorV1::Me,
            "everyone" => HouseholdSelectorV1::Everyone,
            _ => HouseholdSelectorV1::Member(selector.trim().to_owned()),
        };
        return begin_household_load(model, HouseholdLoadIntentV1::SelectScope { selector });
    }
    if model.operation.is_active() {
        push_notice(
            model,
            "Finish or stop the active work before changing the household target.",
        );
        return Vec::new();
    }
    let operation_id = model.next_operation_id;
    model.next_operation_id = model.next_operation_id.saturating_add(1);
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::User,
        text: format!("/for {selector}"),
        streaming: false,
    });
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::Assistant,
        text: String::new(),
        streaming: true,
    });
    model.operation = OperationState::Running(operation_id);
    model.activity = Some("Changing household target…".into());
    model.idle_exit_armed = false;
    follow_tail(model);
    vec![Effect::SelectHousehold {
        operation_id,
        selector: selector.to_owned(),
    }]
}

fn open_panel(model: &mut AppModel, panel: PanelRequest) -> Vec<Effect> {
    if model.operation.is_active() {
        push_notice(
            model,
            "Finish or stop the active work before opening another panel.",
        );
        return Vec::new();
    }
    let operation_id = model.next_operation_id;
    model.next_operation_id = model.next_operation_id.saturating_add(1);
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::User,
        text: panel.command().into(),
        streaming: false,
    });
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::Assistant,
        text: String::new(),
        streaming: true,
    });
    model.operation = OperationState::Running(operation_id);
    model.activity = Some(format!("Loading {}…", panel.title()));
    model.idle_exit_armed = false;
    follow_tail(model);
    vec![Effect::OpenPanel {
        operation_id,
        panel,
    }]
}

fn allocate_household_operation(model: &mut AppModel) -> Option<HouseholdOperationIdV1> {
    let operation_id = model.next_household_operation_id?;
    model.next_household_operation_id = operation_id.checked_next().ok();
    Some(operation_id)
}

fn allocate_household_correlation(model: &mut AppModel) -> Option<HouseholdReducerCorrelationV1> {
    let correlation = model.next_household_correlation?;
    model.next_household_correlation = correlation.checked_next().ok();
    Some(correlation)
}

fn household_counter_exhausted(model: &mut AppModel) {
    model.household_turn_gate = HouseholdTurnGateV1::CounterExhausted;
    model.operation = OperationState::Idle;
    model.activity = None;
    push_notice(
        model,
        "Household work is unavailable because the local operation counter is exhausted. Restart with a fresh authenticated session.",
    );
}

fn begin_household_load(model: &mut AppModel, mut intent: HouseholdLoadIntentV1) -> Vec<Effect> {
    let Some(generation) = model.household_generation.clone() else {
        push_notice(
            model,
            "Native household management is unavailable in this session.",
        );
        return Vec::new();
    };
    if matches!(
        model.household_turn_gate,
        HouseholdTurnGateV1::ReconciliationRequired
    ) && matches!(intent, HouseholdLoadIntentV1::Panel)
    {
        intent = HouseholdLoadIntentV1::Bootstrap;
        push_notice(
            model,
            "Reloading the live household before the panel can open.",
        );
    } else if matches!(
        model.household_turn_gate,
        HouseholdTurnGateV1::ReconciliationRequired | HouseholdTurnGateV1::CounterExhausted
    ) {
        push_notice(
            model,
            "Household state must be reloaded before more household work can begin.",
        );
        return Vec::new();
    }
    let is_mutation_intent = matches!(
        intent,
        HouseholdLoadIntentV1::AddMember
            | HouseholdLoadIntentV1::OnboardMember { .. }
            | HouseholdLoadIntentV1::SelectScope { .. }
    );
    if is_mutation_intent && generation.mode != HouseholdPresentationModeV1::NativeEnabled {
        push_notice(
            model,
            "Household changes are unavailable in native rollback read-only mode.",
        );
        return Vec::new();
    }
    if model.operation.is_active()
        || model.pending_household_load.is_some()
        || model.pending_household_mutation.is_some()
        || model.pending_household_choice.is_some()
        || model.onboarding.is_some()
        || model.pending_confirmation.is_some()
        || model.profile_consent_review.is_some()
        || model.pending_profile_action.is_some()
    {
        push_notice(
            model,
            "Finish or cancel the active work before starting household management.",
        );
        return Vec::new();
    }
    let Some(operation_id) = allocate_household_operation(model) else {
        household_counter_exhausted(model);
        return Vec::new();
    };
    let Some(reducer_correlation) = allocate_household_correlation(model) else {
        household_counter_exhausted(model);
        return Vec::new();
    };
    let purpose = intent.purpose();
    if !matches!(intent, HouseholdLoadIntentV1::Bootstrap) {
        let command = match &intent {
            HouseholdLoadIntentV1::Panel => "/household".to_owned(),
            HouseholdLoadIntentV1::AddMember => "/household add".to_owned(),
            HouseholdLoadIntentV1::OnboardMember { selector } => {
                format!("/onboard --for {selector}")
            }
            HouseholdLoadIntentV1::SelectScope {
                selector: HouseholdSelectorV1::Me,
            } => "/for me".to_owned(),
            HouseholdLoadIntentV1::SelectScope {
                selector: HouseholdSelectorV1::Everyone,
            } => "/for everyone".to_owned(),
            HouseholdLoadIntentV1::SelectScope {
                selector: HouseholdSelectorV1::Member(selector),
            } => format!("/for {selector}"),
            HouseholdLoadIntentV1::Bootstrap => unreachable!(),
        };
        model.scrollback.push(SemanticEntry {
            speaker: Speaker::User,
            text: command,
            streaming: false,
        });
        model.scrollback.push(SemanticEntry {
            speaker: Speaker::Assistant,
            text: String::new(),
            streaming: true,
        });
    } else {
        model.household_turn_gate = HouseholdTurnGateV1::Loading;
        model.household_snapshot = None;
        model.household_chrome_label = None;
    }
    model.pending_household_load = Some(PendingHouseholdLoadV1 {
        operation_id,
        session_mode_generation: generation.session_mode_generation,
        expected_account_binding_digest: generation.account_binding_digest,
        reducer_correlation,
        intent,
        cancel_requested: false,
    });
    model.operation = OperationState::Running(operation_id.get());
    model.activity = Some(match purpose {
        HouseholdManagementLoadPurposeV1::Bootstrap => "Loading household context…".into(),
        HouseholdManagementLoadPurposeV1::Panel => "Loading Household…".into(),
        HouseholdManagementLoadPurposeV1::AddMember => {
            "Loading the current household before member setup…".into()
        }
        HouseholdManagementLoadPurposeV1::OnboardMember => {
            "Loading the current household before profile onboarding…".into()
        }
        HouseholdManagementLoadPurposeV1::SelectScope => {
            "Loading the current household before changing scope…".into()
        }
    });
    model.idle_exit_armed = false;
    follow_tail(model);
    vec![Effect::LoadHouseholdManagementV1 {
        operation_id,
        session_mode_generation: generation.session_mode_generation,
        expected_account_binding_digest: generation.account_binding_digest,
        reducer_correlation,
        purpose,
    }]
}

fn finish_household_command_stream(model: &mut AppModel, text: impl Into<String>) {
    let old_lines = model.scrollback.rendered_lines();
    let text = text.into();
    model.scrollback.mutate_last_assistant(|entry| {
        entry.text = text;
        entry.streaming = false;
    });
    account_for_new_lines(model, old_lines);
}

fn validate_household_snapshot(
    household_revision: HouseholdRevision,
    active_scope: HouseholdScope,
    members: Vec<HouseholdMemberPresentationV1>,
) -> Option<HouseholdManagementSnapshotV1> {
    if members.is_empty() || members.len() > 257 {
        return None;
    }
    let mut subjects = HashSet::with_capacity(members.len());
    let mut owner_count = 0usize;
    for member in &members {
        if !subjects.insert(member.subject().clone()) {
            return None;
        }
        match member.subject() {
            HouseholdSubjectId::Self_ => {
                owner_count = owner_count.checked_add(1)?;
                if member.relationship() != RelationshipV1::Self_
                    || member.lifecycle() != HouseholdLifecycleV1::Active
                {
                    return None;
                }
            }
            HouseholdSubjectId::Member(_) if member.relationship() == RelationshipV1::Self_ => {
                return None;
            }
            HouseholdSubjectId::Member(_) => {}
        }
    }
    if owner_count != 1 {
        return None;
    }
    match &active_scope {
        HouseholdScope::Subject(subject) => {
            let selected = members.iter().find(|member| member.subject() == subject)?;
            if selected.lifecycle() != HouseholdLifecycleV1::Active {
                return None;
            }
        }
        HouseholdScope::Everyone => {
            if members
                .iter()
                .filter(|member| member.lifecycle() == HouseholdLifecycleV1::Active)
                .count()
                < 2
            {
                return None;
            }
        }
    }
    Some(HouseholdManagementSnapshotV1 {
        household_revision,
        active_scope,
        members,
    })
}

fn household_subject_is_scope_eligible(member: &HouseholdMemberPresentationV1) -> bool {
    if member.lifecycle() != HouseholdLifecycleV1::Active {
        return false;
    }
    match member.subject() {
        HouseholdSubjectId::Self_ => matches!(
            member.profile_readiness(),
            HouseholdProfileStateV1::LocalOnly
                | HouseholdProfileStateV1::PendingSync
                | HouseholdProfileStateV1::Synced
        ),
        HouseholdSubjectId::Member(_) => {
            member.profile_readiness() == HouseholdProfileStateV1::LocalOnly
        }
    }
}

fn household_scope_label(
    snapshot: &HouseholdManagementSnapshotV1,
    scope: &HouseholdScope,
) -> Option<String> {
    match scope {
        HouseholdScope::Subject(HouseholdSubjectId::Self_) => Some("Me".into()),
        HouseholdScope::Subject(subject @ HouseholdSubjectId::Member(_)) => snapshot
            .members
            .iter()
            .find(|member| member.subject() == subject)
            .map(|member| member.display_label().to_owned()),
        HouseholdScope::Everyone => Some("Everyone".into()),
    }
}

fn active_member_matches<'a>(
    snapshot: &'a HouseholdManagementSnapshotV1,
    selector: &str,
) -> Vec<&'a HouseholdMemberPresentationV1> {
    let stable_id_match = snapshot.members.iter().find(|member| {
        member.lifecycle() == HouseholdLifecycleV1::Active
            && member
                .subject()
                .as_member()
                .is_some_and(|member_id| member_id.as_str() == selector)
    });
    if let Some(member) = stable_id_match {
        return vec![member];
    }
    snapshot
        .members
        .iter()
        .filter(|member| {
            member.lifecycle() == HouseholdLifecycleV1::Active
                && matches!(member.subject(), HouseholdSubjectId::Member(_))
                && member.display_label() == selector
        })
        .collect()
}

fn start_new_member_onboarding(
    model: &mut AppModel,
    household_revision: HouseholdRevision,
    reducer_correlation: HouseholdReducerCorrelationV1,
) {
    model.operation = OperationState::Idle;
    model.activity = None;
    model.onboarding = Some(OnboardingFlow {
        step: OnboardingStep::MemberRelationship,
        profile: OnboardingProfileInput::default(),
        copy_mode: OnboardingCopyMode::NativeLocalFirst,
        target: OnboardingTargetV1::NewMember {
            bounded_draft: None,
            expected_household_revision: household_revision,
            reducer_correlation,
        },
        member_relationship: None,
        member_name: None,
        member_age_evidence: None,
        household_correlation: Some(reducer_correlation),
    });
    finish_household_command_stream(
        model,
        "Add a household member\n\nThe member and complete declared profile will be saved locally in one transaction.",
    );
    push_onboarding_prompt(model);
}

fn start_existing_member_onboarding(
    model: &mut AppModel,
    household_revision: HouseholdRevision,
    reducer_correlation: HouseholdReducerCorrelationV1,
    member: HouseholdMemberPresentationV1,
) {
    match member.profile_readiness() {
        HouseholdProfileStateV1::Conflicted => {
            model.operation = OperationState::Idle;
            model.activity = None;
            finish_household_command_stream(
                model,
                "This member has a profile conflict. Conflict resolution is not yet available in the native TUI; nothing was changed.",
            );
        }
        HouseholdProfileStateV1::PendingSync | HouseholdProfileStateV1::Synced => {
            model.operation = OperationState::Idle;
            model.activity = None;
            finish_household_command_stream(
                model,
                "This member cannot use local member onboarding; nothing was changed.",
            );
        }
        HouseholdProfileStateV1::Incomplete | HouseholdProfileStateV1::LocalOnly => {
            let member_id = member.subject().clone();
            if !matches!(member_id, HouseholdSubjectId::Member(_)) {
                model.operation = OperationState::Idle;
                model.activity = None;
                finish_household_command_stream(
                    model,
                    "Owner onboarding uses /onboard without --for.",
                );
                return;
            }
            let display_label = member.display_label().to_owned();
            model.operation = OperationState::Idle;
            model.activity = None;
            model.onboarding = Some(OnboardingFlow {
                step: OnboardingStep::Diets,
                profile: OnboardingProfileInput::default(),
                copy_mode: OnboardingCopyMode::NativeLocalFirst,
                target: OnboardingTargetV1::ExistingMember {
                    member_id,
                    expected_household_revision: household_revision,
                    expected_profile_revision: member.profile_revision(),
                    display_label: display_label.clone(),
                },
                member_relationship: None,
                member_name: None,
                member_age_evidence: None,
                household_correlation: Some(reducer_correlation),
            });
            finish_household_command_stream(
                model,
                format!(
                    "Declared dietary profile for {display_label}\n\nThis profile will remain local to this device."
                ),
            );
            push_onboarding_prompt(model);
        }
    }
}

fn begin_duplicate_member_choice(
    model: &mut AppModel,
    purpose: HouseholdChoicePurposeV1,
    household_revision: HouseholdRevision,
    reducer_correlation: HouseholdReducerCorrelationV1,
    candidates: Vec<&HouseholdMemberPresentationV1>,
) {
    let candidates = candidates.into_iter().cloned().collect::<Vec<_>>();
    let has_indistinguishable_candidates =
        candidates.iter().enumerate().any(|(index, candidate)| {
            candidates[..index].iter().any(|other| {
                other.display_label() == candidate.display_label()
                    && other.relationship() == candidate.relationship()
            })
        });
    if has_indistinguishable_candidates {
        finish_household_command_stream(
            model,
            "More than one active member has the same display name and relationship, so hey.food can’t distinguish them safely. Make their labels unique in household management or the mobile app, then try again. Nothing was changed.",
        );
        model.pending_household_choice = None;
        model.operation = OperationState::Idle;
        model.activity = None;
        return;
    }

    let mut copy =
        String::from("More than one active member has that exact display name. Choose one:\n\n");
    for (index, candidate) in candidates.iter().enumerate() {
        let _ = writeln!(
            copy,
            " {}. {} ({})",
            index + 1,
            candidate.display_label(),
            relationship_label(candidate.relationship())
        );
    }
    copy.push_str(
        "\nType the number to continue or /cancel to stop. No member is chosen automatically.",
    );
    finish_household_command_stream(model, copy);
    model.pending_household_choice = Some(PendingHouseholdChoiceV1 {
        purpose,
        household_revision,
        reducer_correlation,
        candidates,
    });
    model.operation = OperationState::Idle;
    model.activity = None;
}

fn relationship_label(relationship: RelationshipV1) -> &'static str {
    match relationship {
        RelationshipV1::Self_ => "self",
        RelationshipV1::Spouse => "spouse",
        RelationshipV1::Partner => "partner",
        RelationshipV1::Parent => "parent",
        RelationshipV1::Child => "child",
        RelationshipV1::Sibling => "sibling",
        RelationshipV1::Grandparent => "grandparent",
        RelationshipV1::Friend => "friend",
        RelationshipV1::Other => "other",
    }
}

fn submit_household_choice(model: &mut AppModel) -> Vec<Effect> {
    let answer = std::mem::take(&mut model.draft);
    model.cursor = 0;
    if matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "cancel" | "/cancel" | "back"
    ) {
        return cancel_household_draft(model);
    }
    let Some(index) = answer
        .trim()
        .parse::<usize>()
        .ok()
        .and_then(|number| number.checked_sub(1))
    else {
        push_notice(
            model,
            "Choose one of the numbered household members or type /cancel.",
        );
        return Vec::new();
    };
    let Some(choice) = model.pending_household_choice.clone() else {
        return Vec::new();
    };
    let Some(member) = choice.candidates.get(index).cloned() else {
        push_notice(
            model,
            "That number is outside the household member choices.",
        );
        return Vec::new();
    };
    let snapshot_matches = model
        .household_snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.household_revision == choice.household_revision);
    if !snapshot_matches {
        model.pending_household_choice = None;
        push_notice(
            model,
            "The household changed before the choice. Open /household and try again.",
        );
        return Vec::new();
    }
    model.pending_household_choice = None;
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::User,
        text: format!("Choose household member #{}", index + 1),
        streaming: false,
    });
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::Assistant,
        text: String::new(),
        streaming: true,
    });
    match choice.purpose {
        HouseholdChoicePurposeV1::OnboardMember => {
            start_existing_member_onboarding(
                model,
                choice.household_revision,
                choice.reducer_correlation,
                member,
            );
            Vec::new()
        }
        HouseholdChoicePurposeV1::SelectScope => dispatch_scope_mutation(
            model,
            choice.household_revision,
            choice.reducer_correlation,
            HouseholdScope::Subject(member.subject().clone()),
            member.display_label().to_owned(),
        ),
    }
}

fn dispatch_scope_mutation(
    model: &mut AppModel,
    expected_household_revision: HouseholdRevision,
    reducer_correlation: HouseholdReducerCorrelationV1,
    selected_scope: HouseholdScope,
    bounded_active_label: String,
) -> Vec<Effect> {
    let Some(generation) = model.household_generation.clone() else {
        return Vec::new();
    };
    if generation.mode != HouseholdPresentationModeV1::NativeEnabled {
        finish_household_command_stream(
            model,
            "Household changes are unavailable in native rollback read-only mode.",
        );
        model.operation = OperationState::Idle;
        model.activity = None;
        return Vec::new();
    }
    let Some(operation_id) = allocate_household_operation(model) else {
        household_counter_exhausted(model);
        return Vec::new();
    };
    let binding = HouseholdOperationBindingV1::new(
        operation_id,
        generation.session_mode_generation,
        generation.account_binding_digest,
        expected_household_revision,
        reducer_correlation,
    );
    let affected_subject = match &selected_scope {
        HouseholdScope::Subject(subject) => Some(subject.clone()),
        HouseholdScope::Everyone => None,
    };
    model.pending_household_mutation = Some(PendingHouseholdMutationV1 {
        binding: binding.clone(),
        kind: HouseholdMutationKindV1::SelectScope,
        affected_subject,
        expected_active_scope: Some(selected_scope.clone()),
        bounded_active_label,
        affected_display_label: None,
        phase: PendingHouseholdMutationPhaseV1::Dispatched,
    });
    model.operation = OperationState::Running(operation_id.get());
    model.activity = Some("Saving the household target…".into());
    vec![Effect::SelectHouseholdScopeV1 {
        binding,
        selected_scope,
    }]
}

fn resolve_scope_selection(
    model: &mut AppModel,
    snapshot: &HouseholdManagementSnapshotV1,
    selector: HouseholdSelectorV1,
    reducer_correlation: HouseholdReducerCorrelationV1,
) -> Vec<Effect> {
    match selector {
        HouseholdSelectorV1::Me => {
            let Some(owner) = snapshot
                .members
                .iter()
                .find(|member| matches!(member.subject(), HouseholdSubjectId::Self_))
            else {
                finish_household_command_stream(
                    model,
                    "The live household did not contain Me. Nothing was changed.",
                );
                model.operation = OperationState::Idle;
                model.activity = None;
                return Vec::new();
            };
            if !household_subject_is_scope_eligible(owner) {
                finish_household_command_stream(
                    model,
                    "Me does not have a complete profile for household targeting. Nothing was changed.",
                );
                model.operation = OperationState::Idle;
                model.activity = None;
                return Vec::new();
            }
            dispatch_scope_mutation(
                model,
                snapshot.household_revision,
                reducer_correlation,
                HouseholdScope::Subject(HouseholdSubjectId::self_()),
                "Me".into(),
            )
        }
        HouseholdSelectorV1::Everyone => {
            if snapshot
                .members
                .iter()
                .filter(|member| household_subject_is_scope_eligible(member))
                .count()
                < 2
            {
                finish_household_command_stream(
                    model,
                    "Everyone requires at least two active household subjects with complete eligible profiles. Nothing was changed.",
                );
                model.operation = OperationState::Idle;
                model.activity = None;
                return Vec::new();
            }
            dispatch_scope_mutation(
                model,
                snapshot.household_revision,
                reducer_correlation,
                HouseholdScope::Everyone,
                "Everyone".into(),
            )
        }
        HouseholdSelectorV1::Member(selector) => {
            let matches = active_member_matches(snapshot, &selector);
            match matches.as_slice() {
                [] => {
                    finish_household_command_stream(
                        model,
                        "No active household member matched exactly. Nothing was changed.",
                    );
                    model.operation = OperationState::Idle;
                    model.activity = None;
                    Vec::new()
                }
                [member] => {
                    if !household_subject_is_scope_eligible(member) {
                        let copy = match member.profile_readiness() {
                            HouseholdProfileStateV1::Incomplete => {
                                "This member needs a complete declared profile. Run /onboard --for with the exact member name or ID."
                            }
                            HouseholdProfileStateV1::Conflicted => {
                                "This member has a profile conflict. Conflict resolution is not yet available in the native TUI."
                            }
                            HouseholdProfileStateV1::LocalOnly
                            | HouseholdProfileStateV1::PendingSync
                            | HouseholdProfileStateV1::Synced => {
                                "This member is not eligible for the requested scope."
                            }
                        };
                        finish_household_command_stream(
                            model,
                            format!("{copy} Nothing was changed."),
                        );
                        model.operation = OperationState::Idle;
                        model.activity = None;
                        return Vec::new();
                    }
                    dispatch_scope_mutation(
                        model,
                        snapshot.household_revision,
                        reducer_correlation,
                        HouseholdScope::Subject(member.subject().clone()),
                        member.display_label().to_owned(),
                    )
                }
                _ => {
                    begin_duplicate_member_choice(
                        model,
                        HouseholdChoicePurposeV1::SelectScope,
                        snapshot.household_revision,
                        reducer_correlation,
                        matches,
                    );
                    Vec::new()
                }
            }
        }
    }
}

fn handle_household_management_loaded(
    model: &mut AppModel,
    input: HouseholdManagementLoadedInputV1,
) -> Vec<Effect> {
    let HouseholdManagementLoadedInputV1 {
        operation_id,
        session_mode_generation,
        reducer_correlation,
        purpose,
        account_binding_digest,
        household_revision,
        active_scope,
        members,
    } = input;
    let Some(pending) = model.pending_household_load.as_ref() else {
        return Vec::new();
    };
    if pending.operation_id != operation_id
        || pending.session_mode_generation != session_mode_generation
        || pending.reducer_correlation != reducer_correlation
        || pending.intent.purpose() != purpose
        || pending.expected_account_binding_digest != account_binding_digest
        || model
            .household_generation
            .as_ref()
            .is_none_or(|generation| {
                generation.session_mode_generation != session_mode_generation
                    || generation.account_binding_digest != account_binding_digest
            })
    {
        return Vec::new();
    }
    let intent = pending.intent.clone();
    if pending.cancel_requested {
        model.pending_household_load = None;
        model.operation = OperationState::Idle;
        model.activity = None;
        if matches!(intent, HouseholdLoadIntentV1::Bootstrap) {
            model.pending_native_startup_onboarding = None;
            model.household_turn_gate = HouseholdTurnGateV1::ReconciliationRequired;
            push_notice(
                model,
                "Household bootstrap was cancelled after the current local read finished. New turns remain blocked.",
            );
        } else {
            finish_household_command_stream(
                model,
                "Household action cancelled after the current local read finished. No household mutation was dispatched.",
            );
        }
        return Vec::new();
    }
    let Some(snapshot) = validate_household_snapshot(household_revision, active_scope, members)
    else {
        model.pending_household_load = None;
        model.operation = OperationState::Idle;
        model.activity = None;
        model.household_turn_gate = HouseholdTurnGateV1::ReconciliationRequired;
        if !matches!(intent, HouseholdLoadIntentV1::Bootstrap) {
            finish_household_command_stream(
                model,
                "The household response was invalid. No household state was changed.",
            );
        } else {
            push_notice(
                model,
                "Household context could not be validated. New turns are blocked.",
            );
        }
        return Vec::new();
    };
    model.pending_household_load = None;
    model.household_snapshot = Some(snapshot.clone());
    model.household_turn_gate = household_turn_gate_for_scope(&snapshot.active_scope);
    let active_label = household_scope_label(&snapshot, &snapshot.active_scope)
        .expect("validated household scope has a presentation label");
    model.household_chrome_label = Some(active_label);
    match intent {
        HouseholdLoadIntentV1::Bootstrap => {
            model.operation = OperationState::Idle;
            model.activity = None;
            if model
                .household_generation
                .as_ref()
                .is_some_and(|generation| {
                    generation.mode == HouseholdPresentationModeV1::NativeEnabled
                })
                && let Some(message) = model.pending_native_startup_onboarding.take()
            {
                begin_native_owner_onboarding(model, &message);
            }
            Vec::new()
        }
        HouseholdLoadIntentV1::Panel => {
            let management_enabled =
                model
                    .household_generation
                    .as_ref()
                    .is_some_and(|generation| {
                        generation.mode == HouseholdPresentationModeV1::NativeEnabled
                    });
            let body = crate::render::household_panel_copy(
                &snapshot.members,
                &snapshot.active_scope,
                management_enabled,
            );
            finish_household_command_stream(model, body);
            model.operation = OperationState::Idle;
            model.activity = None;
            Vec::new()
        }
        HouseholdLoadIntentV1::AddMember => {
            start_new_member_onboarding(model, household_revision, reducer_correlation);
            Vec::new()
        }
        HouseholdLoadIntentV1::OnboardMember { selector } => {
            let matches = active_member_matches(&snapshot, &selector);
            match matches.as_slice() {
                [] => {
                    finish_household_command_stream(
                        model,
                        "No active household member matched exactly. Nothing was changed.",
                    );
                    model.operation = OperationState::Idle;
                    model.activity = None;
                }
                [member] => start_existing_member_onboarding(
                    model,
                    household_revision,
                    reducer_correlation,
                    (*member).clone(),
                ),
                _ => begin_duplicate_member_choice(
                    model,
                    HouseholdChoicePurposeV1::OnboardMember,
                    household_revision,
                    reducer_correlation,
                    matches,
                ),
            }
            Vec::new()
        }
        HouseholdLoadIntentV1::SelectScope { selector } => {
            resolve_scope_selection(model, &snapshot, selector, reducer_correlation)
        }
    }
}

fn handle_household_management_failed(
    model: &mut AppModel,
    input: HouseholdManagementFailedInputV1,
) -> Vec<Effect> {
    let HouseholdManagementFailedInputV1 {
        operation_id,
        session_mode_generation,
        reducer_correlation,
        purpose,
        account_binding_digest,
        observed_household_revision: _observed_household_revision,
        reason,
    } = input;
    let Some(pending) = model.pending_household_load.as_ref() else {
        return Vec::new();
    };
    if pending.operation_id != operation_id
        || pending.session_mode_generation != session_mode_generation
        || pending.reducer_correlation != reducer_correlation
        || pending.intent.purpose() != purpose
        || pending.expected_account_binding_digest != account_binding_digest
        || model
            .household_generation
            .as_ref()
            .is_none_or(|generation| {
                generation.session_mode_generation != session_mode_generation
                    || generation.account_binding_digest != account_binding_digest
            })
    {
        return Vec::new();
    }
    let bootstrap = matches!(pending.intent, HouseholdLoadIntentV1::Bootstrap);
    let cancel_requested = pending.cancel_requested;
    model.pending_household_load = None;
    model.operation = OperationState::Idle;
    model.activity = None;
    if cancel_requested {
        if bootstrap {
            model.pending_native_startup_onboarding = None;
            model.household_turn_gate = HouseholdTurnGateV1::ReconciliationRequired;
            push_notice(
                model,
                "Household bootstrap cancellation finished. New turns remain blocked.",
            );
        } else {
            finish_household_command_stream(
                model,
                "Household action cancelled. No household mutation was dispatched.",
            );
        }
        return Vec::new();
    }
    if bootstrap {
        model.household_turn_gate = HouseholdTurnGateV1::ReconciliationRequired;
        push_notice(
            model,
            "Household context could not be loaded. New turns are blocked until a fresh authenticated household bootstrap succeeds.",
        );
    } else {
        let copy = match reason {
            HouseholdManagementFailureV1::AccountChanged => {
                "The authenticated account changed before the household loaded."
            }
            HouseholdManagementFailureV1::ModeChanged => {
                "The household mode changed before the household loaded."
            }
            HouseholdManagementFailureV1::StateChanged => {
                "The household changed while it was loading."
            }
            HouseholdManagementFailureV1::Unavailable => {
                "Native household management is unavailable."
            }
            HouseholdManagementFailureV1::MalformedPresentation => {
                "The household response could not be validated."
            }
        };
        finish_household_command_stream(model, format!("{copy} Nothing was changed."));
    }
    Vec::new()
}

fn dispatch_member_onboarding_save(
    model: &mut AppModel,
    profile: OnboardingProfileInput,
) -> Vec<Effect> {
    let Some(flow) = model.onboarding.as_ref() else {
        return Vec::new();
    };
    let Some(generation) = model.household_generation.clone() else {
        model.onboarding = None;
        push_notice(
            model,
            "The household session ended before save. Nothing was changed.",
        );
        return Vec::new();
    };
    if generation.mode != HouseholdPresentationModeV1::NativeEnabled {
        model.onboarding = None;
        push_notice(
            model,
            "Household changes are unavailable in native rollback read-only mode.",
        );
        return Vec::new();
    }
    let Some(snapshot) = model.household_snapshot.clone() else {
        model.onboarding = None;
        push_notice(
            model,
            "The household must be reloaded before save. Nothing was changed.",
        );
        return Vec::new();
    };
    let Some(reducer_correlation) = flow.household_correlation else {
        model.onboarding = None;
        push_notice(
            model,
            "Household member setup lost its local binding. Nothing was changed.",
        );
        return Vec::new();
    };
    let target = flow.target.clone();
    let (expected_revision, label) = match &target {
        OnboardingTargetV1::NewMember {
            bounded_draft: Some(draft),
            expected_household_revision,
            ..
        } => (
            *expected_household_revision,
            draft.display_name().to_owned(),
        ),
        OnboardingTargetV1::ExistingMember {
            expected_household_revision,
            display_label,
            ..
        } => (*expected_household_revision, display_label.clone()),
        OnboardingTargetV1::Owner
        | OnboardingTargetV1::NewMember {
            bounded_draft: None,
            ..
        } => {
            model.onboarding = None;
            push_notice(
                model,
                "Household member setup is incomplete. Nothing was changed.",
            );
            return Vec::new();
        }
    };
    if snapshot.household_revision != expected_revision {
        model.onboarding = None;
        push_notice(
            model,
            "The household changed before save. Nothing was added. Open /household and try again.",
        );
        return Vec::new();
    }
    let Some(operation_id) = allocate_household_operation(model) else {
        model.onboarding = None;
        household_counter_exhausted(model);
        return Vec::new();
    };
    let binding = HouseholdOperationBindingV1::new(
        operation_id,
        generation.session_mode_generation,
        generation.account_binding_digest,
        expected_revision,
        reducer_correlation,
    );
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::User,
        text: format!("Save declared dietary profile for {label}"),
        streaming: false,
    });
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::Assistant,
        text: String::new(),
        streaming: true,
    });
    model.operation = OperationState::Running(operation_id.get());
    model.activity = Some("Saving the local household profile…".into());
    model.idle_exit_armed = false;
    follow_tail(model);
    match target {
        OnboardingTargetV1::NewMember {
            bounded_draft: Some(bounded_member_draft),
            ..
        } => {
            model.pending_household_mutation = Some(PendingHouseholdMutationV1 {
                binding: binding.clone(),
                kind: HouseholdMutationKindV1::CreateMember,
                affected_subject: None,
                expected_active_scope: None,
                bounded_active_label: label,
                affected_display_label: None,
                phase: PendingHouseholdMutationPhaseV1::Dispatched,
            });
            vec![Effect::CreateMemberWithDeclaredProfileV1 {
                binding,
                bounded_member_draft,
                onboarding_profile_input: Box::new(profile),
            }]
        }
        OnboardingTargetV1::ExistingMember {
            member_id,
            expected_profile_revision,
            ..
        } => {
            if !matches!(member_id, HouseholdSubjectId::Member(_)) {
                model.pending_household_mutation = None;
                model.onboarding = None;
                model.operation = OperationState::Idle;
                model.activity = None;
                finish_household_command_stream(
                    model,
                    "Owner onboarding must use /onboard. Nothing was changed.",
                );
                return Vec::new();
            }
            let Some(active_label) = household_scope_label(&snapshot, &snapshot.active_scope)
            else {
                model.onboarding = None;
                model.operation = OperationState::Idle;
                model.activity = None;
                return Vec::new();
            };
            model.pending_household_mutation = Some(PendingHouseholdMutationV1 {
                binding: binding.clone(),
                kind: HouseholdMutationKindV1::SaveMemberProfile,
                affected_subject: Some(member_id.clone()),
                expected_active_scope: Some(snapshot.active_scope),
                bounded_active_label: active_label,
                affected_display_label: Some(label),
                phase: PendingHouseholdMutationPhaseV1::Dispatched,
            });
            vec![Effect::SaveMemberDeclaredProfileV1 {
                binding,
                subject: member_id,
                expected_profile_revision,
                onboarding_profile_input: Box::new(profile),
            }]
        }
        OnboardingTargetV1::Owner
        | OnboardingTargetV1::NewMember {
            bounded_draft: None,
            ..
        } => unreachable!("target shape checked before operation allocation"),
    }
}

fn handle_household_mutation_committed(
    model: &mut AppModel,
    binding: HouseholdOperationBindingV1,
    kind: HouseholdMutationKindV1,
    resulting_household_revision: HouseholdRevision,
    affected_subject: Option<HouseholdSubjectId>,
    active_scope: HouseholdScope,
    bounded_active_label: String,
) -> Vec<Effect> {
    let Some(pending) = model.pending_household_mutation.as_mut() else {
        return Vec::new();
    };
    if pending.binding != binding
        || pending.kind != kind
        || !matches!(
            pending.phase,
            PendingHouseholdMutationPhaseV1::Dispatched
                | PendingHouseholdMutationPhaseV1::Cancelling
        )
        || binding.expected_household_revision().checked_next().ok()
            != Some(resulting_household_revision)
        || model
            .household_generation
            .as_ref()
            .is_none_or(|generation| {
                generation.session_mode_generation != binding.session_mode_generation()
                    || generation.account_binding_digest != binding.account_binding_digest()
            })
    {
        return Vec::new();
    }
    let Ok(validated_label) = required_text(&bounded_active_label, 80) else {
        return Vec::new();
    };
    if validated_label != bounded_active_label
        || pending.bounded_active_label != bounded_active_label
    {
        return Vec::new();
    }
    let evidence_valid = match kind {
        HouseholdMutationKindV1::CreateMember => {
            let Some(subject @ HouseholdSubjectId::Member(_)) = affected_subject.as_ref() else {
                return Vec::new();
            };
            active_scope == HouseholdScope::Subject(subject.clone())
        }
        HouseholdMutationKindV1::SaveMemberProfile => {
            pending.affected_subject == affected_subject
                && pending.expected_active_scope.as_ref() == Some(&active_scope)
        }
        HouseholdMutationKindV1::SelectScope => {
            pending.affected_subject == affected_subject
                && pending.expected_active_scope.as_ref() == Some(&active_scope)
        }
    };
    if !evidence_valid {
        return Vec::new();
    }
    pending.affected_subject = affected_subject.clone();
    pending.expected_active_scope = Some(active_scope.clone());
    pending.phase = PendingHouseholdMutationPhaseV1::Finishing {
        resulting_household_revision,
    };
    model.operation = OperationState::Finishing(binding.operation_id().get());
    model.activity = Some("Applying the committed household context…".into());
    vec![Effect::ApplyCommittedHouseholdContextV1 {
        binding,
        resulting_household_revision,
        affected_subject,
        active_scope,
        bounded_active_label,
    }]
}

fn handle_household_mutation_failed(
    model: &mut AppModel,
    binding: HouseholdOperationBindingV1,
    kind: HouseholdMutationKindV1,
    affected_subject: Option<HouseholdSubjectId>,
    _observed_household_revision: Option<HouseholdRevision>,
    reason: HouseholdMutationFailureV1,
) -> Vec<Effect> {
    let Some(pending) = model.pending_household_mutation.as_ref() else {
        return Vec::new();
    };
    if pending.binding != binding
        || pending.kind != kind
        || (pending.affected_subject.is_some() && pending.affected_subject != affected_subject)
        || matches!(
            pending.phase,
            PendingHouseholdMutationPhaseV1::Finishing { .. }
        )
        || model
            .household_generation
            .as_ref()
            .is_none_or(|generation| {
                generation.session_mode_generation != binding.session_mode_generation()
                    || generation.account_binding_digest != binding.account_binding_digest()
            })
    {
        return Vec::new();
    }
    model.pending_household_mutation = None;
    model.onboarding = None;
    model.operation = OperationState::Idle;
    model.activity = None;
    let text = match reason {
        HouseholdMutationFailureV1::BeforeCommitCancelled => {
            "Household change cancelled before save. Nothing was changed."
        }
        HouseholdMutationFailureV1::StaleRevision
            if kind == HouseholdMutationKindV1::CreateMember =>
        {
            "The household changed before save. Nothing was added. Open /household and try again."
        }
        HouseholdMutationFailureV1::StaleRevision => {
            "The household changed before save. Nothing was changed. Open /household and try again."
        }
        HouseholdMutationFailureV1::Ineligible => {
            "The selected household subject is no longer eligible. Nothing was changed."
        }
        HouseholdMutationFailureV1::ConflictResolutionRequired => {
            "This member has a profile conflict. Conflict resolution is not yet available in the native TUI; nothing was changed."
        }
        HouseholdMutationFailureV1::OutcomeUncertain => {
            "The household save outcome is uncertain. No success is being shown while the live household is reloaded."
        }
        HouseholdMutationFailureV1::Unavailable => {
            "Native household management became unavailable. No success is being shown."
        }
    };
    finish_household_command_stream(model, text);
    if reason == HouseholdMutationFailureV1::OutcomeUncertain {
        model.household_turn_gate = HouseholdTurnGateV1::Loading;
        return begin_household_load(model, HouseholdLoadIntentV1::Bootstrap);
    }
    Vec::new()
}

fn clear_subject_bound_transients(model: &mut AppModel) {
    model.pending_confirmation = None;
    model.pending_choice_labels.clear();
    model.pending_agent_partial.clear();
    model.pending_household_choice = None;
    model.profile_consent_review = None;
    model.owner_profile_actions = None;
    model.pending_profile_action = None;
    model.voice_phase = VoicePhase::Idle;
    model.draft_before_voice.clear();
    model.draft.clear();
    model.cursor = 0;
    model.scrollback.clear();
    follow_tail(model);
}

fn handle_household_context_applied(
    model: &mut AppModel,
    binding: HouseholdOperationBindingV1,
    resulting_household_revision: HouseholdRevision,
    active_scope: HouseholdScope,
    bounded_active_label: String,
) -> Vec<Effect> {
    let Some(pending) = model.pending_household_mutation.clone() else {
        return Vec::new();
    };
    if pending.binding != binding
        || pending.expected_active_scope.as_ref() != Some(&active_scope)
        || pending.bounded_active_label != bounded_active_label
        || !matches!(
            pending.phase,
            PendingHouseholdMutationPhaseV1::Finishing {
                resulting_household_revision: expected
            } if expected == resulting_household_revision
        )
        || model
            .household_generation
            .as_ref()
            .is_none_or(|generation| {
                generation.session_mode_generation != binding.session_mode_generation()
                    || generation.account_binding_digest != binding.account_binding_digest()
            })
    {
        return Vec::new();
    }
    let Some(mut snapshot) = model.household_snapshot.clone() else {
        return Vec::new();
    };
    snapshot.household_revision = resulting_household_revision;
    snapshot.active_scope = active_scope.clone();
    match pending.kind {
        HouseholdMutationKindV1::CreateMember => {
            let Some(HouseholdSubjectId::Member(member_id)) = pending.affected_subject.clone()
            else {
                return Vec::new();
            };
            let Some(OnboardingFlow {
                target:
                    OnboardingTargetV1::NewMember {
                        bounded_draft: Some(draft),
                        ..
                    },
                ..
            }) = model.onboarding.as_ref()
            else {
                return Vec::new();
            };
            let presentation = HouseholdMemberPresentationV1::new(
                HouseholdSubjectId::member(member_id),
                draft.display_name(),
                draft.relationship(),
                HouseholdLifecycleV1::Active,
                HouseholdProfileStateV1::LocalOnly,
                ProfileRevision::new(1).ok(),
            )
            .ok();
            let Some(presentation) = presentation else {
                return Vec::new();
            };
            snapshot.members.push(presentation);
        }
        HouseholdMutationKindV1::SaveMemberProfile => {
            let Some(subject) = pending.affected_subject.as_ref() else {
                return Vec::new();
            };
            let Some(member) = snapshot
                .members
                .iter_mut()
                .find(|member| member.subject() == subject)
            else {
                return Vec::new();
            };
            member.profile_readiness = HouseholdProfileStateV1::LocalOnly;
            member.profile_revision = match member.profile_revision {
                Some(revision) => revision.checked_next().ok(),
                None => ProfileRevision::new(1).ok(),
            };
            if member.profile_revision.is_none() {
                return Vec::new();
            }
        }
        HouseholdMutationKindV1::SelectScope => {}
    }
    if validate_household_snapshot(
        snapshot.household_revision,
        snapshot.active_scope.clone(),
        snapshot.members.clone(),
    )
    .is_none()
    {
        return Vec::new();
    }
    model.household_snapshot = Some(snapshot);
    model.household_chrome_label = Some(bounded_active_label.clone());
    model.household_turn_gate = household_turn_gate_for_scope(&active_scope);
    model.pending_household_mutation = None;
    model.onboarding = None;
    model.operation = OperationState::Idle;
    model.activity = None;
    clear_subject_bound_transients(model);
    let copy = match pending.kind {
        HouseholdMutationKindV1::CreateMember => format!(
            "Added {bounded_active_label}. Their declared dietary profile is saved on this device. For: {bounded_active_label}"
        ),
        HouseholdMutationKindV1::SaveMemberProfile => {
            let affected_label = pending
                .affected_display_label
                .as_deref()
                .unwrap_or("household member");
            format!(
                "Saved {affected_label}'s declared dietary profile on this device. For: {bounded_active_label}"
            )
        }
        HouseholdMutationKindV1::SelectScope => {
            format!("Household target changed. For: {bounded_active_label}")
        }
    };
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::Assistant,
        text: copy,
        streaming: false,
    });
    follow_tail(model);
    Vec::new()
}

fn handle_household_context_apply_failed(
    model: &mut AppModel,
    binding: HouseholdOperationBindingV1,
    resulting_household_revision: HouseholdRevision,
    _reason: HouseholdContextApplyFailureV1,
) -> Vec<Effect> {
    let Some(pending) = model.pending_household_mutation.as_ref() else {
        return Vec::new();
    };
    if pending.binding != binding
        || !matches!(
            pending.phase,
            PendingHouseholdMutationPhaseV1::Finishing {
                resulting_household_revision: expected
            } if expected == resulting_household_revision
        )
        || model
            .household_generation
            .as_ref()
            .is_none_or(|generation| {
                generation.session_mode_generation != binding.session_mode_generation()
                    || generation.account_binding_digest != binding.account_binding_digest()
            })
    {
        return Vec::new();
    }
    model.pending_household_mutation = None;
    model.onboarding = None;
    model.operation = OperationState::Idle;
    model.activity = None;
    model.household_turn_gate = HouseholdTurnGateV1::Loading;
    finish_household_command_stream(
        model,
        "The household save committed, but its process context could not be applied. No success is being shown while the live household is reloaded.",
    );
    begin_household_load(model, HouseholdLoadIntentV1::Bootstrap)
}

fn load_owner_profile_actions(
    model: &mut AppModel,
    purpose: OwnerProfileActionLoadPurposeV1,
) -> Vec<Effect> {
    if model.operation.is_active()
        || model.profile_consent_review.is_some()
        || model.pending_profile_action.is_some()
    {
        push_notice(
            model,
            "Finish or stop the active work before opening another Profile action.",
        );
        return Vec::new();
    }
    let operation_id = model.next_operation_id;
    model.next_operation_id = model.next_operation_id.saturating_add(1);
    let command = match purpose {
        OwnerProfileActionLoadPurposeV1::View => "/profile",
        OwnerProfileActionLoadPurposeV1::ExplicitRetry => "/profile retry-sync",
    };
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::User,
        text: command.into(),
        streaming: false,
    });
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::Assistant,
        text: String::new(),
        streaming: true,
    });
    model.operation = OperationState::Running(operation_id);
    model.activity = Some(match purpose {
        OwnerProfileActionLoadPurposeV1::View => "Loading Dietary profile…".into(),
        OwnerProfileActionLoadPurposeV1::ExplicitRetry => {
            "Checking the exact saved owner profile sync…".into()
        }
    });
    model.pending_profile_action = Some(PendingProfileActionV1::Loading {
        operation_id,
        purpose,
        mode: model.profile_presentation_mode,
    });
    model.idle_exit_armed = false;
    follow_tail(model);
    vec![Effect::LoadOwnerProfileActionsV1 {
        operation_id,
        purpose,
    }]
}

fn submit_profile_consent_review(model: &mut AppModel) -> Vec<Effect> {
    if !matches!(
        model.profile_consent_review,
        Some(ProfileConsentReview::Reviewing)
    ) {
        return Vec::new();
    }
    let answer = model.draft.trim().to_ascii_lowercase();
    match answer.as_str() {
        "y" | "yes" => profile_event(model, RuntimeEvent::ProfileConsentConfirmed),
        "n" | "no" | "cancel" => profile_event(model, RuntimeEvent::ProfileConsentCancelled),
        _ => {
            push_notice(
                model,
                &crate::render::profile_copy(ProfileCopyStateV1::ConsentReviewPrompt),
            );
            Vec::new()
        }
    }
}

fn profile_event(model: &mut AppModel, event: RuntimeEvent) -> Vec<Effect> {
    match event {
        RuntimeEvent::ProfileConsentRequested => {
            if !matches!(
                model.profile_presentation_mode,
                ProfilePresentationModeV1::NativeEnabled
            ) {
                push_notice(
                    model,
                    "Profile-sync consent review is unavailable in this mode.",
                );
                return Vec::new();
            }
            if model.operation.is_active()
                || model.onboarding.is_some()
                || model.pending_confirmation.is_some()
                || model.pending_profile_action.is_some()
                || model.profile_consent_review.is_some()
            {
                push_notice(
                    model,
                    "Finish or stop the active work before reviewing profile-sync consent.",
                );
                return Vec::new();
            }
            model.draft.clear();
            model.cursor = 0;
            model.profile_consent_review = Some(ProfileConsentReview::Reviewing);
            model.scrollback.push(SemanticEntry {
                speaker: Speaker::User,
                text: "/profile consent".into(),
                streaming: false,
            });
            model.scrollback.push(SemanticEntry {
                speaker: Speaker::Assistant,
                text: format!(
                    "{}\n\n{}",
                    crate::render::profile_copy(ProfileCopyStateV1::ConsentReview),
                    crate::render::profile_copy(ProfileCopyStateV1::ConsentReviewPrompt)
                ),
                streaming: false,
            });
            model.activity = None;
            model.idle_exit_armed = false;
            follow_tail(model);
            Vec::new()
        }
        RuntimeEvent::ProfileConsentConfirmed
            if matches!(
                model.profile_consent_review,
                Some(ProfileConsentReview::Reviewing)
            ) && matches!(
                model.profile_presentation_mode,
                ProfilePresentationModeV1::NativeEnabled
            ) =>
        {
            model.draft.clear();
            model.cursor = 0;
            let operation_id = model.next_operation_id;
            model.next_operation_id = model.next_operation_id.saturating_add(1);
            model.profile_consent_review = Some(ProfileConsentReview::Granting { operation_id });
            model.scrollback.push(SemanticEntry {
                speaker: Speaker::User,
                text: "Grant consent".into(),
                streaming: false,
            });
            model.scrollback.push(SemanticEntry {
                speaker: Speaker::Assistant,
                text: String::new(),
                streaming: true,
            });
            model.operation = OperationState::Running(operation_id);
            model.activity = Some("Granting profile-sync consent…".into());
            model.idle_exit_armed = false;
            follow_tail(model);
            vec![Effect::GrantOwnerProfileConsentV1 { operation_id }]
        }
        RuntimeEvent::ProfileConsentConfirmed => Vec::new(),
        RuntimeEvent::ProfileConsentCancelled
            if matches!(
                model.profile_consent_review,
                Some(ProfileConsentReview::Reviewing)
            ) =>
        {
            model.draft.clear();
            model.cursor = 0;
            model.profile_consent_review = None;
            model.scrollback.push(SemanticEntry {
                speaker: Speaker::Assistant,
                text: crate::render::profile_copy(ProfileCopyStateV1::ConsentCancelled),
                streaming: false,
            });
            model.activity = None;
            model.idle_exit_armed = false;
            follow_tail(model);
            Vec::new()
        }
        RuntimeEvent::ProfileConsentCancelled => Vec::new(),
        RuntimeEvent::ProfileConsentFinished {
            operation_id,
            result,
        } if matches!(
            model.profile_consent_review,
            Some(ProfileConsentReview::Granting {
                operation_id: current
            }) if current == operation_id
        ) && matches!(
            model.profile_presentation_mode,
            ProfilePresentationModeV1::NativeEnabled
        ) && matches!(
            model.operation,
            OperationState::Running(current) | OperationState::Cancelling(current)
                if current == operation_id
        ) =>
        {
            model.profile_consent_review = None;
            let text = match result {
                Ok(finished) => {
                    let granted = crate::render::profile_copy(ProfileCopyStateV1::ConsentGranted {
                        consent_version: finished.consent_version,
                    });
                    if finished.retry_offered {
                        format!(
                            "{granted}\n\n{}",
                            crate::render::profile_copy(ProfileCopyStateV1::RetryOffered {
                                consent_version: finished.consent_version,
                            })
                        )
                    } else {
                        granted
                    }
                }
                Err(ProfileConsentFailureV1::Cancelled) => {
                    crate::render::profile_copy(ProfileCopyStateV1::ConsentCancelled)
                }
                Err(ProfileConsentFailureV1::Unavailable) => {
                    "Profile-sync consent is unavailable for this account or mode.".into()
                }
                Err(ProfileConsentFailureV1::Uncertain) => {
                    "Profile-sync consent outcome is uncertain. Open /profile before trying again."
                        .into()
                }
                Err(ProfileConsentFailureV1::MalformedResponse) => {
                    "Profile-sync consent could not be verified.".into()
                }
            };
            finish_profile_action(model, operation_id, text);
            Vec::new()
        }
        RuntimeEvent::ProfileConsentFinished { .. } => Vec::new(),
        RuntimeEvent::ProfileRetrySyncRequested => {
            if !matches!(
                model.profile_presentation_mode,
                ProfilePresentationModeV1::NativeEnabled
            ) {
                push_notice(
                    model,
                    "Owner profile sync retry is unavailable in this mode.",
                );
                return Vec::new();
            }
            load_owner_profile_actions(model, OwnerProfileActionLoadPurposeV1::ExplicitRetry)
        }
        RuntimeEvent::ProfileActionsLoaded {
            operation_id,
            loaded,
        } => profile_actions_loaded(model, operation_id, loaded),
        RuntimeEvent::ProfileRetrySyncFinished {
            operation_id,
            outcome,
        } if matches!(
            model.pending_profile_action,
            Some(PendingProfileActionV1::Retrying {
                operation_id: current,
                mode: ProfilePresentationModeV1::NativeEnabled,
            }) if current == operation_id
        ) && matches!(
            model.profile_presentation_mode,
            ProfilePresentationModeV1::NativeEnabled
        ) && matches!(
            model.operation,
            OperationState::Running(current) | OperationState::Cancelling(current)
                if current == operation_id
        ) =>
        {
            model.pending_profile_action = None;
            let text = match outcome {
                ProfileRetrySyncFinishedV1::SyncPending => {
                    crate::render::profile_copy(ProfileCopyStateV1::SyncPending)
                }
                ProfileRetrySyncFinishedV1::Interrupted => {
                    crate::render::profile_copy(ProfileCopyStateV1::InterruptedRetry)
                }
                ProfileRetrySyncFinishedV1::ConsentVersionChangedRequiresNewSave => {
                    crate::render::profile_copy(ProfileCopyStateV1::ConsentVersionChanged)
                }
                ProfileRetrySyncFinishedV1::ConsentRevokedRegrantRequired => {
                    crate::render::profile_copy(ProfileCopyStateV1::ConsentRevoked)
                }
                ProfileRetrySyncFinishedV1::Unavailable { reason } => {
                    unavailable_retry_copy(reason)
                }
            };
            finish_profile_action(model, operation_id, text);
            Vec::new()
        }
        RuntimeEvent::ProfileRetrySyncFinished { .. } => Vec::new(),
        _ => unreachable!("profile_event accepts only Profile events"),
    }
}

fn profile_actions_loaded(
    model: &mut AppModel,
    operation_id: u64,
    loaded: ProfileActionsLoadedV1,
) -> Vec<Effect> {
    let Some(PendingProfileActionV1::Loading {
        operation_id: current,
        purpose,
        mode,
    }) = model.pending_profile_action
    else {
        return Vec::new();
    };
    if current != operation_id || model.profile_presentation_mode != mode {
        return Vec::new();
    }
    if model.operation == OperationState::Cancelling(operation_id) {
        model.pending_profile_action = None;
        let text = match purpose {
            OwnerProfileActionLoadPurposeV1::View => "Profile action cancelled.",
            OwnerProfileActionLoadPurposeV1::ExplicitRetry => {
                "Owner profile sync retry was cancelled before it started."
            }
        };
        finish_profile_action(model, operation_id, text.into());
        return Vec::new();
    }
    if model.operation != OperationState::Running(operation_id) {
        return Vec::new();
    }
    match (purpose, loaded) {
        (OwnerProfileActionLoadPurposeV1::View, ProfileActionsLoadedV1::LegacyPanel { body })
            if matches!(mode, ProfilePresentationModeV1::LegacyCompatibility) =>
        {
            model.pending_profile_action = None;
            finish_profile_action(model, operation_id, legacy_profile_panel_text(&body));
            Vec::new()
        }
        (OwnerProfileActionLoadPurposeV1::View, ProfileActionsLoadedV1::NativeActions(actions))
            if matches!(
                mode,
                ProfilePresentationModeV1::NativeEnabled
                    | ProfilePresentationModeV1::NativeRollbackReadOnly
            ) =>
        {
            let text = native_profile_actions_copy(&actions);
            model.owner_profile_actions = Some(actions);
            model.pending_profile_action = None;
            finish_profile_action(model, operation_id, text);
            Vec::new()
        }
        (
            OwnerProfileActionLoadPurposeV1::ExplicitRetry,
            ProfileActionsLoadedV1::NativeActions(actions),
        ) => {
            if !matches!(mode, ProfilePresentationModeV1::NativeEnabled) {
                return neutralize_loaded_profile_action(model, operation_id);
            }
            let Some(action) = actions.retry.available_action() else {
                let text = native_profile_actions_copy(&actions);
                model.owner_profile_actions = Some(actions);
                model.pending_profile_action = None;
                finish_profile_action(model, operation_id, text);
                return Vec::new();
            };
            let Some(intent) = actions.intent.clone() else {
                model.owner_profile_actions = Some(actions);
                model.pending_profile_action = None;
                finish_profile_action(
                    model,
                    operation_id,
                    "Owner profile sync retry is unavailable.".into(),
                );
                return Vec::new();
            };
            model.owner_profile_actions = Some(actions);
            model.pending_profile_action =
                Some(PendingProfileActionV1::Retrying { operation_id, mode });
            model.activity = Some("Retrying the exact saved owner profile sync…".into());
            vec![Effect::RetryOwnerProfileSyncV1 {
                operation_id,
                action,
                intent,
            }]
        }
        (
            OwnerProfileActionLoadPurposeV1::ExplicitRetry,
            ProfileActionsLoadedV1::LegacyPanel { .. },
        ) => {
            model.pending_profile_action = None;
            finish_profile_action(
                model,
                operation_id,
                "Owner profile sync retry is unavailable.".into(),
            );
            Vec::new()
        }
        (OwnerProfileActionLoadPurposeV1::View, _) => {
            neutralize_loaded_profile_action(model, operation_id)
        }
    }
}

fn neutralize_loaded_profile_action(model: &mut AppModel, operation_id: u64) -> Vec<Effect> {
    model.pending_profile_action = None;
    finish_profile_action(
        model,
        operation_id,
        "Profile actions are unavailable in this mode.".into(),
    );
    Vec::new()
}

fn native_profile_actions_copy(actions: &OwnerProfileActionEligibilityV1) -> String {
    let state = match actions.retry {
        OwnerProfileRetryEligibilityV1::StartLocalOnlyAfterConsent => actions
            .active_consent_version
            .map_or(ProfileCopyStateV1::RetryUnavailable, |consent_version| {
                ProfileCopyStateV1::RetryOffered { consent_version }
            }),
        OwnerProfileRetryEligibilityV1::ResumeNeedsConsentCheck
        | OwnerProfileRetryEligibilityV1::ResumeNeedsRemoteBase
        | OwnerProfileRetryEligibilityV1::ResumeReadyToDispatch
        | OwnerProfileRetryEligibilityV1::ReconcileDispatchingOutcomeUnknown
        | OwnerProfileRetryEligibilityV1::ReconcileOutcomeUncertain => {
            ProfileCopyStateV1::InterruptedRetry
        }
        OwnerProfileRetryEligibilityV1::Unavailable {
            reason: OwnerProfileRetryUnavailableReasonV1::ConsentVersionChangedRequiresNewSave,
        } => ProfileCopyStateV1::ConsentVersionChanged,
        OwnerProfileRetryEligibilityV1::Unavailable {
            reason: OwnerProfileRetryUnavailableReasonV1::ConsentRevokedRegrantRequired,
        } => ProfileCopyStateV1::ConsentRevoked,
        OwnerProfileRetryEligibilityV1::Unavailable { .. } => ProfileCopyStateV1::RetryUnavailable,
    };
    crate::render::profile_copy(state)
}

fn unavailable_retry_copy(reason: OwnerProfileRetryUnavailableReasonV1) -> String {
    match reason {
        OwnerProfileRetryUnavailableReasonV1::ConsentVersionChangedRequiresNewSave => {
            crate::render::profile_copy(ProfileCopyStateV1::ConsentVersionChanged)
        }
        OwnerProfileRetryUnavailableReasonV1::ConsentRevokedRegrantRequired => {
            crate::render::profile_copy(ProfileCopyStateV1::ConsentRevoked)
        }
        _ => "Owner profile sync retry is unavailable.".into(),
    }
}

fn legacy_profile_panel_text(body: &str) -> String {
    let body = terminal_safe_text(body);
    if body.trim().is_empty() {
        "Dietary profile\n\nNo information is available.".into()
    } else {
        format!("Dietary profile\n\n{}", body.trim_end())
    }
}

fn finish_profile_action(model: &mut AppModel, operation_id: u64, text: String) {
    if model.operation.operation_id() != Some(operation_id) {
        return;
    }
    let old_lines = model.scrollback.rendered_lines();
    model.scrollback.mutate_last_assistant(|entry| {
        entry.text = text;
        entry.streaming = false;
    });
    model.operation = OperationState::Idle;
    model.activity = None;
    model.idle_exit_armed = false;
    account_for_new_lines(model, old_lines);
}

fn show_help(model: &mut AppModel) {
    let mut help = String::from("Commands\n");
    for spec in SLASH_COMMAND_REGISTRY {
        let _ = writeln!(help, "  {:<14} {}", spec.usage, spec.description);
    }
    help.push_str(
        "\nKeys\n  Enter send/stop recording · Shift+Enter/Ctrl+J newline · Up/Down history\n  Ctrl+Space/F8 voice · Esc cancel voice · Tab complete\n  PageUp/PageDown scroll · End follow · Ctrl+C stop · Ctrl+D exit",
    );
    push_notice(model, &help);
}

fn push_notice(model: &mut AppModel, text: &str) {
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::Notice,
        text: text.into(),
        streaming: false,
    });
    follow_tail(model);
}

fn remember_prompt(model: &mut AppModel, prompt: &str) {
    if model
        .prompt_history
        .back()
        .is_none_or(|last| last != prompt)
    {
        model.prompt_history.push_back(prompt.to_owned());
        while model.prompt_history.len() > MAX_PROMPT_HISTORY {
            model.prompt_history.pop_front();
        }
    }
    reset_history_navigation(model);
}

fn reset_history_navigation(model: &mut AppModel) {
    model.history_index = None;
    model.history_draft.clear();
}

fn history_previous(model: &mut AppModel) {
    if model.prompt_history.is_empty() {
        return;
    }
    let next = match model.history_index {
        None => {
            model.history_draft = model.draft.clone();
            model.prompt_history.len() - 1
        }
        Some(index) => index.saturating_sub(1),
    };
    model.history_index = Some(next);
    model.draft = model.prompt_history[next].clone();
    model.cursor = model.draft.chars().count();
}

fn history_next(model: &mut AppModel) {
    let Some(index) = model.history_index else {
        return;
    };
    if index + 1 < model.prompt_history.len() {
        let next = index + 1;
        model.history_index = Some(next);
        model.draft = model.prompt_history[next].clone();
    } else {
        model.history_index = None;
        model.draft = std::mem::take(&mut model.history_draft);
    }
    model.cursor = model.draft.chars().count();
}

fn complete_slash(model: &mut AppModel) {
    let suggestions = slash_suggestions(model, 2);
    if let [spec] = suggestions.as_slice() {
        model.draft = spec.name.to_owned();
        model.cursor = model.draft.chars().count();
    }
}

fn has_active_household_work(model: &AppModel) -> bool {
    model.pending_household_load.is_some()
        || model.pending_household_mutation.is_some()
        || model.pending_household_choice.is_some()
        || model
            .onboarding
            .as_ref()
            .is_some_and(|flow| !matches!(flow.target, OnboardingTargetV1::Owner))
}

fn has_pending_household_driver_operation(model: &AppModel) -> bool {
    model.pending_household_load.is_some() || model.pending_household_mutation.is_some()
}

fn cancel_household_draft(model: &mut AppModel) -> Vec<Effect> {
    model.draft.clear();
    model.cursor = 0;
    if let Some(pending) = model.pending_household_mutation.as_mut() {
        match pending.phase {
            PendingHouseholdMutationPhaseV1::Dispatched => {
                pending.phase = PendingHouseholdMutationPhaseV1::Cancelling;
                model.operation = OperationState::Cancelling(pending.binding.operation_id().get());
                model.activity = Some("Requesting household cancellation…".into());
                return vec![Effect::CancelHouseholdOperationV1 {
                    binding: pending.binding.clone(),
                }];
            }
            PendingHouseholdMutationPhaseV1::Cancelling
            | PendingHouseholdMutationPhaseV1::Finishing { .. } => return Vec::new(),
        }
    }
    if let Some(pending) = model.pending_household_load.as_mut() {
        pending.cancel_requested = true;
        model.operation = OperationState::Cancelling(pending.operation_id.get());
        model.activity = Some(
            "Finishing the current local household read without dispatching a mutation…".into(),
        );
        return Vec::new();
    }
    if model.pending_household_choice.take().is_some() || model.onboarding.take().is_some() {
        model.operation = OperationState::Idle;
        model.activity = None;
        model.idle_exit_armed = false;
        push_notice(
            model,
            "Household member setup cancelled. No member or member profile was changed.",
        );
    }
    Vec::new()
}

fn household_turn_is_authorized(model: &mut AppModel) -> bool {
    let Some(generation) = model.household_generation.as_ref() else {
        return true;
    };
    if generation.mode != HouseholdPresentationModeV1::NativeEnabled {
        push_notice(
            model,
            "Conversational turns are unavailable in native rollback read-only mode.",
        );
        return false;
    }
    match model.household_turn_gate {
        HouseholdTurnGateV1::HostedReady => {}
        HouseholdTurnGateV1::Legacy
        | HouseholdTurnGateV1::Loading
        | HouseholdTurnGateV1::ReconciliationRequired
        | HouseholdTurnGateV1::CounterExhausted => {
            push_notice(
                model,
                "Household context is not ready. No turn was dispatched.",
            );
            return false;
        }
    }
    let Some(_snapshot) = model.household_snapshot.as_ref() else {
        push_notice(
            model,
            "Household context is not ready. No turn was dispatched.",
        );
        return false;
    };
    true
}

fn cancel_or_exit(model: &mut AppModel) -> Vec<Effect> {
    if has_active_household_work(model) {
        return cancel_household_draft(model);
    }
    if !matches!(model.voice_phase, VoicePhase::Idle) {
        return cancel_voice(model);
    }
    if matches!(
        model.profile_consent_review,
        Some(ProfileConsentReview::Reviewing)
    ) {
        return profile_event(model, RuntimeEvent::ProfileConsentCancelled);
    }
    if model.pending_confirmation.is_some() && !model.operation.is_active() {
        return submit_confirmation(model, ConfirmationDecisionWire::Cancel, None);
    }
    if !model.draft.is_empty() {
        model.draft.clear();
        model.cursor = 0;
        model.idle_exit_armed = false;
        return Vec::new();
    }
    if model.onboarding.is_some() && !model.operation.is_active() {
        let native_local_first = model
            .onboarding
            .as_ref()
            .is_some_and(|flow| matches!(flow.copy_mode, OnboardingCopyMode::NativeLocalFirst));
        model.onboarding = None;
        model.idle_exit_armed = false;
        model.activity = None;
        if native_local_first {
            push_notice(
                model,
                &crate::render::profile_copy(ProfileCopyStateV1::OnboardingSaveCancelled),
            );
        } else {
            push_notice(
                model,
                "Dietary onboarding cancelled. Nothing was sent or saved.",
            );
        }
        return Vec::new();
    }
    match model.operation {
        OperationState::Running(operation_id) => {
            model.operation = OperationState::Cancelling(operation_id);
            model.activity = Some("Stopping…".into());
            vec![Effect::CancelTurn { operation_id }]
        }
        OperationState::Cancelling(_) | OperationState::Finishing(_) => Vec::new(),
        OperationState::Idle if model.idle_exit_armed => begin_exit(model, ExitReason::Requested),
        OperationState::Idle => {
            model.idle_exit_armed = true;
            model.activity = Some("Press Ctrl+C again to exit".into());
            Vec::new()
        }
        OperationState::Exiting(_) => Vec::new(),
    }
}

fn begin_exit(model: &mut AppModel, reason: ExitReason) -> Vec<Effect> {
    let mut effects = Vec::new();
    if let Some(pending) = model.pending_household_mutation.as_ref() {
        if matches!(
            pending.phase,
            PendingHouseholdMutationPhaseV1::Dispatched
                | PendingHouseholdMutationPhaseV1::Cancelling
        ) {
            effects.push(Effect::CancelHouseholdOperationV1 {
                binding: pending.binding.clone(),
            });
        }
    } else if let Some(operation_id) = voice_operation_id(model.voice_phase) {
        effects.push(Effect::CancelVoice { operation_id });
    } else if let Some(operation_id) = model.operation.operation_id() {
        effects.push(Effect::CancelTurn { operation_id });
    }
    model.pending_household_load = None;
    model.pending_household_choice = None;
    model.onboarding = None;
    model.pending_native_startup_onboarding = None;
    model.operation = OperationState::Exiting(reason);
    effects.push(Effect::Exit(reason));
    effects
}

fn change_profile_presentation_mode(model: &mut AppModel, mode: ProfilePresentationModeV1) {
    if model.profile_presentation_mode == mode {
        return;
    }
    model.profile_presentation_mode = mode;
    model.owner_profile_actions = None;
    if mode != ProfilePresentationModeV1::NativeEnabled {
        model.pending_native_startup_onboarding = None;
    }

    if matches!(
        model.profile_consent_review,
        Some(ProfileConsentReview::Reviewing)
    ) {
        let _ = profile_event(model, RuntimeEvent::ProfileConsentCancelled);
    }

    let granting_operation = match model.profile_consent_review {
        Some(ProfileConsentReview::Granting { operation_id }) => Some(operation_id),
        Some(ProfileConsentReview::Reviewing) | None => None,
    };
    if let Some(operation_id) = granting_operation {
        model.profile_consent_review = None;
        finish_profile_action(
            model,
            operation_id,
            "Profile-sync consent is unavailable in this mode.".into(),
        );
    }

    let pending_operation = match model.pending_profile_action {
        Some(PendingProfileActionV1::Loading { operation_id, .. })
        | Some(PendingProfileActionV1::Retrying { operation_id, .. }) => Some(operation_id),
        None => None,
    };
    if let Some(operation_id) = pending_operation {
        model.pending_profile_action = None;
        finish_profile_action(
            model,
            operation_id,
            "Profile actions are unavailable in this mode.".into(),
        );
    }
}

fn accept_household_generation(
    model: &mut AppModel,
    session_mode_generation: HouseholdModeGenerationV1,
    mode: HouseholdPresentationModeV1,
    account_binding_digest: HouseholdAccountBindingDigestV1,
) -> Vec<Effect> {
    if model.household_generation.is_some()
        || model
            .highest_household_generation
            .is_some_and(|highest| session_mode_generation <= highest)
    {
        return Vec::new();
    }
    model.highest_household_generation = Some(session_mode_generation);
    model.household_generation = Some(HouseholdGenerationStateV1 {
        session_mode_generation,
        mode,
        account_binding_digest,
    });
    model.household_snapshot = None;
    model.pending_household_load = None;
    model.pending_household_mutation = None;
    model.pending_household_choice = None;
    model.household_chrome_label = None;
    model.household_turn_gate = HouseholdTurnGateV1::Loading;
    model.profile_presentation_mode = match mode {
        HouseholdPresentationModeV1::NativeEnabled => ProfilePresentationModeV1::NativeEnabled,
        HouseholdPresentationModeV1::NativeRollbackReadOnly => {
            ProfilePresentationModeV1::NativeRollbackReadOnly
        }
    };
    begin_household_load(model, HouseholdLoadIntentV1::Bootstrap)
}

fn invalidate_household_generation(
    model: &mut AppModel,
    session_mode_generation: HouseholdModeGenerationV1,
) -> Vec<Effect> {
    let Some(generation) = model.household_generation.as_ref() else {
        return Vec::new();
    };
    if generation.session_mode_generation != session_mode_generation {
        return Vec::new();
    }
    let cancel = model
        .pending_household_mutation
        .as_ref()
        .filter(|pending| {
            matches!(
                pending.phase,
                PendingHouseholdMutationPhaseV1::Dispatched
                    | PendingHouseholdMutationPhaseV1::Cancelling
            )
        })
        .map(|pending| Effect::CancelHouseholdOperationV1 {
            binding: pending.binding.clone(),
        });
    model.household_generation = None;
    model.household_snapshot = None;
    model.pending_household_load = None;
    model.pending_household_mutation = None;
    model.pending_household_choice = None;
    model.household_chrome_label = None;
    model.household_turn_gate = HouseholdTurnGateV1::Legacy;
    model.onboarding = None;
    model.pending_confirmation = None;
    model.pending_choice_labels.clear();
    model.pending_agent_partial.clear();
    model.profile_consent_review = None;
    model.owner_profile_actions = None;
    model.pending_profile_action = None;
    model.pending_native_startup_onboarding = None;
    model.draft.clear();
    model.cursor = 0;
    model.activity = None;
    model.operation = OperationState::Idle;
    model.profile_presentation_mode = ProfilePresentationModeV1::LegacyCompatibility;
    model.scrollback.clear();
    cancel.into_iter().collect()
}

fn household_runtime_event(model: &mut AppModel, runtime: RuntimeEvent) -> Vec<Effect> {
    match runtime {
        RuntimeEvent::HouseholdGenerationReadyV1 {
            session_mode_generation,
            mode,
            account_binding_digest,
        } => accept_household_generation(
            model,
            session_mode_generation,
            mode,
            account_binding_digest,
        ),
        RuntimeEvent::HouseholdGenerationInvalidatedV1 {
            session_mode_generation,
        } => invalidate_household_generation(model, session_mode_generation),
        RuntimeEvent::HouseholdManagementLoadedV1 {
            operation_id,
            session_mode_generation,
            reducer_correlation,
            purpose,
            account_binding_digest,
            household_revision,
            active_scope,
            members,
        } => handle_household_management_loaded(
            model,
            HouseholdManagementLoadedInputV1 {
                operation_id,
                session_mode_generation,
                reducer_correlation,
                purpose,
                account_binding_digest,
                household_revision,
                active_scope,
                members,
            },
        ),
        RuntimeEvent::HouseholdManagementLoadFailedV1 {
            operation_id,
            session_mode_generation,
            reducer_correlation,
            purpose,
            account_binding_digest,
            observed_household_revision,
            reason,
        } => handle_household_management_failed(
            model,
            HouseholdManagementFailedInputV1 {
                operation_id,
                session_mode_generation,
                reducer_correlation,
                purpose,
                account_binding_digest,
                observed_household_revision,
                reason,
            },
        ),
        RuntimeEvent::HouseholdMutationCommittedV1 {
            binding,
            kind,
            resulting_household_revision,
            affected_subject,
            active_scope,
            bounded_active_label,
        } => handle_household_mutation_committed(
            model,
            binding,
            kind,
            resulting_household_revision,
            affected_subject,
            active_scope,
            bounded_active_label,
        ),
        RuntimeEvent::HouseholdMutationFailedV1 {
            binding,
            kind,
            affected_subject,
            observed_household_revision,
            reason,
        } => handle_household_mutation_failed(
            model,
            binding,
            kind,
            affected_subject,
            observed_household_revision,
            reason,
        ),
        RuntimeEvent::HouseholdContextAppliedV1 {
            binding,
            resulting_household_revision,
            active_scope,
            bounded_active_label,
        } => handle_household_context_applied(
            model,
            binding,
            resulting_household_revision,
            active_scope,
            bounded_active_label,
        ),
        RuntimeEvent::HouseholdContextApplyFailedV1 {
            binding,
            resulting_household_revision,
            reason,
        } => handle_household_context_apply_failed(
            model,
            binding,
            resulting_household_revision,
            reason,
        ),
        _ => Vec::new(),
    }
}

fn runtime_event(model: &mut AppModel, runtime: RuntimeEvent) -> Vec<Effect> {
    if matches!(
        &runtime,
        RuntimeEvent::HouseholdGenerationReadyV1 { .. }
            | RuntimeEvent::HouseholdGenerationInvalidatedV1 { .. }
            | RuntimeEvent::HouseholdManagementLoadedV1 { .. }
            | RuntimeEvent::HouseholdManagementLoadFailedV1 { .. }
            | RuntimeEvent::HouseholdMutationCommittedV1 { .. }
            | RuntimeEvent::HouseholdMutationFailedV1 { .. }
            | RuntimeEvent::HouseholdContextAppliedV1 { .. }
            | RuntimeEvent::HouseholdContextApplyFailedV1 { .. }
    ) {
        return household_runtime_event(model, runtime);
    }
    match runtime {
        RuntimeEvent::ExternalSignal(reason) => return begin_exit(model, reason),
        RuntimeEvent::ProfileActionsLoaded { .. }
        | RuntimeEvent::ProfileConsentRequested
        | RuntimeEvent::ProfileConsentConfirmed
        | RuntimeEvent::ProfileConsentCancelled
        | RuntimeEvent::ProfileConsentFinished { .. }
        | RuntimeEvent::ProfileRetrySyncRequested
        | RuntimeEvent::ProfileRetrySyncFinished { .. } => {
            return profile_event(model, runtime);
        }
        RuntimeEvent::ProfilePresentationMode(mode) => {
            change_profile_presentation_mode(model, mode);
        }
        RuntimeEvent::Notice { message } => push_notice(model, &terminal_safe_text(&message)),
        RuntimeEvent::VoiceAvailability(availability) => {
            model.voice_availability = availability;
        }
        RuntimeEvent::VoiceRecordingElapsed {
            operation_id,
            seconds,
        } if matches!(
            model.voice_phase,
            VoicePhase::Recording {
                operation_id: current
            } if current == operation_id
        ) =>
        {
            model.activity = Some(format!(
                "Recording {seconds}s · Enter, Ctrl+Space, or F8 to transcribe · Esc to cancel"
            ));
        }
        RuntimeEvent::VoiceTranscriptReady {
            operation_id,
            transcript,
        } if voice_operation_id(model.voice_phase) == Some(operation_id) => {
            finish_voice_transcription(model, Ok(transcript));
        }
        RuntimeEvent::VoiceFailed {
            operation_id,
            message,
        } if voice_operation_id(model.voice_phase) == Some(operation_id) => {
            finish_voice_transcription(model, Err(message));
        }
        RuntimeEvent::VoiceCancelled { operation_id }
            if voice_operation_id(model.voice_phase) == Some(operation_id) =>
        {
            finish_voice_cancel(model);
        }
        RuntimeEvent::BeginOnboarding { message } => {
            if model.profile_presentation_mode == ProfilePresentationModeV1::LegacyCompatibility
                && model.operation == OperationState::Idle
                && model.onboarding.is_none()
            {
                begin_onboarding(model, &terminal_safe_text(&message));
            } else if model.profile_presentation_mode
                == ProfilePresentationModeV1::NativeRollbackReadOnly
            {
                push_notice(
                    model,
                    "Dietary onboarding is unavailable in native rollback read-only mode.",
                );
            }
        }
        RuntimeEvent::BeginNativeOwnerOnboarding { message } => {
            if model.profile_presentation_mode != ProfilePresentationModeV1::NativeEnabled {
                model.pending_native_startup_onboarding = None;
            } else if model.operation == OperationState::Idle
                && model.onboarding.is_none()
                && model.household_management_ready()
            {
                begin_native_owner_onboarding(model, &terminal_safe_text(&message));
            } else if model.onboarding.is_none() {
                model.pending_native_startup_onboarding = Some(terminal_safe_text(&message));
            }
        }
        RuntimeEvent::OnboardingSaved { operation_id }
            if model.operation.operation_id() == Some(operation_id)
                && !has_pending_household_driver_operation(model) =>
        {
            finish_onboarding(model, Ok(()));
        }
        RuntimeEvent::NativeOwnerOnboardingSaved {
            operation_id,
            status,
        } if model.operation.operation_id() == Some(operation_id)
            && !has_pending_household_driver_operation(model) =>
        {
            finish_native_owner_onboarding(model, operation_id, status);
        }
        RuntimeEvent::OnboardingFailed {
            operation_id,
            message,
        } if model.operation.operation_id() == Some(operation_id)
            && !has_pending_household_driver_operation(model) =>
        {
            finish_onboarding(model, Err(message));
        }
        RuntimeEvent::OnboardingCancelled {
            operation_id,
            outcome,
        } if model.operation.operation_id() == Some(operation_id)
            && !has_pending_household_driver_operation(model) =>
        {
            finish_onboarding_cancel(model, outcome);
        }
        RuntimeEvent::TurnEvent {
            operation_id,
            event,
        } if model.operation.operation_id() == Some(operation_id)
            && !has_pending_household_driver_operation(model) =>
        {
            apply_agent_event(model, event)
        }
        RuntimeEvent::TurnFinished {
            operation_id,
            outcome,
        } if model.operation.operation_id() == Some(operation_id)
            && !has_pending_household_driver_operation(model) =>
        {
            finish_stream(model, outcome);
        }
        RuntimeEvent::TurnFailed {
            operation_id,
            failure,
        } if model.operation.operation_id() == Some(operation_id)
            && !has_pending_household_driver_operation(model) =>
        {
            finish_failed_stream(model, failure);
        }
        RuntimeEvent::PanelReady {
            operation_id,
            panel,
            body,
        } if model.operation.operation_id() == Some(operation_id)
            && !has_pending_household_driver_operation(model) =>
        {
            finish_panel(model, panel, Ok(body));
        }
        RuntimeEvent::PanelFailed {
            operation_id,
            panel,
            message,
        } if model.operation.operation_id() == Some(operation_id)
            && !has_pending_household_driver_operation(model) =>
        {
            finish_panel(model, panel, Err(message));
        }
        RuntimeEvent::HouseholdScopeReady {
            operation_id,
            label,
        } if model.operation.operation_id() == Some(operation_id)
            && model.household_generation.is_none() =>
        {
            finish_household_scope(model, Ok(label));
        }
        RuntimeEvent::HouseholdScopeFailed {
            operation_id,
            message,
        } if model.operation.operation_id() == Some(operation_id)
            && model.household_generation.is_none() =>
        {
            finish_household_scope(model, Err(message));
        }
        RuntimeEvent::HouseholdGenerationReadyV1 { .. }
        | RuntimeEvent::HouseholdGenerationInvalidatedV1 { .. }
        | RuntimeEvent::HouseholdManagementLoadedV1 { .. }
        | RuntimeEvent::HouseholdManagementLoadFailedV1 { .. }
        | RuntimeEvent::HouseholdMutationCommittedV1 { .. }
        | RuntimeEvent::HouseholdMutationFailedV1 { .. }
        | RuntimeEvent::HouseholdContextAppliedV1 { .. }
        | RuntimeEvent::HouseholdContextApplyFailedV1 { .. }
        | RuntimeEvent::OnboardingSaved { .. }
        | RuntimeEvent::NativeOwnerOnboardingSaved { .. }
        | RuntimeEvent::OnboardingFailed { .. }
        | RuntimeEvent::OnboardingCancelled { .. }
        | RuntimeEvent::TurnEvent { .. }
        | RuntimeEvent::TurnFinished { .. }
        | RuntimeEvent::TurnFailed { .. }
        | RuntimeEvent::PanelReady { .. }
        | RuntimeEvent::PanelFailed { .. }
        | RuntimeEvent::HouseholdScopeReady { .. }
        | RuntimeEvent::HouseholdScopeFailed { .. }
        | RuntimeEvent::VoiceRecordingElapsed { .. }
        | RuntimeEvent::VoiceTranscriptReady { .. }
        | RuntimeEvent::VoiceFailed { .. }
        | RuntimeEvent::VoiceCancelled { .. } => {}
    }
    Vec::new()
}

fn finish_onboarding(model: &mut AppModel, result: Result<(), String>) {
    let old_lines = model.scrollback.rendered_lines();
    match result {
        Ok(()) => {
            model.scrollback.mutate_last_assistant(|entry| {
                entry.text = "Dietary profile saved\n\nYour hello.food guidance now uses this synced profile across supported experiences.".into();
                entry.streaming = false;
            });
            model.onboarding = None;
        }
        Err(message) => {
            if let Some(flow) = model.onboarding.as_mut() {
                flow.step = OnboardingStep::Review;
            }
            let review = model
                .onboarding
                .as_ref()
                .map(onboarding_review)
                .unwrap_or_default();
            model.scrollback.mutate_last_assistant(|entry| {
                entry.text = format!(
                    "Dietary profile was not saved: {}\n\n{}",
                    terminal_safe_text(&message),
                    review
                );
                entry.streaming = false;
            });
        }
    }
    model.operation = OperationState::Idle;
    model.activity = None;
    model.idle_exit_armed = false;
    account_for_new_lines(model, old_lines);
}

fn finish_native_owner_onboarding(
    model: &mut AppModel,
    operation_id: u64,
    status: NativeOwnerProfileSaveStatusV1,
) {
    let text = match status {
        NativeOwnerProfileSaveStatusV1::SavedWithAbsentConsent => {
            crate::render::profile_copy(ProfileCopyStateV1::SavedWithAbsentConsent)
        }
        NativeOwnerProfileSaveStatusV1::SyncPending => {
            crate::render::profile_copy(ProfileCopyStateV1::SyncPending)
        }
    };
    model.onboarding = None;
    finish_profile_action(model, operation_id, text);
}

fn finish_onboarding_cancel(model: &mut AppModel, outcome: RunTurnOutcome) {
    let old_lines = model.scrollback.rendered_lines();
    let native_local_first = model
        .onboarding
        .as_ref()
        .is_some_and(|flow| matches!(flow.copy_mode, OnboardingCopyMode::NativeLocalFirst));
    model.scrollback.mutate_last_assistant(|entry| {
        entry.text = if native_local_first {
            crate::render::profile_copy(ProfileCopyStateV1::OnboardingSaveCancelled)
        } else {
            match outcome {
            RunTurnOutcome::CancelledAfterDispatchOutcomeUnknown => "Dietary profile save stopped after dispatch, and the server outcome is unknown. Open `/profile` to inspect current state before starting onboarding again.".into(),
            RunTurnOutcome::CancelledBeforeServerAcceptance
            | RunTurnOutcome::CancelledAfterServerAcceptance
            | RunTurnOutcome::StaleGeneration
            | RunTurnOutcome::Completed => "Dietary profile save cancelled. The profile upload was not dispatched; profile-sync consent may already have been granted.".into(),
            }
        };
        entry.streaming = false;
    });
    model.onboarding = None;
    model.operation = OperationState::Idle;
    model.activity = None;
    model.idle_exit_armed = false;
    account_for_new_lines(model, old_lines);
}

fn finish_household_scope(model: &mut AppModel, result: Result<String, String>) {
    let old_lines = model.scrollback.rendered_lines();
    model.scrollback.mutate_last_assistant(|entry| {
        entry.text = match result {
            Ok(label) => format!(
                "Household target\n\nFuture turns will consider {}.",
                terminal_safe_text(&label)
            ),
            Err(message) => format!(
                "Unable to change the household target: {}",
                terminal_safe_text(&message)
            ),
        };
        entry.streaming = false;
    });
    model.operation = OperationState::Idle;
    model.activity = None;
    model.idle_exit_armed = false;
    account_for_new_lines(model, old_lines);
}

fn finish_panel(model: &mut AppModel, panel: PanelRequest, result: Result<String, String>) {
    let old_lines = model.scrollback.rendered_lines();
    model.scrollback.mutate_last_assistant(|entry| {
        entry.text = match result {
            Ok(body) => {
                let body = terminal_safe_text(&body);
                if body.trim().is_empty() {
                    format!("{}\n\nNo information is available.", panel.title())
                } else {
                    format!("{}\n\n{}", panel.title(), body.trim_end())
                }
            }
            Err(message) => format!(
                "Unable to open {}: {}",
                panel.title(),
                terminal_safe_text(&message)
            ),
        };
        entry.streaming = false;
    });
    model.operation = OperationState::Idle;
    model.activity = None;
    model.idle_exit_armed = false;
    account_for_new_lines(model, old_lines);
}

fn thinking_activity(stage: Option<&str>, message: Option<&str>) -> String {
    let safe_message = message.map(terminal_safe_text);
    if let Some(message) = safe_message
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .filter(|value| !contains_private_household_identifier(value))
        .filter(|value| !looks_like_machine_identifier(value.trim()))
    {
        return message.to_owned();
    }

    [stage, safe_message.as_deref()]
        .into_iter()
        .flatten()
        .find_map(|value| human_activity_for_identifier(value.trim()))
        .unwrap_or_else(|| "Working through your question…".into())
}

fn human_activity_for_identifier(value: &str) -> Option<String> {
    let message = match value {
        "resolving_restaurant" | "search_restaurants" => "Finding the right restaurant…",
        "loading_menu"
        | "get_cached_menu"
        | "request_menu_fetch"
        | "check_menu_fetch_status"
        | "search_menu_items" => "Loading the latest menu…",
        "evaluating_menu" | "evaluate_menu" => "Checking the menu against your profile…",
        "applying_dietary_graph" | "describe_dietary_graph" | "get_food_preferences" => {
            "Considering your dietary profile…"
        }
        "searching_recipes" | "search_recipes" | "get_recipe_details" => {
            "Finding recipes that fit…"
        }
        "checking_food" | "check_food_safety" => "Checking this food against your profile…",
        _ => return None,
    };
    Some(message.into())
}

fn looks_like_machine_identifier(value: &str) -> bool {
    value.contains('_')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn contains_private_model_text(model: &AppModel, value: &str) -> bool {
    contains_private_household_identifier(value)
        || model.household_snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.members.iter().any(|member| {
                member
                    .subject()
                    .as_member()
                    .is_some_and(|member_id| value.contains(member_id.as_str()))
            })
        })
}

fn append_agent_partial_buffer(model: &mut AppModel, text: &str) {
    if model.pending_agent_partial == "_self" {
        return;
    }
    if model.pending_agent_partial.len().saturating_add(text.len()) > MAX_SCROLLBACK_BYTES {
        model.pending_agent_partial.clear();
        model.pending_agent_partial.push_str("_self");
        return;
    }
    model.pending_agent_partial.push_str(text);
}

fn apply_agent_event(model: &mut AppModel, event: AgentEvent) {
    let old_lines = model.scrollback.rendered_lines();
    match event {
        AgentEvent::Thinking { stage, message } => {
            let activity = thinking_activity(stage.as_deref(), message.as_deref());
            model.activity = Some(if contains_private_model_text(model, &activity) {
                "Working through your question…".into()
            } else {
                activity
            });
        }
        AgentEvent::Progress {
            message,
            current,
            total,
        } => {
            let message = terminal_safe_text(&message);
            let message = if contains_private_model_text(model, &message) {
                "Making progress…".into()
            } else if looks_like_machine_identifier(message.trim()) {
                human_activity_for_identifier(message.trim())
                    .unwrap_or_else(|| "Making progress…".into())
            } else {
                message
            };
            model.activity = match (current, total) {
                (Some(current), Some(total)) => Some(format!("{message} ({current}/{total})")),
                _ => Some(message),
            };
        }
        AgentEvent::Partial { text } => {
            let text = terminal_safe_text(&text);
            append_agent_partial_buffer(model, &text);
            model.activity = Some("Responding…".into());
        }
        AgentEvent::Choices { choices, .. } => {
            let choice_labels = choices
                .iter()
                .map(|choice| terminal_safe_text(&choice.label))
                .collect::<Vec<_>>();
            if choice_labels
                .iter()
                .any(|label| contains_private_model_text(model, label))
            {
                model.pending_choice_labels.clear();
                model.scrollback.mutate_last_assistant(|entry| {
                    entry.text = UNPRESENTABLE_AGENT_CHOICES_MESSAGE.into();
                });
                model.activity = Some("Responding…".into());
            } else {
                model.pending_choice_labels = choice_labels.clone();
                model.scrollback.mutate_last_assistant(|entry| {
                    if !entry.text.is_empty() {
                        entry.text.push('\n');
                    }
                    for label in choice_labels {
                        entry.text.push_str("• ");
                        entry.text.push_str(&label);
                        entry.text.push('\n');
                    }
                });
                model.activity = Some("Choose an option".into());
            }
        }
        AgentEvent::Result { document, .. } => {
            let focus_full_menu = is_full_household_menu(&document);
            let confirmation = ActionConfirmationEnvelopeWire::from_result_document(&document);
            let household_evaluation_candidate = household_evaluation_document(&document).is_some();
            let result = agent_result_text(&document).map(terminal_safe_text);
            let household_evaluation = render_household_evaluation(&document);
            let household_menu = render_household_menu(&document);
            let mut choice_labels = std::mem::take(&mut model.pending_choice_labels);
            let buffered_partial = std::mem::take(&mut model.pending_agent_partial);
            let result_is_private = result
                .as_ref()
                .is_some_and(|value| contains_private_model_text(model, value));
            let buffered_partial_is_private = contains_private_model_text(model, &buffered_partial);
            if choice_labels
                .iter()
                .any(|label| contains_private_model_text(model, label))
            {
                choice_labels.clear();
            }
            model.scrollback.mutate_last_assistant(|entry| {
                match confirmation.as_ref() {
                    Ok(Some(envelope)) => {
                        entry.text = render_action_confirmation(envelope);
                    }
                    Err(message) => {
                        entry.text = format!(
                            "Unable to present this confirmation safely: {}",
                            terminal_safe_text(message)
                        );
                    }
                    Ok(None) => {
                        if let Err(error) = household_evaluation.as_ref() {
                            entry.text = error.to_string();
                        } else if let Some(evaluation) = household_evaluation
                            .as_ref()
                            .ok()
                            .and_then(|evaluation| evaluation.as_ref())
                        {
                            entry.text = evaluation.clone();
                        } else if let Some(menu) = household_menu {
                            entry.text = menu;
                        } else if household_evaluation_candidate {
                            entry.text = result
                                .filter(|value| !value.is_empty() && !result_is_private)
                                .or_else(|| {
                                    (!buffered_partial.is_empty() && !buffered_partial_is_private)
                                        .then(|| buffered_partial.clone())
                                })
                                .unwrap_or_else(|| UNRENDERABLE_AGENT_RESULT_MESSAGE.into());
                        } else if let Some(result) =
                            result.filter(|value| !value.is_empty() && !result_is_private)
                        {
                            entry.text = result;
                            append_choice_labels(&mut entry.text, &choice_labels);
                        } else if result_is_private {
                            entry.text = UNRENDERABLE_AGENT_RESULT_MESSAGE.into();
                        } else if !buffered_partial.is_empty()
                            && !buffered_partial_is_private
                            && document.get("structured").is_none()
                            && document.get("structured_content").is_none()
                            && document.get("structuredContent").is_none()
                        {
                            entry.text = buffered_partial.clone();
                            append_choice_labels(&mut entry.text, &choice_labels);
                        } else if entry.text.is_empty() {
                            entry.text = UNRENDERABLE_AGENT_RESULT_MESSAGE.into();
                        }
                    }
                }
                entry.streaming = false;
            });
            match confirmation {
                Ok(Some(envelope)) => {
                    let editable_items = editable_grocery_items(&envelope);
                    model.pending_confirmation = Some(PendingActionConfirmation {
                        confirmation_id: envelope.confirmation_id,
                        idempotency_key: envelope.idempotency_key,
                        editable_items,
                    });
                }
                Ok(None) | Err(_) => model.pending_confirmation = None,
            }
            mark_finishing(model);
            model.focus_latest_result_on_finish = focus_full_menu;
            model.activity = Some("Finishing…".into());
            model.idle_exit_armed = false;
        }
        AgentEvent::Error { error } => {
            model.pending_choice_labels.clear();
            model.pending_agent_partial.clear();
            if !confirmation_error_preserves_pending(&error.code) {
                model.pending_confirmation = None;
            }
            let mut message = terminal_safe_text(&error.message);
            if message.trim().is_empty() {
                message = "hey.food could not complete this request. You can try again now.".into();
            }
            let message_is_private = contains_private_model_text(model, &message);
            model.scrollback.mutate_last_assistant(|entry| {
                if message_is_private || contains_private_household_identifier(&entry.text) {
                    entry.text =
                        "hey.food could not complete this request. You can try again now.".into();
                } else {
                    if !entry.text.is_empty() {
                        entry.text.push_str("\n\n");
                    }
                    entry.text.push_str(&message);
                }
                entry.streaming = false;
            });
            mark_finishing(model);
            model.activity = Some("Finishing…".into());
        }
    }
    account_for_new_lines(model, old_lines);
}

fn confirmation_error_preserves_pending(code: &str) -> bool {
    matches!(code, "edit_invalid" | "temporarily_unavailable")
}

fn render_action_confirmation(envelope: &ActionConfirmationEnvelopeWire) -> String {
    let mut output = format!(
        "Review before changing anything\n\n{}\n",
        terminal_safe_text(&envelope.preview)
    );
    if let Some(items) = envelope
        .structured_preview
        .as_ref()
        .and_then(|preview| preview.get("items"))
        .and_then(serde_json::Value::as_array)
    {
        for (index, item) in items.iter().enumerate() {
            let name = ["name", "requested_name", "canonical_name"]
                .into_iter()
                .find_map(|key| item.get(key).and_then(serde_json::Value::as_str))
                .map(terminal_safe_text)
                .unwrap_or_else(|| "item".into());
            let intended_for = item.get("intended_for").and_then(serde_json::Value::as_str);
            let intended = match intended_for {
                Some("_self") => " for you".to_owned(),
                Some(_) => " for a household member".to_owned(),
                None => String::new(),
            };
            let quantity = item.get("quantity").and_then(|value| {
                value
                    .as_str()
                    .map(terminal_safe_text)
                    .or_else(|| value.as_f64().map(|value| value.to_string()))
            });
            let unit = item
                .get("unit")
                .and_then(serde_json::Value::as_str)
                .map(terminal_safe_text);
            let amount = match (quantity, unit) {
                (Some(quantity), Some(unit)) => format!(" · {quantity} {unit}"),
                (Some(quantity), None) => format!(" · {quantity}"),
                _ => String::new(),
            };
            let _ = writeln!(output, "{}. {name}{intended}{amount}", index + 1);
            render_confirmation_sources(&mut output, item);
            render_confirmation_safety(&mut output, item, intended_for);
        }
    }
    if let Some(expires_at) = envelope.expires_at.as_deref() {
        let _ = writeln!(output, "\nExpires: {}", terminal_safe_text(expires_at));
    }
    output.push_str(
        "\nNothing has changed yet. Type `y` to confirm or `n` to cancel. Ctrl+C cancels.",
    );
    if editable_grocery_items(envelope).is_some() {
        output.push_str(
            "\nTo replace one item name and confirm the correction, type `edit #N <replacement>`.",
        );
    }
    output
}

fn editable_grocery_items(
    envelope: &ActionConfirmationEnvelopeWire,
) -> Option<Vec<serde_json::Map<String, serde_json::Value>>> {
    if !matches!(
        envelope.action.as_str(),
        "grocery_list_add_items" | "add_items"
    ) {
        return None;
    }
    let items = envelope
        .structured_preview
        .as_ref()?
        .get("items")?
        .as_array()?;
    if items.is_empty() || items.len() > 25 {
        return None;
    }
    let editable_items = items
        .iter()
        .map(editable_grocery_item)
        .collect::<Option<Vec<_>>>()?;
    let mut patch = serde_json::Map::new();
    patch.insert(
        "items".into(),
        serde_json::Value::Array(
            editable_items
                .iter()
                .cloned()
                .map(serde_json::Value::Object)
                .collect(),
        ),
    );
    GroceryEditPatch::new(patch).ok()?;
    Some(editable_items)
}

fn editable_grocery_item(
    item: &serde_json::Value,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let name = ["name", "requested_name"]
        .into_iter()
        .find_map(|key| item.get(key).and_then(serde_json::Value::as_str))
        .and_then(|value| required_text(value, 255).ok())?;
    let mut editable = serde_json::Map::new();
    editable.insert("name".into(), serde_json::Value::String(name));

    if let Some(quantity) = item.get("quantity").and_then(serde_json::Value::as_f64)
        && quantity.is_finite()
        && quantity >= 0.0
        && let Some(quantity) = serde_json::Number::from_f64(quantity)
    {
        editable.insert("quantity".into(), serde_json::Value::Number(quantity));
    }
    if let Some(package_quantity) = item
        .get("package_quantity")
        .and_then(serde_json::Value::as_i64)
        .filter(|value| *value >= 0)
    {
        editable.insert(
            "package_quantity".into(),
            serde_json::Value::Number(package_quantity.into()),
        );
    }
    for (field, maximum) in [("unit", 40), ("note", 255), ("intended_for", 64)] {
        if let Some(value) = item
            .get(field)
            .and_then(serde_json::Value::as_str)
            .and_then(|value| required_text(value, maximum).ok())
        {
            editable.insert(field.into(), serde_json::Value::String(value));
        }
    }
    editable.insert(
        "source_type".into(),
        serde_json::Value::String("manual".into()),
    );
    Some(editable)
}

fn render_confirmation_sources(output: &mut String, item: &serde_json::Value) {
    let sources = item.get("sources").and_then(serde_json::Value::as_array);
    let mut rendered_sources = 0;
    if let Some(sources) = sources {
        for source in sources.iter().take(MAX_CONFIRMATION_SOURCES_PER_ITEM) {
            let Some(source_type) = source
                .get("source_type")
                .and_then(serde_json::Value::as_str)
                .and_then(|value| required_text(value, 64).ok())
            else {
                continue;
            };
            let mut provenance = terminal_safe_text(&source_type);
            if let Some(source_ref) = source
                .get("source_ref")
                .and_then(serde_json::Value::as_str)
                .and_then(|value| required_text(value, 255).ok())
            {
                provenance.push(':');
                provenance.push_str(&terminal_safe_text(&source_ref));
            }
            if let Some(source_detail) = source
                .get("source_detail")
                .and_then(serde_json::Value::as_str)
                .and_then(|value| required_text(value, 255).ok())
            {
                provenance.push_str(" · ");
                provenance.push_str(&terminal_safe_text(&source_detail));
            }
            let _ = writeln!(output, "   source: {provenance}");
            rendered_sources += 1;
        }
        if rendered_sources > 0 && sources.len() > MAX_CONFIRMATION_SOURCES_PER_ITEM {
            let hidden = sources.len() - MAX_CONFIRMATION_SOURCES_PER_ITEM;
            let _ = writeln!(output, "   source: … and {hidden} more");
        }
    }
    if rendered_sources == 0
        && let Some(provenance) = item
            .get("provenance")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| required_text(value, 255).ok())
    {
        let _ = writeln!(output, "   source: {}", terminal_safe_text(&provenance));
    }
}

fn render_confirmation_safety(
    output: &mut String,
    item: &serde_json::Value,
    intended_for: Option<&str>,
) {
    // The generic C3 v1 item card placed flags at `item.safety_flags`.
    // Grocery Phase A's frozen production fixture specializes that shape as
    // `item.safety.{status,member_flags,label_hint}`. Prefer the production
    // Grocery shape while retaining the additive generic-C3 compatibility.
    let nested_safety = item.get("safety");
    if let Some(status) = nested_safety
        .and_then(|safety| safety.get("status"))
        .and_then(serde_json::Value::as_str)
    {
        let status = human_safety_status(status);
        let _ = writeln!(output, "   ingredient screening: {status}");
    }
    let flags = nested_safety
        .and_then(|safety| safety.get("member_flags"))
        .and_then(serde_json::Value::as_array)
        .or_else(|| {
            item.get("safety_flags")
                .and_then(serde_json::Value::as_array)
        });
    if let Some(flags) = flags {
        for flag in flags {
            let member_id = flag.get("member_id").and_then(serde_json::Value::as_str);
            let member = flag
                .get("label")
                .and_then(serde_json::Value::as_str)
                .and_then(|value| required_text(value, 80).ok())
                .map(|value| terminal_safe_text(&value))
                .or_else(|| (member_id == Some("_self")).then(|| "You".to_owned()))
                .unwrap_or_else(|| "Household member".into());
            let status = flag
                .get("status")
                .and_then(serde_json::Value::as_str)
                .map(human_safety_status)
                .unwrap_or("unable to evaluate");
            let intended = member_id
                .filter(|member| Some(*member) == intended_for)
                .map_or("", |_| " · intended");
            let _ = writeln!(output, "   • {member}: {status}{intended}");
            if let Some(reason) = flag.get("reason").and_then(serde_json::Value::as_str) {
                let _ = writeln!(output, "     {}", terminal_safe_text(reason));
            }
            if let Some(substitutions) = flag
                .get("substitutions")
                .and_then(serde_json::Value::as_array)
            {
                let substitutions = substitutions
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(terminal_safe_text)
                    .collect::<Vec<_>>()
                    .join(", ");
                if !substitutions.is_empty() {
                    let _ = writeln!(output, "     try: {substitutions}");
                }
            }
        }
    }
    if let Some(label_hint) = nested_safety
        .and_then(|safety| safety.get("label_hint"))
        .and_then(serde_json::Value::as_str)
    {
        let _ = writeln!(output, "   {}", terminal_safe_text(label_hint));
    }
}

fn human_safety_status(status: &str) -> &'static str {
    match status {
        "generally_safer" => "generally safer",
        "risky" => "risky",
        "avoid" => "avoid",
        "unable_to_evaluate" => "unable to evaluate",
        _ => "unable to evaluate",
    }
}

fn append_choice_labels(output: &mut String, choices: &[String]) {
    if choices.is_empty() {
        return;
    }
    if !output.is_empty() {
        output.push_str("\n\n");
    }
    output.push_str("Options\n");
    for choice in choices {
        output.push_str("• ");
        output.push_str(choice);
        output.push('\n');
    }
    output.pop();
}

fn mark_finishing(model: &mut AppModel) {
    if let Some(operation_id) = model.operation.operation_id() {
        model.operation = OperationState::Finishing(operation_id);
    }
}

fn finish_stream(model: &mut AppModel, outcome: RunTurnOutcome) {
    model.pending_choice_labels.clear();
    model.pending_agent_partial.clear();
    let old_lines = model.scrollback.rendered_lines();
    model.scrollback.mutate_last_assistant(|entry| {
        let notice = match outcome {
            RunTurnOutcome::Completed => None,
            RunTurnOutcome::CancelledBeforeServerAcceptance => Some("Turn cancelled."),
            RunTurnOutcome::CancelledAfterServerAcceptance => Some(
                "Turn cancelled after server acceptance. Check the conversation before retrying.",
            ),
            RunTurnOutcome::CancelledAfterDispatchOutcomeUnknown => Some(
                "Cancellation happened after dispatch and the server outcome is unknown. Check current state before retrying.",
            ),
            RunTurnOutcome::StaleGeneration => {
                Some("Turn stopped because the active account or context changed.")
            }
        };
        if let Some(notice) = notice {
            if !entry.text.is_empty() {
                entry.text.push_str("\n\n");
            }
            entry.text.push_str(notice);
        }
        entry.streaming = false;
    });
    model.operation = OperationState::Idle;
    model.activity = None;
    model.idle_exit_armed = false;
    if std::mem::take(&mut model.focus_latest_result_on_finish) {
        model.focus_latest_result_start = true;
        model.latest_result_start_offset = 0;
        model.follow_tail = false;
        model.scroll_from_tail = 0;
        model.unseen_lines = 0;
    } else {
        account_for_new_lines(model, old_lines);
    }
}

fn finish_failed_stream(model: &mut AppModel, failure: TurnFailure) {
    model.focus_latest_result_on_finish = false;
    model.pending_choice_labels.clear();
    let buffered_partial = std::mem::take(&mut model.pending_agent_partial);
    let release_buffered_partial =
        !buffered_partial.is_empty() && !contains_private_model_text(model, &buffered_partial);
    let old_lines = model.scrollback.rendered_lines();
    model.scrollback.mutate_last_assistant(|entry| {
        let notice = match failure.kind {
            TurnFailureKind::Inactivity => {
                "This response stopped before it finished. hey.food did not retry it. You can ask a new question now."
            }
            TurnFailureKind::StreamInterrupted => {
                "This response was interrupted before it finished. hey.food did not retry it. You can ask a new question now."
            }
            TurnFailureKind::DispatchOutcomeUnknown => {
                "This turn may have reached hello.food, but its result was not received. It was not retried. Check the conversation before trying the same request again."
            }
            TurnFailureKind::AuthenticationRequired => {
                "Your hello.food sign-in expired. Exit heyfood, run `heyfood login`, then reopen the TUI. This turn was not sent."
            }
            TurnFailureKind::AuthenticationChanged => {
                "The connected hello.food account changed. Exit and reopen heyfood before continuing. This turn was not sent."
            }
            TurnFailureKind::Unavailable => {
                "hey.food couldn’t start this turn. Check your connection, then ask again."
            }
            TurnFailureKind::Internal => {
                "hey.food couldn’t finish this turn. It was not retried. You can ask a new question now."
            }
        };
        if release_buffered_partial {
            let trailing = std::mem::take(&mut entry.text);
            entry.text = buffered_partial.clone();
            if !trailing.is_empty() {
                entry.text.push('\n');
                entry.text.push_str(&trailing);
            }
        }
        if !entry.text.is_empty() {
            entry.text.push_str("\n\n");
        }
        entry.text.push_str(notice);
        entry.streaming = false;
    });
    model.operation = OperationState::Idle;
    model.activity = None;
    model.idle_exit_armed = false;
    account_for_new_lines(model, old_lines);
}

fn account_for_new_lines(model: &mut AppModel, old_lines: usize) {
    if model.follow_tail {
        return;
    }
    let added = model
        .scrollback
        .rendered_lines()
        .saturating_sub(old_lines)
        .max(1);
    model.scroll_from_tail = model.scroll_from_tail.saturating_add(added);
    model.unseen_lines = model.unseen_lines.saturating_add(added);
}

fn follow_tail(model: &mut AppModel) {
    model.focus_latest_result_start = false;
    model.latest_result_start_offset = 0;
    model.follow_tail = true;
    model.scroll_from_tail = 0;
    model.unseen_lines = 0;
}

fn insert_at_cursor(model: &mut AppModel, text: &str) {
    let byte = byte_index(&model.draft, model.cursor);
    model.draft.insert_str(byte, text);
    model.cursor += text.chars().count();
}

fn backspace(model: &mut AppModel) {
    if model.cursor == 0 {
        return;
    }
    let start = byte_index(&model.draft, model.cursor - 1);
    let end = byte_index(&model.draft, model.cursor);
    model.draft.replace_range(start..end, "");
    model.cursor -= 1;
    model.idle_exit_armed = false;
}

fn delete(model: &mut AppModel) {
    let characters = model.draft.chars().count();
    if model.cursor >= characters {
        return;
    }
    let start = byte_index(&model.draft, model.cursor);
    let end = byte_index(&model.draft, model.cursor + 1);
    model.draft.replace_range(start..end, "");
    model.idle_exit_armed = false;
}

fn byte_index(text: &str, character_index: usize) -> usize {
    text.char_indices()
        .nth(character_index)
        .map_or(text.len(), |(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use heyfood_core::{
        AgentFailure, HouseholdOutboxId, HouseholdRevision, MemberId, OutboxRevision,
        ProfileRevision,
    };

    fn submit_text(model: &mut AppModel, value: &str) -> Vec<Effect> {
        model.draft = value.into();
        model.cursor = value.chars().count();
        dispatch(model, Action::Submit)
    }

    fn native_model() -> AppModel {
        let mut model = AppModel::default();
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::ProfilePresentationMode(
                ProfilePresentationModeV1::NativeEnabled,
            )),
        );
        model
    }

    fn available_owner_actions(
        retry: OwnerProfileRetryEligibilityV1,
    ) -> OwnerProfileActionEligibilityV1 {
        OwnerProfileActionEligibilityV1 {
            active_consent_version: Some(ConsentVersionV1::new(2).unwrap()),
            retry,
            intent: Some(OwnerSyncIntentHandleV1 {
                outbox_id: HouseholdOutboxId::parse_legacy("opaque-owner-intent").unwrap(),
                expected_household_revision: HouseholdRevision::new(11).unwrap(),
                expected_profile_revision: ProfileRevision::new(7).unwrap(),
                expected_outbox_revision: OutboxRevision::new(3).unwrap(),
            }),
        }
    }

    fn advance_to_onboarding_review(model: &mut AppModel) {
        assert!(submit_text(model, "1, vegan").is_empty());
        assert!(submit_text(model, "none").is_empty());
        assert!(submit_text(model, "celiac").is_empty());
        assert!(submit_text(model, "5").is_empty());
        assert!(submit_text(model, "raw onion").is_empty());
        assert!(submit_text(model, "2").is_empty());
        assert!(submit_text(model, "Mexican, 2").is_empty());
        assert!(submit_text(model, "none").is_empty());
        assert_eq!(
            model.onboarding.as_ref().map(|flow| flow.step),
            Some(OnboardingStep::Review)
        );
    }

    #[test]
    fn onboarding_is_local_until_explicit_review_and_save() {
        let mut model = AppModel::default();
        assert!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::BeginOnboarding {
                    message: "Complete your dietary profile.".into(),
                })
            )
            .is_empty()
        );
        advance_to_onboarding_review(&mut model);

        let effects = submit_text(&mut model, "save");
        assert_eq!(effects.len(), 1);
        let Effect::SaveOnboarding {
            operation_id,
            profile,
        } = &effects[0]
        else {
            panic!("expected an onboarding save effect");
        };
        assert_eq!(*operation_id, 1);
        assert_eq!(profile.diet_style_ids, ["gluten_free", "vegan"]);
        assert_eq!(profile.health_condition_ids, ["celiac"]);
        assert_eq!(profile.severity_level, Some(5));
        assert_eq!(profile.avoid_ingredients, ["raw onion"]);
        assert_eq!(profile.activity_level.as_deref(), Some("moderate"));
        assert_eq!(profile.cuisine_preferences, ["mexican", "italian"]);
        assert_eq!(model.operation, OperationState::Running(1));
    }

    #[test]
    fn onboarding_cancel_discards_local_answers_without_a_mutation_effect() {
        let mut model = AppModel::default();
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::BeginOnboarding {
                message: "Complete your dietary profile.".into(),
            }),
        );
        assert!(submit_text(&mut model, "vegan").is_empty());
        assert!(submit_text(&mut model, "cancel").is_empty());
        assert!(model.onboarding.is_none());
        assert_eq!(model.operation, OperationState::Idle);
        assert!(
            model
                .scrollback
                .entries()
                .back()
                .is_some_and(|entry| entry.text.contains("Nothing was sent or saved"))
        );
    }

    #[test]
    fn native_onboarding_uses_local_first_review_cancel_and_save_copy() {
        let mut review = AppModel::default();
        let _ = dispatch(
            &mut review,
            Action::Runtime(RuntimeEvent::ProfilePresentationMode(
                ProfilePresentationModeV1::NativeEnabled,
            )),
        );
        let _ = submit_text(&mut review, "/onboard");
        advance_to_onboarding_review(&mut review);
        let text = &review.scrollback.entries().back().unwrap().text;
        assert!(text.contains(&crate::render::profile_copy(
            ProfileCopyStateV1::OnboardingSaveReview
        )));
        assert!(!text.contains("Type `save` to grant profile-sync consent"));

        let mut cancelled = review.clone();
        assert!(submit_text(&mut cancelled, "cancel").is_empty());
        assert_eq!(
            cancelled.scrollback.entries().back().unwrap().text,
            crate::render::profile_copy(ProfileCopyStateV1::OnboardingSaveCancelled)
        );

        let effects = submit_text(&mut review, "save");
        assert!(matches!(
            effects.as_slice(),
            [Effect::SaveOnboarding {
                operation_id: 1,
                ..
            }]
        ));
        let _ = dispatch(
            &mut review,
            Action::Runtime(RuntimeEvent::NativeOwnerOnboardingSaved {
                operation_id: 1,
                status: NativeOwnerProfileSaveStatusV1::SavedWithAbsentConsent,
            }),
        );
        assert_eq!(
            review.scrollback.entries().back().unwrap().text,
            crate::render::profile_copy(ProfileCopyStateV1::SavedWithAbsentConsent)
        );
    }

    #[test]
    fn rollback_read_only_onboard_refuses_before_any_legacy_flow_or_copy() {
        let mut model = AppModel::default();
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::ProfilePresentationMode(
                ProfilePresentationModeV1::NativeRollbackReadOnly,
            )),
        );

        assert!(submit_text(&mut model, "/onboard").is_empty());
        assert!(model.onboarding.is_none());
        assert_eq!(model.operation, OperationState::Idle);
        let text = &model.scrollback.entries().back().unwrap().text;
        assert_eq!(
            text,
            "Dietary onboarding is unavailable in native rollback read-only mode."
        );
        assert!(!text.contains("replaces your synced profile"));
        assert!(!text.contains("Complete your dietary profile"));
    }

    #[test]
    fn native_startup_onboarding_waits_for_exact_household_bootstrap() {
        let mut model = AppModel::default();
        let generation = HouseholdModeGenerationV1::new(1).unwrap();
        let digest = HouseholdAccountBindingDigestV1::from_bytes([9; 32]);
        assert!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::ProfilePresentationMode(
                    ProfilePresentationModeV1::NativeEnabled,
                )),
            )
            .is_empty()
        );
        let bootstrap = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::HouseholdGenerationReadyV1 {
                session_mode_generation: generation,
                mode: HouseholdPresentationModeV1::NativeEnabled,
                account_binding_digest: digest,
            }),
        );
        let [
            Effect::LoadHouseholdManagementV1 {
                operation_id,
                reducer_correlation,
                ..
            },
        ] = bootstrap.as_slice()
        else {
            panic!("expected native bootstrap");
        };
        let message = "Build the native owner profile";
        assert!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::BeginNativeOwnerOnboarding {
                    message: message.into(),
                }),
            )
            .is_empty()
        );
        assert!(model.onboarding.is_none());
        assert_eq!(
            model.pending_native_startup_onboarding.as_deref(),
            Some(message)
        );
        let owner = HouseholdMemberPresentationV1::new(
            HouseholdSubjectId::self_(),
            "Me",
            RelationshipV1::Self_,
            HouseholdLifecycleV1::Active,
            HouseholdProfileStateV1::Incomplete,
            None,
        )
        .unwrap();
        assert!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::HouseholdManagementLoadedV1 {
                    operation_id: *operation_id,
                    session_mode_generation: generation,
                    reducer_correlation: *reducer_correlation,
                    purpose: HouseholdManagementLoadPurposeV1::Bootstrap,
                    account_binding_digest: digest,
                    household_revision: HouseholdRevision::new(1).unwrap(),
                    active_scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
                    members: vec![owner],
                }),
            )
            .is_empty()
        );
        assert!(model.pending_native_startup_onboarding.is_none());
        assert!(model.onboarding.as_ref().is_some_and(|flow| {
            flow.copy_mode == OnboardingCopyMode::NativeLocalFirst
                && matches!(flow.target, OnboardingTargetV1::Owner)
        }));
    }

    #[test]
    fn rollback_rejects_direct_legacy_startup_onboarding_event() {
        let mut model = AppModel::default();
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::ProfilePresentationMode(
                ProfilePresentationModeV1::NativeRollbackReadOnly,
            )),
        );
        assert!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::BeginOnboarding {
                    message: "legacy-copy-canary".into(),
                }),
            )
            .is_empty()
        );
        assert!(model.onboarding.is_none());
        assert!(model.pending_native_startup_onboarding.is_none());
        assert!(model.scrollback.entries().iter().all(|entry| {
            !entry.text.contains("legacy-copy-canary")
                && !entry.text.contains("replaces your synced profile")
        }));
        assert!(
            model
                .scrollback
                .entries()
                .back()
                .is_some_and(|entry| entry.text.contains("rollback read-only"))
        );
    }

    #[test]
    fn failed_onboarding_save_returns_to_the_review_for_an_explicit_retry() {
        let mut model = AppModel::default();
        begin_onboarding(&mut model, "Complete your dietary profile.");
        advance_to_onboarding_review(&mut model);
        let _ = submit_text(&mut model, "save");
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::OnboardingFailed {
                operation_id: 1,
                message: "profile version changed".into(),
            }),
        );
        assert_eq!(model.operation, OperationState::Idle);
        assert_eq!(
            model.onboarding.as_ref().map(|flow| flow.step),
            Some(OnboardingStep::Review)
        );
        assert!(
            model
                .scrollback
                .entries()
                .back()
                .unwrap()
                .text
                .contains("profile version changed")
        );
        assert!(matches!(
            submit_text(&mut model, "save").as_slice(),
            [Effect::SaveOnboarding {
                operation_id: 2,
                ..
            }]
        ));
    }

    #[test]
    fn invalid_onboarding_selection_stays_on_the_same_step() {
        let mut model = AppModel::default();
        begin_onboarding(&mut model, "Complete your dietary profile.");
        assert!(submit_text(&mut model, "99").is_empty());
        assert_eq!(
            model.onboarding.as_ref().map(|flow| flow.step),
            Some(OnboardingStep::Diets)
        );
        assert!(
            model
                .scrollback
                .entries()
                .iter()
                .rev()
                .any(|entry| entry.text.contains("numeric choice"))
        );
    }

    #[test]
    fn onboarding_accepts_numeric_ranges_and_bounded_custom_entries() {
        let selected = parse_multi_options("1-3, family recipe diet", diet_options(), 10, 40)
            .expect("valid range and custom diet");
        assert_eq!(selected.ids, ["gluten_free", "dairy_free", "vegetarian"]);
        assert_eq!(selected.custom, ["family recipe diet"]);
    }

    #[test]
    fn draft_remains_editable_while_streaming_and_is_not_auto_submitted() {
        let mut model = AppModel {
            draft: "lunch".into(),
            cursor: 5,
            ..AppModel::default()
        };
        let effects = dispatch(&mut model, Action::Submit);
        assert_eq!(effects.len(), 1);
        let _ = dispatch(&mut model, Action::InsertText("follow up".into()));
        assert_eq!(model.draft, "follow up");
        assert!(dispatch(&mut model, Action::Submit).is_empty());

        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Partial {
                    text: "Your meal ".into(),
                },
            }),
        );
        assert_eq!(model.draft, "follow up");
        assert!(model.scrollback.entries().back().unwrap().text.is_empty());
        assert_eq!(model.pending_agent_partial, "Your meal ");
    }

    #[test]
    fn stale_runtime_events_are_ignored() {
        let mut model = AppModel {
            draft: "question".into(),
            cursor: 8,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 99,
                event: AgentEvent::Error {
                    error: AgentFailure {
                        code: "stale".into(),
                        message: "must not appear".into(),
                        retryable: false,
                    },
                },
            }),
        );
        assert!(model.scrollback.entries().back().unwrap().text.is_empty());
        assert_eq!(model.operation, OperationState::Running(1));
    }

    #[test]
    fn runtime_text_is_terminal_safe_even_when_an_adapter_constructs_events_directly() {
        let mut model = AppModel {
            draft: "question".into(),
            cursor: 8,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Partial {
                    text: "safe\u{1b}]52;clipboard\u{7}".into(),
                },
            }),
        );
        let text = &model.scrollback.entries().back().unwrap().text;
        assert!(text.is_empty());
        assert_eq!(model.pending_agent_partial, "safe]52;clipboard");
        assert!(!text.chars().any(|character| character == '\u{1b}'));
    }

    #[test]
    fn agent_errors_render_human_messages_without_protocol_codes() {
        let mut model = AppModel {
            draft: "confirm".into(),
            cursor: 7,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Error {
                    error: AgentFailure {
                        code: "list_version_conflict".into(),
                        message:
                            "Stale Grocery list authority rejected; fetch the active list again."
                                .into(),
                        retryable: false,
                    },
                },
            }),
        );

        let text = &model.scrollback.entries().back().unwrap().text;
        assert_eq!(
            text,
            "Stale Grocery list authority rejected; fetch the active list again."
        );
        assert!(!text.contains("list_version_conflict"));
    }

    #[test]
    fn agent_errors_discard_buffered_private_partial_text() {
        let member_id = "3f1c9c2e-2f5a-4a5b-8f1e-9d2b7c6a4e01";
        let mut model = AppModel {
            draft: "question".into(),
            cursor: 8,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Partial {
                    text: format!("Private draft for {member_id}."),
                },
            }),
        );
        assert!(model.scrollback.entries().back().unwrap().text.is_empty());
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Error {
                    error: AgentFailure {
                        code: "service_error".into(),
                        message: "The request could not be completed.".into(),
                        retryable: true,
                    },
                },
            }),
        );

        let text = &model.scrollback.entries().back().unwrap().text;
        assert_eq!(text, "The request could not be completed.");
        assert!(!text.contains(member_id));
        assert!(!text.contains("Private draft"));
        assert!(model.pending_agent_partial.is_empty());
    }

    #[test]
    fn terminal_result_rejects_known_opaque_member_ids_even_without_household_shape() {
        let member_id = MemberId::parse_preserved("opaque-member-seven").unwrap();
        let member = HouseholdMemberPresentationV1::new(
            HouseholdSubjectId::member(member_id.clone()),
            "Maya",
            RelationshipV1::Child,
            HouseholdLifecycleV1::Active,
            HouseholdProfileStateV1::LocalOnly,
            Some(ProfileRevision::new(1).unwrap()),
        )
        .unwrap();
        let mut model = AppModel {
            draft: "question".into(),
            cursor: 8,
            household_snapshot: Some(HouseholdManagementSnapshotV1 {
                household_revision: HouseholdRevision::new(1).unwrap(),
                active_scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
                members: vec![
                    HouseholdMemberPresentationV1::new(
                        HouseholdSubjectId::self_(),
                        "Me",
                        RelationshipV1::Self_,
                        HouseholdLifecycleV1::Active,
                        HouseholdProfileStateV1::LocalOnly,
                        Some(ProfileRevision::new(1).unwrap()),
                    )
                    .unwrap(),
                    member,
                ],
            }),
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Partial {
                    text: format!("Partial for {}.", member_id.as_str()),
                },
            }),
        );
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Result {
                    document: serde_json::json!({
                        "text": format!("Final for {}.", member_id.as_str()),
                        "structured_content": {"type": "generic"}
                    }),
                    conversation_id: None,
                },
            }),
        );

        let text = &model.scrollback.entries().back().unwrap().text;
        assert_eq!(text, UNRENDERABLE_AGENT_RESULT_MESSAGE);
        assert!(!text.contains(member_id.as_str()));
    }

    #[test]
    fn private_choice_labels_are_never_rendered_or_retained() {
        let member_id = MemberId::parse_preserved("opaque-member-seven").unwrap();
        let member = HouseholdMemberPresentationV1::new(
            HouseholdSubjectId::member(member_id.clone()),
            "Maya",
            RelationshipV1::Child,
            HouseholdLifecycleV1::Active,
            HouseholdProfileStateV1::LocalOnly,
            Some(ProfileRevision::new(1).unwrap()),
        )
        .unwrap();
        let mut model = AppModel {
            draft: "question".into(),
            cursor: 8,
            household_snapshot: Some(HouseholdManagementSnapshotV1 {
                household_revision: HouseholdRevision::new(1).unwrap(),
                active_scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
                members: vec![
                    HouseholdMemberPresentationV1::new(
                        HouseholdSubjectId::self_(),
                        "Me",
                        RelationshipV1::Self_,
                        HouseholdLifecycleV1::Active,
                        HouseholdProfileStateV1::LocalOnly,
                        Some(ProfileRevision::new(1).unwrap()),
                    )
                    .unwrap(),
                    member,
                ],
            }),
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Choices {
                    choices: vec![
                        heyfood_core::AgentChoice::from_untrusted(
                            format!("Choose {}", member_id.as_str()),
                            None,
                        )
                        .unwrap(),
                    ],
                    allow_multiple: false,
                },
            }),
        );

        let text = &model.scrollback.entries().back().unwrap().text;
        assert_eq!(text, UNPRESENTABLE_AGENT_CHOICES_MESSAGE);
        assert!(!text.contains(member_id.as_str()));
        assert!(model.pending_choice_labels.is_empty());

        model.pending_choice_labels = vec![format!("Choose {}", member_id.as_str())];
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Result {
                    document: serde_json::json!({"text": "Choose a reviewed option."}),
                    conversation_id: None,
                },
            }),
        );
        let text = &model.scrollback.entries().back().unwrap().text;
        assert_eq!(text, "Choose a reviewed option.");
        assert!(!text.contains(member_id.as_str()));
    }

    #[test]
    fn empty_agent_error_messages_have_a_truthful_human_fallback() {
        let mut model = AppModel {
            draft: "question".into(),
            cursor: 8,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Error {
                    error: AgentFailure {
                        code: "service_error".into(),
                        message: " \n ".into(),
                        retryable: true,
                    },
                },
            }),
        );

        assert_eq!(
            model.scrollback.entries().back().unwrap().text,
            "hey.food could not complete this request. You can try again now."
        );
    }

    #[test]
    fn agent_error_messages_are_terminal_safe() {
        let mut model = AppModel {
            draft: "question".into(),
            cursor: 8,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Error {
                    error: AgentFailure {
                        code: "service_error".into(),
                        message: "Safe guidance\u{1b}]52;clipboard\u{7}".into(),
                        retryable: false,
                    },
                },
            }),
        );

        let text = &model.scrollback.entries().back().unwrap().text;
        assert_eq!(text, "Safe guidance]52;clipboard");
        assert!(!text.chars().any(|character| character == '\u{1b}'));
    }

    #[test]
    fn thinking_stages_are_always_presented_as_human_progress() {
        for (stage, expected) in [
            ("resolving_restaurant", "Finding the right restaurant…"),
            ("loading_menu", "Loading the latest menu…"),
            ("evaluating_menu", "Checking the menu against your profile…"),
            (
                "applying_dietary_graph",
                "Considering your dietary profile…",
            ),
            ("searching_recipes", "Finding recipes that fit…"),
            ("checking_food", "Checking this food against your profile…"),
        ] {
            assert_eq!(thinking_activity(Some(stage), None), expected);
        }

        assert_eq!(
            thinking_activity(None, Some("evaluate_menu")),
            "Checking the menu against your profile…"
        );
        assert_eq!(
            thinking_activity(Some("tool_use"), Some("evaluating_menu")),
            "Checking the menu against your profile…"
        );
        assert_eq!(
            thinking_activity(Some("future_internal_stage"), None),
            "Working through your question…"
        );
        assert_eq!(
            thinking_activity(Some("evaluating_menu"), Some("Reviewing 22 menu items…")),
            "Reviewing 22 menu items…"
        );
    }

    #[test]
    fn thinking_runtime_event_never_exposes_unknown_machine_identifiers() {
        let mut model = AppModel {
            draft: "question".into(),
            cursor: 8,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Thinking {
                    stage: Some("future_internal_stage".into()),
                    message: None,
                },
            }),
        );
        assert_eq!(
            model.activity.as_deref(),
            Some("Working through your question…")
        );
        assert!(!model.activity.as_deref().unwrap().contains('_'));
    }

    #[test]
    fn progress_runtime_event_never_exposes_machine_identifiers() {
        let mut model = AppModel {
            draft: "question".into(),
            cursor: 8,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Progress {
                    message: "evaluating_menu".into(),
                    current: Some(1),
                    total: Some(2),
                },
            }),
        );
        assert_eq!(
            model.activity.as_deref(),
            Some("Checking the menu against your profile… (1/2)")
        );
        assert!(!model.activity.as_deref().unwrap().contains('_'));
    }

    #[test]
    fn activity_events_never_expose_private_household_identifiers() {
        let member_id = "3f1c9c2e-2f5a-4a5b-8f1e-9d2b7c6a4e01";
        let mut model = AppModel {
            draft: "question".into(),
            cursor: 8,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Thinking {
                    stage: None,
                    message: Some(format!("Checking {member_id}")),
                },
            }),
        );
        assert_eq!(
            model.activity.as_deref(),
            Some("Working through your question…")
        );
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Progress {
                    message: format!("Checking {member_id}"),
                    current: Some(1),
                    total: Some(2),
                },
            }),
        );
        assert_eq!(model.activity.as_deref(), Some("Making progress… (1/2)"));
        assert!(!model.activity.as_deref().unwrap().contains(member_id));
    }

    #[test]
    fn scrollback_is_bounded_by_entries_and_lines() {
        let mut scrollback = Scrollback::bounded(3, 4, 1_024);
        for number in 0..8 {
            scrollback.push(SemanticEntry {
                speaker: Speaker::Notice,
                text: format!("entry {number}\nline"),
                streaming: false,
            });
        }
        assert!(scrollback.entries().len() <= 3);
        assert!(scrollback.rendered_lines() <= 4);
        assert!(scrollback.entries().back().unwrap().text.contains('7'));

        scrollback.push(SemanticEntry {
            speaker: Speaker::Assistant,
            text: (0..20)
                .map(|line| format!("line {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
            streaming: true,
        });
        assert_eq!(scrollback.rendered_lines(), 4);
        assert_eq!(scrollback.entries().len(), 1);
        assert!(
            scrollback
                .entries()
                .back()
                .unwrap()
                .text
                .contains("line 19")
        );
    }

    #[test]
    fn one_unbroken_stream_is_bounded_by_utf8_bytes() {
        let mut scrollback = Scrollback::bounded(3, 100, 96);
        scrollback.push(SemanticEntry {
            speaker: Speaker::Assistant,
            text: String::new(),
            streaming: true,
        });
        scrollback.mutate_last_assistant(|entry| {
            entry.text.push_str(&"é".repeat(1_000));
        });
        assert!(scrollback.rendered_bytes() <= 96);
        assert!(scrollback.entries().back().unwrap().text.ends_with('é'));
        assert!(
            scrollback
                .entries()
                .back()
                .unwrap()
                .text
                .starts_with(TRUNCATION_NOTICE)
        );
    }

    #[test]
    fn uncertain_post_dispatch_cancellation_is_not_presented_as_safe_to_retry() {
        let mut model = AppModel {
            draft: "mutating question".into(),
            cursor: 17,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnFinished {
                operation_id: 1,
                outcome: RunTurnOutcome::CancelledAfterDispatchOutcomeUnknown,
            }),
        );
        let text = &model.scrollback.entries().back().unwrap().text;
        assert!(text.contains("server outcome is unknown"));
        assert!(text.contains("Check current state before retrying"));
    }

    #[test]
    fn inactivity_releases_screened_partial_content_and_restores_a_usable_composer() {
        let mut model = AppModel {
            draft: "What do you know about me?".into(),
            cursor: 26,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Partial {
                    text: "I can consider your saved dietary profile.".into(),
                },
            }),
        );
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Choices {
                    choices: vec![
                        heyfood_core::AgentChoice::from_untrusted(
                            "Review my profile".into(),
                            Some("review".into()),
                        )
                        .unwrap(),
                    ],
                    allow_multiple: false,
                },
            }),
        );
        let failure = TurnFailure::from_port_error(&heyfood_application::PortError::new(
            "sse_inactivity",
            "event stream inactivity deadline expired",
        ));
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnFailed {
                operation_id: 1,
                failure,
            }),
        );

        let text = &model.scrollback.entries().back().unwrap().text;
        assert!(text.contains("I can consider your saved dietary profile."));
        assert_eq!(text.matches("Review my profile").count(), 1);
        assert!(text.contains("This response stopped before it finished."));
        assert!(text.contains("hey.food did not retry it."));
        assert!(text.contains("You can ask a new question now."));
        assert!(!text.contains("sse_inactivity"));
        assert!(!text.contains("inactivity deadline expired"));
        assert!(!model.scrollback.entries().back().unwrap().streaming);
        assert_eq!(model.operation, OperationState::Idle);
        assert!(model.activity.is_none());
        assert!(model.pending_choice_labels.is_empty());

        model.draft = "Try an independent question".into();
        model.cursor = model.draft.chars().count();
        assert!(matches!(
            dispatch(&mut model, Action::Submit).as_slice(),
            [Effect::SubmitTurn {
                operation_id: 2,
                prompt
            }] if prompt == "Try an independent question"
        ));
    }

    #[test]
    fn interrupted_stream_never_releases_a_known_opaque_household_member_id() {
        let member_id = MemberId::parse_preserved("opaque-member-seven").unwrap();
        let owner = HouseholdMemberPresentationV1::new(
            HouseholdSubjectId::self_(),
            "Me",
            RelationshipV1::Self_,
            HouseholdLifecycleV1::Active,
            HouseholdProfileStateV1::LocalOnly,
            Some(ProfileRevision::new(1).unwrap()),
        )
        .unwrap();
        let member = HouseholdMemberPresentationV1::new(
            HouseholdSubjectId::member(member_id.clone()),
            "Maya",
            RelationshipV1::Child,
            HouseholdLifecycleV1::Active,
            HouseholdProfileStateV1::LocalOnly,
            Some(ProfileRevision::new(1).unwrap()),
        )
        .unwrap();
        let mut model = AppModel {
            draft: "question".into(),
            cursor: 8,
            household_snapshot: Some(HouseholdManagementSnapshotV1 {
                household_revision: HouseholdRevision::new(1).unwrap(),
                active_scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
                members: vec![owner, member],
            }),
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Partial {
                    text: format!("Unreviewed prose for {}.", member_id.as_str()),
                },
            }),
        );
        assert!(model.scrollback.entries().back().unwrap().text.is_empty());
        let failure = TurnFailure::from_port_error(&heyfood_application::PortError::new(
            "sse_inactivity",
            "event stream inactivity deadline expired",
        ));
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnFailed {
                operation_id: 1,
                failure,
            }),
        );

        let text = &model.scrollback.entries().back().unwrap().text;
        assert!(!text.contains(member_id.as_str()));
        assert!(!text.contains("Unreviewed prose"));
        assert!(text.contains("This response stopped before it finished."));
    }

    #[test]
    fn expired_sign_in_names_the_login_recovery_without_guessing_about_connectivity() {
        let mut model = AppModel {
            draft: "What can I eat?".into(),
            cursor: 15,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let failure = TurnFailure::from_port_error(&heyfood_application::PortError::new(
            "login_required",
            "private authorization detail",
        ));
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnFailed {
                operation_id: 1,
                failure,
            }),
        );

        let text = &model.scrollback.entries().back().unwrap().text;
        assert!(text.contains("sign-in expired"));
        assert!(text.contains("heyfood login"));
        assert!(text.contains("This turn was not sent"));
        assert!(!text.contains("Check your connection"));
        assert!(!text.contains("private authorization detail"));
        assert_eq!(model.operation, OperationState::Idle);
    }

    #[test]
    fn changed_account_requires_a_clean_tui_restart_without_dispatch_advice() {
        let mut model = AppModel {
            draft: "What can I eat?".into(),
            cursor: 15,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let failure = TurnFailure::from_port_error(&heyfood_application::PortError::new(
            "interactive_account_changed",
            "private account detail",
        ));
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnFailed {
                operation_id: 1,
                failure,
            }),
        );

        let text = &model.scrollback.entries().back().unwrap().text;
        assert!(text.contains("account changed"));
        assert!(text.contains("Exit and reopen heyfood"));
        assert!(text.contains("This turn was not sent"));
        assert!(!text.contains("private account detail"));
        assert_eq!(model.operation, OperationState::Idle);
    }

    #[test]
    fn scrolling_away_preserves_position_and_counts_streamed_updates() {
        let mut model = AppModel {
            draft: "question".into(),
            cursor: 8,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(&mut model, Action::ScrollUp(5));
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Partial {
                    text: "one\ntwo".into(),
                },
            }),
        );
        assert!(!model.follow_tail);
        assert!(model.scroll_from_tail > 5);
        assert!(model.unseen_lines > 0);
        let _ = dispatch(&mut model, Action::FollowTail);
        assert!(model.follow_tail);
        assert_eq!(model.unseen_lines, 0);
    }

    #[test]
    fn keyboard_cancel_has_clear_cancel_and_double_exit_states() {
        let mut model = AppModel {
            draft: "draft".into(),
            cursor: 5,
            ..AppModel::default()
        };
        assert!(dispatch(&mut model, Action::CancelOrExit).is_empty());
        assert!(model.draft.is_empty());

        model.draft = "turn".into();
        model.cursor = 4;
        let _ = dispatch(&mut model, Action::Submit);
        assert_eq!(
            dispatch(&mut model, Action::CancelOrExit),
            vec![Effect::CancelTurn { operation_id: 1 }]
        );
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnFinished {
                operation_id: 1,
                outcome: RunTurnOutcome::CancelledAfterServerAcceptance,
            }),
        );
        assert!(dispatch(&mut model, Action::CancelOrExit).is_empty());
        assert!(model.idle_exit_armed);
        assert_eq!(
            dispatch(&mut model, Action::CancelOrExit),
            vec![Effect::Exit(ExitReason::Requested)]
        );
    }

    #[test]
    fn external_signal_cancels_and_exits_with_platform_code() {
        let mut model = AppModel {
            draft: "turn".into(),
            cursor: 4,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        assert_eq!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::ExternalSignal(ExitReason::Terminate))
            ),
            vec![
                Effect::CancelTurn { operation_id: 1 },
                Effect::Exit(ExitReason::Terminate),
            ]
        );
        assert_eq!(ExitReason::Terminate.exit_code(), 143);
    }

    #[test]
    fn slash_commands_are_local_and_new_resets_conversation() {
        let mut model = AppModel {
            draft: "/help".into(),
            cursor: 5,
            ..AppModel::default()
        };
        assert!(dispatch(&mut model, Action::Submit).is_empty());
        assert!(model.draft.is_empty());
        assert!(
            model
                .scrollback
                .entries()
                .back()
                .unwrap()
                .text
                .contains("/new")
        );

        model.draft = "/new".into();
        model.cursor = 4;
        assert_eq!(
            dispatch(&mut model, Action::Submit),
            vec![Effect::ResetConversation]
        );
    }

    #[test]
    fn prompt_history_restores_the_unsent_draft() {
        let mut model = AppModel {
            draft: "first".into(),
            cursor: 5,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnFinished {
                operation_id: 1,
                outcome: RunTurnOutcome::Completed,
            }),
        );
        model.draft = "working draft".into();
        model.cursor = model.draft.chars().count();
        let _ = dispatch(&mut model, Action::HistoryPrevious);
        assert_eq!(model.draft, "first");
        let _ = dispatch(&mut model, Action::HistoryNext);
        assert_eq!(model.draft, "working draft");
    }

    #[test]
    fn tab_completes_a_unique_slash_prefix() {
        let mut model = AppModel {
            draft: "/sta".into(),
            cursor: 4,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::CompleteSlash);
        assert_eq!(model.draft, "/status");
        assert_eq!(model.cursor, 7);
    }

    #[test]
    fn command_registry_drives_aliases_help_and_discovery() {
        let mut model = AppModel {
            draft: "/".into(),
            cursor: 1,
            ..AppModel::default()
        };
        assert_eq!(slash_suggestions(&model, 3).len(), 3);
        let _ = dispatch(&mut model, Action::CompleteSlash);
        assert_eq!(model.draft, "/", "ambiguous prefixes must remain editable");

        model.draft = "/quit".into();
        model.cursor = 5;
        assert_eq!(
            dispatch(&mut model, Action::Submit),
            vec![Effect::Exit(ExitReason::Requested)]
        );
    }

    #[test]
    fn terminal_message_and_response_fields_use_normalized_result_text() {
        for (document, expected) in [
            (
                serde_json::json!({"message": "final message"}),
                "final message",
            ),
            (
                serde_json::json!({"response": "final response"}),
                "final response",
            ),
        ] {
            let mut model = AppModel {
                draft: "question".into(),
                cursor: 8,
                ..AppModel::default()
            };
            let _ = dispatch(&mut model, Action::Submit);
            let _ = dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::TurnEvent {
                    operation_id: 1,
                    event: AgentEvent::Partial {
                        text: "streamed draft".into(),
                    },
                }),
            );
            let _ = dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::TurnEvent {
                    operation_id: 1,
                    event: AgentEvent::Result {
                        document,
                        conversation_id: None,
                    },
                }),
            );
            let entry = model.scrollback.entries().back().unwrap();
            assert_eq!(entry.text, expected);
            assert!(!entry.text.contains('{'));
            assert!(!entry.streaming);
        }
    }

    #[test]
    fn terminal_result_never_dumps_an_unrecognized_structured_result() {
        let mut model = AppModel {
            draft: "Can I see the full menu?".into(),
            cursor: 24,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Result {
                    document: serde_json::json!({
                        "structured": {
                            "type": "future_menu_presentation",
                            "sections": [{
                                "name": "Tea",
                                "items": [{
                                    "item_id": "18fbb9d6-85a1-4e04-bd44-a8348507048c",
                                    "name": "12 oz Chai Latte",
                                    "price_cents": 450,
                                    "safety": {
                                        "_self": {
                                            "level": "caution",
                                            "reason": "Verify sweetness level."
                                        }
                                    }
                                }]
                            }]
                        }
                    }),
                    conversation_id: None,
                },
            }),
        );

        let entry = model.scrollback.entries().back().unwrap();
        assert_eq!(
            entry.text,
            heyfood_application::household_menu::UNPRESENTABLE_HOUSEHOLD_MENU_MESSAGE
        );
        for protocol_fragment in ["item_id", "\"safety\"", "_self", "{", "}"] {
            assert!(!entry.text.contains(protocol_fragment), "{}", entry.text);
        }
        assert!(!entry.streaming);
    }

    #[test]
    fn terminal_result_renders_the_structured_household_menu_without_model_prose() {
        let mut model = AppModel {
            draft: "Show me this week's menu".into(),
            cursor: 24,
            height: 8,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Result {
                    document: serde_json::json!({
                        "text": "Here are the current options.",
                        "structured": {
                            "type": "household_menu",
                            "presentation": "full_menu",
                            "restaurant_name": "Abby Jane Bakeshop",
                            "source_url": "https://example.test/abby-jane",
                            "menu_freshness": "Menu updated 2 hours ago",
                            "captured_at": "2026-07-26T17:27:14Z",
                            "freshness_hours": 2.0,
                            "requested_max_age_seconds": 86400,
                            "is_stale": false,
                            "sections": [{
                                "name": "Bread",
                                "items": [
                                    {
                                        "name": "Baguette",
                                        "price_cents": 400,
                                        "composite_level": "avoid"
                                    },
                                    {
                                        "name": "Big Country",
                                        "price_cents": 900,
                                        "composite_level": "caution"
                                    }
                                ]
                            }]
                        }
                    }),
                    conversation_id: None,
                },
            }),
        );
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnFinished {
                operation_id: 1,
                outcome: RunTurnOutcome::Completed,
            }),
        );

        let entry = model.scrollback.entries().back().unwrap();
        assert!(
            entry
                .text
                .starts_with("Current menu at Abby Jane Bakeshop\n")
        );
        assert!(!entry.text.contains("Here are the current options."));
        for expected in [
            "Current menu at Abby Jane Bakeshop",
            "Source: https://example.test/abby-jane",
            "Freshness: Menu updated 2 hours ago",
            "Captured: 2026-07-26T17:27:14Z",
            "1 sections · 2 items · Page Up/Page Down to browse",
            "Bread",
            "• Baguette  $4.00  [avoid]",
            "• Big Country  $9.00  [caution]",
        ] {
            assert!(entry.text.lines().any(|line| line == expected));
        }
        assert_eq!(entry.text.matches("• ").count(), 2);
        assert!(!entry.streaming);
        assert!(!model.follow_tail);
        assert!(model.focus_latest_result_start);
        assert_eq!(model.latest_result_start_offset, 0);
    }

    #[test]
    fn terminal_result_renders_named_household_evaluation_without_protocol_metadata() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/contracts/household-backend/v1/fixtures/household_evaluation/founding_scenario_maya_menu.json"
        )))
        .unwrap();
        let mut model = AppModel {
            draft: "What can everyone eat?".into(),
            cursor: 22,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let private_id = "3f1c9c2e-2f5a-4a5b-8f1e-9d2b7c6a4e01";
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Partial {
                    text: format!("Unfiltered streamed prose for {private_id}."),
                },
            }),
        );
        assert!(model.scrollback.entries().back().unwrap().text.is_empty());
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Result {
                    document: serde_json::json!({
                        "text": format!("Unfiltered final prose for {private_id}."),
                        "structured_content": fixture["result"].clone()
                    }),
                    conversation_id: None,
                },
            }),
        );

        let entry = model.scrollback.entries().back().unwrap();
        for expected in [
            "Household evaluation at Bistro One",
            "Household result: Avoid",
            "Jordan: Generally safer",
            "Maya: Avoid",
        ] {
            assert!(entry.text.contains(expected), "{}", entry.text);
        }
        for forbidden in [
            "3f1c9c2e-2f5a-4a5b-8f1e-9d2b7c6a4e01",
            "54aa3228a67d4e262d383d0cfba6be4f4c0c94f21f5d095f3127d00928586bcb",
            "stub-model-1",
            "dietary-rules-1",
            "member_annotations",
            "context_hash",
            "{\"",
            "Unfiltered",
        ] {
            assert!(!entry.text.contains(forbidden), "{}", entry.text);
        }
        assert!(!entry.streaming);
    }

    #[test]
    fn owner_only_null_label_evaluation_can_release_a_safe_buffered_partial() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/contracts/household-backend/v1/fixtures/household_evaluation/founding_scenario_maya_menu.json"
        )))
        .unwrap();
        let mut result = fixture["result"].clone();
        result["items"][0]["status"] = serde_json::json!("generally_safer");
        result["items"][0]["confidence"] = serde_json::json!(0.95);
        result["items"][0]["summary"] = serde_json::json!("No concerns.");
        for item in result["items"].as_array_mut().unwrap() {
            item["member_annotations"] =
                serde_json::Value::Array(vec![item["member_annotations"][0].clone()]);
            item["member_annotations"][0]["label"] = serde_json::Value::Null;
        }
        result["generally_safer"] = serde_json::json!(["Garlic Noodles", "Steamed Jasmine Rice"]);
        result["avoid"] = serde_json::json!([]);
        result["household"]["members"] =
            serde_json::Value::Array(vec![result["household"]["members"][0].clone()]);
        result["household"]["member_count"] = serde_json::json!(1);

        let mut model = AppModel {
            draft: "What can I eat?".into(),
            cursor: 15,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Partial {
                    text: "Legacy owner guidance.".into(),
                },
            }),
        );
        assert!(model.scrollback.entries().back().unwrap().text.is_empty());
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Result {
                    document: serde_json::json!({"structured_content": result}),
                    conversation_id: None,
                },
            }),
        );

        assert_eq!(
            model.scrollback.entries().back().unwrap().text,
            "Legacy owner guidance."
        );
    }

    #[test]
    fn malformed_household_evaluation_replaces_prose_with_the_safe_refusal() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/contracts/household-backend/v1/fixtures/household_evaluation/founding_scenario_maya_menu.json"
        )))
        .unwrap();
        let mut result = fixture["result"].clone();
        result["items"][0]["member_annotations"][1]
            .as_object_mut()
            .unwrap()
            .remove("label");
        let mut model = AppModel {
            draft: "What can everyone eat?".into(),
            cursor: 22,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Partial {
                    text: "Raw 3f1c9c2e-2f5a-4a5b-8f1e-9d2b7c6a4e01.".into(),
                },
            }),
        );
        assert!(model.scrollback.entries().back().unwrap().text.is_empty());
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Result {
                    document: serde_json::json!({
                        "text": "This unreviewed prose must not survive.",
                        "structured_content": result
                    }),
                    conversation_id: None,
                },
            }),
        );

        let entry = model.scrollback.entries().back().unwrap();
        assert_eq!(
            entry.text,
            heyfood_application::UNPRESENTABLE_HOUSEHOLD_EVALUATION_MESSAGE
        );
        assert!(!entry.text.contains("unreviewed"));
        assert!(!entry.text.contains("3f1c9c2e"));
        assert!(!entry.streaming);
    }

    #[test]
    fn malformed_household_menu_replaces_partial_and_final_prose_with_safe_refusal() {
        let member_id = "3f1c9c2e-2f5a-4a5b-8f1e-9d2b7c6a4e01";
        let mut model = AppModel {
            draft: "Show the household menu".into(),
            cursor: 23,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Partial {
                    text: format!("Raw streamed prose for {member_id}."),
                },
            }),
        );
        assert!(model.scrollback.entries().back().unwrap().text.is_empty());
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Result {
                    document: serde_json::json!({
                        "text": format!("Raw final prose for {member_id}."),
                        "structured": {
                            "type": "household_menu",
                            "presentation": "full_menu",
                            "sections": [{
                                "name": "Dinner",
                                "items": [{
                                    "name": "Soup",
                                    "composite_level": "future_status",
                                    "safety": {
                                        (member_id): {
                                            "level": "future_status",
                                            "reason": "Unreviewed."
                                        }
                                    }
                                }]
                            }]
                        }
                    }),
                    conversation_id: None,
                },
            }),
        );

        let entry = model.scrollback.entries().back().unwrap();
        assert_eq!(
            entry.text,
            heyfood_application::household_menu::UNPRESENTABLE_HOUSEHOLD_MENU_MESSAGE
        );
        assert!(!entry.text.contains(member_id));
        assert!(!entry.text.contains("Raw"));
        assert!(!entry.streaming);
    }

    #[test]
    fn terminal_result_renders_ranked_restaurant_picks_and_a_next_step() {
        let mut model = AppModel {
            draft: "What can I eat there?".into(),
            cursor: 20,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Result {
                    document: serde_json::json!({
                        "text": "I found several options that fit.",
                        "structured": {
                            "type": "household_menu",
                            "restaurant_name": "Harbor Cafe",
                            "menu_freshness": "Menu updated 2 hours ago",
                            "source_url": "https://example.test/menu",
                            "member_summaries": [{
                                "member_id": "_self",
                                "label": null
                            }],
                            "sections": [{
                                "name": "Dinner",
                                "items": [{
                                    "item_id": "item-1",
                                    "name": "Grilled Fish",
                                    "price_cents": 2400,
                                    "safety": {
                                        "_self": {
                                            "level": "safe",
                                            "reason": "No detected conflicts."
                                        }
                                    }
                                }]
                            }],
                            "agent_picks": {
                                "_self": [{
                                    "item_id": "item-1",
                                    "member_id": "_self",
                                    "reason": "A simple preparation with no detected conflicts.",
                                    "tag": "Top pick"
                                }]
                            }
                        }
                    }),
                    conversation_id: None,
                },
            }),
        );

        let entry = model.scrollback.entries().back().unwrap();
        for expected in [
            "Top picks at Harbor Cafe",
            "For you",
            "1. Grilled Fish  $24.00  [generally safer] · Top pick",
            "   A simple preparation with no detected conflicts.",
            "Ask about any pick, or say `show me the full menu` for every evaluated option.",
        ] {
            assert!(entry.text.lines().any(|line| line == expected));
        }
        assert!(!entry.text.contains("I found several options that fit."));
        assert!(!entry.text.contains("_self"));
        assert!(!entry.streaming);
    }

    #[test]
    fn terminal_result_preserves_choices_after_partial_content() {
        for field in ["message", "text", "response"] {
            let mut model = AppModel {
                draft: "question".into(),
                cursor: 8,
                ..AppModel::default()
            };
            let _ = dispatch(&mut model, Action::Submit);
            let _ = dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::TurnEvent {
                    operation_id: 1,
                    event: AgentEvent::Partial {
                        text: "Review the available paths.".into(),
                    },
                }),
            );
            let _ = dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::TurnEvent {
                    operation_id: 1,
                    event: AgentEvent::Choices {
                        choices: vec![
                            heyfood_core::AgentChoice::from_untrusted(
                                "Cook at home".into(),
                                Some("cook".into()),
                            )
                            .unwrap(),
                            heyfood_core::AgentChoice::from_untrusted(
                                "Eat out".into(),
                                Some("restaurant".into()),
                            )
                            .unwrap(),
                        ],
                        allow_multiple: false,
                    },
                }),
            );
            let mut document = serde_json::json!({});
            document[field] = serde_json::Value::String("Which path works for you?".into());
            let _ = dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::TurnEvent {
                    operation_id: 1,
                    event: AgentEvent::Result {
                        document,
                        conversation_id: Some("conversation-choices".into()),
                    },
                }),
            );
            let entry = model.scrollback.entries().back().unwrap();
            assert_eq!(
                entry.text,
                "Which path works for you?\n\nOptions\n• Cook at home\n• Eat out"
            );
            assert!(!entry.streaming);
        }
    }

    #[test]
    fn production_grocery_confirmation_renders_safety_and_requires_typed_accept_or_cancel() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/contracts/grocery-backend/phase-a/fixtures/grocery/confirmation_round_trip.json"
        ))
        .unwrap();
        let mut structured = fixture["card"].clone();
        let structured = structured.as_object_mut().unwrap();
        structured.insert(
            "confirmation_id".into(),
            fixture["accept_payload"]["confirmation_id"].clone(),
        );
        structured.insert(
            "idempotency_key".into(),
            fixture["accept_payload"]["idempotency_key"].clone(),
        );
        structured.insert(
            "preview".into(),
            serde_json::json!("Add one screened ingredient"),
        );
        structured.insert(
            "expires_at".into(),
            serde_json::json!("2026-07-22T12:05:00Z"),
        );
        let confirmation_document = serde_json::json!({
            "text": "I prepared a grocery update.",
            "structured": structured
        });
        let mut model = AppModel {
            draft: "add ingredients".into(),
            cursor: 15,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Result {
                    document: confirmation_document,
                    conversation_id: Some("conversation-grocery".into()),
                },
            }),
        );
        let card = &model.scrollback.entries().back().unwrap().text;
        assert!(card.contains("Review before changing anything"));
        assert!(card.contains("1. onion · 1"));
        assert!(card.contains("source: manual"));
        assert!(card.contains("ingredient screening: risky"));
        assert!(card.contains("Household member: risky"));
        assert!(!card.contains("maya-uuid"));
        assert!(card.contains("Onion is high-FODMAP."));
        assert!(card.contains("try: scallion greens"));
        assert!(card.contains("Screened at ingredient level — verify the product label."));
        assert!(card.contains("Type `y` to confirm or `n` to cancel"));
        assert!(card.contains("edit #N <replacement>"));
        assert!(!card.contains("confirmation_id"));
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnFinished {
                operation_id: 1,
                outcome: RunTurnOutcome::Completed,
            }),
        );

        model.draft = "y".into();
        model.cursor = 1;
        let effects = dispatch(&mut model, Action::Submit);
        assert!(matches!(
            effects.as_slice(),
            [Effect::ConfirmAction { operation_id: 2, command }]
                if command.decision == ConfirmationDecisionWire::Accept
                    && command.edits.is_none()
                    && command.confirmation_id.as_uuid().to_string()
                        == "00000000-0000-0000-0000-000000000001"
        ));
    }

    #[test]
    fn confirmation_safety_fails_closed_without_rendering_ids_or_unknown_enums() {
        let item = serde_json::json!({
            "intended_for": "550e8400-e29b-41d4-a716-446655440000",
            "safety": {
                "status": "future_protocol_status",
                "member_flags": [{
                    "member_id": "550e8400-e29b-41d4-a716-446655440000",
                    "status": "future_protocol_status",
                    "reason": "Verify ingredients."
                }]
            }
        });
        let mut rendered = String::new();
        render_confirmation_safety(
            &mut rendered,
            &item,
            item.get("intended_for").and_then(serde_json::Value::as_str),
        );
        assert!(rendered.contains("unable to evaluate"));
        assert!(rendered.contains("Household member"));
        assert!(rendered.contains("intended"));
        assert!(!rendered.contains("future_protocol_status"));
        assert!(!rendered.contains("550e8400-e29b-41d4-a716-446655440000"));
    }

    #[test]
    fn grocery_add_item_edit_is_bounded_explicit_and_sent_as_c3_edits() {
        let envelope: ActionConfirmationEnvelopeWire = serde_json::from_value(serde_json::json!({
            "type": "action_confirmation",
            "confirmation_id": "00000000-0000-4000-8000-000000000001",
            "idempotency_key": "00000000-0000-4000-8000-000000000002",
            "action": "grocery_list_add_items",
            "preview": "Add two screened ingredients",
            "card_form": "item_list",
            "structured_preview": {
                "items": [
                    {
                        "requested_name": "onion",
                        "quantity": 1,
                        "unit": "each",
                        "intended_for": "maya",
                        "safety": {"status": "risky"}
                    },
                    {
                        "name": "milk",
                        "quantity": 2,
                        "unit": "cartons"
                    }
                ]
            }
        }))
        .unwrap();
        let editable_items = editable_grocery_items(&envelope).unwrap();
        let mut model = AppModel {
            draft: "edit #1 scallion greens".into(),
            cursor: 23,
            pending_confirmation: Some(PendingActionConfirmation {
                confirmation_id: envelope.confirmation_id,
                idempotency_key: envelope.idempotency_key,
                editable_items: Some(editable_items),
            }),
            ..AppModel::default()
        };

        let effects = dispatch(&mut model, Action::Submit);
        let command = match effects.as_slice() {
            [
                Effect::ConfirmAction {
                    operation_id: 1,
                    command,
                },
            ] => command,
            effects => panic!("expected edited confirmation, got {effects:?}"),
        };
        assert_eq!(command.decision, ConfirmationDecisionWire::Accept);
        assert_eq!(
            serde_json::to_value(command.edits.as_ref().unwrap()).unwrap(),
            serde_json::json!({
                "items": [
                    {
                        "name": "scallion greens",
                        "quantity": 1.0,
                        "unit": "each",
                        "intended_for": "maya",
                        "source_type": "manual"
                    },
                    {
                        "name": "milk",
                        "quantity": 2.0,
                        "unit": "cartons",
                        "source_type": "manual"
                    }
                ]
            })
        );
        assert!(model.pending_confirmation.is_some());
        assert!(model.draft.is_empty());
        assert_eq!(
            model.scrollback.entries().iter().rev().nth(1).unwrap().text,
            "Edit and confirm"
        );
    }

    #[test]
    fn grocery_edit_eligibility_matches_the_frozen_twenty_five_item_limit() {
        let item = serde_json::json!({
            "requested_name": "oats",
            "quantity": 1,
            "unit": "bag",
            "note": "rolled",
            "intended_for": "maya"
        });
        let make_envelope = |count| {
            serde_json::from_value::<ActionConfirmationEnvelopeWire>(serde_json::json!({
                "type": "action_confirmation",
                "confirmation_id": "00000000-0000-4000-8000-000000000001",
                "idempotency_key": "00000000-0000-4000-8000-000000000002",
                "action": "grocery_list_add_items",
                "preview": "Add screened ingredients",
                "card_form": "item_list",
                "structured_preview": {
                    "items": vec![item.clone(); count]
                }
            }))
            .unwrap()
        };

        assert_eq!(
            editable_grocery_items(&make_envelope(25)).unwrap().len(),
            25
        );
        assert!(editable_grocery_items(&make_envelope(26)).is_none());
    }

    #[test]
    fn phase_a_source_provenance_is_visible_and_bounded() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/contracts/grocery-backend/phase-a/fixtures/grocery/export_json.json"
        )))
        .unwrap();
        let production_item = fixture["list"]["items"][0].clone();
        let envelope: ActionConfirmationEnvelopeWire = serde_json::from_value(serde_json::json!({
            "type": "action_confirmation",
            "confirmation_id": "00000000-0000-4000-8000-000000000001",
            "idempotency_key": "00000000-0000-4000-8000-000000000002",
            "action": "grocery_list_add_items",
            "preview": "Add a recipe ingredient",
            "card_form": "item_list",
            "structured_preview": {"items": [production_item]}
        }))
        .unwrap();

        let card = render_action_confirmation(&envelope);
        assert!(card.contains("source: recipe:dahl-001 · Red Lentil Dahl"));

        let mut over_limit_item = fixture["list"]["items"][0].clone();
        over_limit_item["sources"] = serde_json::Value::Array(
            (0..=MAX_CONFIRMATION_SOURCES_PER_ITEM)
                .map(|index| {
                    serde_json::json!({
                        "source_type": "recipe",
                        "source_ref": format!("recipe-{index}")
                    })
                })
                .collect(),
        );
        let envelope: ActionConfirmationEnvelopeWire = serde_json::from_value(serde_json::json!({
            "type": "action_confirmation",
            "confirmation_id": "00000000-0000-4000-8000-000000000001",
            "idempotency_key": "00000000-0000-4000-8000-000000000002",
            "action": "grocery_list_add_items",
            "preview": "Add a recipe ingredient",
            "card_form": "item_list",
            "structured_preview": {"items": [over_limit_item]}
        }))
        .unwrap();
        let card = render_action_confirmation(&envelope);
        assert!(card.contains("source: recipe:recipe-7"));
        assert!(!card.contains("source: recipe:recipe-8"));
        assert!(card.contains("source: … and 1 more"));
    }

    #[test]
    fn generic_c3_safety_flags_and_targeting_remain_visible() {
        let envelope: ActionConfirmationEnvelopeWire = serde_json::from_value(serde_json::json!({
            "type": "action_confirmation",
            "confirmation_id": "00000000-0000-4000-8000-000000000001",
            "idempotency_key": "00000000-0000-4000-8000-000000000002",
            "action": "grocery_list_add_items",
            "preview": "Add a targeted ingredient",
            "card_form": "item_list",
            "structured_preview": {
                "items": [{
                    "name": "tomato",
                    "quantity": 2,
                    "unit": "each",
                    "intended_for": "maya",
                    "provenance": "menu",
                    "safety_flags": [{
                        "member_id": "maya",
                        "status": "avoid",
                        "reason": "Member-specific conflict"
                    }]
                }]
            }
        }))
        .unwrap();
        let card = render_action_confirmation(&envelope);
        assert!(card.contains("1. tomato for a household member · 2 each"));
        assert!(card.contains("source: menu"));
        assert!(card.contains("Household member: avoid · intended"));
        assert!(card.contains("Member-specific conflict"));
        assert!(!card.contains("maya"));
    }

    #[test]
    fn ctrl_c_cancels_a_pending_action_confirmation_through_the_server() {
        let mut model = AppModel {
            draft: "an unsubmitted answer".into(),
            cursor: 21,
            pending_confirmation: Some(PendingActionConfirmation {
                confirmation_id: heyfood_core::GroceryConfirmationId::parse(
                    "00000000-0000-4000-8000-000000000001",
                )
                .unwrap(),
                idempotency_key: heyfood_core::GroceryIdempotencyKey::parse(
                    "00000000-0000-4000-8000-000000000002",
                )
                .unwrap(),
                editable_items: None,
            }),
            ..AppModel::default()
        };
        let effects = dispatch(&mut model, Action::CancelOrExit);
        assert!(matches!(
            effects.as_slice(),
            [Effect::ConfirmAction { operation_id: 1, command }]
                if command.decision == ConfirmationDecisionWire::Cancel
        ));
        assert!(model.draft.is_empty());
        assert!(!model.idle_exit_armed);
    }

    #[test]
    fn confirmation_store_outage_preserves_exact_ids_for_accept_and_cancel_replay() {
        for (answer, decision) in [
            ("y", ConfirmationDecisionWire::Accept),
            ("n", ConfirmationDecisionWire::Cancel),
        ] {
            let mut model = AppModel {
                draft: answer.into(),
                cursor: 1,
                pending_confirmation: Some(PendingActionConfirmation {
                    confirmation_id: heyfood_core::GroceryConfirmationId::parse(
                        "00000000-0000-4000-8000-000000000001",
                    )
                    .unwrap(),
                    idempotency_key: heyfood_core::GroceryIdempotencyKey::parse(
                        "00000000-0000-4000-8000-000000000002",
                    )
                    .unwrap(),
                    editable_items: None,
                }),
                ..AppModel::default()
            };
            let first = dispatch(&mut model, Action::Submit);
            let first_command = match first.as_slice() {
                [Effect::ConfirmAction { command, .. }] => command.clone(),
                effects => panic!("expected confirmation effect, got {effects:?}"),
            };
            assert_eq!(first_command.decision, decision);

            let _ = dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::TurnEvent {
                    operation_id: 1,
                    event: AgentEvent::Error {
                        error: AgentFailure {
                            code: "temporarily_unavailable".into(),
                            message: "confirmation store unavailable".into(),
                            retryable: true,
                        },
                    },
                }),
            );
            let _ = dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::TurnFinished {
                    operation_id: 1,
                    outcome: RunTurnOutcome::Completed,
                }),
            );

            model.draft = answer.into();
            model.cursor = 1;
            let replay = dispatch(&mut model, Action::Submit);
            assert!(matches!(
                replay.as_slice(),
                [Effect::ConfirmAction { operation_id: 2, command }]
                    if command == &first_command
            ));
        }
    }

    #[test]
    fn edit_invalid_keeps_pending_confirmation_authority() {
        let pending = PendingActionConfirmation {
            confirmation_id: heyfood_core::GroceryConfirmationId::parse(
                "00000000-0000-4000-8000-000000000001",
            )
            .unwrap(),
            idempotency_key: heyfood_core::GroceryIdempotencyKey::parse(
                "00000000-0000-4000-8000-000000000002",
            )
            .unwrap(),
            editable_items: None,
        };
        let mut model = AppModel {
            operation: OperationState::Running(1),
            pending_confirmation: Some(pending.clone()),
            ..AppModel::default()
        };
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Error {
                    error: AgentFailure {
                        code: "edit_invalid".into(),
                        message: "invalid edit".into(),
                        retryable: false,
                    },
                },
            }),
        );
        assert_eq!(model.pending_confirmation, Some(pending));
    }

    #[test]
    fn partial_only_terminal_document_preserves_the_streamed_answer() {
        let mut model = AppModel {
            draft: "question".into(),
            cursor: 8,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Partial {
                    text: "complete streamed answer".into(),
                },
            }),
        );
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Result {
                    document: serde_json::json!({"conversation_id": "conversation-1"}),
                    conversation_id: Some("conversation-1".into()),
                },
            }),
        );
        let entry = model.scrollback.entries().back().unwrap();
        assert_eq!(entry.text, "complete streamed answer");
        assert!(!entry.streaming);
    }

    #[test]
    fn voice_capture_transcription_review_and_submit_share_the_typed_turn_path() {
        let mut model = AppModel::default();
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::VoiceAvailability(VoiceAvailability::Ready)),
        );
        assert!(resolve_slash_command("/voice").is_some());
        assert_eq!(
            dispatch(&mut model, Action::VoiceToggle),
            vec![Effect::StartVoice { operation_id: 1 }]
        );
        assert_eq!(model.operation, OperationState::Running(1));
        assert!(matches!(
            model.voice_phase,
            VoicePhase::Recording { operation_id: 1 }
        ));
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::VoiceRecordingElapsed {
                operation_id: 1,
                seconds: 3,
            }),
        );
        assert!(model.activity.as_deref().unwrap().contains("Recording 3s"));
        assert_eq!(
            dispatch(&mut model, Action::Submit),
            vec![Effect::StopVoice { operation_id: 1 }]
        );
        assert!(matches!(
            model.voice_phase,
            VoicePhase::Transcribing { operation_id: 1 }
        ));
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::VoiceTranscriptReady {
                operation_id: 1,
                transcript: "Log oatmeal and berries".into(),
            }),
        );
        assert_eq!(model.draft, "Log oatmeal and berries");
        assert_eq!(model.voice_phase, VoicePhase::Review);
        assert_eq!(model.operation, OperationState::Idle);

        model.draft.push_str(" for breakfast");
        model.cursor = model.draft.chars().count();
        assert_eq!(
            dispatch(&mut model, Action::Submit),
            vec![Effect::SubmitTurn {
                operation_id: 2,
                prompt: "Log oatmeal and berries for breakfast".into(),
            }]
        );
        assert_eq!(model.voice_phase, VoicePhase::Idle);
    }

    #[test]
    fn voice_scope_preflight_and_cancel_never_open_or_submit_audio() {
        let mut model = AppModel {
            draft: "typed draft".into(),
            cursor: 11,
            voice_availability: VoiceAvailability::AuthorizationRequired,
            ..AppModel::default()
        };
        assert_eq!(
            dispatch(&mut model, Action::VoiceToggle),
            vec![Effect::StartVoice { operation_id: 1 }]
        );
        assert!(model.draft.is_empty());
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::VoiceAvailability(
                VoiceAvailability::AuthorizationRequired,
            )),
        );
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::VoiceFailed {
                operation_id: 1,
                message: "Additional authorization is required. No microphone was opened.".into(),
            }),
        );
        assert_eq!(model.draft, "typed draft");
        assert_eq!(model.operation, OperationState::Idle);
        assert_eq!(model.voice_phase, VoicePhase::Idle);
        assert!(
            model
                .scrollback
                .entries()
                .back()
                .unwrap()
                .text
                .contains("No microphone was opened")
        );

        model.voice_availability = VoiceAvailability::Ready;
        assert_eq!(
            dispatch(&mut model, Action::VoiceToggle),
            vec![Effect::StartVoice { operation_id: 2 }]
        );
        assert!(model.draft.is_empty());
        assert_eq!(
            dispatch(&mut model, Action::CancelVoice),
            vec![Effect::CancelVoice { operation_id: 2 }]
        );
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::VoiceCancelled { operation_id: 2 }),
        );
        assert_eq!(model.draft, "typed draft");
        assert_eq!(model.operation, OperationState::Idle);
        assert_eq!(model.voice_phase, VoicePhase::Idle);
    }

    #[test]
    fn active_voice_rejects_composer_edits_and_preserves_drafts_deterministically() {
        fn recording_model() -> AppModel {
            let mut model = AppModel {
                draft: "original typed draft".into(),
                cursor: 20,
                voice_availability: VoiceAvailability::Ready,
                ..AppModel::default()
            };
            assert_eq!(
                dispatch(&mut model, Action::VoiceToggle),
                vec![Effect::StartVoice { operation_id: 1 }]
            );
            model
        }

        let mut success = recording_model();
        assert!(dispatch(&mut success, Action::InsertText("fallback".into())).is_empty());
        assert!(success.draft.is_empty());
        assert!(
            success
                .activity
                .as_deref()
                .unwrap()
                .contains("composer editing is paused")
        );
        assert_eq!(
            dispatch(&mut success, Action::Submit),
            vec![Effect::StopVoice { operation_id: 1 }]
        );
        assert!(dispatch(&mut success, Action::Insert('x')).is_empty());
        let _ = dispatch(
            &mut success,
            Action::Runtime(RuntimeEvent::VoiceTranscriptReady {
                operation_id: 1,
                transcript: "recognized transcript".into(),
            }),
        );
        assert_eq!(success.draft, "recognized transcript");
        assert_eq!(success.voice_phase, VoicePhase::Review);

        let mut failure = recording_model();
        let _ = dispatch(&mut failure, Action::InsertText("fallback".into()));
        let _ = dispatch(&mut failure, Action::Submit);
        let _ = dispatch(
            &mut failure,
            Action::Runtime(RuntimeEvent::VoiceFailed {
                operation_id: 1,
                message: "offline".into(),
            }),
        );
        assert_eq!(failure.draft, "original typed draft");
        assert_eq!(failure.voice_phase, VoicePhase::Idle);

        let mut cancelled = recording_model();
        let _ = dispatch(&mut cancelled, Action::InsertText("fallback".into()));
        assert_eq!(
            dispatch(&mut cancelled, Action::CancelVoice),
            vec![Effect::CancelVoice { operation_id: 1 }]
        );
        let _ = dispatch(
            &mut cancelled,
            Action::Runtime(RuntimeEvent::VoiceCancelled { operation_id: 1 }),
        );
        assert_eq!(cancelled.draft, "original typed draft");
        assert_eq!(cancelled.voice_phase, VoicePhase::Idle);
    }

    #[test]
    fn household_target_dispatches_and_reports_the_resolved_scope() {
        let mut model = AppModel {
            draft: "/for Sarah".into(),
            cursor: 10,
            ..AppModel::default()
        };
        assert_eq!(
            dispatch(&mut model, Action::Submit),
            vec![Effect::SelectHousehold {
                operation_id: 1,
                selector: "Sarah".into(),
            }]
        );
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::HouseholdScopeReady {
                operation_id: 1,
                label: "Sarah".into(),
            }),
        );
        assert_eq!(model.operation, OperationState::Idle);
        assert_eq!(
            model.scrollback.entries().back().unwrap().text,
            "Household target\n\nFuture turns will consider Sarah."
        );
    }

    #[test]
    fn runtime_notices_are_visible_and_terminal_safe() {
        let mut model = AppModel::default();
        assert!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::Notice {
                    message: "Account connected.\u{1b}[31m hidden".into(),
                }),
            )
            .is_empty()
        );
        let entry = model.scrollback.entries().back().unwrap();
        assert_eq!(entry.speaker, Speaker::Notice);
        assert_eq!(entry.text, "Account connected.[31m hidden");
    }

    #[test]
    fn available_panel_commands_dispatch_typed_effects() {
        for (command, panel) in [
            ("/status", PanelRequest::Status),
            ("/grocery", PanelRequest::Grocery),
            ("/watch", PanelRequest::Watch),
            ("/household", PanelRequest::Household),
            ("/location", PanelRequest::Location),
        ] {
            let mut model = AppModel {
                draft: command.into(),
                cursor: command.len(),
                ..AppModel::default()
            };
            assert_eq!(
                dispatch(&mut model, Action::Submit),
                vec![Effect::OpenPanel {
                    operation_id: 1,
                    panel,
                }]
            );
            assert_eq!(model.operation, OperationState::Running(1));
            let user_entry = model
                .scrollback
                .entries()
                .iter()
                .rev()
                .find(|entry| entry.speaker == Speaker::User)
                .unwrap();
            assert_eq!(user_entry.text, command);
            assert_eq!(
                model.scrollback.entries().back().unwrap().speaker,
                Speaker::Assistant
            );

            let _ = dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::PanelReady {
                    operation_id: 1,
                    panel,
                    body: "Live service result".into(),
                }),
            );
            let result = model.scrollback.entries().back().unwrap();
            assert!(result.text.starts_with(panel.title()));
            assert!(result.text.contains("Live service result"));
            assert!(!result.streaming);
            assert_eq!(model.operation, OperationState::Idle);
        }
    }

    #[test]
    fn deferred_health_panel_is_not_advertised_or_dispatched() {
        assert!(
            !SLASH_COMMAND_REGISTRY
                .iter()
                .any(|command| command.name == "/health")
        );
        let mut model = AppModel {
            draft: "/health".into(),
            cursor: "/health".len(),
            ..AppModel::default()
        };
        assert!(dispatch(&mut model, Action::Submit).is_empty());
        assert_eq!(model.operation, OperationState::Idle);
        assert_eq!(
            model.scrollback.entries().back().unwrap().text,
            "Health integrations are deferred from the supported heyfood v0.6.3 contract."
        );
    }

    #[test]
    fn profile_parser_accepts_only_the_closed_owner_grammar() {
        for (command, purpose) in [
            ("/profile", OwnerProfileActionLoadPurposeV1::View),
            (
                "/profile retry-sync",
                OwnerProfileActionLoadPurposeV1::ExplicitRetry,
            ),
        ] {
            let mut model = if matches!(purpose, OwnerProfileActionLoadPurposeV1::ExplicitRetry) {
                native_model()
            } else {
                AppModel::default()
            };
            assert_eq!(
                submit_text(&mut model, command),
                vec![Effect::LoadOwnerProfileActionsV1 {
                    operation_id: 1,
                    purpose,
                }]
            );
        }

        let mut consent = native_model();
        assert!(submit_text(&mut consent, "/profile consent").is_empty());
        assert_eq!(
            consent.profile_consent_review,
            Some(ProfileConsentReview::Reviewing)
        );

        for invalid in [
            "/prof",
            "/profiles",
            "/PROFILE",
            "/profile-consent",
            "/profile retry",
            "/profile retries-sync",
            "/profile consent now",
            "/profile retry-sync now",
            "/profile --for",
            "/profile --for member-1",
            "/profile member-1",
            "/profile consent member-1",
            "/profile retry-sync member-1",
        ] {
            let mut model = AppModel::default();
            assert!(
                submit_text(&mut model, invalid).is_empty(),
                "{invalid} must not dispatch"
            );
            assert_eq!(model.operation, OperationState::Idle, "{invalid}");
            assert!(model.profile_consent_review.is_none(), "{invalid}");
            assert!(
                model
                    .scrollback
                    .entries()
                    .iter()
                    .all(|entry| !entry.streaming),
                "{invalid}"
            );
        }
    }

    #[test]
    fn consent_review_confirm_is_at_most_once_and_success_never_retries() {
        let mut model = native_model();
        assert!(submit_text(&mut model, "/profile consent").is_empty());
        assert_eq!(
            model.scrollback.entries().back().unwrap().text,
            format!(
                "{}\n\n{}",
                crate::render::profile_copy(ProfileCopyStateV1::ConsentReview),
                crate::render::profile_copy(ProfileCopyStateV1::ConsentReviewPrompt)
            )
        );

        assert_eq!(
            submit_text(&mut model, "y"),
            vec![Effect::GrantOwnerProfileConsentV1 { operation_id: 1 }]
        );
        assert!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::ProfileConsentConfirmed)
            )
            .is_empty()
        );

        let finished = RuntimeEvent::ProfileConsentFinished {
            operation_id: 1,
            result: Ok(ProfileConsentFinishedV1 {
                consent_version: ConsentVersionV1::new(3).unwrap(),
                retry_offered: true,
            }),
        };
        assert!(dispatch(&mut model, Action::Runtime(finished.clone())).is_empty());
        let text = &model.scrollback.entries().back().unwrap().text;
        assert!(text.contains("version 3"));
        assert!(text.contains("Run /profile retry-sync to retry."));
        assert!(!text.contains("opaque-owner-intent"));
        let entry_count = model.scrollback.entries().len();
        assert!(dispatch(&mut model, Action::Runtime(finished)).is_empty());
        assert_eq!(model.scrollback.entries().len(), entry_count);
        assert!(model.pending_profile_action.is_none());
    }

    #[test]
    fn consent_and_retry_are_unavailable_outside_native_enabled_mode() {
        for mode in [
            ProfilePresentationModeV1::LegacyCompatibility,
            ProfilePresentationModeV1::NativeRollbackReadOnly,
        ] {
            let mut view = AppModel::default();
            let _ = dispatch(
                &mut view,
                Action::Runtime(RuntimeEvent::ProfilePresentationMode(mode)),
            );
            assert_eq!(
                submit_text(&mut view, "/profile"),
                vec![Effect::LoadOwnerProfileActionsV1 {
                    operation_id: 1,
                    purpose: OwnerProfileActionLoadPurposeV1::View,
                }]
            );

            let mut consent = AppModel::default();
            let _ = dispatch(
                &mut consent,
                Action::Runtime(RuntimeEvent::ProfilePresentationMode(mode)),
            );
            assert!(submit_text(&mut consent, "/profile consent").is_empty());
            assert!(consent.profile_consent_review.is_none());
            assert_eq!(consent.operation, OperationState::Idle);

            let mut retry = AppModel::default();
            let _ = dispatch(
                &mut retry,
                Action::Runtime(RuntimeEvent::ProfilePresentationMode(mode)),
            );
            assert!(submit_text(&mut retry, "/profile retry-sync").is_empty());
            assert!(retry.pending_profile_action.is_none());
            assert_eq!(retry.operation, OperationState::Idle);

            let mut direct = AppModel::default();
            let _ = dispatch(
                &mut direct,
                Action::Runtime(RuntimeEvent::ProfilePresentationMode(mode)),
            );
            assert!(
                dispatch(
                    &mut direct,
                    Action::Runtime(RuntimeEvent::ProfileConsentRequested)
                )
                .is_empty()
            );
            assert!(
                dispatch(
                    &mut direct,
                    Action::Runtime(RuntimeEvent::ProfileRetrySyncRequested)
                )
                .is_empty()
            );
            assert!(direct.profile_consent_review.is_none());
            assert!(direct.pending_profile_action.is_none());
        }
    }

    #[test]
    fn mode_change_neutralizes_native_profile_load_and_ignores_its_completion() {
        let mut model = native_model();
        let _ = submit_text(&mut model, "/profile");
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::ProfilePresentationMode(
                ProfilePresentationModeV1::NativeRollbackReadOnly,
            )),
        );
        let neutralized_text = model.scrollback.entries().back().unwrap().text.clone();
        assert_eq!(model.operation, OperationState::Idle);
        assert!(model.pending_profile_action.is_none());

        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::ProfilePresentationMode(
                ProfilePresentationModeV1::NativeEnabled,
            )),
        );
        assert!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::ProfileActionsLoaded {
                    operation_id: 1,
                    loaded: ProfileActionsLoadedV1::NativeActions(available_owner_actions(
                        OwnerProfileRetryEligibilityV1::ResumeReadyToDispatch,
                    )),
                }),
            )
            .is_empty()
        );
        assert_eq!(
            model.scrollback.entries().back().unwrap().text,
            neutralized_text
        );
        assert!(model.owner_profile_actions.is_none());
    }

    #[test]
    fn mode_change_neutralizes_native_consent_grant_and_ignores_its_completion() {
        let mut model = native_model();
        let _ = submit_text(&mut model, "/profile consent");
        assert_eq!(
            submit_text(&mut model, "y"),
            vec![Effect::GrantOwnerProfileConsentV1 { operation_id: 1 }]
        );
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::ProfilePresentationMode(
                ProfilePresentationModeV1::NativeRollbackReadOnly,
            )),
        );
        let neutralized_text = model.scrollback.entries().back().unwrap().text.clone();
        assert_eq!(model.operation, OperationState::Idle);
        assert!(model.profile_consent_review.is_none());

        assert!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::ProfileConsentFinished {
                    operation_id: 1,
                    result: Ok(ProfileConsentFinishedV1 {
                        consent_version: ConsentVersionV1::new(3).unwrap(),
                        retry_offered: true,
                    }),
                }),
            )
            .is_empty()
        );
        assert_eq!(
            model.scrollback.entries().back().unwrap().text,
            neutralized_text
        );
    }

    #[test]
    fn mode_change_neutralizes_native_retry_and_ignores_its_completion() {
        let mut model = native_model();
        let _ = submit_text(&mut model, "/profile retry-sync");
        let actions =
            available_owner_actions(OwnerProfileRetryEligibilityV1::ResumeReadyToDispatch);
        assert!(matches!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::ProfileActionsLoaded {
                    operation_id: 1,
                    loaded: ProfileActionsLoadedV1::NativeActions(actions),
                }),
            )
            .as_slice(),
            [Effect::RetryOwnerProfileSyncV1 {
                operation_id: 1,
                ..
            }]
        ));
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::ProfilePresentationMode(
                ProfilePresentationModeV1::NativeRollbackReadOnly,
            )),
        );
        let neutralized_text = model.scrollback.entries().back().unwrap().text.clone();
        assert_eq!(model.operation, OperationState::Idle);
        assert!(model.pending_profile_action.is_none());

        assert!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::ProfileRetrySyncFinished {
                    operation_id: 1,
                    outcome: ProfileRetrySyncFinishedV1::SyncPending,
                }),
            )
            .is_empty()
        );
        assert_eq!(
            model.scrollback.entries().back().unwrap().text,
            neutralized_text
        );
    }

    #[test]
    fn consent_review_cancel_escape_and_eof_emit_no_effect() {
        for action in [
            Action::Runtime(RuntimeEvent::ProfileConsentCancelled),
            Action::CancelVoice,
            Action::Exit,
            Action::CancelOrExit,
        ] {
            let mut model = native_model();
            assert!(submit_text(&mut model, "/profile consent").is_empty());
            assert!(dispatch(&mut model, action).is_empty());
            assert!(model.profile_consent_review.is_none());
            assert_eq!(model.operation, OperationState::Idle);
            assert_eq!(
                model.scrollback.entries().back().unwrap().text,
                crate::render::profile_copy(ProfileCopyStateV1::ConsentCancelled)
            );
        }
    }

    #[test]
    fn cancelling_explicit_retry_load_never_dispatches_the_retry() {
        let mut model = native_model();
        assert_eq!(
            submit_text(&mut model, "/profile retry-sync"),
            vec![Effect::LoadOwnerProfileActionsV1 {
                operation_id: 1,
                purpose: OwnerProfileActionLoadPurposeV1::ExplicitRetry,
            }]
        );
        assert_eq!(
            dispatch(&mut model, Action::CancelOrExit),
            vec![Effect::CancelTurn { operation_id: 1 }]
        );
        assert_eq!(model.operation, OperationState::Cancelling(1));
        assert!(matches!(
            model.pending_profile_action,
            Some(PendingProfileActionV1::Loading {
                operation_id: 1,
                ..
            })
        ));

        let effects = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::ProfileActionsLoaded {
                operation_id: 1,
                loaded: ProfileActionsLoadedV1::NativeActions(available_owner_actions(
                    OwnerProfileRetryEligibilityV1::ResumeReadyToDispatch,
                )),
            }),
        );
        assert!(effects.is_empty());
        assert_eq!(model.operation, OperationState::Idle);
        assert!(model.pending_profile_action.is_none());
        assert_eq!(
            model.scrollback.entries().back().unwrap().text,
            "Owner profile sync retry was cancelled before it started."
        );
    }

    #[test]
    fn cancelling_active_profile_retry_finishes_from_its_terminal_outcome() {
        let mut model = native_model();
        let _ = submit_text(&mut model, "/profile retry-sync");
        let actions =
            available_owner_actions(OwnerProfileRetryEligibilityV1::ResumeReadyToDispatch);
        assert!(matches!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::ProfileActionsLoaded {
                    operation_id: 1,
                    loaded: ProfileActionsLoadedV1::NativeActions(actions),
                }),
            )
            .as_slice(),
            [Effect::RetryOwnerProfileSyncV1 {
                operation_id: 1,
                ..
            }]
        ));
        assert_eq!(
            dispatch(&mut model, Action::CancelOrExit),
            vec![Effect::CancelTurn { operation_id: 1 }]
        );
        assert_eq!(model.operation, OperationState::Cancelling(1));
        assert!(matches!(
            model.pending_profile_action,
            Some(PendingProfileActionV1::Retrying {
                operation_id: 1,
                ..
            })
        ));

        assert!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::ProfileRetrySyncFinished {
                    operation_id: 1,
                    outcome: ProfileRetrySyncFinishedV1::Interrupted,
                }),
            )
            .is_empty()
        );
        assert_eq!(model.operation, OperationState::Idle);
        assert!(model.pending_profile_action.is_none());
        assert_eq!(
            model.scrollback.entries().back().unwrap().text,
            crate::render::profile_copy(ProfileCopyStateV1::InterruptedRetry)
        );
    }

    #[test]
    fn cancelling_consent_grant_finishes_from_its_terminal_result() {
        let mut cancelled = native_model();
        let _ = submit_text(&mut cancelled, "/profile consent");
        assert_eq!(
            submit_text(&mut cancelled, "y"),
            vec![Effect::GrantOwnerProfileConsentV1 { operation_id: 1 }]
        );
        assert_eq!(
            dispatch(&mut cancelled, Action::CancelOrExit),
            vec![Effect::CancelTurn { operation_id: 1 }]
        );
        assert!(
            dispatch(
                &mut cancelled,
                Action::Runtime(RuntimeEvent::ProfileConsentFinished {
                    operation_id: 1,
                    result: Err(ProfileConsentFailureV1::Cancelled),
                }),
            )
            .is_empty()
        );
        assert_eq!(cancelled.operation, OperationState::Idle);
        assert!(cancelled.profile_consent_review.is_none());
        assert_eq!(
            cancelled.scrollback.entries().back().unwrap().text,
            crate::render::profile_copy(ProfileCopyStateV1::ConsentCancelled)
        );

        let mut completed = native_model();
        let _ = submit_text(&mut completed, "/profile consent");
        let _ = submit_text(&mut completed, "y");
        let _ = dispatch(&mut completed, Action::CancelOrExit);
        assert!(
            dispatch(
                &mut completed,
                Action::Runtime(RuntimeEvent::ProfileConsentFinished {
                    operation_id: 1,
                    result: Ok(ProfileConsentFinishedV1 {
                        consent_version: ConsentVersionV1::new(3).unwrap(),
                        retry_offered: false,
                    }),
                }),
            )
            .is_empty()
        );
        assert_eq!(completed.operation, OperationState::Idle);
        assert!(completed.profile_consent_review.is_none());
        assert!(
            completed
                .scrollback
                .entries()
                .back()
                .unwrap()
                .text
                .contains("version 3")
        );
    }

    #[test]
    fn explicit_retry_requires_matching_loaded_eligibility_and_handle() {
        let available = [
            OwnerProfileRetryEligibilityV1::StartLocalOnlyAfterConsent,
            OwnerProfileRetryEligibilityV1::ResumeNeedsConsentCheck,
            OwnerProfileRetryEligibilityV1::ResumeNeedsRemoteBase,
            OwnerProfileRetryEligibilityV1::ResumeReadyToDispatch,
            OwnerProfileRetryEligibilityV1::ReconcileDispatchingOutcomeUnknown,
            OwnerProfileRetryEligibilityV1::ReconcileOutcomeUncertain,
        ];
        for retry in available {
            let mut model = native_model();
            assert_eq!(
                submit_text(&mut model, "/profile retry-sync"),
                vec![Effect::LoadOwnerProfileActionsV1 {
                    operation_id: 1,
                    purpose: OwnerProfileActionLoadPurposeV1::ExplicitRetry,
                }]
            );
            let actions = available_owner_actions(retry);
            let expected_intent = actions.intent.clone().unwrap();
            assert_eq!(
                dispatch(
                    &mut model,
                    Action::Runtime(RuntimeEvent::ProfileActionsLoaded {
                        operation_id: 1,
                        loaded: ProfileActionsLoadedV1::NativeActions(actions),
                    })
                ),
                vec![Effect::RetryOwnerProfileSyncV1 {
                    operation_id: 1,
                    action: retry.available_action().unwrap(),
                    intent: expected_intent,
                }]
            );
            assert!(matches!(
                model.pending_profile_action,
                Some(PendingProfileActionV1::Retrying {
                    operation_id: 1,
                    ..
                })
            ));
        }
    }

    #[test]
    fn unavailable_missing_and_stale_retry_loads_emit_no_retry() {
        let unavailable = OwnerProfileActionEligibilityV1 {
            active_consent_version: None,
            retry: OwnerProfileRetryEligibilityV1::Unavailable {
                reason: OwnerProfileRetryUnavailableReasonV1::ConsentRequired,
            },
            intent: None,
        };
        let mut model = native_model();
        let _ = submit_text(&mut model, "/profile retry-sync");
        assert!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::ProfileActionsLoaded {
                    operation_id: 99,
                    loaded: ProfileActionsLoadedV1::NativeActions(unavailable.clone()),
                })
            )
            .is_empty()
        );
        assert!(matches!(
            model.pending_profile_action,
            Some(PendingProfileActionV1::Loading {
                operation_id: 1,
                ..
            })
        ));
        assert!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::ProfileActionsLoaded {
                    operation_id: 1,
                    loaded: ProfileActionsLoadedV1::NativeActions(unavailable),
                })
            )
            .is_empty()
        );
        assert_eq!(model.operation, OperationState::Idle);
        assert!(model.pending_profile_action.is_none());

        let mut missing_handle = native_model();
        let _ = submit_text(&mut missing_handle, "/profile retry-sync");
        let actions = OwnerProfileActionEligibilityV1 {
            active_consent_version: Some(ConsentVersionV1::new(2).unwrap()),
            retry: OwnerProfileRetryEligibilityV1::ResumeReadyToDispatch,
            intent: None,
        };
        assert!(
            dispatch(
                &mut missing_handle,
                Action::Runtime(RuntimeEvent::ProfileActionsLoaded {
                    operation_id: 1,
                    loaded: ProfileActionsLoadedV1::NativeActions(actions),
                })
            )
            .is_empty()
        );
        assert!(missing_handle.pending_profile_action.is_none());
    }

    #[test]
    fn a_view_load_never_emits_retry_and_legacy_panel_copy_is_preserved() {
        let mut native = native_model();
        let _ = submit_text(&mut native, "/profile");
        assert!(
            dispatch(
                &mut native,
                Action::Runtime(RuntimeEvent::ProfileActionsLoaded {
                    operation_id: 1,
                    loaded: ProfileActionsLoadedV1::NativeActions(available_owner_actions(
                        OwnerProfileRetryEligibilityV1::ResumeReadyToDispatch,
                    )),
                })
            )
            .is_empty()
        );
        assert_eq!(native.operation, OperationState::Idle);
        assert!(
            native
                .scrollback
                .entries()
                .back()
                .unwrap()
                .text
                .contains("Run /profile retry-sync to resume the exact saved owner profile.")
        );

        let mut legacy = AppModel::default();
        let _ = submit_text(&mut legacy, "/profile");
        let body = "Released profile panel body";
        assert!(
            dispatch(
                &mut legacy,
                Action::Runtime(RuntimeEvent::ProfileActionsLoaded {
                    operation_id: 1,
                    loaded: ProfileActionsLoadedV1::LegacyPanel { body: body.into() },
                })
            )
            .is_empty()
        );
        assert_eq!(
            legacy.scrollback.entries().back().unwrap().text,
            format!("Dietary profile\n\n{body}")
        );
    }

    #[test]
    fn stale_retry_completion_is_ignored_and_output_is_content_free() {
        let mut model = native_model();
        let _ = submit_text(&mut model, "/profile retry-sync");
        let actions =
            available_owner_actions(OwnerProfileRetryEligibilityV1::ReconcileOutcomeUncertain);
        let debug = format!("{actions:?}");
        assert!(!debug.contains("opaque-owner-intent"));
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::ProfileActionsLoaded {
                operation_id: 1,
                loaded: ProfileActionsLoadedV1::NativeActions(actions),
            }),
        );
        assert!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::ProfileRetrySyncFinished {
                    operation_id: 99,
                    outcome: ProfileRetrySyncFinishedV1::SyncPending,
                })
            )
            .is_empty()
        );
        assert_eq!(model.operation, OperationState::Running(1));
        assert!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::ProfileRetrySyncFinished {
                    operation_id: 1,
                    outcome: ProfileRetrySyncFinishedV1::Interrupted,
                })
            )
            .is_empty()
        );
        assert_eq!(
            model.scrollback.entries().back().unwrap().text,
            crate::render::profile_copy(ProfileCopyStateV1::InterruptedRetry)
        );
        assert!(
            model
                .scrollback
                .entries()
                .iter()
                .all(|entry| !entry.text.contains("opaque-owner-intent"))
        );
    }

    #[test]
    fn terminal_event_keeps_single_flight_closed_until_turn_finished() {
        let mut model = AppModel {
            draft: "first".into(),
            cursor: 5,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Result {
                    document: Default::default(),
                    conversation_id: Some("conversation-1".into()),
                },
            }),
        );
        assert_eq!(model.operation, OperationState::Finishing(1));

        let _ = dispatch(&mut model, Action::InsertText("second".into()));
        assert!(dispatch(&mut model, Action::Submit).is_empty());

        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnFinished {
                operation_id: 1,
                outcome: RunTurnOutcome::Completed,
            }),
        );
        assert_eq!(model.operation, OperationState::Idle);
        assert_eq!(dispatch(&mut model, Action::Submit).len(), 1);
    }

    #[test]
    fn household_counters_use_the_last_value_once_then_fail_closed() {
        let mut model = AppModel {
            next_household_operation_id: HouseholdOperationIdV1::new(u64::MAX).ok(),
            next_household_correlation: HouseholdReducerCorrelationV1::new(u64::MAX).ok(),
            ..AppModel::default()
        };
        let digest = HouseholdAccountBindingDigestV1::from_bytes([4; 32]);
        let generation = HouseholdModeGenerationV1::new(1).unwrap();
        let bootstrap = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::HouseholdGenerationReadyV1 {
                session_mode_generation: generation,
                mode: HouseholdPresentationModeV1::NativeEnabled,
                account_binding_digest: digest,
            }),
        );
        let [
            Effect::LoadHouseholdManagementV1 {
                operation_id,
                reducer_correlation,
                ..
            },
        ] = bootstrap.as_slice()
        else {
            panic!("expected one bootstrap load");
        };
        assert_eq!(operation_id.get(), u64::MAX);
        assert_eq!(reducer_correlation.get(), u64::MAX);
        assert!(model.next_household_operation_id.is_none());
        assert!(model.next_household_correlation.is_none());

        // A generic event with the same numeric ID cannot complete household work.
        assert!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::PanelReady {
                    operation_id: u64::MAX,
                    panel: PanelRequest::Household,
                    body: "forged".into(),
                }),
            )
            .is_empty()
        );
        assert!(model.pending_household_load.is_some());

        let owner = HouseholdMemberPresentationV1::new(
            HouseholdSubjectId::self_(),
            "Owner",
            RelationshipV1::Self_,
            HouseholdLifecycleV1::Active,
            HouseholdProfileStateV1::LocalOnly,
            Some(ProfileRevision::new(1).unwrap()),
        )
        .unwrap();
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::HouseholdManagementLoadedV1 {
                operation_id: *operation_id,
                session_mode_generation: generation,
                reducer_correlation: *reducer_correlation,
                purpose: HouseholdManagementLoadPurposeV1::Bootstrap,
                account_binding_digest: digest,
                household_revision: HouseholdRevision::new(1).unwrap(),
                active_scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
                members: vec![owner],
            }),
        );
        assert!(submit_text(&mut model, "/household").is_empty());
        assert!(matches!(
            model.household_turn_gate,
            HouseholdTurnGateV1::CounterExhausted
        ));
    }

    #[test]
    fn cancelled_household_load_remains_owned_until_its_terminal_event() {
        let mut model = AppModel::default();
        let generation = HouseholdModeGenerationV1::new(1).unwrap();
        let digest = HouseholdAccountBindingDigestV1::from_bytes([8; 32]);
        let bootstrap = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::HouseholdGenerationReadyV1 {
                session_mode_generation: generation,
                mode: HouseholdPresentationModeV1::NativeEnabled,
                account_binding_digest: digest,
            }),
        );
        let [
            Effect::LoadHouseholdManagementV1 {
                operation_id: bootstrap_operation,
                reducer_correlation: bootstrap_correlation,
                ..
            },
        ] = bootstrap.as_slice()
        else {
            panic!("expected bootstrap load");
        };
        let owner = HouseholdMemberPresentationV1::new(
            HouseholdSubjectId::self_(),
            "Me",
            RelationshipV1::Self_,
            HouseholdLifecycleV1::Active,
            HouseholdProfileStateV1::LocalOnly,
            Some(ProfileRevision::new(1).unwrap()),
        )
        .unwrap();
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::HouseholdManagementLoadedV1 {
                operation_id: *bootstrap_operation,
                session_mode_generation: generation,
                reducer_correlation: *bootstrap_correlation,
                purpose: HouseholdManagementLoadPurposeV1::Bootstrap,
                account_binding_digest: digest,
                household_revision: HouseholdRevision::new(1).unwrap(),
                active_scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
                members: vec![owner.clone()],
            }),
        );

        let add_member = submit_text(&mut model, "/household add");
        let [
            Effect::LoadHouseholdManagementV1 {
                operation_id,
                reducer_correlation,
                purpose: HouseholdManagementLoadPurposeV1::AddMember,
                ..
            },
        ] = add_member.as_slice()
        else {
            panic!("expected add-member load");
        };
        assert!(dispatch(&mut model, Action::CancelOrExit).is_empty());
        assert!(
            model
                .pending_household_load
                .as_ref()
                .is_some_and(|pending| pending.cancel_requested)
        );
        assert_eq!(
            model.operation,
            OperationState::Cancelling(operation_id.get())
        );
        assert!(submit_text(&mut model, "/household").is_empty());
        assert!(model.pending_household_load.is_some());

        assert!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::HouseholdManagementLoadedV1 {
                    operation_id: *operation_id,
                    session_mode_generation: generation,
                    reducer_correlation: *reducer_correlation,
                    purpose: HouseholdManagementLoadPurposeV1::AddMember,
                    account_binding_digest: digest,
                    household_revision: HouseholdRevision::new(1).unwrap(),
                    active_scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
                    members: vec![owner],
                }),
            )
            .is_empty()
        );
        assert!(model.pending_household_load.is_none());
        assert!(model.onboarding.is_none());
        assert_eq!(model.operation, OperationState::Idle);
        assert!(
            model
                .scrollback
                .entries()
                .iter()
                .any(|entry| entry.text.contains("No household mutation was dispatched"))
        );
    }

    #[test]
    fn invalidated_household_generation_is_never_reused() {
        let mut model = AppModel::default();
        let generation = HouseholdModeGenerationV1::new(u64::MAX).unwrap();
        let digest = HouseholdAccountBindingDigestV1::from_bytes([5; 32]);
        assert_eq!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::HouseholdGenerationReadyV1 {
                    session_mode_generation: generation,
                    mode: HouseholdPresentationModeV1::NativeEnabled,
                    account_binding_digest: digest,
                }),
            )
            .len(),
            1
        );
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::HouseholdGenerationInvalidatedV1 {
                session_mode_generation: generation,
            }),
        );
        assert!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::HouseholdGenerationReadyV1 {
                    session_mode_generation: generation,
                    mode: HouseholdPresentationModeV1::NativeEnabled,
                    account_binding_digest: digest,
                }),
            )
            .is_empty()
        );
        assert!(model.household_generation.is_none());
        assert_eq!(generation.checked_next(), Err(HouseholdCounterExhaustedV1));
    }

    #[test]
    fn household_debug_carriers_never_expose_labels_profiles_or_stable_ids() {
        let member_label = "member-label-debug-canary";
        let member_id = MemberId::parse_preserved("member-id-debug-canary").unwrap();
        let subject = HouseholdSubjectId::member(member_id);
        let binding = HouseholdOperationBindingV1::new(
            HouseholdOperationIdV1::new(7).unwrap(),
            HouseholdModeGenerationV1::new(3).unwrap(),
            HouseholdAccountBindingDigestV1::from_bytes([0x6b; 32]),
            HouseholdRevision::new(11).unwrap(),
            HouseholdReducerCorrelationV1::new(13).unwrap(),
        );
        let profile = OnboardingProfileInput {
            avoid_ingredients: vec!["profile-content-debug-canary".into()],
            notes: Some("profile-notes-debug-canary".into()),
            ..OnboardingProfileInput::default()
        };
        let effect = Effect::CreateMemberWithDeclaredProfileV1 {
            binding: binding.clone(),
            bounded_member_draft: BoundedHouseholdMemberDraftV1::new(
                member_label,
                RelationshipV1::Child,
                HouseholdAgeEvidenceInputV1::Age13To17,
            )
            .unwrap(),
            onboarding_profile_input: Box::new(profile),
        };
        let event = RuntimeEvent::HouseholdMutationCommittedV1 {
            binding,
            kind: HouseholdMutationKindV1::CreateMember,
            resulting_household_revision: HouseholdRevision::new(12).unwrap(),
            affected_subject: Some(subject),
            active_scope: HouseholdScope::Everyone,
            bounded_active_label: member_label.into(),
        };
        let mut model = AppModel {
            draft: "draft-content-debug-canary".into(),
            cursor: "draft-content-debug-canary".chars().count(),
            prompt_history: VecDeque::from(["prompt-history-debug-canary".into()]),
            household_chrome_label: Some(member_label.into()),
            ..AppModel::default()
        };
        model.scrollback.push(SemanticEntry {
            speaker: Speaker::Assistant,
            text: "scrollback-content-debug-canary".into(),
            streaming: false,
        });

        let combined = format!("{effect:?}\n{event:?}\n{model:?}");
        for canary in [
            member_label,
            "member-id-debug-canary",
            "profile-content-debug-canary",
            "profile-notes-debug-canary",
            "draft-content-debug-canary",
            "prompt-history-debug-canary",
            "scrollback-content-debug-canary",
        ] {
            assert!(!combined.contains(canary), "{canary} leaked through Debug");
        }
    }
}
