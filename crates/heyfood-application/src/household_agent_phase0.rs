//! Non-routable Phase-0 composition proof for household agent contracts.
//!
//! The controller proves application/port boundaries for read, prepare,
//! status, and pre-dispatch cancellation. No CLI command or MCP tool composes
//! this controller in Phase 0.

use std::{fmt, sync::Arc};

use heyfood_core::{
    AGENT_HOUSEHOLD_CONTRACT_VERSION, AGENT_HOUSEHOLD_MAX_MEMBERS_PER_PAGE, AccountId,
    AgentDisclosureGrantSetV1, AgentDisclosureGrantSubjectV1, AgentDisclosurePurposeV1,
    AgentHouseholdOutcomeReceiptV1, AgentHouseholdPrepareRequestKindV1,
    AgentHouseholdPrepareRequestV1, AgentHouseholdProjectionV1, AgentHouseholdProposalIdV1,
    AgentHouseholdProposalPresentationV1, AgentHouseholdProposalStateV1,
    AgentHouseholdReadRequestKindV1, AgentHouseholdReadRequestV1, AgentHouseholdReadResultKindV1,
    AgentHouseholdReadSnapshotV1, AgentHouseholdSubjectV1, CanonicalDigestV1, GenerationId,
    HouseholdScope, HouseholdSubjectId,
};
use tokio_util::sync::CancellationToken;

use crate::{HouseholdAgentPhase0Port, PortError};

#[derive(Clone, Eq, PartialEq)]
pub struct BoundAgentHouseholdReadV1 {
    pub account: AccountId,
    pub snapshot: AgentHouseholdReadSnapshotV1,
}

#[derive(Clone, Eq, PartialEq)]
pub struct BoundAgentHouseholdDisclosureV1 {
    pub account: AccountId,
    pub grants: AgentDisclosureGrantSetV1,
}

impl fmt::Debug for BoundAgentHouseholdDisclosureV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundAgentHouseholdDisclosureV1")
            .field("grants", &self.grants)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct AuthorizedAgentHouseholdPrepareV1 {
    pub request: AgentHouseholdPrepareRequestV1,
    pub maximum_projection: AgentHouseholdProjectionV1,
    pub frozen_disclosure: FrozenAgentHouseholdDisclosureV1,
}

impl fmt::Debug for AuthorizedAgentHouseholdPrepareV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthorizedAgentHouseholdPrepareV1")
            .field("request", &self.request)
            .field("maximum_projection", &self.maximum_projection)
            .field("frozen_disclosure", &self.frozen_disclosure)
            .finish_non_exhaustive()
    }
}

/// Non-serializable disclosure authority retained only in the encrypted local
/// proposal journal. Agent-visible proposal documents never contain it.
#[derive(Clone, Eq, PartialEq)]
pub struct FrozenAgentHouseholdDisclosureV1 {
    account: AccountId,
    purpose: AgentDisclosurePurposeV1,
    generation: GenerationId,
    grant_set_digest: CanonicalDigestV1,
    subjects: Vec<AgentDisclosureGrantSubjectV1>,
    maximum_projection: AgentHouseholdProjectionV1,
}

impl fmt::Debug for FrozenAgentHouseholdDisclosureV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FrozenAgentHouseholdDisclosureV1")
            .field("purpose", &self.purpose)
            .field("generation", &self.generation)
            .field("subject_count", &self.subjects.len())
            .field("maximum_projection", &self.maximum_projection)
            .finish_non_exhaustive()
    }
}

impl FrozenAgentHouseholdDisclosureV1 {
    fn from_grants(
        account: AccountId,
        purpose: AgentDisclosurePurposeV1,
        mut subjects: Vec<AgentDisclosureGrantSubjectV1>,
        maximum_projection: AgentHouseholdProjectionV1,
        grants: &AgentDisclosureGrantSetV1,
    ) -> Self {
        subjects.sort();
        subjects.dedup();
        Self {
            account,
            purpose,
            generation: grants.generation(),
            grant_set_digest: grants.revision_set_digest(),
            subjects,
            maximum_projection,
        }
    }

    #[must_use]
    pub fn subjects(&self) -> &[AgentDisclosureGrantSubjectV1] {
        &self.subjects
    }
}

impl fmt::Debug for BoundAgentHouseholdReadV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundAgentHouseholdReadV1")
            .field("snapshot", &self.snapshot)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct BoundAgentHouseholdProposalV1 {
    pub account: AccountId,
    pub presentation: AgentHouseholdProposalPresentationV1,
    pub frozen_disclosure: FrozenAgentHouseholdDisclosureV1,
}

