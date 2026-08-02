//! Phase-0-only household agent contracts.
//!
//! These types freeze the local disclosure, proposal, and review state model.
//! They are deliberately not wired to CLI or MCP routes in Phase 0.

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use unicode_width::UnicodeWidthChar;
use uuid::Uuid;

use crate::{
    AccountId, CanonicalDigestV1, CanonicalTimestampV1, CommitId, DisplayName, GenerationId,
    HouseholdEffectFingerprintV1, HouseholdLifecycleV1, HouseholdProfileStateV1, HouseholdRevision,
    HouseholdScope, MemberId, MinorStatusV1, ProfileRevision, RelationshipV1,
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

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AgentDisclosurePurposeV1 {
    HouseholdAgentRead,
    HouseholdAgentProposalStatus,
}

/// A disclosure grant is always scoped to one exact person. `Everyone` is a
/// request-time aggregation that must prove a grant for every included person.
#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub enum AgentDisclosureGrantSubjectV1 {
    Self_,
    Member(MemberId),
}

impl fmt::Debug for AgentDisclosureGrantSubjectV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Self_ => formatter.write_str("AgentDisclosureGrantSubjectV1::Self_"),
            Self::Member(_) => {
                formatter.write_str("AgentDisclosureGrantSubjectV1::Member([REDACTED])")
            }
        }
    }
}

impl AgentDisclosureGrantSubjectV1 {
    #[must_use]
    pub fn from_agent_subject(subject: &AgentHouseholdSubjectV1) -> Option<Self> {
        match subject {
            AgentHouseholdSubjectV1::Self_ => Some(Self::Self_),
            AgentHouseholdSubjectV1::Member(member) => Some(Self::Member(member.clone())),
            AgentHouseholdSubjectV1::Everyone => None,
        }
    }

    fn digest_bytes(&self) -> Vec<u8> {
        match self {
            Self::Self_ => b"self".to_vec(),
            Self::Member(member) => {
                let mut bytes = b"member\0".to_vec();
                bytes.extend_from_slice(member.as_str().as_bytes());
                bytes
            }
        }
    }
}

/// Encrypted local authority record. It is intentionally not serializable:
/// account binding and grant authority never become an agent result.
#[derive(Clone, Eq, PartialEq)]
pub struct AgentDisclosureGrantV1 {
    account: AccountId,
    subject: AgentDisclosureGrantSubjectV1,
    subject_minor_status: MinorStatusV1,
    data_classes: Vec<AgentDisclosureDataClassV1>,
    purpose: AgentDisclosurePurposeV1,
    granting_authority: AgentDisclosureGrantingAuthorityV1,
    revision: u64,
    generation: GenerationId,
    state: AgentDisclosureGrantStateV1,
    issued_at: CanonicalTimestampV1,
    expires_at: Option<CanonicalTimestampV1>,
}

impl fmt::Debug for AgentDisclosureGrantV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentDisclosureGrantV1")
            .field("subject_minor_status", &self.subject_minor_status)
            .field("data_classes", &self.data_classes)
            .field("purpose", &self.purpose)
            .field("granting_authority", &self.granting_authority)
            .field("revision", &self.revision)
            .field("generation", &self.generation)
            .field("state", &self.state)
            .field("has_expiry", &self.expires_at.is_some())
            .finish_non_exhaustive()
    }
}

impl AgentDisclosureGrantV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        account: AccountId,
        subject: AgentDisclosureGrantSubjectV1,
        subject_minor_status: MinorStatusV1,
        data_classes: Vec<AgentDisclosureDataClassV1>,
        purpose: AgentDisclosurePurposeV1,
        granting_authority: AgentDisclosureGrantingAuthorityV1,
        revision: u64,
        generation: GenerationId,
        state: AgentDisclosureGrantStateV1,
        issued_at: CanonicalTimestampV1,
        expires_at: Option<CanonicalTimestampV1>,
    ) -> Result<Self, AgentHouseholdContractErrorV1> {
        let valid_classes = matches!(
            data_classes.as_slice(),
            [AgentDisclosureDataClassV1::Roster]
                | [
                    AgentDisclosureDataClassV1::Roster,
                    AgentDisclosureDataClassV1::MinimizedDeclaredProfile
                ]
        );
        let valid_authority = match granting_authority {
            AgentDisclosureGrantingAuthorityV1::AccountOwnerAdultAuthorization => {
                subject_minor_status == MinorStatusV1::Adult
            }
            AgentDisclosureGrantingAuthorityV1::AuthorizedGuardianRosterOnly => {
                subject_minor_status != MinorStatusV1::Adult
                    && data_classes.as_slice() == [AgentDisclosureDataClassV1::Roster]
            }
        };
        if revision == 0
            || !valid_classes
            || !valid_authority
            || expires_at
                .as_ref()
                .is_some_and(|expiry| expiry <= &issued_at)
        {
            return Err(AgentHouseholdContractErrorV1::InvalidDisclosureGrant);
        }
        Ok(Self {
            account,
            subject,
            subject_minor_status,
            data_classes,
            purpose,
            granting_authority,
            revision,
            generation,
            state,
            issued_at,
            expires_at,
        })
    }

    #[must_use]
    pub fn subject(&self) -> &AgentDisclosureGrantSubjectV1 {
        &self.subject
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub fn permits_for(
        &self,
        account: &AccountId,
        subject: &AgentDisclosureGrantSubjectV1,
        generation: GenerationId,
        purpose: AgentDisclosurePurposeV1,
        data_class: AgentDisclosureDataClassV1,
        observed_at: &CanonicalTimestampV1,
    ) -> bool {
        if &self.account != account
            || &self.subject != subject
            || self.generation != generation
            || self.purpose != purpose
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

#[derive(Clone, Eq, PartialEq)]
pub struct AgentDisclosureGrantSetV1 {
    account: AccountId,
    generation: GenerationId,
    purpose: AgentDisclosurePurposeV1,
    observed_at: CanonicalTimestampV1,
    grants: Vec<AgentDisclosureGrantV1>,
    revision_set_digest: CanonicalDigestV1,
}

impl fmt::Debug for AgentDisclosureGrantSetV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentDisclosureGrantSetV1")
            .field("generation", &self.generation)
            .field("purpose", &self.purpose)
            .field("grant_count", &self.grants.len())
            .finish_non_exhaustive()
    }
}

