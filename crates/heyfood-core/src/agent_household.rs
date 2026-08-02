//! Phase-0-only household agent contracts.
//!
//! These types freeze the local disclosure, proposal, and review state model.
//! They are deliberately not wired to CLI or MCP routes in Phase 0.

use std::{collections::BTreeSet, fmt};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _, ser::Error as _};
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

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
        hash_length_prefixed(&mut hasher, account.as_str().as_bytes());
        hasher.update(generation.get().to_be_bytes());
        hasher.update([purpose as u8]);
        for grant in &grants {
            let subject = grant.subject.digest_bytes();
            hash_length_prefixed(&mut hasher, &subject);
            hasher.update([grant.subject_minor_status as u8]);
            hasher.update((grant.data_classes.len() as u64).to_be_bytes());
            for data_class in &grant.data_classes {
                hasher.update([*data_class as u8]);
            }
            hasher.update([grant.purpose as u8]);
            hasher.update([grant.granting_authority as u8]);
            hasher.update(grant.revision.to_be_bytes());
            hasher.update(grant.generation.get().to_be_bytes());
            hasher.update([grant.state as u8]);
            hash_length_prefixed(&mut hasher, grant.issued_at.as_str().as_bytes());
            match &grant.expires_at {
                Some(expires_at) => {
                    hasher.update([1]);
                    hash_length_prefixed(&mut hasher, expires_at.as_str().as_bytes());
                }
                None => hasher.update([0]),
            }
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

fn hash_length_prefixed(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentHouseholdProjectionV1 {
    ContentFree,
    Roster,
    Profile,
}

const fn projection_rank(projection: AgentHouseholdProjectionV1) -> u8 {
    match projection {
        AgentHouseholdProjectionV1::ContentFree => 0,
        AgentHouseholdProjectionV1::Roster => 1,
        AgentHouseholdProjectionV1::Profile => 2,
    }
}

fn bounded_wire_text(value: &str, maximum_characters: usize) -> bool {
    let character_count = value.chars().count();
    character_count > 0
        && character_count <= maximum_characters
        && !value
            .chars()
            .any(|character| matches!(character, '\u{0000}'..='\u{001f}' | '\u{007f}'))
}

fn bounded_wire_values(values: &[String], maximum_items: usize, maximum_characters: usize) -> bool {
    values.len() <= maximum_items
        && values
            .iter()
            .all(|value| bounded_wire_text(value, maximum_characters))
}

fn bounded_unique_wire_values(
    values: &[String],
    maximum_items: usize,
    maximum_characters: usize,
) -> bool {
    bounded_wire_values(values, maximum_items, maximum_characters)
        && values
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            .len()
            == values.len()
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

impl AgentHouseholdMemberProjectionV1 {
    fn validate_wire_shape(&self) -> Result<(), AgentHouseholdContractErrorV1> {
        if self.profile_schema_version == Some(0)
            || self
                .minimized_declared_profile
                .as_ref()
                .is_some_and(|profile| profile.validate_wire_shape().is_err())
        {
            Err(AgentHouseholdContractErrorV1::InvalidWireShape)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct AgentMinimizedDeclaredProfileV1 {
    pub diet_styles: Vec<String>,
    pub allergies: Vec<String>,
    pub restrictions: Vec<String>,
    pub health_conditions: Vec<String>,
    pub avoid_ingredients: Vec<String>,
}

impl Serialize for AgentMinimizedDeclaredProfileV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate_wire_shape().map_err(S::Error::custom)?;
        #[derive(Serialize)]
        struct Wire<'a> {
            diet_styles: &'a [String],
            allergies: &'a [String],
            restrictions: &'a [String],
            health_conditions: &'a [String],
            avoid_ingredients: &'a [String],
        }
        Wire {
            diet_styles: &self.diet_styles,
            allergies: &self.allergies,
            restrictions: &self.restrictions,
            health_conditions: &self.health_conditions,
            avoid_ingredients: &self.avoid_ingredients,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for AgentMinimizedDeclaredProfileV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            diet_styles: Vec<String>,
            allergies: Vec<String>,
            restrictions: Vec<String>,
            health_conditions: Vec<String>,
            avoid_ingredients: Vec<String>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let value = Self {
            diet_styles: wire.diet_styles,
            allergies: wire.allergies,
            restrictions: wire.restrictions,
            health_conditions: wire.health_conditions,
            avoid_ingredients: wire.avoid_ingredients,
        };
        value.validate_wire_shape().map_err(D::Error::custom)?;
        Ok(value)
    }
}

impl AgentMinimizedDeclaredProfileV1 {
    pub fn validate_wire_shape(&self) -> Result<(), AgentHouseholdContractErrorV1> {
        for values in [
            &self.diet_styles,
            &self.allergies,
            &self.restrictions,
            &self.health_conditions,
            &self.avoid_ingredients,
        ] {
            if !bounded_unique_wire_values(values, 64, 256) {
                return Err(AgentHouseholdContractErrorV1::InvalidWireShape);
            }
        }
        Ok(())
    }
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

#[derive(Clone, Eq, PartialEq)]
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

impl Serialize for AgentHouseholdReadSnapshotV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate_wire_shape().map_err(S::Error::custom)?;
        #[derive(Serialize)]
        struct Wire<'a> {
            schema_version: u16,
            kind: AgentHouseholdReadResultKindV1,
            projection: AgentHouseholdProjectionV1,
            resolved_subject: &'a Option<AgentHouseholdSubjectV1>,
            resolved_from_active_scope: bool,
            active_scope: &'a Option<HouseholdScope>,
            household_revision: HouseholdRevision,
            disclosure_generation: GenerationId,
            eligible_member_count: u16,
            restricted_member_count: u16,
            members: &'a [AgentHouseholdMemberProjectionV1],
            next_cursor: &'a Option<String>,
        }
        Wire {
            schema_version: self.schema_version,
            kind: self.kind,
            projection: self.projection,
            resolved_subject: &self.resolved_subject,
            resolved_from_active_scope: self.resolved_from_active_scope,
            active_scope: &self.active_scope,
            household_revision: self.household_revision,
            disclosure_generation: self.disclosure_generation,
            eligible_member_count: self.eligible_member_count,
            restricted_member_count: self.restricted_member_count,
            members: &self.members,
            next_cursor: &self.next_cursor,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for AgentHouseholdReadSnapshotV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            schema_version: u16,
            kind: AgentHouseholdReadResultKindV1,
            projection: AgentHouseholdProjectionV1,
            resolved_subject: Option<AgentHouseholdSubjectV1>,
            resolved_from_active_scope: bool,
            active_scope: Option<HouseholdScope>,
            household_revision: HouseholdRevision,
            disclosure_generation: GenerationId,
            eligible_member_count: u16,
            restricted_member_count: u16,
            members: Vec<AgentHouseholdMemberProjectionV1>,
            next_cursor: Option<String>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let value = Self {
            schema_version: wire.schema_version,
            kind: wire.kind,
            projection: wire.projection,
            resolved_subject: wire.resolved_subject,
            resolved_from_active_scope: wire.resolved_from_active_scope,
            active_scope: wire.active_scope,
            household_revision: wire.household_revision,
            disclosure_generation: wire.disclosure_generation,
            eligible_member_count: wire.eligible_member_count,
            restricted_member_count: wire.restricted_member_count,
            members: wire.members,
            next_cursor: wire.next_cursor,
        };
        value.validate_wire_shape().map_err(D::Error::custom)?;
        Ok(value)
    }
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

    pub fn validate_wire_shape(&self) -> Result<(), AgentHouseholdContractErrorV1> {
        if self.schema_version != AGENT_HOUSEHOLD_CONTRACT_VERSION
            || self.kind != AgentHouseholdReadResultKindV1::HouseholdReadResult
            || self.members.len() > usize::from(AGENT_HOUSEHOLD_MAX_MEMBERS_PER_PAGE)
            || self.eligible_member_count > AGENT_HOUSEHOLD_MAX_MEMBERS_PER_PAGE
            || self.restricted_member_count > AGENT_HOUSEHOLD_MAX_MEMBERS_PER_PAGE
            || self
                .next_cursor
                .as_ref()
                .is_some_and(|value| !bounded_wire_text(value, 512))
            || self
                .members
                .iter()
                .any(|member| member.validate_wire_shape().is_err())
        {
            return Err(AgentHouseholdContractErrorV1::InvalidWireShape);
        }
        match self.projection {
            AgentHouseholdProjectionV1::ContentFree
                if self.resolved_subject.is_some()
                    || self.active_scope.is_some()
                    || !self.members.is_empty()
                    || self.next_cursor.is_some() =>
            {
                Err(AgentHouseholdContractErrorV1::InvalidWireShape)
            }
            AgentHouseholdProjectionV1::Roster
                if self
                    .members
                    .iter()
                    .any(|member| member.minimized_declared_profile.is_some()) =>
            {
                Err(AgentHouseholdContractErrorV1::InvalidWireShape)
            }
            _ => Ok(()),
        }
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

#[derive(Clone, Eq, PartialEq)]
pub struct AgentHouseholdChangeV1 {
    pub field: AgentHouseholdChangeFieldV1,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

impl Serialize for AgentHouseholdChangeV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate_wire_shape().map_err(S::Error::custom)?;
        #[derive(Serialize)]
        struct Wire<'a> {
            field: AgentHouseholdChangeFieldV1,
            before: &'a [String],
            after: &'a [String],
        }
        Wire {
            field: self.field,
            before: &self.before,
            after: &self.after,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for AgentHouseholdChangeV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            field: AgentHouseholdChangeFieldV1,
            before: Vec<String>,
            after: Vec<String>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let value = Self {
            field: wire.field,
            before: wire.before,
            after: wire.after,
        };
        value.validate_wire_shape().map_err(D::Error::custom)?;
        Ok(value)
    }
}

impl AgentHouseholdChangeV1 {
    pub fn validate_wire_shape(&self) -> Result<(), AgentHouseholdContractErrorV1> {
        if bounded_wire_values(&self.before, 64, 256) && bounded_wire_values(&self.after, 64, 256) {
            Ok(())
        } else {
            Err(AgentHouseholdContractErrorV1::InvalidWireShape)
        }
    }
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

#[derive(Clone, Eq, PartialEq)]
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

impl Serialize for AgentHouseholdProposalPresentationV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate_wire_shape().map_err(S::Error::custom)?;
        #[derive(Serialize)]
        struct Wire<'a> {
            schema_version: u16,
            proposal_ref: AgentHouseholdProposalIdV1,
            operation: AgentHouseholdOperationV1,
            state: AgentHouseholdProposalStateV1,
            projection: AgentHouseholdProjectionV1,
            disclosure_generation: GenerationId,
            affected_member_ref: &'a Option<MemberId>,
            affected_member_label: &'a Option<DisplayName>,
            changes: &'a [AgentHouseholdChangeV1],
            consequences: &'a [AgentHouseholdConsequenceV1],
            recoverability: AgentHouseholdRecoverabilityV1,
            created_at: &'a CanonicalTimestampV1,
            expires_at: &'a CanonicalTimestampV1,
            human_status: &'a str,
            handoff_command: &'a str,
            handoff_instruction: &'a str,
        }
        Wire {
            schema_version: self.schema_version,
            proposal_ref: self.proposal_ref,
            operation: self.operation,
            state: self.state,
            projection: self.projection,
            disclosure_generation: self.disclosure_generation,
            affected_member_ref: &self.affected_member_ref,
            affected_member_label: &self.affected_member_label,
            changes: &self.changes,
            consequences: &self.consequences,
            recoverability: self.recoverability,
            created_at: &self.created_at,
            expires_at: &self.expires_at,
            human_status: &self.human_status,
            handoff_command: &self.handoff_command,
            handoff_instruction: &self.handoff_instruction,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for AgentHouseholdProposalPresentationV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            schema_version: u16,
            proposal_ref: AgentHouseholdProposalIdV1,
            operation: AgentHouseholdOperationV1,
            state: AgentHouseholdProposalStateV1,
            projection: AgentHouseholdProjectionV1,
            disclosure_generation: GenerationId,
            affected_member_ref: Option<MemberId>,
            affected_member_label: Option<DisplayName>,
            changes: Vec<AgentHouseholdChangeV1>,
            consequences: Vec<AgentHouseholdConsequenceV1>,
            recoverability: AgentHouseholdRecoverabilityV1,
            created_at: CanonicalTimestampV1,
            expires_at: CanonicalTimestampV1,
            human_status: String,
            handoff_command: String,
            handoff_instruction: String,
        }
        let wire = Wire::deserialize(deserializer)?;
        let value = Self {
            schema_version: wire.schema_version,
            proposal_ref: wire.proposal_ref,
            operation: wire.operation,
            state: wire.state,
            projection: wire.projection,
            disclosure_generation: wire.disclosure_generation,
            affected_member_ref: wire.affected_member_ref,
            affected_member_label: wire.affected_member_label,
            changes: wire.changes,
            consequences: wire.consequences,
            recoverability: wire.recoverability,
            created_at: wire.created_at,
            expires_at: wire.expires_at,
            human_status: wire.human_status,
            handoff_command: wire.handoff_command,
            handoff_instruction: wire.handoff_instruction,
        };
        value.validate_wire_shape().map_err(D::Error::custom)?;
        Ok(value)
    }
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

    pub fn validate_wire_shape(&self) -> Result<(), AgentHouseholdContractErrorV1> {
        let mut consequences = self.consequences.clone();
        consequences.sort_by_key(|value| *value as u8);
        let duplicate_consequence = consequences.windows(2).any(|pair| pair[0] == pair[1]);
        if self.schema_version != AGENT_HOUSEHOLD_CONTRACT_VERSION
            || self.changes.len() > 128
            || self
                .changes
                .iter()
                .any(|change| change.validate_wire_shape().is_err())
            || self.consequences.len() > 8
            || duplicate_consequence
            || !bounded_wire_text(&self.human_status, 160)
            || !self.has_canonical_copy()
        {
            return Err(AgentHouseholdContractErrorV1::InvalidWireShape);
        }
        match self.projection {
            AgentHouseholdProjectionV1::ContentFree
                if self.affected_member_ref.is_some()
                    || self.affected_member_label.is_some()
                    || !self.changes.is_empty()
                    || !self.consequences.is_empty() =>
            {
                Err(AgentHouseholdContractErrorV1::InvalidWireShape)
            }
            AgentHouseholdProjectionV1::Roster
                if self.changes.iter().any(|change| change.field.is_profile()) =>
            {
                Err(AgentHouseholdContractErrorV1::InvalidWireShape)
            }
            _ => Ok(()),
        }
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
    account: AccountId,
    proposal_ref: AgentHouseholdProposalIdV1,
    operation: AgentHouseholdOperationV1,
    disclosure_generation: GenerationId,
    disclosure_grant_set_digest: CanonicalDigestV1,
    disclosure_purpose: AgentDisclosurePurposeV1,
    lifecycle_generation: GenerationId,
    projection: AgentHouseholdProjectionV1,
    expected_household_revision: HouseholdRevision,
    expected_profile_revision: Option<ProfileRevision>,
    commit_id: CommitId,
    member_id: Option<MemberId>,
    previous_scope: HouseholdScope,
    originating_session_digest: CanonicalDigestV1,
    eligible_host_policy_digest: CanonicalDigestV1,
    created_at: CanonicalTimestampV1,
    expires_at: CanonicalTimestampV1,
}

impl LocalHouseholdProposalBindingV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        account: AccountId,
        proposal_ref: AgentHouseholdProposalIdV1,
        operation: AgentHouseholdOperationV1,
        disclosure_generation: GenerationId,
        disclosure_grant_set_digest: CanonicalDigestV1,
        disclosure_purpose: AgentDisclosurePurposeV1,
        lifecycle_generation: GenerationId,
        projection: AgentHouseholdProjectionV1,
        expected_household_revision: HouseholdRevision,
        expected_profile_revision: Option<ProfileRevision>,
        commit_id: CommitId,
        member_id: Option<MemberId>,
        previous_scope: HouseholdScope,
        originating_session_digest: CanonicalDigestV1,
        eligible_host_policy_digest: CanonicalDigestV1,
        created_at: CanonicalTimestampV1,
        expires_at: CanonicalTimestampV1,
    ) -> Result<Self, AgentHouseholdContractErrorV1> {
        let member_shape_is_valid = match operation {
            AgentHouseholdOperationV1::Add
            | AgentHouseholdOperationV1::Edit
            | AgentHouseholdOperationV1::Archive
            | AgentHouseholdOperationV1::Restore => member_id.is_some(),
            AgentHouseholdOperationV1::Scope => member_id.is_none(),
        };
        let projection_is_valid = match operation {
            AgentHouseholdOperationV1::Add | AgentHouseholdOperationV1::Scope => {
                projection == AgentHouseholdProjectionV1::ContentFree
            }
            AgentHouseholdOperationV1::Edit
            | AgentHouseholdOperationV1::Archive
            | AgentHouseholdOperationV1::Restore => true,
        };
        if expires_at <= created_at
            || disclosure_purpose != AgentDisclosurePurposeV1::HouseholdAgentProposalStatus
            || !member_shape_is_valid
            || !projection_is_valid
        {
            return Err(AgentHouseholdContractErrorV1::InvalidJournal);
        }
        Ok(Self {
            account,
            proposal_ref,
            operation,
            disclosure_generation,
            disclosure_grant_set_digest,
            disclosure_purpose,
            lifecycle_generation,
            projection,
            expected_household_revision,
            expected_profile_revision,
            commit_id,
            member_id,
            previous_scope,
            originating_session_digest,
            eligible_host_policy_digest,
            created_at,
            expires_at,
        })
    }

    #[must_use]
    pub const fn proposal_ref(&self) -> AgentHouseholdProposalIdV1 {
        self.proposal_ref
    }

    #[must_use]
    pub const fn commit_id(&self) -> CommitId {
        self.commit_id
    }
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
    proposal_digest: CanonicalDigestV1,
    effect_fingerprint: HouseholdEffectFingerprintV1,
    before_document_digest: CanonicalDigestV1,
    after_document_digest: CanonicalDigestV1,
    resulting_scope: HouseholdScope,
    conversation_continuity_reset: bool,
    frozen_semantic_timestamp: CanonicalTimestampV1,
}

impl LocalHouseholdFrozenCandidateV1 {
    #[must_use]
    pub fn new(
        proposal_digest: CanonicalDigestV1,
        effect_fingerprint: HouseholdEffectFingerprintV1,
        before_document_digest: CanonicalDigestV1,
        after_document_digest: CanonicalDigestV1,
        resulting_scope: HouseholdScope,
        conversation_continuity_reset: bool,
        frozen_semantic_timestamp: CanonicalTimestampV1,
    ) -> Self {
        Self {
            proposal_digest,
            effect_fingerprint,
            before_document_digest,
            after_document_digest,
            resulting_scope,
            conversation_continuity_reset,
            frozen_semantic_timestamp,
        }
    }

    #[must_use]
    pub const fn proposal_digest(&self) -> CanonicalDigestV1 {
        self.proposal_digest
    }

    #[must_use]
    pub const fn effect_fingerprint(&self) -> HouseholdEffectFingerprintV1 {
        self.effect_fingerprint
    }
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
    account: AccountId,
    disclosure_generation: GenerationId,
    disclosure_grant_set_digest: CanonicalDigestV1,
    disclosure_purpose: AgentDisclosurePurposeV1,
    maximum_projection: AgentHouseholdProjectionV1,
    lifecycle_generation: GenerationId,
    household_revision: HouseholdRevision,
    profile_revision: Option<ProfileRevision>,
    observed_at: CanonicalTimestampV1,
}

impl LocalHouseholdAuthoritySnapshotV1 {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        account: AccountId,
        disclosure_generation: GenerationId,
        disclosure_grant_set_digest: CanonicalDigestV1,
        disclosure_purpose: AgentDisclosurePurposeV1,
        maximum_projection: AgentHouseholdProjectionV1,
        lifecycle_generation: GenerationId,
        household_revision: HouseholdRevision,
        profile_revision: Option<ProfileRevision>,
        observed_at: CanonicalTimestampV1,
    ) -> Self {
        Self {
            account,
            disclosure_generation,
            disclosure_grant_set_digest,
            disclosure_purpose,
            maximum_projection,
            lifecycle_generation,
            household_revision,
            profile_revision,
            observed_at,
        }
    }
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
    binding: LocalHouseholdProposalBindingV1,
    state: AgentHouseholdProposalStateV1,
    proposal_generation: GenerationId,
    frozen: Option<LocalHouseholdFrozenCandidateV1>,
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

    #[must_use]
    pub const fn state(&self) -> AgentHouseholdProposalStateV1 {
        self.state
    }

    #[must_use]
    pub const fn proposal_generation(&self) -> GenerationId {
        self.proposal_generation
    }

    #[must_use]
    pub fn frozen(&self) -> Option<&LocalHouseholdFrozenCandidateV1> {
        self.frozen.as_ref()
    }

    #[must_use]
    pub const fn binding(&self) -> &LocalHouseholdProposalBindingV1 {
        &self.binding
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
        if projection_rank(current.maximum_projection) < projection_rank(self.binding.projection) {
            return Err(AgentHouseholdContractErrorV1::DisclosureProjectionChanged);
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

/// Opaque compare-and-swap token obtained from one exact durable journal
/// revision. Callers cannot construct or modify its fields.
#[derive(Clone, Eq, PartialEq)]
pub struct LocalHouseholdProposalCasTokenV1 {
    journal_revision: u64,
    state: AgentHouseholdProposalStateV1,
    proposal_generation: GenerationId,
    proposal_digest: Option<CanonicalDigestV1>,
}

impl fmt::Debug for LocalHouseholdProposalCasTokenV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalHouseholdProposalCasTokenV1")
            .field("journal_revision", &self.journal_revision)
            .field("state", &self.state)
            .field("proposal_generation", &self.proposal_generation)
            .field("has_proposal_digest", &self.proposal_digest.is_some())
            .finish_non_exhaustive()
    }
}

/// Closed durable proposal-journal record. Persistence is only available via
/// validated bytes; authority and frozen candidate fields remain private.
#[derive(Clone, Eq, PartialEq)]
pub struct LocalHouseholdProposalJournalV1 {
    journal_revision: u64,
    authority: LocalHouseholdProposalAuthorityV1,
}

impl fmt::Debug for LocalHouseholdProposalJournalV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalHouseholdProposalJournalV1")
            .field("journal_revision", &self.journal_revision)
            .field("authority", &self.authority)
            .finish_non_exhaustive()
    }
}