impl fmt::Debug for BoundAgentHouseholdProposalV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundAgentHouseholdProposalV1")
            .field("presentation", &self.presentation)
            .field("frozen_disclosure", &self.frozen_disclosure)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct BoundAgentHouseholdOutcomeReceiptV1 {
    pub account: AccountId,
    pub receipt: AgentHouseholdOutcomeReceiptV1,
}

impl fmt::Debug for BoundAgentHouseholdOutcomeReceiptV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundAgentHouseholdOutcomeReceiptV1")
            .field("receipt", &self.receipt)
            .finish_non_exhaustive()
    }
}

pub struct HouseholdAgentPhase0Proof {
    port: Arc<dyn HouseholdAgentPhase0Port>,
}

impl HouseholdAgentPhase0Proof {
    #[must_use]
    pub fn new(port: Arc<dyn HouseholdAgentPhase0Port>) -> Self {
        Self { port }
    }

    pub async fn read(
        &self,
        account: AccountId,
        request: AgentHouseholdReadRequestV1,
        cancellation: CancellationToken,
    ) -> Result<AgentHouseholdReadSnapshotV1, PortError> {
        check_cancelled(&cancellation)?;
        if request.schema_version != AGENT_HOUSEHOLD_CONTRACT_VERSION
            || request.kind != AgentHouseholdReadRequestKindV1::HouseholdReadRequest
            || request.limit == 0
            || request.limit > AGENT_HOUSEHOLD_MAX_MEMBERS_PER_PAGE
        {
            return Err(phase0_error(
                "household_agent_read_contract",
                "household agent read request is outside the closed contract",
            ));
        }
        let result = self
            .port
            .read(account.clone(), request.clone(), cancellation.clone())
            .await?;
        ensure_account(&account, &result.account)?;
        let subjects = validate_raw_read(&request, &result.snapshot)?;
        let disclosure = self
            .port
            .disclosure(
                account.clone(),
                AgentDisclosurePurposeV1::HouseholdAgentRead,
                cancellation,
            )
            .await?;
        ensure_account(&account, &disclosure.account)?;
        validate_disclosure(
            &account,
            AgentDisclosurePurposeV1::HouseholdAgentRead,
            &disclosure.grants,
        )?;
        ensure_expected_disclosure_generation(
            request.expected_disclosure_generation,
            &disclosure.grants,
        )?;
        let allowed = minimum_projection(
            request.requested_projection,
            disclosure.grants.maximum_projection_for(&subjects),
        );
        let mut snapshot = result.snapshot.filtered_to(allowed);
        snapshot.disclosure_generation = disclosure.grants.generation();
        validate_filtered_read(&request, &snapshot)?;
        Ok(snapshot)
    }