impl AgentDisclosureGrantSetV1 {
    pub fn new(
        account: AccountId,
        generation: GenerationId,
        purpose: AgentDisclosurePurposeV1,
        observed_at: CanonicalTimestampV1,
        mut grants: Vec<AgentDisclosureGrantV1>,
    ) -> Result<Self, AgentHouseholdContractErrorV1> {
        grants.sort_by(|left, right| left.subject.cmp(&right.subject));
        if grants
            .windows(2)
            .any(|pair| pair[0].subject == pair[1].subject)
            || grants.iter().any(|grant| {
                grant.account != account
                    || grant.generation != generation
                    || grant.purpose != purpose
            })
        {
            return Err(AgentHouseholdContractErrorV1::InvalidDisclosureGrantSet);
        }
        let mut hasher = Sha256::new();
        hasher.update(b"heyfood.agent.disclosure.revision-set.v1\0");
        hasher.update(generation.get().to_be_bytes());
        hasher.update([purpose as u8]);
        for grant in &grants {
            let subject = grant.subject.digest_bytes();
            hasher.update((subject.len() as u64).to_be_bytes());
            hasher.update(subject);
            hasher.update(grant.revision.to_be_bytes());
        }
        let revision_set_digest = CanonicalDigestV1::from_bytes(hasher.finalize().into());
        Ok(Self {
            account,
            generation,
            purpose,
            observed_at,
            grants,
            revision_set_digest,
        })
    }

    #[must_use]
    pub const fn generation(&self) -> GenerationId {
        self.generation
    }

    #[must_use]
    pub const fn purpose(&self) -> AgentDisclosurePurposeV1 {
        self.purpose
    }

    #[must_use]
    pub const fn revision_set_digest(&self) -> CanonicalDigestV1 {
        self.revision_set_digest
    }

    #[must_use]
    pub fn account_matches(&self, account: &AccountId) -> bool {
        &self.account == account
    }

