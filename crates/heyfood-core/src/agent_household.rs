//! Phase-0-only household agent contracts.
//!
//! These types freeze the local disclosure, proposal, and review state model.
//! They are deliberately not wired to CLI or MCP routes in Phase 0.

use std::fmt;

use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthChar;
use uuid::Uuid;

use crate::{
    AccountId, CanonicalDigestV1, CanonicalTimestampV1, CommitId, DisplayName, GenerationId,
    HouseholdEffectFingerprintV1, HouseholdRevision, HouseholdScope, MemberId, MinorStatusV1,
    ProfileRevision,
};

pub const AGENT_HOUSEHOLD_CONTRACT_VERSION: u16 = 1;
pub const AGENT_HOUSEHOLD_MAX_MEMBERS_PER_PAGE: u16 = 100;
pub const AGENT_HOUSEHOLD_REVIEW_MINIMUM_WIDTH: usize = 20;
pub const AGENT_HOUSEHOLD_REVIEW_MAXIMUM_WIDTH: usize = 240;

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentHouseholdProposalIdV1(Uuid);

impl AgentHouseholdProposalIdV1 {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for AgentHouseholdProposalIdV1 {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for AgentHouseholdProposalIdV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AgentHouseholdProposalIdV1([REDACTED])")
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "member_ref",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AgentHouseholdSubjectV1 {
    #[serde(rename = "self")]
    Self_,
    Member(MemberId),
    Everyone,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentDisclosureDataClassV1 {
    Roster,
    MinimizedDeclaredProfile,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentDisclosureGrantStateV1 {
    Active,
    Expired,
    Revoked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentDisclosureGrantingAuthorityV1 {
    AccountOwnerAdultAuthorization,
    AuthorizedGuardianRosterOnly,
}

/// Encrypted local authority record. It is intentionally not serializable:
/// account binding and grant authority never become an agent result.
#[derive(Clone, Eq, PartialEq)]
pub struct AgentDisclosureGrantV1 {
    pub account: AccountId,
    pub subject: AgentHouseholdSubjectV1,
    pub subject_minor_status: MinorStatusV1,
    pub data_classes: Vec<AgentDisclosureDataClassV1>,
    pub granting_authority: AgentDisclosureGrantingAuthorityV1,
    pub revision: u64,
    pub generation: GenerationId,
    pub state: AgentDisclosureGrantStateV1,
    pub expires_at: Option<CanonicalTimestampV1>,
}

impl fmt::Debug for AgentDisclosureGrantV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentDisclosureGrantV1")
            .field("subject_minor_status", &self.subject_minor_status)
            .field("data_classes", &self.data_classes)
            .field("granting_authority", &self.granting_authority)
            .field("revision", &self.revision)
            .field("generation", &self.generation)
            .field("state", &self.state)
            .field("has_expiry", &self.expires_at.is_some())
            .finish_non_exhaustive()
    }
}

impl AgentDisclosureGrantV1 {
    #[must_use]
    pub fn permits_for(
        &self,
        account: &AccountId,
        subject: &AgentHouseholdSubjectV1,
        generation: GenerationId,
        data_class: AgentDisclosureDataClassV1,
        observed_at: &CanonicalTimestampV1,
    ) -> bool {
        if &self.account != account
            || &self.subject != subject
            || self.generation != generation
            || self.state != AgentDisclosureGrantStateV1::Active
            || self.revision == 0
            || self
                .expires_at
                .as_ref()
                .is_some_and(|expires_at| observed_at >= expires_at)
        {
            return false;
        }
        if data_class == AgentDisclosureDataClassV1::MinimizedDeclaredProfile
            && (self.subject_minor_status != MinorStatusV1::Adult
                || self.granting_authority
                    != AgentDisclosureGrantingAuthorityV1::AccountOwnerAdultAuthorization)
        {
            return false;
        }
        self.data_classes.contains(&data_class)
            && (data_class != AgentDisclosureDataClassV1::MinimizedDeclaredProfile
                || self
                    .data_classes
                    .contains(&AgentDisclosureDataClassV1::Roster))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentHouseholdProjectionV1 {
    ContentFree,
    Roster,
    Profile,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentHouseholdOperationV1 {
    Add,
    Edit,
    Archive,
    Restore,
    Scope,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentHouseholdProposalStateV1 {
    Prepared,
    AwaitingLocalInput,
    AwaitingLocalReview,
    Committing,
    Committed,
    Cancelled,
    Expired,
    Stale,
    Rejected,
    ProvenUncommitted,
    ReconciliationRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentHouseholdNextActionV1 {
    None,
    OpenAttachedTui,
    ObserveStatus,
    Reconcile,
    PrepareFresh,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentHouseholdRetryClassV1 {
    NotApplicable,
    SafeRead,
    NoBlindRetry,
    ReconcileBeforeRetry,
}

impl AgentHouseholdProposalStateV1 {
    #[must_use]
    pub const fn human_status(self) -> &'static str {
        match self {
            Self::Prepared => "Getting this change ready…",
            Self::AwaitingLocalInput => "More information needed",
            Self::AwaitingLocalReview => "Ready for your review",
            Self::Committing => "Saving securely…",
            Self::Committed => "Saved",
            Self::Cancelled => "Cancelled — nothing was saved",
            Self::Expired => "Expired — start a new change",
            Self::Stale => "Household changed — review a fresh proposal",
            Self::Rejected => "Can't use this change",
            Self::ProvenUncommitted => "Not saved — heyfood verified no household change was made",
            Self::ReconciliationRequired => "Checking whether this was saved…",
        }
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Committed
                | Self::Cancelled
                | Self::Expired
                | Self::Stale
                | Self::Rejected
                | Self::ProvenUncommitted
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentHouseholdMemberProjectionV1 {
    pub member_ref: MemberId,
    pub display_label: DisplayName,
    pub profile_revision: Option<ProfileRevision>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentHouseholdReadRequestV1 {
    pub subject: Option<AgentHouseholdSubjectV1>,
    pub requested_projection: AgentHouseholdProjectionV1,
    pub expected_disclosure_generation: GenerationId,
    pub cursor: Option<String>,
    pub limit: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentHouseholdReadSnapshotV1 {
    pub schema_version: u16,
    pub projection: AgentHouseholdProjectionV1,
    pub resolved_subject: AgentHouseholdSubjectV1,
    pub resolved_from_active_scope: bool,
    pub active_scope: HouseholdScope,
    pub household_revision: HouseholdRevision,
    pub disclosure_generation: GenerationId,
    pub eligible_member_count: u16,
    pub restricted_member_count: u16,
    pub members: Vec<AgentHouseholdMemberProjectionV1>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentHouseholdPrepareRequestV1 {
    pub operation: AgentHouseholdOperationV1,
    pub requested_projection: AgentHouseholdProjectionV1,
    pub expected_disclosure_generation: GenerationId,
    pub expected_household_revision: HouseholdRevision,
    pub affected_member_ref: Option<MemberId>,
    pub bundled_scope: Option<HouseholdScope>,
}

impl AgentHouseholdPrepareRequestV1 {
    pub fn validate_shape(&self) -> Result<(), AgentHouseholdContractErrorV1> {
        let valid = match self.operation {
            AgentHouseholdOperationV1::Add => self.affected_member_ref.is_none(),
            AgentHouseholdOperationV1::Edit | AgentHouseholdOperationV1::Restore => {
                self.affected_member_ref.is_some() && self.bundled_scope.is_none()
            }
            AgentHouseholdOperationV1::Archive => self.affected_member_ref.is_some(),
            AgentHouseholdOperationV1::Scope => {
                self.affected_member_ref.is_none() && self.bundled_scope.is_some()
            }
        };
        if valid {
            Ok(())
        } else {
            Err(AgentHouseholdContractErrorV1::InvalidOperationShape)
        }
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentHouseholdProposalPresentationV1 {
    pub schema_version: u16,
    pub proposal_ref: AgentHouseholdProposalIdV1,
    pub operation: AgentHouseholdOperationV1,
    pub state: AgentHouseholdProposalStateV1,
    pub projection: AgentHouseholdProjectionV1,
    pub disclosure_generation: GenerationId,
    pub affected_member_ref: Option<MemberId>,
    pub affected_member_label: Option<DisplayName>,
    pub profile_change_count: Option<u16>,
    pub created_at: CanonicalTimestampV1,
    pub expires_at: CanonicalTimestampV1,
    pub human_status: String,
    pub handoff_command: String,
    pub handoff_instruction: String,
}

impl fmt::Debug for AgentHouseholdProposalPresentationV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentHouseholdProposalPresentationV1")
            .field("operation", &self.operation)
            .field("state", &self.state)
            .field("projection", &self.projection)
            .field("disclosure_generation", &self.disclosure_generation)
            .field("has_member_identity", &self.affected_member_ref.is_some())
            .field("has_profile_changes", &self.profile_change_count.is_some())
            .finish_non_exhaustive()
    }
}

impl AgentHouseholdProposalPresentationV1 {
    #[must_use]
    pub fn content_free(
        proposal_ref: AgentHouseholdProposalIdV1,
        operation: AgentHouseholdOperationV1,
        state: AgentHouseholdProposalStateV1,
        disclosure_generation: GenerationId,
        created_at: CanonicalTimestampV1,
        expires_at: CanonicalTimestampV1,
    ) -> Self {
        Self {
            schema_version: AGENT_HOUSEHOLD_CONTRACT_VERSION,
            proposal_ref,
            operation,
            state,
            projection: AgentHouseholdProjectionV1::ContentFree,
            disclosure_generation,
            affected_member_ref: None,
            affected_member_label: None,
            profile_change_count: None,
            created_at,
            expires_at,
            human_status: state.human_status().to_owned(),
            handoff_command: "heyfood".to_owned(),
            handoff_instruction: "Open `/household changes` to review this change locally."
                .to_owned(),
        }
    }

    #[must_use]
    pub fn filtered_to(self, projection: AgentHouseholdProjectionV1) -> Self {
        match projection {
            AgentHouseholdProjectionV1::ContentFree => Self::content_free(
                self.proposal_ref,
                self.operation,
                self.state,
                self.disclosure_generation,
                self.created_at,
                self.expires_at,
            ),
            AgentHouseholdProjectionV1::Roster => Self {
                projection,
                profile_change_count: None,
                ..self
            },
            AgentHouseholdProjectionV1::Profile => Self { projection, ..self },
        }
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentHouseholdOutcomeReceiptV1 {
    pub schema_version: u16,
    pub proposal_ref: AgentHouseholdProposalIdV1,
    pub state: AgentHouseholdProposalStateV1,
    pub household_revision_before: HouseholdRevision,
    pub household_revision_after: Option<HouseholdRevision>,
    pub known_no_household_mutation: bool,
    pub retry_class: AgentHouseholdRetryClassV1,
    pub next_action: AgentHouseholdNextActionV1,
}

impl fmt::Debug for AgentHouseholdOutcomeReceiptV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentHouseholdOutcomeReceiptV1")
            .field("state", &self.state)
            .field(
                "known_no_household_mutation",
                &self.known_no_household_mutation,
            )
            .field("retry_class", &self.retry_class)
            .field("next_action", &self.next_action)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct LocalHouseholdProposalBindingV1 {
    pub account: AccountId,
    pub proposal_ref: AgentHouseholdProposalIdV1,
    pub operation: AgentHouseholdOperationV1,
    pub disclosure_generation: GenerationId,
    pub lifecycle_generation: GenerationId,
    pub projection: AgentHouseholdProjectionV1,
    pub expected_household_revision: HouseholdRevision,
    pub expected_profile_revision: Option<ProfileRevision>,
    pub commit_id: CommitId,
    pub member_id: Option<MemberId>,
    pub previous_scope: HouseholdScope,
    pub originating_session_digest: CanonicalDigestV1,
    pub eligible_host_policy_digest: CanonicalDigestV1,
    pub created_at: CanonicalTimestampV1,
    pub expires_at: CanonicalTimestampV1,
}

impl fmt::Debug for LocalHouseholdProposalBindingV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalHouseholdProposalBindingV1")
            .field("operation", &self.operation)
            .field("disclosure_generation", &self.disclosure_generation)
            .field("lifecycle_generation", &self.lifecycle_generation)
            .field("projection", &self.projection)
            .field(
                "expected_household_revision",
                &self.expected_household_revision,
            )
            .field(
                "has_profile_revision",
                &self.expected_profile_revision.is_some(),
            )
            .field("has_member_id", &self.member_id.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct LocalHouseholdFrozenCandidateV1 {
    pub proposal_digest: CanonicalDigestV1,
    pub effect_fingerprint: HouseholdEffectFingerprintV1,
    pub before_document_digest: CanonicalDigestV1,
    pub after_document_digest: CanonicalDigestV1,
    pub resulting_scope: HouseholdScope,
    pub conversation_continuity_reset: bool,
    pub frozen_semantic_timestamp: CanonicalTimestampV1,
}

impl fmt::Debug for LocalHouseholdFrozenCandidateV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalHouseholdFrozenCandidateV1")
            .field(
                "conversation_continuity_reset",
                &self.conversation_continuity_reset,
            )
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct LocalHouseholdAuthoritySnapshotV1 {
    pub account: AccountId,
    pub disclosure_generation: GenerationId,
    pub lifecycle_generation: GenerationId,
    pub household_revision: HouseholdRevision,
    pub profile_revision: Option<ProfileRevision>,
    pub observed_at: CanonicalTimestampV1,
}

impl fmt::Debug for LocalHouseholdAuthoritySnapshotV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalHouseholdAuthoritySnapshotV1")
            .field("disclosure_generation", &self.disclosure_generation)
            .field("lifecycle_generation", &self.lifecycle_generation)
            .field("household_revision", &self.household_revision)
            .field("has_profile_revision", &self.profile_revision.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct LocalHouseholdProposalAuthorityV1 {
    pub binding: LocalHouseholdProposalBindingV1,
    pub state: AgentHouseholdProposalStateV1,
    pub proposal_generation: GenerationId,
    pub frozen: Option<LocalHouseholdFrozenCandidateV1>,
}

impl fmt::Debug for LocalHouseholdProposalAuthorityV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalHouseholdProposalAuthorityV1")
            .field("binding", &self.binding)
            .field("state", &self.state)
            .field("proposal_generation", &self.proposal_generation)
            .field("is_frozen", &self.frozen.is_some())
            .finish_non_exhaustive()
    }
}

impl LocalHouseholdProposalAuthorityV1 {
    #[must_use]
    pub fn prepared(binding: LocalHouseholdProposalBindingV1) -> Self {
        Self {
            binding,
            state: AgentHouseholdProposalStateV1::Prepared,
            proposal_generation: GenerationId::INITIAL,
            frozen: None,
        }
    }

    #[must_use]
    pub fn awaiting_local_input(binding: LocalHouseholdProposalBindingV1) -> Self {
        let mut authority = Self::prepared(binding);
        authority.state = AgentHouseholdProposalStateV1::AwaitingLocalInput;
        authority
    }

    pub fn begin_local_input(&mut self) -> Result<(), AgentHouseholdContractErrorV1> {
        if self.state != AgentHouseholdProposalStateV1::Prepared {
            return Err(AgentHouseholdContractErrorV1::InvalidTransition);
        }
        self.state = AgentHouseholdProposalStateV1::AwaitingLocalInput;
        Ok(())
    }

    pub fn freeze_for_review(
        &mut self,
        current: &LocalHouseholdAuthoritySnapshotV1,
        frozen: LocalHouseholdFrozenCandidateV1,
    ) -> Result<(), AgentHouseholdContractErrorV1> {
        if !matches!(
            self.state,
            AgentHouseholdProposalStateV1::Prepared
                | AgentHouseholdProposalStateV1::AwaitingLocalInput
        ) {
            return Err(AgentHouseholdContractErrorV1::InvalidTransition);
        }
        self.validate_current(current)?;
        self.frozen = Some(frozen);
        self.proposal_generation = self.proposal_generation.next();
        self.state = AgentHouseholdProposalStateV1::AwaitingLocalReview;
        Ok(())
    }

    pub fn begin_commit(
        &mut self,
        current: &LocalHouseholdAuthoritySnapshotV1,
        expected_proposal_generation: GenerationId,
        expected_proposal_digest: CanonicalDigestV1,
    ) -> Result<(), AgentHouseholdContractErrorV1> {
        if self.state != AgentHouseholdProposalStateV1::AwaitingLocalReview {
            return Err(AgentHouseholdContractErrorV1::InvalidTransition);
        }
        self.validate_current(current)?;
        if self.proposal_generation != expected_proposal_generation {
            return Err(AgentHouseholdContractErrorV1::ProposalGenerationChanged);
        }
        let frozen = self
            .frozen
            .as_ref()
            .ok_or(AgentHouseholdContractErrorV1::MissingFrozenAuthority)?;
        if frozen.proposal_digest != expected_proposal_digest {
            return Err(AgentHouseholdContractErrorV1::ProposalDigestChanged);
        }
        self.state = AgentHouseholdProposalStateV1::Committing;
        Ok(())
    }

    pub fn cancel_before_commit(&mut self) -> Result<(), AgentHouseholdContractErrorV1> {
        self.finish_before_commit(AgentHouseholdProposalStateV1::Cancelled)
    }

    pub fn finish_before_commit(
        &mut self,
        terminal_state: AgentHouseholdProposalStateV1,
    ) -> Result<(), AgentHouseholdContractErrorV1> {
        if !matches!(
            terminal_state,
            AgentHouseholdProposalStateV1::Cancelled
                | AgentHouseholdProposalStateV1::Expired
                | AgentHouseholdProposalStateV1::Stale
                | AgentHouseholdProposalStateV1::Rejected
        ) {
            return Err(AgentHouseholdContractErrorV1::InvalidTransition);
        }
        match self.state {
            AgentHouseholdProposalStateV1::Prepared
            | AgentHouseholdProposalStateV1::AwaitingLocalInput
            | AgentHouseholdProposalStateV1::AwaitingLocalReview => {
                self.state = terminal_state;
                Ok(())
            }
            AgentHouseholdProposalStateV1::Committing
            | AgentHouseholdProposalStateV1::ReconciliationRequired => {
                Err(AgentHouseholdContractErrorV1::CancelTooLate)
            }
            _ => Err(AgentHouseholdContractErrorV1::InvalidTransition),
        }
    }

    pub fn mark_committed(&mut self) -> Result<(), AgentHouseholdContractErrorV1> {
        if self.state != AgentHouseholdProposalStateV1::Committing {
            return Err(AgentHouseholdContractErrorV1::InvalidTransition);
        }
        self.state = AgentHouseholdProposalStateV1::Committed;
        Ok(())
    }

    pub fn mark_reconciliation_required(&mut self) -> Result<(), AgentHouseholdContractErrorV1> {
        if self.state != AgentHouseholdProposalStateV1::Committing {
            return Err(AgentHouseholdContractErrorV1::InvalidTransition);
        }
        self.state = AgentHouseholdProposalStateV1::ReconciliationRequired;
        Ok(())
    }

    pub fn reconcile_committed(&mut self) -> Result<(), AgentHouseholdContractErrorV1> {
        if self.state != AgentHouseholdProposalStateV1::ReconciliationRequired {
            return Err(AgentHouseholdContractErrorV1::InvalidTransition);
        }
        self.state = AgentHouseholdProposalStateV1::Committed;
        Ok(())
    }

    pub fn reconcile_proven_uncommitted(&mut self) -> Result<(), AgentHouseholdContractErrorV1> {
        if self.state != AgentHouseholdProposalStateV1::ReconciliationRequired {
            return Err(AgentHouseholdContractErrorV1::InvalidTransition);
        }
        self.state = AgentHouseholdProposalStateV1::ProvenUncommitted;
        Ok(())
    }

    fn validate_current(
        &self,
        current: &LocalHouseholdAuthoritySnapshotV1,
    ) -> Result<(), AgentHouseholdContractErrorV1> {
        if self.binding.account != current.account {
            return Err(AgentHouseholdContractErrorV1::AccountChanged);
        }
        if self.binding.disclosure_generation != current.disclosure_generation {
            return Err(AgentHouseholdContractErrorV1::DisclosureGenerationChanged);
        }
        if self.binding.lifecycle_generation != current.lifecycle_generation {
            return Err(AgentHouseholdContractErrorV1::LifecycleGenerationChanged);
        }
        if self.binding.expected_household_revision != current.household_revision {
            return Err(AgentHouseholdContractErrorV1::HouseholdRevisionChanged);
        }
        if self.binding.expected_profile_revision != current.profile_revision {
            return Err(AgentHouseholdContractErrorV1::ProfileRevisionChanged);
        }
        if current.observed_at >= self.binding.expires_at {
            return Err(AgentHouseholdContractErrorV1::Expired);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentHouseholdContractErrorV1 {
    InvalidTransition,
    InvalidOperationShape,
    AccountChanged,
    DisclosureGenerationChanged,
    LifecycleGenerationChanged,
    HouseholdRevisionChanged,
    ProfileRevisionChanged,
    ProposalGenerationChanged,
    ProposalDigestChanged,
    Expired,
    InvalidReviewWidth,
    MissingFrozenAuthority,
    CancelTooLate,
}

impl fmt::Display for AgentHouseholdContractErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidTransition => "household proposal transition is invalid",
            Self::InvalidOperationShape => "household proposal operation shape is invalid",
            Self::AccountChanged => "household proposal account changed",
            Self::DisclosureGenerationChanged => "household agent disclosure generation changed",
            Self::LifecycleGenerationChanged => "household lifecycle generation changed",
            Self::HouseholdRevisionChanged => "household revision changed",
            Self::ProfileRevisionChanged => "household profile revision changed",
            Self::ProposalGenerationChanged => "household proposal generation changed",
            Self::ProposalDigestChanged => "household proposal digest changed",
            Self::Expired => "household proposal expired",
            Self::InvalidReviewWidth => "household review width is invalid",
            Self::MissingFrozenAuthority => "household proposal authority is not frozen",
            Self::CancelTooLate => "household cancellation lost the commit race",
        })
    }
}

impl std::error::Error for AgentHouseholdContractErrorV1 {}

/// Render untrusted household content as unmistakable terminal data.
///
/// Newlines, tabs, terminal controls, bidi controls, and invisible separators
/// are shown explicitly. No content is truncated.
#[must_use]
pub fn household_review_safe_text_v1(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        let codepoint = character as u32;
        let visible_escape = character.is_control()
            || matches!(codepoint, 0x0080..=0x009f)
            || matches!(codepoint, 0x061c | 0x200b..=0x200f | 0x2028..=0x202e)
            || matches!(codepoint, 0x2060 | 0x2066..=0x2069 | 0xfeff);
        if visible_escape {
            use std::fmt::Write as _;
            let _ = write!(output, "<U+{codepoint:04X}>");
        } else {
            output.push(character);
        }
    }
    output
}

/// Wrap an escaped value without dropping any data.
///
/// The caller owns labels, headings, and action controls. This function
/// accepts only the untrusted value and therefore cannot let it create UI
/// structure. Width is measured in terminal cells after visible escaping.
pub fn household_review_safe_lines_v1(
    value: &str,
    width: usize,
) -> Result<Vec<String>, AgentHouseholdContractErrorV1> {
    if !(AGENT_HOUSEHOLD_REVIEW_MINIMUM_WIDTH..=AGENT_HOUSEHOLD_REVIEW_MAXIMUM_WIDTH)
        .contains(&width)
    {
        return Err(AgentHouseholdContractErrorV1::InvalidReviewWidth);
    }
    let escaped = household_review_safe_text_v1(value);
    if escaped.is_empty() {
        return Ok(vec![String::new()]);
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0usize;
    for character in escaped.chars() {
        let character_width = character.width().unwrap_or(0);
        if used.saturating_add(character_width) > width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
            used = 0;
        }
        line.push(character);
        used = used.saturating_add(character_width);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HouseholdSubjectId;
    use serde_json::json;

    fn revision() -> HouseholdRevision {
        HouseholdRevision::new(7).expect("revision")
    }

    fn timestamp() -> CanonicalTimestampV1 {
        CanonicalTimestampV1::parse("2026-08-02T12:00:00.000Z").expect("timestamp")
    }

    fn binding(
        operation: AgentHouseholdOperationV1,
        disclosure_generation: GenerationId,
        profile_revision: Option<ProfileRevision>,
        member_id: Option<MemberId>,
    ) -> LocalHouseholdProposalBindingV1 {
        LocalHouseholdProposalBindingV1 {
            account: AccountId::parse("phase0-proposal-account").expect("account"),
            proposal_ref: AgentHouseholdProposalIdV1::new(),
            operation,
            disclosure_generation,
            lifecycle_generation: GenerationId::new(11),
            projection: AgentHouseholdProjectionV1::Profile,
            expected_household_revision: revision(),
            expected_profile_revision: profile_revision,
            commit_id: CommitId::new(),
            member_id,
            previous_scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
            originating_session_digest: CanonicalDigestV1::from_bytes([9; 32]),
            eligible_host_policy_digest: CanonicalDigestV1::from_bytes([10; 32]),
            created_at: timestamp(),
            expires_at: CanonicalTimestampV1::parse("2026-08-02T12:10:00.000Z").expect("expiry"),
        }
    }

    fn snapshot(
        disclosure_generation: GenerationId,
        profile_revision: Option<ProfileRevision>,
    ) -> LocalHouseholdAuthoritySnapshotV1 {
        LocalHouseholdAuthoritySnapshotV1 {
            account: AccountId::parse("phase0-proposal-account").expect("account"),
            disclosure_generation,
            lifecycle_generation: GenerationId::new(11),
            household_revision: revision(),
            profile_revision,
            observed_at: CanonicalTimestampV1::parse("2026-08-02T12:05:00.000Z")
                .expect("observation"),
        }
    }

    fn frozen_candidate(
        proposal_digest: CanonicalDigestV1,
        effect_digest: CanonicalDigestV1,
    ) -> LocalHouseholdFrozenCandidateV1 {
        LocalHouseholdFrozenCandidateV1 {
            proposal_digest,
            effect_fingerprint: HouseholdEffectFingerprintV1::from_digest(effect_digest),
            before_document_digest: CanonicalDigestV1::from_bytes([11; 32]),
            after_document_digest: CanonicalDigestV1::from_bytes([12; 32]),
            resulting_scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
            conversation_continuity_reset: false,
            frozen_semantic_timestamp: timestamp(),
        }
    }

    #[test]
    fn intake_freezes_authority_only_at_review_transition() {
        let mut authority = LocalHouseholdProposalAuthorityV1::awaiting_local_input(binding(
            AgentHouseholdOperationV1::Add,
            GenerationId::new(3),
            None,
            Some(MemberId::new()),
        ));
        assert!(authority.frozen.is_none());

        let proposal_digest = CanonicalDigestV1::from_bytes([1; 32]);
        authority
            .freeze_for_review(
                &snapshot(GenerationId::new(3), None),
                frozen_candidate(proposal_digest, CanonicalDigestV1::from_bytes([2; 32])),
            )
            .expect("freeze");

        assert_eq!(
            authority.state,
            AgentHouseholdProposalStateV1::AwaitingLocalReview
        );
        assert_eq!(authority.proposal_generation, GenerationId::new(1));
        assert!(authority.frozen.is_some());
        authority
            .begin_commit(
                &snapshot(GenerationId::new(3), None),
                GenerationId::new(1),
                proposal_digest,
            )
            .expect("commit begins");
        assert_eq!(authority.state, AgentHouseholdProposalStateV1::Committing);
    }

    #[test]
    fn disclosure_change_and_cancel_commit_race_fail_closed() {
        let profile_revision = ProfileRevision::new(2).ok();
        let mut authority = LocalHouseholdProposalAuthorityV1::awaiting_local_input(binding(
            AgentHouseholdOperationV1::Edit,
            GenerationId::new(4),
            profile_revision,
            None,
        ));
        assert_eq!(
            authority.freeze_for_review(
                &snapshot(GenerationId::new(5), profile_revision),
                frozen_candidate(
                    CanonicalDigestV1::from_bytes([3; 32]),
                    CanonicalDigestV1::from_bytes([4; 32]),
                ),
            ),
            Err(AgentHouseholdContractErrorV1::DisclosureGenerationChanged)
        );
        authority
            .cancel_before_commit()
            .expect("cancel wins before commit");
        assert_eq!(authority.state, AgentHouseholdProposalStateV1::Cancelled);
    }

    #[test]
    fn commit_revalidates_every_frozen_authority_and_reconciles_a_lost_cancel_race() {
        let profile_revision = ProfileRevision::new(2).ok();
        let mut authority = LocalHouseholdProposalAuthorityV1::awaiting_local_input(binding(
            AgentHouseholdOperationV1::Edit,
            GenerationId::new(4),
            profile_revision,
            Some(MemberId::new()),
        ));
        let proposal_digest = CanonicalDigestV1::from_bytes([21; 32]);
        let current = snapshot(GenerationId::new(4), profile_revision);
        authority
            .freeze_for_review(
                &current,
                frozen_candidate(proposal_digest, CanonicalDigestV1::from_bytes([22; 32])),
            )
            .expect("freeze");

        let wrong_account = LocalHouseholdAuthoritySnapshotV1 {
            account: AccountId::parse("another-account").expect("account"),
            ..current.clone()
        };
        assert_eq!(
            authority.begin_commit(&wrong_account, GenerationId::new(1), proposal_digest,),
            Err(AgentHouseholdContractErrorV1::AccountChanged)
        );
        assert_eq!(
            authority.begin_commit(
                &current,
                GenerationId::new(1),
                CanonicalDigestV1::from_bytes([23; 32]),
            ),
            Err(AgentHouseholdContractErrorV1::ProposalDigestChanged)
        );
        authority
            .begin_commit(&current, GenerationId::new(1), proposal_digest)
            .expect("commit CAS wins");
        assert_eq!(
            authority.cancel_before_commit(),
            Err(AgentHouseholdContractErrorV1::CancelTooLate)
        );
        authority
            .mark_reconciliation_required()
            .expect("uncertain outcome");
        authority
            .reconcile_committed()
            .expect("ledger proves exact commit");
        assert_eq!(authority.state, AgentHouseholdProposalStateV1::Committed);
    }

    #[test]
    fn expired_authority_cannot_freeze_or_commit() {
        let binding = binding(
            AgentHouseholdOperationV1::Scope,
            GenerationId::new(2),
            None,
            None,
        );
        let mut authority = LocalHouseholdProposalAuthorityV1::prepared(binding);
        let expired = LocalHouseholdAuthoritySnapshotV1 {
            account: AccountId::parse("phase0-proposal-account").expect("account"),
            disclosure_generation: GenerationId::new(2),
            lifecycle_generation: GenerationId::new(11),
            household_revision: revision(),
            profile_revision: None,
            observed_at: CanonicalTimestampV1::parse("2026-08-02T12:10:00.000Z")
                .expect("expiry boundary"),
        };
        assert_eq!(
            authority.freeze_for_review(
                &expired,
                frozen_candidate(
                    CanonicalDigestV1::from_bytes([24; 32]),
                    CanonicalDigestV1::from_bytes([25; 32]),
                ),
            ),
            Err(AgentHouseholdContractErrorV1::Expired)
        );
        assert!(authority.frozen.is_none());
        assert_eq!(authority.state, AgentHouseholdProposalStateV1::Prepared);
    }

    #[test]
    fn presentation_downgrades_without_repeating_identity_or_profile_counts() {
        let presentation = AgentHouseholdProposalPresentationV1 {
            schema_version: AGENT_HOUSEHOLD_CONTRACT_VERSION,
            proposal_ref: AgentHouseholdProposalIdV1::new(),
            operation: AgentHouseholdOperationV1::Edit,
            state: AgentHouseholdProposalStateV1::AwaitingLocalReview,
            projection: AgentHouseholdProjectionV1::Profile,
            disclosure_generation: GenerationId::new(9),
            affected_member_ref: Some(MemberId::new()),
            affected_member_label: Some(DisplayName::parse("Fixture").expect("label")),
            profile_change_count: Some(3),
            created_at: timestamp(),
            expires_at: CanonicalTimestampV1::parse("2026-08-02T12:10:00.000Z").expect("expiry"),
            human_status: "Ready for your review".to_owned(),
            handoff_command: "heyfood".to_owned(),
            handoff_instruction: "Open `/household changes` to review this change locally."
                .to_owned(),
        };
        let filtered = presentation.filtered_to(AgentHouseholdProjectionV1::ContentFree);
        assert!(filtered.affected_member_ref.is_none());
        assert!(filtered.affected_member_label.is_none());
        assert!(filtered.profile_change_count.is_none());
        assert_eq!(filtered.handoff_command, "heyfood");
    }

    #[test]
    fn disclosure_grants_keep_roster_and_profile_authority_separate() {
        let account = AccountId::parse("phase0-disclosure-account").expect("account");
        let member = AgentHouseholdSubjectV1::Member(MemberId::new());
        let observed_at = timestamp();
        let adult = AgentDisclosureGrantV1 {
            account: account.clone(),
            subject: member.clone(),
            subject_minor_status: MinorStatusV1::Adult,
            data_classes: vec![
                AgentDisclosureDataClassV1::Roster,
                AgentDisclosureDataClassV1::MinimizedDeclaredProfile,
            ],
            granting_authority: AgentDisclosureGrantingAuthorityV1::AccountOwnerAdultAuthorization,
            revision: 1,
            generation: GenerationId::new(3),
            state: AgentDisclosureGrantStateV1::Active,
            expires_at: None,
        };
        assert!(adult.permits_for(
            &account,
            &member,
            GenerationId::new(3),
            AgentDisclosureDataClassV1::Roster,
            &observed_at,
        ));
        assert!(adult.permits_for(
            &account,
            &member,
            GenerationId::new(3),
            AgentDisclosureDataClassV1::MinimizedDeclaredProfile,
            &observed_at,
        ));

        let minor = AgentDisclosureGrantV1 {
            subject: member,
            subject_minor_status: MinorStatusV1::Minor,
            granting_authority: AgentDisclosureGrantingAuthorityV1::AuthorizedGuardianRosterOnly,
            ..adult.clone()
        };
        assert!(minor.permits_for(
            &account,
            &minor.subject,
            GenerationId::new(3),
            AgentDisclosureDataClassV1::Roster,
            &observed_at,
        ));
        assert!(!minor.permits_for(
            &account,
            &minor.subject,
            GenerationId::new(3),
            AgentDisclosureDataClassV1::MinimizedDeclaredProfile,
            &observed_at,
        ));

        let revoked = AgentDisclosureGrantV1 {
            state: AgentDisclosureGrantStateV1::Revoked,
            ..adult
        };
        assert!(!revoked.permits_for(
            &account,
            &revoked.subject,
            GenerationId::new(3),
            AgentDisclosureDataClassV1::Roster,
            &observed_at,
        ));
        assert!(!revoked.permits_for(
            &account,
            &revoked.subject,
            GenerationId::new(3),
            AgentDisclosureDataClassV1::MinimizedDeclaredProfile,
            &observed_at,
        ));
        let other_account = AccountId::parse("other-disclosure-account").expect("account");
        assert!(!minor.permits_for(
            &other_account,
            &minor.subject,
            GenerationId::new(3),
            AgentDisclosureDataClassV1::Roster,
            &observed_at,
        ));

        let guardian_profile = AgentDisclosureGrantV1 {
            state: AgentDisclosureGrantStateV1::Active,
            subject_minor_status: MinorStatusV1::Adult,
            granting_authority: AgentDisclosureGrantingAuthorityV1::AuthorizedGuardianRosterOnly,
            ..minor.clone()
        };
        assert!(!guardian_profile.permits_for(
            &account,
            &guardian_profile.subject,
            GenerationId::new(3),
            AgentDisclosureDataClassV1::MinimizedDeclaredProfile,
            &observed_at,
        ));

        let expired = AgentDisclosureGrantV1 {
            expires_at: Some(observed_at.clone()),
            ..minor
        };
        assert!(!expired.permits_for(
            &account,
            &expired.subject,
            GenerationId::new(3),
            AgentDisclosureDataClassV1::Roster,
            &observed_at,
        ));
    }

    #[test]
    fn review_text_makes_control_and_directional_characters_visible() {
        let rendered = household_review_safe_text_v1("Julie\n\u{1b}[31m\u{202e}x\u{200b}");
        assert_eq!(rendered, "Julie<U+000A><U+001B>[31m<U+202E>x<U+200B>");
    }

    #[test]
    fn safe_review_wrapping_is_complete_at_compact_standard_and_wide_widths() {
        let source =
            "Synthetic <b>profile</b> https://invalid.example/\n\u{1b}[31m\u{202e}value ".repeat(8);
        let escaped = household_review_safe_text_v1(&source);
        for width in [40, 80, 120] {
            let lines = household_review_safe_lines_v1(&source, width).expect("safe lines");
            assert_eq!(lines.concat(), escaped);
            assert!(lines.iter().all(|line| {
                line.chars()
                    .map(|character| character.width().unwrap_or(0))
                    .sum::<usize>()
                    <= width
            }));
            assert!(
                lines
                    .iter()
                    .all(|line| !line.contains('\n') && !line.contains('\u{1b}'))
            );
        }
        assert_eq!(
            household_review_safe_lines_v1("value", 19),
            Err(AgentHouseholdContractErrorV1::InvalidReviewWidth)
        );
    }

    #[test]
    fn public_subject_and_outcome_wire_shapes_are_closed_and_human_neutral() {
        assert_eq!(
            serde_json::to_value(AgentHouseholdSubjectV1::Self_).expect("self subject"),
            json!({"kind": "self"})
        );
        let member =
            MemberId::parse_preserved("10000000-0000-4000-8000-000000000001").expect("member");
        assert_eq!(
            serde_json::to_value(AgentHouseholdSubjectV1::Member(member)).expect("member subject"),
            json!({
                "kind": "member",
                "member_ref": "10000000-0000-4000-8000-000000000001"
            })
        );
        assert_eq!(
            serde_json::to_value(AgentHouseholdSubjectV1::Everyone).expect("everyone subject"),
            json!({"kind": "everyone"})
        );

        let revision = revision();
        let receipt = AgentHouseholdOutcomeReceiptV1 {
            schema_version: AGENT_HOUSEHOLD_CONTRACT_VERSION,
            proposal_ref: AgentHouseholdProposalIdV1::from_uuid(
                Uuid::parse_str("20000000-0000-4000-8000-000000000001").expect("proposal"),
            ),
            state: AgentHouseholdProposalStateV1::Cancelled,
            household_revision_before: revision,
            household_revision_after: Some(revision),
            known_no_household_mutation: true,
            retry_class: AgentHouseholdRetryClassV1::NotApplicable,
            next_action: AgentHouseholdNextActionV1::None,
        };
        let encoded = serde_json::to_string(&receipt).expect("receipt");
        assert!(!encoded.contains("account"));
        assert!(!encoded.contains("commit"));
        assert!(!encoded.contains("fingerprint"));
    }
}