impl LocalHouseholdProposalJournalV1 {
    #[must_use]
    pub fn new(authority: LocalHouseholdProposalAuthorityV1) -> Self {
        Self {
            journal_revision: 1,
            authority,
        }
    }

    #[must_use]
    pub fn cas_token(&self) -> LocalHouseholdProposalCasTokenV1 {
        LocalHouseholdProposalCasTokenV1 {
            journal_revision: self.journal_revision,
            state: self.authority.state,
            proposal_generation: self.authority.proposal_generation,
            proposal_digest: self
                .authority
                .frozen
                .as_ref()
                .map(|frozen| frozen.proposal_digest),
        }
    }

    #[must_use]
    pub const fn state(&self) -> AgentHouseholdProposalStateV1 {
        self.authority.state
    }

    #[must_use]
    pub const fn proposal_ref(&self) -> AgentHouseholdProposalIdV1 {
        self.authority.binding.proposal_ref
    }

    #[must_use]
    pub const fn proposal_generation(&self) -> GenerationId {
        self.authority.proposal_generation
    }

    #[must_use]
    pub fn frozen_candidate(&self) -> Option<&LocalHouseholdFrozenCandidateV1> {
        self.authority.frozen.as_ref()
    }

    pub fn freeze_for_review(
        &mut self,
        expected: &LocalHouseholdProposalCasTokenV1,
        current: &LocalHouseholdAuthoritySnapshotV1,
        frozen: LocalHouseholdFrozenCandidateV1,
    ) -> Result<(), AgentHouseholdContractErrorV1> {
        self.apply_cas(expected, |authority| {
            authority.freeze_for_review(current, frozen)
        })
    }