    pub async fn prepare(
        &self,
        account: AccountId,
        request: AgentHouseholdPrepareRequestV1,
        cancellation: CancellationToken,
    ) -> Result<AgentHouseholdProposalPresentationV1, PortError> {
        check_cancelled(&cancellation)?;
        if request.schema_version != AGENT_HOUSEHOLD_CONTRACT_VERSION
            || request.kind != AgentHouseholdPrepareRequestKindV1::PrepareHouseholdChange
        {
            return Err(phase0_error(
                "household_agent_prepare_contract",
                "household agent prepare request is outside the closed contract",
            ));
        }
        request.validate_shape().map_err(|_| {
            phase0_error(
                "household_agent_operation_shape",
                "household agent proposal operation shape is invalid",
            )
        })?;
        let subjects = disclosure_subjects_for_prepare(&request);
        let initial_disclosure = self
            .port
            .disclosure(
                account.clone(),
                AgentDisclosurePurposeV1::HouseholdAgentProposalStatus,
                cancellation.clone(),
            )
            .await?;
        ensure_account(&account, &initial_disclosure.account)?;
        validate_disclosure(
            &account,
            AgentDisclosurePurposeV1::HouseholdAgentProposalStatus,
            &initial_disclosure.grants,
        )?;
        ensure_expected_disclosure_generation(
            request.expected_disclosure_generation,
            &initial_disclosure.grants,
        )?;
        let maximum_projection = minimum_projection(
            request.requested_projection,
            initial_disclosure.grants.maximum_projection_for(&subjects),
        );
        let authorized = AuthorizedAgentHouseholdPrepareV1 {
            request: request.clone(),
            maximum_projection,
            frozen_disclosure: FrozenAgentHouseholdDisclosureV1::from_grants(
                account.clone(),
                AgentDisclosurePurposeV1::HouseholdAgentProposalStatus,
                subjects.clone(),
                maximum_projection,
                &initial_disclosure.grants,
            ),
        };
        let result = self
            .port
            .prepare(account.clone(), authorized.clone(), cancellation.clone())
            .await?;
        ensure_account(&account, &result.account)?;
        validate_returned_proposal(&request, &result.presentation)?;
        if result.frozen_disclosure != authorized.frozen_disclosure {
            return Err(phase0_error(
                "household_agent_disclosure_binding",
                "household proposal did not preserve its frozen disclosure authority",
            ));
        }
        let current_disclosure = self
            .port
            .disclosure(
                account.clone(),
                AgentDisclosurePurposeV1::HouseholdAgentProposalStatus,
                cancellation,
            )
            .await?;
        ensure_account(&account, &current_disclosure.account)?;
        validate_disclosure(
            &account,
            AgentDisclosurePurposeV1::HouseholdAgentProposalStatus,
            &current_disclosure.grants,
        )?;
        let disclosure_changed = current_disclosure.grants.generation()
            != initial_disclosure.grants.generation()
            || current_disclosure.grants.revision_set_digest()
                != initial_disclosure.grants.revision_set_digest();
        let current_maximum = if disclosure_changed {
            AgentHouseholdProjectionV1::ContentFree
        } else {
            minimum_projection(
                request.requested_projection,
                current_disclosure.grants.maximum_projection_for(&subjects),
            )
        };
        let mut presentation = result.presentation.filtered_to(current_maximum);
        presentation.disclosure_generation = current_disclosure.grants.generation();
        if disclosure_changed {
            presentation.state = AgentHouseholdProposalStateV1::Stale;
            presentation.human_status = presentation.state.human_status().to_owned();
        }
        validate_presentation(&presentation, current_maximum, None)?;
        Ok(presentation)
    }

    pub async fn status(
        &self,
        account: AccountId,
        proposal_ref: AgentHouseholdProposalIdV1,
        cancellation: CancellationToken,
    ) -> Result<AgentHouseholdProposalPresentationV1, PortError> {
        check_cancelled(&cancellation)?;
        let result = self
            .port
            .status(account.clone(), proposal_ref, cancellation.clone())
            .await?;
        ensure_account(&account, &result.account)?;
        if result.presentation.proposal_ref != proposal_ref {
            return Err(phase0_error(
                "household_agent_proposal_mismatch",
                "household agent proposal reference changed across status",
            ));
        }
        validate_status_subject_binding(&result.presentation, &result.frozen_disclosure)?;
        let disclosure = self
            .port
            .disclosure(
                account.clone(),
                AgentDisclosurePurposeV1::HouseholdAgentProposalStatus,
                cancellation,
            )
            .await?;
        ensure_account(&account, &disclosure.account)?;
        validate_disclosure(
            &account,
            AgentDisclosurePurposeV1::HouseholdAgentProposalStatus,
            &disclosure.grants,
        )?;
        let current_authorized_projection = disclosure
            .grants
            .maximum_projection_for(result.frozen_disclosure.subjects());
        let disclosure_changed = result.frozen_disclosure.account != account
            || result.frozen_disclosure.purpose
                != AgentDisclosurePurposeV1::HouseholdAgentProposalStatus
            || result.frozen_disclosure.generation != disclosure.grants.generation()
            || result.frozen_disclosure.grant_set_digest != disclosure.grants.revision_set_digest()
            || projection_rank(current_authorized_projection)
                < projection_rank(result.frozen_disclosure.maximum_projection);
        let maximum = if disclosure_changed {
            AgentHouseholdProjectionV1::ContentFree
        } else {
            minimum_projection(
                result.frozen_disclosure.maximum_projection,
                current_authorized_projection,
            )
        };
        let mut presentation = result.presentation.filtered_to(maximum);
        presentation.disclosure_generation = disclosure.grants.generation();
        if disclosure_changed {
            presentation.state = AgentHouseholdProposalStateV1::Stale;
            presentation.human_status = presentation.state.human_status().to_owned();
        }
        validate_presentation(&presentation, maximum, None)?;
        Ok(presentation)
    }

