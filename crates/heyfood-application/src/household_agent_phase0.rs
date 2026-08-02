//! Non-routable Phase-0 composition proof for household agent contracts.
//!
//! The controller proves application/port boundaries for read, prepare,
//! status, and pre-dispatch cancellation. No CLI command or MCP tool composes
//! this controller in Phase 0.

use std::{fmt, sync::Arc};

use heyfood_core::{
    AGENT_HOUSEHOLD_CONTRACT_VERSION, AGENT_HOUSEHOLD_MAX_MEMBERS_PER_PAGE, AccountId,
    AgentHouseholdOutcomeReceiptV1, AgentHouseholdPrepareRequestV1, AgentHouseholdProjectionV1,
    AgentHouseholdProposalIdV1, AgentHouseholdProposalPresentationV1,
    AgentHouseholdProposalStateV1, AgentHouseholdReadRequestV1, AgentHouseholdReadSnapshotV1,
};
use tokio_util::sync::CancellationToken;

use crate::{HouseholdAgentPhase0Port, PortError};

#[derive(Clone, Eq, PartialEq)]
pub struct BoundAgentHouseholdReadV1 {
    pub account: AccountId,
    pub snapshot: AgentHouseholdReadSnapshotV1,
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
}

impl fmt::Debug for BoundAgentHouseholdProposalV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundAgentHouseholdProposalV1")
            .field("presentation", &self.presentation)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundAgentHouseholdOutcomeReceiptV1 {
    pub account: AccountId,
    pub receipt: AgentHouseholdOutcomeReceiptV1,
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
        if request.limit == 0 || request.limit > AGENT_HOUSEHOLD_MAX_MEMBERS_PER_PAGE {
            return Err(phase0_error(
                "household_agent_read_limit",
                "household agent read limit is outside the closed contract",
            ));
        }
        let result = self
            .port
            .read(account.clone(), request.clone(), cancellation)
            .await?;
        ensure_account(&account, &result.account)?;
        validate_read(&request, &result.snapshot)?;
        Ok(result.snapshot)
    }

    pub async fn prepare(
        &self,
        account: AccountId,
        request: AgentHouseholdPrepareRequestV1,
        cancellation: CancellationToken,
    ) -> Result<AgentHouseholdProposalPresentationV1, PortError> {
        check_cancelled(&cancellation)?;
        request.validate_shape().map_err(|_| {
            phase0_error(
                "household_agent_operation_shape",
                "household agent proposal operation shape is invalid",
            )
        })?;
        let result = self
            .port
            .prepare(account.clone(), request.clone(), cancellation)
            .await?;
        ensure_account(&account, &result.account)?;
        validate_presentation(
            &result.presentation,
            request.requested_projection,
            Some(request.expected_disclosure_generation),
        )?;
        Ok(result.presentation)
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
            .status(account.clone(), proposal_ref, cancellation)
            .await?;
        ensure_account(&account, &result.account)?;
        if result.presentation.proposal_ref != proposal_ref {
            return Err(phase0_error(
                "household_agent_proposal_mismatch",
                "household agent proposal reference changed across status",
            ));
        }
        validate_presentation(&result.presentation, result.presentation.projection, None)?;
        Ok(result.presentation)
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
        if receipt.receipt.proposal_ref != proposal_ref
            || receipt.receipt.state != AgentHouseholdProposalStateV1::Cancelled
            || !receipt.receipt.known_no_household_mutation
            || receipt.receipt.household_revision_after
                != Some(receipt.receipt.household_revision_before)
        {
            return Err(phase0_error(
                "household_agent_cancel_unproven",
                "household cancellation did not prove a non-mutating outcome",
            ));
        }
        Ok(receipt.receipt)
    }
}

fn validate_read(
    request: &AgentHouseholdReadRequestV1,
    snapshot: &AgentHouseholdReadSnapshotV1,
) -> Result<(), PortError> {
    if snapshot.schema_version != AGENT_HOUSEHOLD_CONTRACT_VERSION
        || snapshot.disclosure_generation != request.expected_disclosure_generation
    {
        return Err(phase0_error(
            "household_agent_read_generation",
            "household agent read contract or disclosure generation changed",
        ));
    }
    if projection_rank(snapshot.projection) > projection_rank(request.requested_projection) {
        return Err(phase0_error(
            "household_agent_projection_escalation",
            "household agent read exceeded the authorized projection",
        ));
    }
    if request.subject.is_none() != snapshot.resolved_from_active_scope {
        return Err(phase0_error(
            "household_agent_subject_resolution",
            "household agent subject resolution evidence is inconsistent",
        ));
    }
    if let Some(subject) = request.subject.as_ref()
        && subject != &snapshot.resolved_subject
    {
        return Err(phase0_error(
            "household_agent_subject_resolution",
            "household agent read resolved a different subject",
        ));
    }
    if snapshot.projection == AgentHouseholdProjectionV1::ContentFree
        && !snapshot.members.is_empty()
    {
        return Err(phase0_error(
            "household_agent_projection_leak",
            "content-free household projection included member data",
        ));
    }
    if snapshot.members.len() > usize::from(request.limit) {
        return Err(phase0_error(
            "household_agent_read_limit",
            "household agent read exceeded the requested page limit",
        ));
    }
    Ok(())
}

fn validate_presentation(
    presentation: &AgentHouseholdProposalPresentationV1,
    maximum_projection: AgentHouseholdProjectionV1,
    expected_generation: Option<heyfood_core::GenerationId>,
) -> Result<(), PortError> {
    if presentation.schema_version != AGENT_HOUSEHOLD_CONTRACT_VERSION
        || expected_generation.is_some_and(|value| value != presentation.disclosure_generation)
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
                || presentation.profile_change_count.is_some() =>
        {
            Err(phase0_error(
                "household_agent_projection_leak",
                "content-free proposal status included household content",
            ))
        }
        AgentHouseholdProjectionV1::Roster if presentation.profile_change_count.is_some() => {
            Err(phase0_error(
                "household_agent_projection_leak",
                "roster-only proposal status included profile content",
            ))
        }
        _ => Ok(()),
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