    pub fn begin_commit(
        &mut self,
        expected: &LocalHouseholdProposalCasTokenV1,
        current: &LocalHouseholdAuthoritySnapshotV1,
        expected_proposal_digest: CanonicalDigestV1,
    ) -> Result<(), AgentHouseholdContractErrorV1> {
        self.apply_cas(expected, |authority| {
            authority.begin_commit(
                current,
                expected.proposal_generation,
                expected_proposal_digest,
            )
        })
    }

    pub fn cancel_before_commit(
        &mut self,
        expected: &LocalHouseholdProposalCasTokenV1,
    ) -> Result<(), AgentHouseholdContractErrorV1> {
        self.apply_cas(
            expected,
            LocalHouseholdProposalAuthorityV1::cancel_before_commit,
        )
    }

    pub fn mark_reconciliation_required(
        &mut self,
        expected: &LocalHouseholdProposalCasTokenV1,
    ) -> Result<(), AgentHouseholdContractErrorV1> {
        self.apply_cas(
            expected,
            LocalHouseholdProposalAuthorityV1::mark_reconciliation_required,
        )
    }

    pub fn reconcile_applied_commit(
        &mut self,
        expected: &LocalHouseholdProposalCasTokenV1,
        commit_id: CommitId,
        fingerprint: HouseholdEffectFingerprintV1,
    ) -> Result<(), AgentHouseholdContractErrorV1> {
        self.ensure_cas(expected)?;
        let frozen = self
            .authority
            .frozen
            .as_ref()
            .ok_or(AgentHouseholdContractErrorV1::MissingFrozenAuthority)?;
        if self.authority.binding.commit_id != commit_id || frozen.effect_fingerprint != fingerprint
        {
            return Err(AgentHouseholdContractErrorV1::AppliedCommitMismatch);
        }
        let mut replacement = self.authority.clone();
        match replacement.state {
            AgentHouseholdProposalStateV1::Committing => replacement.mark_committed()?,
            AgentHouseholdProposalStateV1::ReconciliationRequired => {
                replacement.reconcile_committed()?
            }
            _ => return Err(AgentHouseholdContractErrorV1::InvalidTransition),
        }
        self.commit_replacement(replacement)
    }