    pub async fn cancel(
        &self,
        account: AccountId,
        proposal_ref: AgentHouseholdProposalIdV1,
        cancellation: CancellationToken,
    ) -> Result<AgentHouseholdOutcomeReceiptV1, PortError> {
        check_cancelled(&cancellation)?;
        let receipt = self
            .port
            .cancel(account.clone(), proposal_ref, cancellation)
            .await?;
        ensure_account(&account, &receipt.account)?;
        if receipt.receipt.proposal_ref() != proposal_ref
            || receipt.receipt.state() != AgentHouseholdProposalStateV1::Cancelled
            || !receipt.receipt.is_valid()
        {
            return Err(phase0_error(
                "household_agent_cancel_unproven",
                "household cancellation did not prove a non-mutating outcome",
            ));
        }
        Ok(receipt.receipt)
    }
}

fn validate_raw_read(
    request: &AgentHouseholdReadRequestV1,
    snapshot: &AgentHouseholdReadSnapshotV1,
) -> Result<Vec<AgentDisclosureGrantSubjectV1>, PortError> {
    snapshot.validate_wire_shape().map_err(|_| {
        phase0_error(
            "household_agent_read_contract",
            "household agent read result is outside the closed wire schema",
        )
    })?;
    if snapshot.schema_version != AGENT_HOUSEHOLD_CONTRACT_VERSION
        || snapshot.kind != AgentHouseholdReadResultKindV1::HouseholdReadResult
    {
        return Err(phase0_error(
            "household_agent_read_contract",
            "household agent read result is outside the closed contract",
        ));
    }
    if request.subject.is_none() != snapshot.resolved_from_active_scope {
        return Err(phase0_error(
            "household_agent_subject_resolution",
            "household agent subject resolution evidence is inconsistent",
        ));
    }
    if let Some(subject) = request.subject.as_ref()
        && snapshot.resolved_subject.as_ref() != Some(subject)
    {
        return Err(phase0_error(
            "household_agent_subject_resolution",
            "household agent read resolved a different subject",
        ));
    }
    if snapshot.members.len() > usize::from(request.limit) {
        return Err(phase0_error(
            "household_agent_read_limit",
            "household agent read exceeded the requested page limit",
        ));
    }
    let subjects = disclosure_subjects_for_read(snapshot)?;
    if snapshot.resolved_from_active_scope
        && !snapshot
            .resolved_subject
            .as_ref()
            .zip(snapshot.active_scope.as_ref())
            .is_some_and(|(subject, scope)| scope_matches_subject(scope, subject))
    {
        return Err(phase0_error(
            "household_agent_subject_resolution",
            "household agent active scope does not match the resolved subject",
        ));
    }
    Ok(subjects)
}

fn validate_filtered_read(
    request: &AgentHouseholdReadRequestV1,
    snapshot: &AgentHouseholdReadSnapshotV1,
) -> Result<(), PortError> {
    if projection_rank(snapshot.projection) > projection_rank(request.requested_projection) {
        return Err(phase0_error(
            "household_agent_projection_escalation",
            "household agent read exceeded the authorized projection",
        ));
    }
    match snapshot.projection {
        AgentHouseholdProjectionV1::ContentFree
            if snapshot.resolved_subject.is_some()
                || snapshot.active_scope.is_some()
                || !snapshot.members.is_empty()
                || snapshot.next_cursor.is_some() =>
        {
            Err(phase0_error(
                "household_agent_projection_leak",
                "content-free household result included identifying data",
            ))
        }
        AgentHouseholdProjectionV1::Roster
            if snapshot
                .members
                .iter()
                .any(|member| member.minimized_declared_profile.is_some()) =>
        {
            Err(phase0_error(
                "household_agent_projection_leak",
                "roster-only household result included profile data",
            ))
        }
        _ => Ok(()),
    }
}