    #[must_use]
    pub fn maximum_projection_for(
        &self,
        subjects: &[AgentDisclosureGrantSubjectV1],
    ) -> AgentHouseholdProjectionV1 {
        if subjects.is_empty() {
            return AgentHouseholdProjectionV1::ContentFree;
        }
        let permits_every = |data_class| {
            subjects.iter().all(|subject| {
                self.grants.iter().any(|grant| {
                    grant.subject == *subject
                        && grant.permits_for(
                            &self.account,
                            subject,
                            self.generation,
                            self.purpose,
                            data_class,
                            &self.observed_at,
                        )
                })
            })
        };
        if permits_every(AgentDisclosureDataClassV1::MinimizedDeclaredProfile) {
            AgentHouseholdProjectionV1::Profile
        } else if permits_every(AgentDisclosureDataClassV1::Roster) {
            AgentHouseholdProjectionV1::Roster
        } else {
            AgentHouseholdProjectionV1::ContentFree
        }
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentHouseholdReadRequestKindV1 {
    HouseholdReadRequest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentHouseholdReadResultKindV1 {
    HouseholdReadResult,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentHouseholdPrepareRequestKindV1 {
    PrepareHouseholdChange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentHouseholdContextInputKindV1 {
    GetHouseholdContext,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentHouseholdMemberInputKindV1 {
    GetHouseholdMember,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentHouseholdProposalRefInputKindV1 {
    GetHouseholdChange,
    CancelHouseholdChange,
    ReconcileHouseholdChange,
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentHouseholdContextInputV1 {
    pub schema_version: u16,
    pub kind: AgentHouseholdContextInputKindV1,
    pub subject: Option<AgentHouseholdSubjectV1>,
    pub requested_projection: AgentHouseholdProjectionV1,
    pub expected_disclosure_generation: GenerationId,
    pub cursor: Option<String>,
    pub limit: u16,
}

impl fmt::Debug for AgentHouseholdContextInputV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentHouseholdContextInputV1")
            .field("schema_version", &self.schema_version)
            .field("kind", &self.kind)
            .field(
                "subject_kind",
                &self.subject.as_ref().map(agent_subject_kind),
            )
            .field("requested_projection", &self.requested_projection)
            .field(
                "expected_disclosure_generation",
                &self.expected_disclosure_generation,
            )
            .field("has_cursor", &self.cursor.is_some())
            .field("limit", &self.limit)
            .finish()
    }
}

impl AgentHouseholdContextInputV1 {
    #[must_use]
    pub fn into_request(self) -> AgentHouseholdReadRequestV1 {
        AgentHouseholdReadRequestV1 {
            schema_version: self.schema_version,
            kind: AgentHouseholdReadRequestKindV1::HouseholdReadRequest,
            subject: self.subject,
            requested_projection: self.requested_projection,
            expected_disclosure_generation: self.expected_disclosure_generation,
            cursor: self.cursor,
            limit: self.limit,
        }
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentHouseholdMemberInputV1 {
    pub schema_version: u16,
    pub kind: AgentHouseholdMemberInputKindV1,
    pub member_ref: MemberId,
    pub requested_projection: AgentHouseholdProjectionV1,
    pub expected_disclosure_generation: GenerationId,
}

impl fmt::Debug for AgentHouseholdMemberInputV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentHouseholdMemberInputV1")
            .field("schema_version", &self.schema_version)
            .field("kind", &self.kind)
            .field("requested_projection", &self.requested_projection)
            .field(
                "expected_disclosure_generation",
                &self.expected_disclosure_generation,
            )
            .finish_non_exhaustive()
    }
}

impl AgentHouseholdMemberInputV1 {
    #[must_use]
    pub fn into_request(self) -> AgentHouseholdReadRequestV1 {
        AgentHouseholdReadRequestV1 {
            schema_version: self.schema_version,
            kind: AgentHouseholdReadRequestKindV1::HouseholdReadRequest,
            subject: Some(AgentHouseholdSubjectV1::Member(self.member_ref)),
            requested_projection: self.requested_projection,
            expected_disclosure_generation: self.expected_disclosure_generation,
            cursor: None,
            limit: 1,
        }
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentHouseholdProposalRefInputV1 {
    pub schema_version: u16,
    pub kind: AgentHouseholdProposalRefInputKindV1,
    pub proposal_ref: AgentHouseholdProposalIdV1,
}

impl fmt::Debug for AgentHouseholdProposalRefInputV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentHouseholdProposalRefInputV1")
            .field("schema_version", &self.schema_version)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentHouseholdMemberProjectionV1 {
    pub member_ref: MemberId,
    pub display_label: DisplayName,
    pub relationship: RelationshipV1,
    pub lifecycle: HouseholdLifecycleV1,
    pub profile_state: HouseholdProfileStateV1,
    pub profile_schema_version: Option<u16>,
    pub profile_revision: Option<ProfileRevision>,
    pub profile_complete: bool,
    pub minimized_declared_profile: Option<AgentMinimizedDeclaredProfileV1>,
}

impl fmt::Debug for AgentHouseholdMemberProjectionV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentHouseholdMemberProjectionV1")
            .field("relationship", &self.relationship)
            .field("lifecycle", &self.lifecycle)
            .field("profile_state", &self.profile_state)
            .field("profile_schema_version", &self.profile_schema_version)
            .field("profile_revision", &self.profile_revision)
            .field("profile_complete", &self.profile_complete)
            .field(
                "has_minimized_declared_profile",
                &self.minimized_declared_profile.is_some(),
            )
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentMinimizedDeclaredProfileV1 {
    pub diet_styles: Vec<String>,
    pub allergies: Vec<String>,
    pub restrictions: Vec<String>,
    pub health_conditions: Vec<String>,
    pub avoid_ingredients: Vec<String>,
}

impl fmt::Debug for AgentMinimizedDeclaredProfileV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentMinimizedDeclaredProfileV1")
            .field("diet_style_count", &self.diet_styles.len())
            .field("allergy_count", &self.allergies.len())
            .field("restriction_count", &self.restrictions.len())
            .field("health_condition_count", &self.health_conditions.len())
            .field("avoid_ingredient_count", &self.avoid_ingredients.len())
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentHouseholdReadRequestV1 {
    pub schema_version: u16,
    pub kind: AgentHouseholdReadRequestKindV1,
    pub subject: Option<AgentHouseholdSubjectV1>,
    pub requested_projection: AgentHouseholdProjectionV1,
    pub expected_disclosure_generation: GenerationId,
    pub cursor: Option<String>,
    pub limit: u16,
}

impl fmt::Debug for AgentHouseholdReadRequestV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentHouseholdReadRequestV1")
            .field("schema_version", &self.schema_version)
            .field("kind", &self.kind)
            .field(
                "subject_kind",
                &self.subject.as_ref().map(agent_subject_kind),
            )
            .field("requested_projection", &self.requested_projection)
            .field(
                "expected_disclosure_generation",
                &self.expected_disclosure_generation,
            )
            .field("has_cursor", &self.cursor.is_some())
            .field("limit", &self.limit)
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentHouseholdReadSnapshotV1 {
    pub schema_version: u16,
    pub kind: AgentHouseholdReadResultKindV1,
    pub projection: AgentHouseholdProjectionV1,
    pub resolved_subject: Option<AgentHouseholdSubjectV1>,
    pub resolved_from_active_scope: bool,
    pub active_scope: Option<HouseholdScope>,
    pub household_revision: HouseholdRevision,
    pub disclosure_generation: GenerationId,
    pub eligible_member_count: u16,
    pub restricted_member_count: u16,
    pub members: Vec<AgentHouseholdMemberProjectionV1>,
    pub next_cursor: Option<String>,
}

impl fmt::Debug for AgentHouseholdReadSnapshotV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentHouseholdReadSnapshotV1")
            .field("schema_version", &self.schema_version)
            .field("kind", &self.kind)
            .field("projection", &self.projection)
            .field(
                "resolved_subject_kind",
                &self.resolved_subject.as_ref().map(agent_subject_kind),
            )
            .field(
                "resolved_from_active_scope",
                &self.resolved_from_active_scope,
            )
            .field(
                "active_scope_kind",
                &self.active_scope.as_ref().map(scope_kind),
            )
            .field("household_revision", &self.household_revision)
            .field("disclosure_generation", &self.disclosure_generation)
            .field("eligible_member_count", &self.eligible_member_count)
            .field("restricted_member_count", &self.restricted_member_count)
            .field("member_count", &self.members.len())
            .field("has_next_cursor", &self.next_cursor.is_some())
            .finish()
    }
}

impl AgentHouseholdReadSnapshotV1 {
    #[must_use]
    pub fn filtered_to(mut self, projection: AgentHouseholdProjectionV1) -> Self {
        self.projection = projection;
        match projection {
            AgentHouseholdProjectionV1::ContentFree => {
                self.resolved_subject = None;
                self.active_scope = None;
                self.members.clear();
                self.next_cursor = None;
            }
            AgentHouseholdProjectionV1::Roster => {
                for member in &mut self.members {
                    member.minimized_declared_profile = None;
                }
            }
            AgentHouseholdProjectionV1::Profile => {}
        }
        self
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentHouseholdPrepareRequestV1 {
    pub schema_version: u16,
    pub kind: AgentHouseholdPrepareRequestKindV1,
    pub operation: AgentHouseholdOperationV1,
    pub requested_projection: AgentHouseholdProjectionV1,
    pub expected_disclosure_generation: GenerationId,
    pub expected_household_revision: HouseholdRevision,
    pub affected_member_ref: Option<MemberId>,
    pub bundled_scope: Option<HouseholdScope>,
}

impl fmt::Debug for AgentHouseholdPrepareRequestV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentHouseholdPrepareRequestV1")
            .field("schema_version", &self.schema_version)
            .field("kind", &self.kind)
            .field("operation", &self.operation)
            .field("requested_projection", &self.requested_projection)
            .field(
                "expected_disclosure_generation",
                &self.expected_disclosure_generation,
            )
            .field(
                "expected_household_revision",
                &self.expected_household_revision,
            )
            .field("has_affected_member", &self.affected_member_ref.is_some())
            .field("bundles_scope", &self.bundled_scope.is_some())
            .finish()
    }
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentHouseholdChangeFieldV1 {
    DisplayLabel,
    Relationship,
    Lifecycle,
    DietStyles,
    Allergies,
    Restrictions,
    HealthConditions,
    AvoidIngredients,
    ActiveScope,
}

impl AgentHouseholdChangeFieldV1 {
    #[must_use]
    pub const fn is_profile(self) -> bool {
        matches!(
            self,
            Self::DietStyles
                | Self::Allergies
                | Self::Restrictions
                | Self::HealthConditions
                | Self::AvoidIngredients
        )
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentHouseholdChangeV1 {
    pub field: AgentHouseholdChangeFieldV1,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

impl fmt::Debug for AgentHouseholdChangeV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentHouseholdChangeV1")
            .field("field", &self.field)
            .field("before_count", &self.before.len())
            .field("after_count", &self.after.len())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentHouseholdConsequenceV1 {
    ConversationContinuityReset,
    ActiveScopeChanged,
    MemberArchived,
    MemberRestored,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentHouseholdRecoverabilityV1 {
    EditableBeforeSave,
    ReversibleArchive,
    NewProposalRequiredAfterSave,
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
    pub changes: Vec<AgentHouseholdChangeV1>,
    pub consequences: Vec<AgentHouseholdConsequenceV1>,
    pub recoverability: AgentHouseholdRecoverabilityV1,
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
            .field("change_count", &self.changes.len())
            .field("consequence_count", &self.consequences.len())
            .field("recoverability", &self.recoverability)
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
            changes: Vec::new(),
            consequences: Vec::new(),
            recoverability: AgentHouseholdRecoverabilityV1::NewProposalRequiredAfterSave,
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
                changes: self
                    .changes
                    .into_iter()
                    .filter(|change| !change.field.is_profile())
                    .collect(),
                ..self
            },
            AgentHouseholdProjectionV1::Profile => Self { projection, ..self },
        }
    }

    #[must_use]
    pub fn has_canonical_copy(&self) -> bool {
        self.human_status == self.state.human_status()
            && self.handoff_command == "heyfood"
            && self.handoff_instruction
                == "Open `/household changes` to review this change locally."
    }
}

#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentHouseholdOutcomeReceiptV1 {
    schema_version: u16,
    proposal_ref: AgentHouseholdProposalIdV1,
    state: AgentHouseholdProposalStateV1,
    household_revision_before: HouseholdRevision,
    household_revision_after: Option<HouseholdRevision>,
    known_no_household_mutation: bool,
    retry_class: AgentHouseholdRetryClassV1,
    next_action: AgentHouseholdNextActionV1,
}

impl<'de> Deserialize<'de> for AgentHouseholdOutcomeReceiptV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            schema_version: u16,
            proposal_ref: AgentHouseholdProposalIdV1,
            state: AgentHouseholdProposalStateV1,
            household_revision_before: HouseholdRevision,
            household_revision_after: Option<HouseholdRevision>,
            known_no_household_mutation: bool,
            retry_class: AgentHouseholdRetryClassV1,
            next_action: AgentHouseholdNextActionV1,
        }

        let wire = Wire::deserialize(deserializer)?;
        let value = Self {
            schema_version: wire.schema_version,
            proposal_ref: wire.proposal_ref,
            state: wire.state,
            household_revision_before: wire.household_revision_before,
            household_revision_after: wire.household_revision_after,
            known_no_household_mutation: wire.known_no_household_mutation,
            retry_class: wire.retry_class,
            next_action: wire.next_action,
        };
        if value.schema_version == AGENT_HOUSEHOLD_CONTRACT_VERSION && value.is_valid() {
            Ok(value)
        } else {
            Err(serde::de::Error::custom(
                "invalid household outcome receipt invariant",
            ))
        }
    }
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

impl AgentHouseholdOutcomeReceiptV1 {
    pub fn committed(
        proposal_ref: AgentHouseholdProposalIdV1,
        before: HouseholdRevision,
        after: HouseholdRevision,
    ) -> Result<Self, AgentHouseholdContractErrorV1> {
        if before.checked_next().ok() != Some(after) {
            return Err(AgentHouseholdContractErrorV1::InvalidOutcomeReceipt);
        }
        Ok(Self::fixed(
            proposal_ref,
            AgentHouseholdProposalStateV1::Committed,
            before,
            Some(after),
            false,
            AgentHouseholdRetryClassV1::NotApplicable,
            AgentHouseholdNextActionV1::None,
        ))
    }

    #[must_use]
    pub fn cancelled(
        proposal_ref: AgentHouseholdProposalIdV1,
        revision: HouseholdRevision,
    ) -> Self {
        Self::no_mutation(
            proposal_ref,
            AgentHouseholdProposalStateV1::Cancelled,
            revision,
            AgentHouseholdNextActionV1::None,
        )
    }

    #[must_use]
    pub fn expired(proposal_ref: AgentHouseholdProposalIdV1, revision: HouseholdRevision) -> Self {
        Self::no_mutation(
            proposal_ref,
            AgentHouseholdProposalStateV1::Expired,
            revision,
            AgentHouseholdNextActionV1::PrepareFresh,
        )
    }

    #[must_use]
    pub fn stale(proposal_ref: AgentHouseholdProposalIdV1, revision: HouseholdRevision) -> Self {
        Self::no_mutation(
            proposal_ref,
            AgentHouseholdProposalStateV1::Stale,
            revision,
            AgentHouseholdNextActionV1::PrepareFresh,
        )
    }

    #[must_use]
    pub fn rejected(proposal_ref: AgentHouseholdProposalIdV1, revision: HouseholdRevision) -> Self {
        Self::no_mutation(
            proposal_ref,
            AgentHouseholdProposalStateV1::Rejected,
            revision,
            AgentHouseholdNextActionV1::PrepareFresh,
        )
    }

    #[must_use]
    pub fn proven_uncommitted(
        proposal_ref: AgentHouseholdProposalIdV1,
        revision: HouseholdRevision,
    ) -> Self {
        Self::no_mutation(
            proposal_ref,
            AgentHouseholdProposalStateV1::ProvenUncommitted,
            revision,
            AgentHouseholdNextActionV1::PrepareFresh,
        )
    }

    #[must_use]
    pub fn reconciliation_required(
        proposal_ref: AgentHouseholdProposalIdV1,
        before: HouseholdRevision,
    ) -> Self {
        Self::fixed(
            proposal_ref,
            AgentHouseholdProposalStateV1::ReconciliationRequired,
            before,
            None,
            false,
            AgentHouseholdRetryClassV1::ReconcileBeforeRetry,
            AgentHouseholdNextActionV1::Reconcile,
        )
    }

    #[must_use]
    pub const fn proposal_ref(&self) -> AgentHouseholdProposalIdV1 {
        self.proposal_ref
    }

    #[must_use]
    pub const fn state(&self) -> AgentHouseholdProposalStateV1 {
        self.state
    }

    #[must_use]
    pub const fn household_revision_before(&self) -> HouseholdRevision {
        self.household_revision_before
    }

    #[must_use]
    pub const fn household_revision_after(&self) -> Option<HouseholdRevision> {
        self.household_revision_after
    }

    #[must_use]
    pub const fn known_no_household_mutation(&self) -> bool {
        self.known_no_household_mutation
    }

    #[must_use]
    pub const fn retry_class(&self) -> AgentHouseholdRetryClassV1 {
        self.retry_class
    }

    #[must_use]
    pub const fn next_action(&self) -> AgentHouseholdNextActionV1 {
        self.next_action
    }

    #[must_use]
    pub fn is_valid(&self) -> bool {
        match self.state {
            AgentHouseholdProposalStateV1::Committed => self
                .household_revision_before
                .checked_next()
                .ok()
                .is_some_and(|next| {
                    self.household_revision_after == Some(next)
                        && !self.known_no_household_mutation
                        && self.retry_class == AgentHouseholdRetryClassV1::NotApplicable
                        && self.next_action == AgentHouseholdNextActionV1::None
                }),
            AgentHouseholdProposalStateV1::Cancelled => {
                self.valid_no_mutation(AgentHouseholdNextActionV1::None)
            }
            AgentHouseholdProposalStateV1::Expired
            | AgentHouseholdProposalStateV1::Stale
            | AgentHouseholdProposalStateV1::Rejected
            | AgentHouseholdProposalStateV1::ProvenUncommitted => {
                self.valid_no_mutation(AgentHouseholdNextActionV1::PrepareFresh)
            }
            AgentHouseholdProposalStateV1::ReconciliationRequired => {
                self.household_revision_after.is_none()
                    && !self.known_no_household_mutation
                    && self.retry_class == AgentHouseholdRetryClassV1::ReconcileBeforeRetry
                    && self.next_action == AgentHouseholdNextActionV1::Reconcile
            }
            _ => false,
        }
    }

    fn no_mutation(
        proposal_ref: AgentHouseholdProposalIdV1,
        state: AgentHouseholdProposalStateV1,
        revision: HouseholdRevision,
        next_action: AgentHouseholdNextActionV1,
    ) -> Self {
        Self::fixed(
            proposal_ref,
            state,
            revision,
            Some(revision),
            true,
            AgentHouseholdRetryClassV1::NotApplicable,
            next_action,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn fixed(
        proposal_ref: AgentHouseholdProposalIdV1,
        state: AgentHouseholdProposalStateV1,
        household_revision_before: HouseholdRevision,
        household_revision_after: Option<HouseholdRevision>,
        known_no_household_mutation: bool,
        retry_class: AgentHouseholdRetryClassV1,
        next_action: AgentHouseholdNextActionV1,
    ) -> Self {
        Self {
            schema_version: AGENT_HOUSEHOLD_CONTRACT_VERSION,
            proposal_ref,
            state,
            household_revision_before,
            household_revision_after,
            known_no_household_mutation,
            retry_class,
            next_action,
        }
    }

    fn valid_no_mutation(&self, next_action: AgentHouseholdNextActionV1) -> bool {
        self.household_revision_after == Some(self.household_revision_before)
            && self.known_no_household_mutation
            && self.retry_class == AgentHouseholdRetryClassV1::NotApplicable
            && self.next_action == next_action
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct LocalHouseholdProposalBindingV1 {
    pub account: AccountId,
    pub proposal_ref: AgentHouseholdProposalIdV1,
    pub operation: AgentHouseholdOperationV1,
    pub disclosure_generation: GenerationId,
    pub disclosure_grant_set_digest: CanonicalDigestV1,
    pub disclosure_purpose: AgentDisclosurePurposeV1,
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
            .field("disclosure_purpose", &self.disclosure_purpose)
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
    pub disclosure_grant_set_digest: CanonicalDigestV1,
    pub disclosure_purpose: AgentDisclosurePurposeV1,
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
            .field("disclosure_purpose", &self.disclosure_purpose)
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
        if self.binding.disclosure_grant_set_digest != current.disclosure_grant_set_digest
            || self.binding.disclosure_purpose != current.disclosure_purpose
        {
            return Err(AgentHouseholdContractErrorV1::DisclosureGrantChanged);
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
    InvalidDisclosureGrant,
    InvalidDisclosureGrantSet,
    InvalidOutcomeReceipt,
    AccountChanged,
    DisclosureGenerationChanged,
    DisclosureGrantChanged,
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
            Self::InvalidDisclosureGrant => "household disclosure grant is invalid",
            Self::InvalidDisclosureGrantSet => "household disclosure grant set is invalid",
            Self::InvalidOutcomeReceipt => "household outcome receipt is invalid",
            Self::AccountChanged => "household proposal account changed",
            Self::DisclosureGenerationChanged => "household agent disclosure generation changed",
            Self::DisclosureGrantChanged => "household disclosure grant authority changed",
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
    let mut output = String::with_capacity(value.len().saturating_add(2));
    output.push('"');
    for character in value.chars() {
        match character {
            'a'..='z'
            | 'A'..='Z'
            | '0'..='9'
            | ' '
            | '-'
            | '_'
            | '.'
            | ','
            | '\''
            | '('
            | ')'
            | '['
            | ']' => output.push(character),
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            _ => {
                use std::fmt::Write as _;
                let _ = write!(output, "\\u{{{:04X}}}", character as u32);
            }
        }
    }
    output.push('"');
    output
}

fn agent_subject_kind(subject: &AgentHouseholdSubjectV1) -> &'static str {
    match subject {
        AgentHouseholdSubjectV1::Self_ => "self",
        AgentHouseholdSubjectV1::Member(_) => "member",
        AgentHouseholdSubjectV1::Everyone => "everyone",
    }
}

fn scope_kind(scope: &HouseholdScope) -> &'static str {
    match scope {
        HouseholdScope::Subject(crate::HouseholdSubjectId::Self_) => "self",
        HouseholdScope::Subject(crate::HouseholdSubjectId::Member(_)) => "member",
        HouseholdScope::Everyone => "everyone",
    }
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
            disclosure_grant_set_digest: CanonicalDigestV1::from_bytes([8; 32]),
            disclosure_purpose: AgentDisclosurePurposeV1::HouseholdAgentProposalStatus,
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
            disclosure_grant_set_digest: CanonicalDigestV1::from_bytes([8; 32]),
            disclosure_purpose: AgentDisclosurePurposeV1::HouseholdAgentProposalStatus,
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
            disclosure_grant_set_digest: CanonicalDigestV1::from_bytes([8; 32]),
            disclosure_purpose: AgentDisclosurePurposeV1::HouseholdAgentProposalStatus,
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
            changes: vec![AgentHouseholdChangeV1 {
                field: AgentHouseholdChangeFieldV1::Allergies,
                before: vec!["milk".to_owned()],
                after: vec!["milk".to_owned(), "egg".to_owned()],
            }],
            consequences: vec![AgentHouseholdConsequenceV1::ConversationContinuityReset],
            recoverability: AgentHouseholdRecoverabilityV1::EditableBeforeSave,
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
        assert!(filtered.changes.is_empty());
        assert_eq!(filtered.handoff_command, "heyfood");
    }

    #[test]
    fn disclosure_grants_keep_roster_and_profile_authority_separate() {
        let account = AccountId::parse("phase0-disclosure-account").expect("account");
        let adult_subject = AgentDisclosureGrantSubjectV1::Self_;
        let minor_subject = AgentDisclosureGrantSubjectV1::Member(MemberId::new());
        let observed_at = timestamp();
        let purpose = AgentDisclosurePurposeV1::HouseholdAgentRead;
        let adult = AgentDisclosureGrantV1::new(
            account.clone(),
            adult_subject.clone(),
            MinorStatusV1::Adult,
            vec![
                AgentDisclosureDataClassV1::Roster,
                AgentDisclosureDataClassV1::MinimizedDeclaredProfile,
            ],
            purpose,
            AgentDisclosureGrantingAuthorityV1::AccountOwnerAdultAuthorization,
            1,
            GenerationId::new(3),
            AgentDisclosureGrantStateV1::Active,
            timestamp(),
            None,
        )
        .expect("adult grant");
        assert!(adult.permits_for(
            &account,
            &adult_subject,
            GenerationId::new(3),
            purpose,
            AgentDisclosureDataClassV1::Roster,
            &observed_at,
        ));
        assert!(adult.permits_for(
            &account,
            &adult_subject,
            GenerationId::new(3),
            purpose,
            AgentDisclosureDataClassV1::MinimizedDeclaredProfile,
            &observed_at,
        ));

        let minor = AgentDisclosureGrantV1::new(
            account.clone(),
            minor_subject,
            MinorStatusV1::Minor,
            vec![AgentDisclosureDataClassV1::Roster],
            purpose,
            AgentDisclosureGrantingAuthorityV1::AuthorizedGuardianRosterOnly,
            2,
            GenerationId::new(3),
            AgentDisclosureGrantStateV1::Active,
            timestamp(),
            None,
        )
        .expect("minor roster grant");
        assert!(minor.permits_for(
            &account,
            minor.subject(),
            GenerationId::new(3),
            purpose,
            AgentDisclosureDataClassV1::Roster,
            &observed_at,
        ));
        assert!(!minor.permits_for(
            &account,
            minor.subject(),
            GenerationId::new(3),
            purpose,
            AgentDisclosureDataClassV1::MinimizedDeclaredProfile,
            &observed_at,
        ));

        let revoked = AgentDisclosureGrantV1 {
            state: AgentDisclosureGrantStateV1::Revoked,
            ..adult.clone()
        };
        assert!(!revoked.permits_for(
            &account,
            revoked.subject(),
            GenerationId::new(3),
            purpose,
            AgentDisclosureDataClassV1::Roster,
            &observed_at,
        ));
        assert!(!revoked.permits_for(
            &account,
            revoked.subject(),
            GenerationId::new(3),
            purpose,
            AgentDisclosureDataClassV1::MinimizedDeclaredProfile,
            &observed_at,
        ));
        let other_account = AccountId::parse("other-disclosure-account").expect("account");
        assert!(!minor.permits_for(
            &other_account,
            minor.subject(),
            GenerationId::new(3),
            purpose,
            AgentDisclosureDataClassV1::Roster,
            &observed_at,
        ));

        assert_eq!(
            AgentDisclosureGrantV1::new(
                account.clone(),
                AgentDisclosureGrantSubjectV1::Self_,
                MinorStatusV1::Adult,
                vec![AgentDisclosureDataClassV1::Roster],
                purpose,
                AgentDisclosureGrantingAuthorityV1::AuthorizedGuardianRosterOnly,
                1,
                GenerationId::new(3),
                AgentDisclosureGrantStateV1::Active,
                timestamp(),
                None,
            ),
            Err(AgentHouseholdContractErrorV1::InvalidDisclosureGrant)
        );

        let expired = AgentDisclosureGrantV1 {
            expires_at: Some(
                CanonicalTimestampV1::parse("2026-08-02T12:01:00.000Z").expect("expiry"),
            ),
            ..minor.clone()
        };
        let after_expiry =
            CanonicalTimestampV1::parse("2026-08-02T12:01:00.000Z").expect("observation");
        assert!(!expired.permits_for(
            &account,
            expired.subject(),
            GenerationId::new(3),
            purpose,
            AgentDisclosureDataClassV1::Roster,
            &after_expiry,
        ));

        let set = AgentDisclosureGrantSetV1::new(
            account,
            GenerationId::new(3),
            purpose,
            observed_at,
            vec![adult, minor],
        )
        .expect("grant set");
        assert_eq!(
            set.maximum_projection_for(&[
                AgentDisclosureGrantSubjectV1::Self_,
                set.grants[1].subject().clone(),
            ]),
            AgentHouseholdProjectionV1::Roster
        );
        assert_eq!(
            set.maximum_projection_for(&[
                AgentDisclosureGrantSubjectV1::Self_,
                AgentDisclosureGrantSubjectV1::Member(MemberId::new()),
            ]),
            AgentHouseholdProjectionV1::ContentFree
        );
    }

    #[test]
    fn review_text_makes_control_and_directional_characters_visible() {
        let rendered = household_review_safe_text_v1("Julie\n\u{1b}[31m\u{202e}x\u{200b}");
        assert_eq!(
            rendered,
            "\"Julie\\u{000A}\\u{001B}[31m\\u{202E}x\\u{200B}\""
        );
        assert_ne!(
            household_review_safe_text_v1("<U+001B>"),
            household_review_safe_text_v1("\u{1b}")
        );
        assert!(!household_review_safe_text_v1("https://example.invalid").contains("https://"));
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
        let receipt = AgentHouseholdOutcomeReceiptV1::cancelled(
            AgentHouseholdProposalIdV1::from_uuid(
                Uuid::parse_str("20000000-0000-4000-8000-000000000001").expect("proposal"),
            ),
            revision,
        );
        let encoded = serde_json::to_string(&receipt).expect("receipt");
        assert!(!encoded.contains("account"));
        assert!(!encoded.contains("commit"));
        assert!(!encoded.contains("fingerprint"));

        for invalid in [
            json!({
                "schema_version": 1,
                "proposal_ref": "20000000-0000-4000-8000-000000000001",
                "state": "cancelled",
                "household_revision_before": 7,
                "household_revision_after": 8,
                "known_no_household_mutation": true,
                "retry_class": "not_applicable",
                "next_action": "none"
            }),
            json!({
                "schema_version": 1,
                "proposal_ref": "20000000-0000-4000-8000-000000000001",
                "state": "reconciliation_required",
                "household_revision_before": 7,
                "household_revision_after": null,
                "known_no_household_mutation": true,
                "retry_class": "safe_read",
                "next_action": "none"
            }),
        ] {
            assert!(
                serde_json::from_value::<AgentHouseholdOutcomeReceiptV1>(invalid).is_err(),
                "invalid outcome invariant was accepted"
            );
        }
    }
}