    pub fn persisted_bytes(&self) -> Result<Vec<u8>, AgentHouseholdContractErrorV1> {
        serde_json::to_vec(&JournalWireV1::from(self))
            .map_err(|_| AgentHouseholdContractErrorV1::InvalidJournal)
    }

    pub fn restore(bytes: &[u8]) -> Result<Self, AgentHouseholdContractErrorV1> {
        let wire: JournalWireV1 = serde_json::from_slice(bytes)
            .map_err(|_| AgentHouseholdContractErrorV1::InvalidJournal)?;
        wire.try_into()
    }

    fn apply_cas(
        &mut self,
        expected: &LocalHouseholdProposalCasTokenV1,
        transition: impl FnOnce(
            &mut LocalHouseholdProposalAuthorityV1,
        ) -> Result<(), AgentHouseholdContractErrorV1>,
    ) -> Result<(), AgentHouseholdContractErrorV1> {
        self.ensure_cas(expected)?;
        let mut replacement = self.authority.clone();
        transition(&mut replacement)?;
        self.commit_replacement(replacement)
    }

    fn ensure_cas(
        &self,
        expected: &LocalHouseholdProposalCasTokenV1,
    ) -> Result<(), AgentHouseholdContractErrorV1> {
        if &self.cas_token() == expected {
            Ok(())
        } else {
            Err(AgentHouseholdContractErrorV1::JournalCasChanged)
        }
    }