fn disclosure_subjects_for_read(
    snapshot: &AgentHouseholdReadSnapshotV1,
) -> Result<Vec<AgentDisclosureGrantSubjectV1>, PortError> {
    let Some(subject) = snapshot.resolved_subject.as_ref() else {
        return Ok(Vec::new());
    };
    match subject {
        AgentHouseholdSubjectV1::Self_ => {
            if !snapshot.members.is_empty() {
                return Err(phase0_error(
                    "household_agent_subject_content_mismatch",
                    "self household read returned another member's record",
                ));
            }
            Ok(vec![AgentDisclosureGrantSubjectV1::Self_])
        }
        AgentHouseholdSubjectV1::Member(member) => {
            if snapshot.members.len() != 1 || snapshot.members[0].member_ref != *member {
                return Err(phase0_error(
                    "household_agent_subject_content_mismatch",
                    "member household read returned a different member's record",
                ));
            }
            Ok(vec![AgentDisclosureGrantSubjectV1::Member(member.clone())])
        }
        AgentHouseholdSubjectV1::Everyone => {
            if snapshot.next_cursor.is_some()
                || usize::from(snapshot.eligible_member_count)
                    != snapshot.members.len().saturating_add(1)
            {
                return Err(phase0_error(
                    "household_agent_everyone_incomplete",
                    "everyone disclosure requires the complete eligible roster",
                ));
            }
            let mut subjects = vec![AgentDisclosureGrantSubjectV1::Self_];
            subjects.extend(
                snapshot
                    .members
                    .iter()
                    .map(|member| AgentDisclosureGrantSubjectV1::Member(member.member_ref.clone())),
            );
            let original_length = subjects.len();
            subjects.sort();
            subjects.dedup();
            if subjects.len() != original_length {
                return Err(phase0_error(
                    "household_agent_subject_content_mismatch",
                    "everyone household read returned a duplicate member record",
                ));
            }
            Ok(subjects)
        }
    }
}

fn scope_matches_subject(scope: &HouseholdScope, subject: &AgentHouseholdSubjectV1) -> bool {
    match (scope, subject) {
        (HouseholdScope::Subject(HouseholdSubjectId::Self_), AgentHouseholdSubjectV1::Self_)
        | (HouseholdScope::Everyone, AgentHouseholdSubjectV1::Everyone) => true,
        (
            HouseholdScope::Subject(HouseholdSubjectId::Member(scope_member)),
            AgentHouseholdSubjectV1::Member(subject_member),
        ) => scope_member == subject_member,
        _ => false,
    }
}

fn disclosure_subjects_for_prepare(
    request: &AgentHouseholdPrepareRequestV1,
) -> Vec<AgentDisclosureGrantSubjectV1> {
    match request.operation {
        heyfood_core::AgentHouseholdOperationV1::Edit
        | heyfood_core::AgentHouseholdOperationV1::Archive
        | heyfood_core::AgentHouseholdOperationV1::Restore => request
            .affected_member_ref
            .clone()
            .map(AgentDisclosureGrantSubjectV1::Member)
            .into_iter()
            .collect(),
        heyfood_core::AgentHouseholdOperationV1::Scope => match request.bundled_scope.as_ref() {
            Some(HouseholdScope::Subject(HouseholdSubjectId::Self_)) => {
                vec![AgentDisclosureGrantSubjectV1::Self_]
            }
            Some(HouseholdScope::Subject(HouseholdSubjectId::Member(member))) => {
                vec![AgentDisclosureGrantSubjectV1::Member(member.clone())]
            }
            Some(HouseholdScope::Everyone) | None => Vec::new(),
        },
        heyfood_core::AgentHouseholdOperationV1::Add => Vec::new(),
    }
}

fn validate_returned_proposal(
    request: &AgentHouseholdPrepareRequestV1,
    presentation: &AgentHouseholdProposalPresentationV1,
) -> Result<(), PortError> {
    if presentation.operation != request.operation {
        return Err(phase0_error(
            "household_agent_proposal_operation_mismatch",
            "household proposal returned a different operation",
        ));
    }
    let expected_member = match request.operation {
        heyfood_core::AgentHouseholdOperationV1::Edit
        | heyfood_core::AgentHouseholdOperationV1::Archive
        | heyfood_core::AgentHouseholdOperationV1::Restore => request.affected_member_ref.as_ref(),
        heyfood_core::AgentHouseholdOperationV1::Add
        | heyfood_core::AgentHouseholdOperationV1::Scope => None,
    };
    if presentation.affected_member_ref.as_ref() != expected_member
        || (expected_member.is_none() && presentation.affected_member_label.is_some())
        || (expected_member.is_some() && presentation.affected_member_label.is_none())
    {
        return Err(phase0_error(
            "household_agent_subject_content_mismatch",
            "household proposal returned content for a different member",
        ));
    }
    Ok(())
}