    fn commit_replacement(
        &mut self,
        replacement: LocalHouseholdProposalAuthorityV1,
    ) -> Result<(), AgentHouseholdContractErrorV1> {
        self.journal_revision = self
            .journal_revision
            .checked_add(1)
            .ok_or(AgentHouseholdContractErrorV1::InvalidJournal)?;
        self.authority = replacement;
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalWireV1 {
    schema_version: u16,
    journal_revision: u64,
    binding: BindingWireV1,
    state: AgentHouseholdProposalStateV1,
    proposal_generation: GenerationId,
    frozen: Option<FrozenWireV1>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BindingWireV1 {
    account: AccountId,
    proposal_ref: AgentHouseholdProposalIdV1,
    operation: AgentHouseholdOperationV1,
    disclosure_generation: GenerationId,
    disclosure_grant_set_digest: CanonicalDigestV1,
    disclosure_purpose: AgentDisclosurePurposeV1,
    lifecycle_generation: GenerationId,
    projection: AgentHouseholdProjectionV1,
    expected_household_revision: HouseholdRevision,
    expected_profile_revision: Option<ProfileRevision>,
    commit_id: CommitId,
    member_id: Option<MemberId>,
    previous_scope: HouseholdScope,
    originating_session_digest: CanonicalDigestV1,
    eligible_host_policy_digest: CanonicalDigestV1,
    created_at: CanonicalTimestampV1,
    expires_at: CanonicalTimestampV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenWireV1 {
    proposal_digest: CanonicalDigestV1,
    effect_fingerprint: HouseholdEffectFingerprintV1,
    before_document_digest: CanonicalDigestV1,
    after_document_digest: CanonicalDigestV1,
    resulting_scope: HouseholdScope,
    conversation_continuity_reset: bool,
    frozen_semantic_timestamp: CanonicalTimestampV1,
}

impl From<&LocalHouseholdProposalJournalV1> for JournalWireV1 {
    fn from(value: &LocalHouseholdProposalJournalV1) -> Self {
        let binding = &value.authority.binding;
        Self {
            schema_version: AGENT_HOUSEHOLD_CONTRACT_VERSION,
            journal_revision: value.journal_revision,
            binding: BindingWireV1 {
                account: binding.account.clone(),
                proposal_ref: binding.proposal_ref,
                operation: binding.operation,
                disclosure_generation: binding.disclosure_generation,
                disclosure_grant_set_digest: binding.disclosure_grant_set_digest,
                disclosure_purpose: binding.disclosure_purpose,
                lifecycle_generation: binding.lifecycle_generation,
                projection: binding.projection,
                expected_household_revision: binding.expected_household_revision,
                expected_profile_revision: binding.expected_profile_revision,
                commit_id: binding.commit_id,
                member_id: binding.member_id.clone(),
                previous_scope: binding.previous_scope.clone(),
                originating_session_digest: binding.originating_session_digest,
                eligible_host_policy_digest: binding.eligible_host_policy_digest,
                created_at: binding.created_at.clone(),
                expires_at: binding.expires_at.clone(),
            },
            state: value.authority.state,
            proposal_generation: value.authority.proposal_generation,
            frozen: value.authority.frozen.as_ref().map(|frozen| FrozenWireV1 {
                proposal_digest: frozen.proposal_digest,
                effect_fingerprint: frozen.effect_fingerprint,
                before_document_digest: frozen.before_document_digest,
                after_document_digest: frozen.after_document_digest,
                resulting_scope: frozen.resulting_scope.clone(),
                conversation_continuity_reset: frozen.conversation_continuity_reset,
                frozen_semantic_timestamp: frozen.frozen_semantic_timestamp.clone(),
            }),
        }
    }
}

impl TryFrom<JournalWireV1> for LocalHouseholdProposalJournalV1 {
    type Error = AgentHouseholdContractErrorV1;

    fn try_from(value: JournalWireV1) -> Result<Self, Self::Error> {
        if value.schema_version != AGENT_HOUSEHOLD_CONTRACT_VERSION
            || value.journal_revision == 0
            || value.proposal_generation.get() > value.journal_revision
        {
            return Err(AgentHouseholdContractErrorV1::InvalidJournal);
        }
        let binding = LocalHouseholdProposalBindingV1::new(
            value.binding.account,
            value.binding.proposal_ref,
            value.binding.operation,
            value.binding.disclosure_generation,
            value.binding.disclosure_grant_set_digest,
            value.binding.disclosure_purpose,
            value.binding.lifecycle_generation,
            value.binding.projection,
            value.binding.expected_household_revision,
            value.binding.expected_profile_revision,
            value.binding.commit_id,
            value.binding.member_id,
            value.binding.previous_scope,
            value.binding.originating_session_digest,
            value.binding.eligible_host_policy_digest,
            value.binding.created_at,
            value.binding.expires_at,
        )?;
        let frozen = value.frozen.map(|frozen| {
            LocalHouseholdFrozenCandidateV1::new(
                frozen.proposal_digest,
                frozen.effect_fingerprint,
                frozen.before_document_digest,
                frozen.after_document_digest,
                frozen.resulting_scope,
                frozen.conversation_continuity_reset,
                frozen.frozen_semantic_timestamp,
            )
        });
        let frozen_required = matches!(
            value.state,
            AgentHouseholdProposalStateV1::AwaitingLocalReview
                | AgentHouseholdProposalStateV1::Committing
                | AgentHouseholdProposalStateV1::Committed
                | AgentHouseholdProposalStateV1::ProvenUncommitted
                | AgentHouseholdProposalStateV1::ReconciliationRequired
        );
        let frozen_forbidden = matches!(
            value.state,
            AgentHouseholdProposalStateV1::Prepared
                | AgentHouseholdProposalStateV1::AwaitingLocalInput
        );
        if (frozen_required && frozen.is_none())
            || (frozen_forbidden && frozen.is_some())
            || (frozen.is_some() && value.proposal_generation == GenerationId::INITIAL)
            || (frozen.is_none() && value.proposal_generation != GenerationId::INITIAL)
        {
            return Err(AgentHouseholdContractErrorV1::InvalidJournal);
        }
        Ok(Self {
            journal_revision: value.journal_revision,
            authority: LocalHouseholdProposalAuthorityV1 {
                binding,
                state: value.state,
                proposal_generation: value.proposal_generation,
                frozen,
            },
        })
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
    DisclosureProjectionChanged,
    LifecycleGenerationChanged,
    HouseholdRevisionChanged,
    ProfileRevisionChanged,
    ProposalGenerationChanged,
    ProposalDigestChanged,
    Expired,
    InvalidReviewWidth,
    MissingFrozenAuthority,
    CancelTooLate,
    JournalCasChanged,
    InvalidJournal,
    AppliedCommitMismatch,
    InvalidWireShape,
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
            Self::DisclosureProjectionChanged => {
                "household disclosure grant no longer permits the frozen projection"
            }
            Self::LifecycleGenerationChanged => "household lifecycle generation changed",
            Self::HouseholdRevisionChanged => "household revision changed",
            Self::ProfileRevisionChanged => "household profile revision changed",
            Self::ProposalGenerationChanged => "household proposal generation changed",
            Self::ProposalDigestChanged => "household proposal digest changed",
            Self::Expired => "household proposal expired",
            Self::InvalidReviewWidth => "household review width is invalid",
            Self::MissingFrozenAuthority => "household proposal authority is not frozen",
            Self::CancelTooLate => "household cancellation lost the commit race",
            Self::JournalCasChanged => "household proposal journal changed concurrently",
            Self::InvalidJournal => "household proposal journal is invalid",
            Self::AppliedCommitMismatch => {
                "household applied commit does not match the reviewed proposal"
            }
            Self::InvalidWireShape => "household agent value is outside the closed wire schema",
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
        LocalHouseholdProposalBindingV1::new(
            AccountId::parse("phase0-proposal-account").expect("account"),
            AgentHouseholdProposalIdV1::new(),
            operation,
            disclosure_generation,
            CanonicalDigestV1::from_bytes([8; 32]),
            AgentDisclosurePurposeV1::HouseholdAgentProposalStatus,
            GenerationId::new(11),
            if matches!(
                operation,
                AgentHouseholdOperationV1::Add | AgentHouseholdOperationV1::Scope
            ) {
                AgentHouseholdProjectionV1::ContentFree
            } else {
                AgentHouseholdProjectionV1::Profile
            },
            revision(),
            profile_revision,
            CommitId::new(),
            member_id,
            HouseholdScope::Subject(HouseholdSubjectId::self_()),
            CanonicalDigestV1::from_bytes([9; 32]),
            CanonicalDigestV1::from_bytes([10; 32]),
            timestamp(),
            CanonicalTimestampV1::parse("2026-08-02T12:10:00.000Z").expect("expiry"),
        )
        .expect("binding")
    }

    fn snapshot(
        disclosure_generation: GenerationId,
        profile_revision: Option<ProfileRevision>,
    ) -> LocalHouseholdAuthoritySnapshotV1 {
        LocalHouseholdAuthoritySnapshotV1::new(
            AccountId::parse("phase0-proposal-account").expect("account"),
            disclosure_generation,
            CanonicalDigestV1::from_bytes([8; 32]),
            AgentDisclosurePurposeV1::HouseholdAgentProposalStatus,
            AgentHouseholdProjectionV1::Profile,
            GenerationId::new(11),
            revision(),
            profile_revision,
            CanonicalTimestampV1::parse("2026-08-02T12:05:00.000Z").expect("observation"),
        )
    }

    fn frozen_candidate(
        proposal_digest: CanonicalDigestV1,
        effect_digest: CanonicalDigestV1,
    ) -> LocalHouseholdFrozenCandidateV1 {
        LocalHouseholdFrozenCandidateV1::new(
            proposal_digest,
            HouseholdEffectFingerprintV1::from_digest(effect_digest),
            CanonicalDigestV1::from_bytes([11; 32]),
            CanonicalDigestV1::from_bytes([12; 32]),
            HouseholdScope::Subject(HouseholdSubjectId::self_()),
            false,
            timestamp(),
        )
    }

    #[test]
    fn intake_freezes_authority_only_at_review_transition() {
        let mut authority = LocalHouseholdProposalAuthorityV1::awaiting_local_input(binding(
            AgentHouseholdOperationV1::Add,
            GenerationId::new(3),
            None,
            Some(MemberId::new()),
        ));
        assert!(authority.frozen().is_none());

        let proposal_digest = CanonicalDigestV1::from_bytes([1; 32]);
        authority
            .freeze_for_review(
                &snapshot(GenerationId::new(3), None),
                frozen_candidate(proposal_digest, CanonicalDigestV1::from_bytes([2; 32])),
            )
            .expect("freeze");

        assert_eq!(
            authority.state(),
            AgentHouseholdProposalStateV1::AwaitingLocalReview
        );
        assert_eq!(authority.proposal_generation(), GenerationId::new(1));
        assert!(authority.frozen().is_some());
        authority
            .begin_commit(
                &snapshot(GenerationId::new(3), None),
                GenerationId::new(1),
                proposal_digest,
            )
            .expect("commit begins");
        assert_eq!(authority.state(), AgentHouseholdProposalStateV1::Committing);
    }

    #[test]
    fn disclosure_change_and_cancel_commit_race_fail_closed() {
        let profile_revision = ProfileRevision::new(2).ok();
        let mut authority = LocalHouseholdProposalAuthorityV1::awaiting_local_input(binding(
            AgentHouseholdOperationV1::Edit,
            GenerationId::new(4),
            profile_revision,
            Some(MemberId::new()),
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
        assert_eq!(authority.state(), AgentHouseholdProposalStateV1::Cancelled);
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

        let wrong_account = LocalHouseholdAuthoritySnapshotV1::new(
            AccountId::parse("another-account").expect("account"),
            current.disclosure_generation,
            current.disclosure_grant_set_digest,
            current.disclosure_purpose,
            current.maximum_projection,
            current.lifecycle_generation,
            current.household_revision,
            current.profile_revision,
            current.observed_at.clone(),
        );
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
        assert_eq!(authority.state(), AgentHouseholdProposalStateV1::Committed);
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
        let expired = LocalHouseholdAuthoritySnapshotV1::new(
            AccountId::parse("phase0-proposal-account").expect("account"),
            GenerationId::new(2),
            CanonicalDigestV1::from_bytes([8; 32]),
            AgentDisclosurePurposeV1::HouseholdAgentProposalStatus,
            AgentHouseholdProjectionV1::Profile,
            GenerationId::new(11),
            revision(),
            None,
            CanonicalTimestampV1::parse("2026-08-02T12:10:00.000Z").expect("expiry boundary"),
        );
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
        assert!(authority.frozen().is_none());
        assert_eq!(authority.state(), AgentHouseholdProposalStateV1::Prepared);
    }

    #[test]
    fn durable_journal_cas_survives_restart_and_binds_the_applied_fingerprint() {
        let binding = binding(
            AgentHouseholdOperationV1::Edit,
            GenerationId::new(4),
            ProfileRevision::new(2).ok(),
            Some(MemberId::new()),
        );
        let commit_id = binding.commit_id();
        let current = snapshot(GenerationId::new(4), ProfileRevision::new(2).ok());
        let proposal_digest = CanonicalDigestV1::from_bytes([31; 32]);
        let effect_fingerprint =
            HouseholdEffectFingerprintV1::from_digest(CanonicalDigestV1::from_bytes([32; 32]));
        let frozen = LocalHouseholdFrozenCandidateV1::new(
            proposal_digest,
            effect_fingerprint,
            CanonicalDigestV1::from_bytes([33; 32]),
            CanonicalDigestV1::from_bytes([34; 32]),
            HouseholdScope::Subject(HouseholdSubjectId::self_()),
            false,
            timestamp(),
        );
        let mut journal = LocalHouseholdProposalJournalV1::new(
            LocalHouseholdProposalAuthorityV1::awaiting_local_input(binding),
        );
        let intake_token = journal.cas_token();
        journal
            .freeze_for_review(&intake_token, &current, frozen)
            .expect("durable freeze CAS");
        assert_eq!(
            journal.cancel_before_commit(&intake_token),
            Err(AgentHouseholdContractErrorV1::JournalCasChanged)
        );

        let bytes = journal.persisted_bytes().expect("journal bytes");
        let mut restarted =
            LocalHouseholdProposalJournalV1::restore(&bytes).expect("journal restart");
        let review_token = restarted.cas_token();
        restarted
            .begin_commit(&review_token, &current, proposal_digest)
            .expect("reviewed digest begins commit");
        assert_eq!(
            restarted.cancel_before_commit(&review_token),
            Err(AgentHouseholdContractErrorV1::JournalCasChanged)
        );
        let committing_bytes = restarted.persisted_bytes().expect("committing journal");
        let mut crash_recovered =
            LocalHouseholdProposalJournalV1::restore(&committing_bytes).expect("crash recovery");
        let committing_token = crash_recovered.cas_token();
        assert_eq!(
            crash_recovered.reconcile_applied_commit(
                &committing_token,
                commit_id,
                HouseholdEffectFingerprintV1::from_digest(CanonicalDigestV1::from_bytes([99; 32])),
            ),
            Err(AgentHouseholdContractErrorV1::AppliedCommitMismatch)
        );
        crash_recovered
            .reconcile_applied_commit(&committing_token, commit_id, effect_fingerprint)
            .expect("exact ledger fingerprint reconciles");
        assert_eq!(
            crash_recovered.state(),
            AgentHouseholdProposalStateV1::Committed
        );

        let mut tampered: serde_json::Value =
            serde_json::from_slice(&committing_bytes).expect("journal JSON");
        tampered["journal_revision"] = serde_json::json!(0);
        assert_eq!(
            LocalHouseholdProposalJournalV1::restore(
                &serde_json::to_vec(&tampered).expect("tampered journal")
            ),
            Err(AgentHouseholdContractErrorV1::InvalidJournal)
        );

        let mut wrong_purpose: serde_json::Value =
            serde_json::from_slice(&committing_bytes).expect("journal JSON");
        wrong_purpose["binding"]["disclosure_purpose"] = serde_json::json!("household_agent_read");
        assert_eq!(
            LocalHouseholdProposalJournalV1::restore(
                &serde_json::to_vec(&wrong_purpose).expect("wrong-purpose journal")
            ),
            Err(AgentHouseholdContractErrorV1::InvalidJournal)
        );

        let mut invalid_member_shape: serde_json::Value =
            serde_json::from_slice(&committing_bytes).expect("journal JSON");
        invalid_member_shape["binding"]["member_id"] = serde_json::Value::Null;
        assert_eq!(
            LocalHouseholdProposalJournalV1::restore(
                &serde_json::to_vec(&invalid_member_shape).expect("invalid-member journal")
            ),
            Err(AgentHouseholdContractErrorV1::InvalidJournal)
        );

        let mut invalid_add_projection: serde_json::Value =
            serde_json::from_slice(&committing_bytes).expect("journal JSON");
        invalid_add_projection["binding"]["operation"] = serde_json::json!("add");
        assert_eq!(
            LocalHouseholdProposalJournalV1::restore(
                &serde_json::to_vec(&invalid_add_projection).expect("invalid-add journal")
            ),
            Err(AgentHouseholdContractErrorV1::InvalidJournal)
        );

        let mut missing_proven_authority: serde_json::Value =
            serde_json::from_slice(&committing_bytes).expect("journal JSON");
        missing_proven_authority["state"] = serde_json::json!("proven_uncommitted");
        missing_proven_authority["frozen"] = serde_json::Value::Null;
        missing_proven_authority["proposal_generation"] = serde_json::json!(0);
        assert_eq!(
            LocalHouseholdProposalJournalV1::restore(
                &serde_json::to_vec(&missing_proven_authority)
                    .expect("missing-proven-authority journal")
            ),
            Err(AgentHouseholdContractErrorV1::InvalidJournal)
        );
    }

    #[test]
    fn proposal_commit_rechecks_live_projection_even_when_digest_is_unchanged() {
        let mut authority = LocalHouseholdProposalAuthorityV1::awaiting_local_input(binding(
            AgentHouseholdOperationV1::Edit,
            GenerationId::new(4),
            ProfileRevision::new(2).ok(),
            Some(MemberId::new()),
        ));
        let current = snapshot(GenerationId::new(4), ProfileRevision::new(2).ok());
        let proposal_digest = CanonicalDigestV1::from_bytes([35; 32]);
        authority
            .freeze_for_review(
                &current,
                frozen_candidate(proposal_digest, CanonicalDigestV1::from_bytes([36; 32])),
            )
            .expect("freeze");
        let expired_grant_view = LocalHouseholdAuthoritySnapshotV1::new(
            current.account.clone(),
            current.disclosure_generation,
            current.disclosure_grant_set_digest,
            current.disclosure_purpose,
            AgentHouseholdProjectionV1::ContentFree,
            current.lifecycle_generation,
            current.household_revision,
            current.profile_revision,
            current.observed_at.clone(),
        );
        assert_eq!(
            authority.begin_commit(
                &expired_grant_view,
                authority.proposal_generation(),
                proposal_digest,
            ),
            Err(AgentHouseholdContractErrorV1::DisclosureProjectionChanged)
        );
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
    fn disclosure_digest_binds_the_complete_authority_envelope() {
        #[allow(clippy::too_many_arguments)]
        fn digest(
            account_value: &str,
            purpose: AgentDisclosurePurposeV1,
            minor_status: MinorStatusV1,
            classes: Vec<AgentDisclosureDataClassV1>,
            authority: AgentDisclosureGrantingAuthorityV1,
            state: AgentDisclosureGrantStateV1,
            issued_at: &str,
            expires_at: Option<&str>,
        ) -> CanonicalDigestV1 {
            let account = AccountId::parse(account_value).expect("account");
            let generation = GenerationId::new(3);
            let grant = AgentDisclosureGrantV1::new(
                account.clone(),
                AgentDisclosureGrantSubjectV1::Self_,
                minor_status,
                classes,
                purpose,
                authority,
                5,
                generation,
                state,
                CanonicalTimestampV1::parse(issued_at).expect("issued"),
                expires_at.map(|value| CanonicalTimestampV1::parse(value).expect("expiry")),
            )
            .expect("grant");
            AgentDisclosureGrantSetV1::new(
                account,
                generation,
                purpose,
                CanonicalTimestampV1::parse("2026-08-02T12:05:00.000Z").expect("observation"),
                vec![grant],
            )
            .expect("set")
            .revision_set_digest()
        }

        let base = digest(
            "digest-account",
            AgentDisclosurePurposeV1::HouseholdAgentRead,
            MinorStatusV1::Adult,
            vec![
                AgentDisclosureDataClassV1::Roster,
                AgentDisclosureDataClassV1::MinimizedDeclaredProfile,
            ],
            AgentDisclosureGrantingAuthorityV1::AccountOwnerAdultAuthorization,
            AgentDisclosureGrantStateV1::Active,
            "2026-08-02T12:00:00.000Z",
            Some("2026-08-02T12:20:00.000Z"),
        );
        let variants = [
            digest(
                "other-digest-account",
                AgentDisclosurePurposeV1::HouseholdAgentRead,
                MinorStatusV1::Adult,
                vec![
                    AgentDisclosureDataClassV1::Roster,
                    AgentDisclosureDataClassV1::MinimizedDeclaredProfile,
                ],
                AgentDisclosureGrantingAuthorityV1::AccountOwnerAdultAuthorization,
                AgentDisclosureGrantStateV1::Active,
                "2026-08-02T12:00:00.000Z",
                Some("2026-08-02T12:20:00.000Z"),
            ),
            digest(
                "digest-account",
                AgentDisclosurePurposeV1::HouseholdAgentProposalStatus,
                MinorStatusV1::Adult,
                vec![
                    AgentDisclosureDataClassV1::Roster,
                    AgentDisclosureDataClassV1::MinimizedDeclaredProfile,
                ],
                AgentDisclosureGrantingAuthorityV1::AccountOwnerAdultAuthorization,
                AgentDisclosureGrantStateV1::Active,
                "2026-08-02T12:00:00.000Z",
                Some("2026-08-02T12:20:00.000Z"),
            ),
            digest(
                "digest-account",
                AgentDisclosurePurposeV1::HouseholdAgentRead,
                MinorStatusV1::Minor,
                vec![AgentDisclosureDataClassV1::Roster],
                AgentDisclosureGrantingAuthorityV1::AuthorizedGuardianRosterOnly,
                AgentDisclosureGrantStateV1::Active,
                "2026-08-02T12:00:00.000Z",
                Some("2026-08-02T12:20:00.000Z"),
            ),
            digest(
                "digest-account",
                AgentDisclosurePurposeV1::HouseholdAgentRead,
                MinorStatusV1::Adult,
                vec![
                    AgentDisclosureDataClassV1::Roster,
                    AgentDisclosureDataClassV1::MinimizedDeclaredProfile,
                ],
                AgentDisclosureGrantingAuthorityV1::AccountOwnerAdultAuthorization,
                AgentDisclosureGrantStateV1::Revoked,
                "2026-08-02T12:00:00.000Z",
                Some("2026-08-02T12:20:00.000Z"),
            ),
            digest(
                "digest-account",
                AgentDisclosurePurposeV1::HouseholdAgentRead,
                MinorStatusV1::Adult,
                vec![
                    AgentDisclosureDataClassV1::Roster,
                    AgentDisclosureDataClassV1::MinimizedDeclaredProfile,
                ],
                AgentDisclosureGrantingAuthorityV1::AccountOwnerAdultAuthorization,
                AgentDisclosureGrantStateV1::Active,
                "2026-08-02T12:00:01.000Z",
                Some("2026-08-02T12:21:00.000Z"),
            ),
        ];
        assert!(variants.into_iter().all(|variant| variant != base));
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

    #[test]
    fn rust_wire_serialization_rejects_schema_invalid_values() {
        let invalid_profile = AgentMinimizedDeclaredProfileV1 {
            diet_styles: vec!["vegan".to_owned(), "vegan".to_owned()],
            allergies: Vec::new(),
            restrictions: Vec::new(),
            health_conditions: Vec::new(),
            avoid_ingredients: Vec::new(),
        };
        assert!(serde_json::to_value(&invalid_profile).is_err());
        assert!(
            serde_json::from_value::<AgentMinimizedDeclaredProfileV1>(serde_json::json!({
                "diet_styles": ["vegan", "vegan"],
                "allergies": [],
                "restrictions": [],
                "health_conditions": [],
                "avoid_ingredients": []
            }))
            .is_err()
        );

        let invalid_change = AgentHouseholdChangeV1 {
            field: AgentHouseholdChangeFieldV1::Allergies,
            before: vec!["milk".to_owned()],
            after: vec!["line\nfeed".to_owned()],
        };
        assert!(serde_json::to_value(&invalid_change).is_err());
        assert!(
            serde_json::from_value::<AgentHouseholdChangeV1>(serde_json::json!({
                "field": "allergies",
                "before": ["milk"],
                "after": ["line\nfeed"]
            }))
            .is_err()
        );
    }
}