fn validate_status_subject_binding(
    presentation: &AgentHouseholdProposalPresentationV1,
    frozen: &FrozenAgentHouseholdDisclosureV1,
) -> Result<(), PortError> {
    let valid = match presentation.operation {
        heyfood_core::AgentHouseholdOperationV1::Edit
        | heyfood_core::AgentHouseholdOperationV1::Archive
        | heyfood_core::AgentHouseholdOperationV1::Restore => presentation
            .affected_member_ref
            .as_ref()
            .is_some_and(|member| {
                frozen.subjects.as_slice()
                    == [AgentDisclosureGrantSubjectV1::Member(member.clone())]
            }),
        heyfood_core::AgentHouseholdOperationV1::Add
        | heyfood_core::AgentHouseholdOperationV1::Scope => {
            presentation.affected_member_ref.is_none()
                && presentation.affected_member_label.is_none()
        }
    };
    if valid {
        Ok(())
    } else {
        Err(phase0_error(
            "household_agent_subject_content_mismatch",
            "household proposal status is not bound to its authorized subject",
        ))
    }
}

fn validate_disclosure(
    account: &AccountId,
    purpose: AgentDisclosurePurposeV1,
    grants: &AgentDisclosureGrantSetV1,
) -> Result<(), PortError> {
    if !grants.account_matches(account) || grants.purpose() != purpose {
        return Err(phase0_error(
            "household_agent_disclosure_binding",
            "household disclosure authority is not account and purpose bound",
        ));
    }
    Ok(())
}

fn ensure_expected_disclosure_generation(
    expected: heyfood_core::GenerationId,
    grants: &AgentDisclosureGrantSetV1,
) -> Result<(), PortError> {
    if grants.generation() == expected {
        Ok(())
    } else {
        Err(phase0_error(
            "household_agent_disclosure_generation_stale",
            "household disclosure generation changed; request fresh authority",
        ))
    }
}

fn validate_presentation(
    presentation: &AgentHouseholdProposalPresentationV1,
    maximum_projection: AgentHouseholdProjectionV1,
    expected_generation: Option<heyfood_core::GenerationId>,
) -> Result<(), PortError> {
    presentation.validate_wire_shape().map_err(|_| {
        phase0_error(
            "household_agent_proposal_contract",
            "household agent proposal is outside the closed wire schema",
        )
    })?;
    if presentation.schema_version != AGENT_HOUSEHOLD_CONTRACT_VERSION
        || expected_generation.is_some_and(|value| value != presentation.disclosure_generation)
        || !presentation.has_canonical_copy()
    {
        return Err(phase0_error(
            "household_agent_proposal_generation",
            "household agent proposal contract or disclosure generation changed",
        ));
    }
    if projection_rank(presentation.projection) > projection_rank(maximum_projection) {
        return Err(phase0_error(
            "household_agent_projection_escalation",
            "household agent proposal exceeded the authorized projection",
        ));
    }
    match presentation.projection {
        AgentHouseholdProjectionV1::ContentFree
            if presentation.affected_member_ref.is_some()
                || presentation.affected_member_label.is_some()
                || !presentation.changes.is_empty()
                || !presentation.consequences.is_empty() =>
        {
            Err(phase0_error(
                "household_agent_projection_leak",
                "content-free proposal status included household content",
            ))
        }
        AgentHouseholdProjectionV1::Roster
            if presentation
                .changes
                .iter()
                .any(|change| change.field.is_profile()) =>
        {
            Err(phase0_error(
                "household_agent_projection_leak",
                "roster-only proposal status included profile content",
            ))
        }
        _ => Ok(()),
    }
}

const fn minimum_projection(
    requested: AgentHouseholdProjectionV1,
    allowed: AgentHouseholdProjectionV1,
) -> AgentHouseholdProjectionV1 {
    if projection_rank(requested) <= projection_rank(allowed) {
        requested
    } else {
        allowed
    }
}

const fn projection_rank(projection: AgentHouseholdProjectionV1) -> u8 {
    match projection {
        AgentHouseholdProjectionV1::ContentFree => 0,
        AgentHouseholdProjectionV1::Roster => 1,
        AgentHouseholdProjectionV1::Profile => 2,
    }
}

fn ensure_account(expected: &AccountId, actual: &AccountId) -> Result<(), PortError> {
    if expected == actual {
        Ok(())
    } else {
        Err(phase0_error(
            "household_account_mismatch",
            "household agent result belongs to another account",
        ))
    }
}

fn check_cancelled(cancellation: &CancellationToken) -> Result<(), PortError> {
    if cancellation.is_cancelled() {
        Err(phase0_error(
            "household_agent_cancelled_before_dispatch",
            "household agent operation was cancelled before dispatch",
        ))
    } else {
        Ok(())
    }
}

fn phase0_error(code: &'static str, message: &'static str) -> PortError {
    PortError::new(code, message)
}
