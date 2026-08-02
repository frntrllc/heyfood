use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use heyfood_application::{
    BoundAgentHouseholdOutcomeReceiptV1, BoundAgentHouseholdProposalV1, BoundAgentHouseholdReadV1,
    BoxFuture, HouseholdAgentPhase0Port, HouseholdAgentPhase0Proof, PortError,
};
use heyfood_core::{
    AGENT_HOUSEHOLD_CONTRACT_VERSION, AccountId, AgentHouseholdMemberProjectionV1,
    AgentHouseholdNextActionV1, AgentHouseholdOperationV1, AgentHouseholdOutcomeReceiptV1,
    AgentHouseholdPrepareRequestV1, AgentHouseholdProjectionV1, AgentHouseholdProposalIdV1,
    AgentHouseholdProposalPresentationV1, AgentHouseholdProposalStateV1,
    AgentHouseholdReadRequestV1, AgentHouseholdReadSnapshotV1, AgentHouseholdRetryClassV1,
    AgentHouseholdSubjectV1, CanonicalTimestampV1, DisplayName, GenerationId, HouseholdRevision,
    HouseholdScope, HouseholdSubjectId, MemberId,
};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const PROOF_MANIFEST: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/release-evidence/agent-household-phase0/phase0-proof-manifest.json"
));
const PROOF_MANIFEST_SHA256: &str =
    "3b828c9749d778ace37776b399ace40771a790e6aa955b6e71ceb0ab47b16ddf";

struct FixtureHouseholdAgentPort {
    account: AccountId,
    member: MemberId,
    proposal_ref: AgentHouseholdProposalIdV1,
    household_revision: AtomicU64,
    disclosure_generation: AtomicU64,
    disclosure_revoked: AtomicBool,
    proposal: Mutex<Option<AgentHouseholdProposalPresentationV1>>,
}

impl FixtureHouseholdAgentPort {
    fn new() -> Self {
        Self {
            account: account(),
            member: MemberId::parse_preserved("10000000-0000-4000-8000-000000000001")
                .expect("member"),
            proposal_ref: AgentHouseholdProposalIdV1::from_uuid(
                Uuid::parse_str("20000000-0000-4000-8000-000000000001").expect("proposal"),
            ),
            household_revision: AtomicU64::new(7),
            disclosure_generation: AtomicU64::new(3),
            disclosure_revoked: AtomicBool::new(false),
            proposal: Mutex::new(None),
        }
    }

    fn current_revision(&self) -> HouseholdRevision {
        HouseholdRevision::new(self.household_revision.load(Ordering::SeqCst))
            .expect("fixture revision")
    }

    fn current_generation(&self) -> GenerationId {
        GenerationId::new(self.disclosure_generation.load(Ordering::SeqCst))
    }

    fn presentation(
        &self,
        request: &AgentHouseholdPrepareRequestV1,
    ) -> AgentHouseholdProposalPresentationV1 {
        AgentHouseholdProposalPresentationV1 {
            schema_version: AGENT_HOUSEHOLD_CONTRACT_VERSION,
            proposal_ref: self.proposal_ref,
            operation: request.operation,
            state: AgentHouseholdProposalStateV1::AwaitingLocalInput,
            projection: request.requested_projection,
            disclosure_generation: self.current_generation(),
            affected_member_ref: Some(self.member.clone()),
            affected_member_label: Some(DisplayName::parse("Fixture Adult").expect("label")),
            profile_change_count: Some(0),
            created_at: timestamp("2026-08-02T12:00:00.000Z"),
            expires_at: timestamp("2026-08-02T12:10:00.000Z"),
            human_status: AgentHouseholdProposalStateV1::AwaitingLocalInput
                .human_status()
                .to_owned(),
            handoff_command: "heyfood".to_owned(),
            handoff_instruction: "Open `/household changes` to review this change locally."
                .to_owned(),
        }
    }
}

impl HouseholdAgentPhase0Port for FixtureHouseholdAgentPort {
    fn read(
        &self,
        account: AccountId,
        request: AgentHouseholdReadRequestV1,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<BoundAgentHouseholdReadV1, PortError>> {
        Box::pin(async move {
            if account != self.account {
                return Err(PortError::new(
                    "household_account_mismatch",
                    "fixture read belongs to another account",
                ));
            }
            if self.disclosure_revoked.load(Ordering::SeqCst) {
                return Err(PortError::new(
                    "household_agent_disclosure_revoked",
                    "household disclosure was revoked",
                ));
            }
            let resolved_subject = request
                .subject
                .clone()
                .unwrap_or(AgentHouseholdSubjectV1::Member(self.member.clone()));
            Ok(BoundAgentHouseholdReadV1 {
                account: self.account.clone(),
                snapshot: AgentHouseholdReadSnapshotV1 {
                    schema_version: AGENT_HOUSEHOLD_CONTRACT_VERSION,
                    projection: AgentHouseholdProjectionV1::Profile,
                    resolved_subject,
                    resolved_from_active_scope: request.subject.is_none(),
                    active_scope: HouseholdScope::Subject(HouseholdSubjectId::member(
                        self.member.clone(),
                    )),
                    household_revision: self.current_revision(),
                    disclosure_generation: self.current_generation(),
                    eligible_member_count: 2,
                    restricted_member_count: 0,
                    members: vec![AgentHouseholdMemberProjectionV1 {
                        member_ref: self.member.clone(),
                        display_label: DisplayName::parse("Fixture Adult").expect("label"),
                        profile_revision: heyfood_core::ProfileRevision::new(2).ok(),
                    }],
                    next_cursor: None,
                },
            })
        })
    }

    fn prepare(
        &self,
        account: AccountId,
        request: AgentHouseholdPrepareRequestV1,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<BoundAgentHouseholdProposalV1, PortError>> {
        Box::pin(async move {
            if account != self.account {
                return Err(PortError::new(
                    "household_account_mismatch",
                    "fixture proposal belongs to another account",
                ));
            }
            if request.expected_household_revision != self.current_revision() {
                return Err(PortError::new(
                    "household_revision_stale",
                    "household revision changed",
                ));
            }
            let presentation = self.presentation(&request);
            *self.proposal.lock().expect("proposal lock") = Some(presentation.clone());
            Ok(BoundAgentHouseholdProposalV1 {
                account: self.account.clone(),
                presentation,
            })
        })
    }

    fn status(
        &self,
        account: AccountId,
        proposal_ref: AgentHouseholdProposalIdV1,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<BoundAgentHouseholdProposalV1, PortError>> {
        Box::pin(async move {
            if account != self.account || proposal_ref != self.proposal_ref {
                return Err(PortError::new(
                    "household_account_mismatch",
                    "fixture status is not account bound",
                ));
            }
            let stored = self
                .proposal
                .lock()
                .expect("proposal lock")
                .clone()
                .ok_or_else(|| {
                    PortError::new("household_proposal_stale", "proposal is unavailable")
                })?;
            let presentation = if self.disclosure_revoked.load(Ordering::SeqCst) {
                stored.filtered_to(AgentHouseholdProjectionV1::ContentFree)
            } else {
                stored
            };
            Ok(BoundAgentHouseholdProposalV1 {
                account: self.account.clone(),
                presentation,
            })
        })
    }

    fn cancel(
        &self,
        account: AccountId,
        proposal_ref: AgentHouseholdProposalIdV1,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<BoundAgentHouseholdOutcomeReceiptV1, PortError>> {
        Box::pin(async move {
            if account != self.account || proposal_ref != self.proposal_ref {
                return Err(PortError::new(
                    "household_account_mismatch",
                    "fixture cancellation is not account bound",
                ));
            }
            let before = self.current_revision();
            if let Some(proposal) = self.proposal.lock().expect("proposal lock").as_mut() {
                proposal.state = AgentHouseholdProposalStateV1::Cancelled;
                proposal.human_status = proposal.state.human_status().to_owned();
            }
            Ok(BoundAgentHouseholdOutcomeReceiptV1 {
                account: self.account.clone(),
                receipt: AgentHouseholdOutcomeReceiptV1 {
                    schema_version: AGENT_HOUSEHOLD_CONTRACT_VERSION,
                    proposal_ref,
                    state: AgentHouseholdProposalStateV1::Cancelled,
                    household_revision_before: before,
                    household_revision_after: Some(self.current_revision()),
                    known_no_household_mutation: true,
                    retry_class: AgentHouseholdRetryClassV1::NotApplicable,
                    next_action: AgentHouseholdNextActionV1::None,
                },
            })
        })
    }
}

fn account() -> AccountId {
    AccountId::parse("phase0-household-account").expect("account")
}

fn timestamp(value: &str) -> CanonicalTimestampV1 {
    CanonicalTimestampV1::parse(value).expect("timestamp")
}

fn assert_internal_manifest() {
    let normalized = std::str::from_utf8(PROOF_MANIFEST)
        .expect("proof manifest UTF-8")
        .replace("\r\n", "\n");
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized.as_bytes())),
        PROOF_MANIFEST_SHA256
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(PROOF_MANIFEST).expect("proof manifest JSON");
    assert_eq!(manifest["proof_only"], true);
    assert_eq!(manifest["public_command"], false);
    assert_eq!(manifest["public_mcp_tool"], false);
    assert_eq!(manifest["public_manifest_changed"], false);
    assert_eq!(manifest["schema_version"], 0);
}

#[test]
fn phase0_does_not_expand_the_public_manifest_schema_index_or_mcp_inventory() {
    let manifest = heyfood_agent_contract::manifest();
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(manifest["commands"].as_array().map(Vec::len), Some(30));
    assert_eq!(heyfood_agent_contract::PUBLIC_SCHEMAS.len(), 11);
    let encoded = heyfood_agent_contract::canonical_json(&manifest);
    assert!(!encoded.contains("heyfood_get_household"));
    assert!(!encoded.contains("prepare_household_change"));

    let expected = [
        (
            "heyfood_get_manifest",
            "99334726611ccf58a148b0814696bfa6fe08c1b2d027e946beccf5a74331c9aa",
            "3e56ca65de2344f97641314242e6a81695de934351aeb00d39e46ef29ea8451c",
        ),
        (
            "heyfood_get_status",
            "99334726611ccf58a148b0814696bfa6fe08c1b2d027e946beccf5a74331c9aa",
            "b211f6a5ead5ce01024f917b5a9dbd7a4478bc09c86727b439e6a435dfb3558d",
        ),
        (
            "heyfood_get_capabilities",
            "99334726611ccf58a148b0814696bfa6fe08c1b2d027e946beccf5a74331c9aa",
            "5e9d9e43208aa8a5fc529d86b015652fe113ea1031c7f88fad3198e404d5b510",
        ),
        (
            "heyfood_get_grocery_list",
            "5125c1a7d77fb5a748f1008689b52612a94bf6d3c64264d15c0a6edb7f6e596b",
            "e2198ddbc3fd192b19b45509ed67224e7caee4462c78b623eb5fd195c1d0d81f",
        ),
        (
            "heyfood_get_grocery_exclusions",
            "5125c1a7d77fb5a748f1008689b52612a94bf6d3c64264d15c0a6edb7f6e596b",
            "2ff5349b4c6e5641c6617f6d9484014dc361344e3f8a1a61682f28053387ca0b",
        ),
        (
            "heyfood_list_menu_watches",
            "5125c1a7d77fb5a748f1008689b52612a94bf6d3c64264d15c0a6edb7f6e596b",
            "a610b384b5a3a95acf59a5c0912ccf48853bb4923717790083b7514c1c91de93",
        ),
    ];
    let tools = heyfood_mcp::HeyfoodMcpServer::tools();
    assert_eq!(tools.len(), expected.len());
    for (tool, (name, input_sha256, result_sha256)) in tools.iter().zip(expected) {
        assert_eq!(tool.name, name);
        let input = serde_json::to_vec(tool.input_schema.as_ref()).expect("tool input schema");
        let result = serde_json::to_vec(tool.output_schema.as_deref().expect("tool result schema"))
            .expect("tool result schema");
        assert_eq!(format!("{:x}", Sha256::digest(input)), input_sha256);
        assert_eq!(format!("{:x}", Sha256::digest(result)), result_sha256);
    }
}

#[tokio::test]
async fn bin_composes_read_prepare_revocation_safe_status_and_non_mutating_cancel() {
    assert_internal_manifest();
    let port = Arc::new(FixtureHouseholdAgentPort::new());
    let controller = HouseholdAgentPhase0Proof::new(port.clone());
    let generation = GenerationId::new(3);
    let read = controller
        .read(
            account(),
            AgentHouseholdReadRequestV1 {
                subject: None,
                requested_projection: AgentHouseholdProjectionV1::Profile,
                expected_disclosure_generation: generation,
                cursor: None,
                limit: 10,
            },
            CancellationToken::new(),
        )
        .await
        .expect("account-bound read");
    assert!(read.resolved_from_active_scope);
    assert_eq!(read.members.len(), 1);

    let revision_before = port.current_revision();
    let prepared = controller
        .prepare(
            account(),
            AgentHouseholdPrepareRequestV1 {
                operation: AgentHouseholdOperationV1::Edit,
                requested_projection: AgentHouseholdProjectionV1::Profile,
                expected_disclosure_generation: generation,
                expected_household_revision: revision_before,
                affected_member_ref: Some(port.member.clone()),
                bundled_scope: None,
            },
            CancellationToken::new(),
        )
        .await
        .expect("non-mutating prepare");
    assert_eq!(
        prepared.state,
        AgentHouseholdProposalStateV1::AwaitingLocalInput
    );
    assert_eq!(port.current_revision(), revision_before);

    port.disclosure_revoked.store(true, Ordering::SeqCst);
    port.disclosure_generation.store(4, Ordering::SeqCst);
    let status = controller
        .status(account(), port.proposal_ref, CancellationToken::new())
        .await
        .expect("content-free status after revocation");
    assert_eq!(status.projection, AgentHouseholdProjectionV1::ContentFree);
    assert!(status.affected_member_ref.is_none());
    assert!(status.affected_member_label.is_none());
    assert!(status.profile_change_count.is_none());

    let receipt = controller
        .cancel(account(), port.proposal_ref, CancellationToken::new())
        .await
        .expect("pre-dispatch cancel");
    assert!(receipt.known_no_household_mutation);
    assert_eq!(receipt.household_revision_before, revision_before);
    assert_eq!(receipt.household_revision_after, Some(revision_before));
}

#[tokio::test]
async fn bin_composition_rejects_cross_account_and_pre_dispatch_cancellation() {
    assert_internal_manifest();
    let port = Arc::new(FixtureHouseholdAgentPort::new());
    let controller = HouseholdAgentPhase0Proof::new(port.clone());

    let invalid = controller
        .prepare(
            account(),
            AgentHouseholdPrepareRequestV1 {
                operation: AgentHouseholdOperationV1::Scope,
                requested_projection: AgentHouseholdProjectionV1::ContentFree,
                expected_disclosure_generation: GenerationId::new(3),
                expected_household_revision: port.current_revision(),
                affected_member_ref: None,
                bundled_scope: None,
            },
            CancellationToken::new(),
        )
        .await
        .expect_err("invalid operation shape fails before dispatch");
    assert_eq!(invalid.code, "household_agent_operation_shape");
    assert!(port.proposal.lock().expect("proposal lock").is_none());

    let error = controller
        .read(
            AccountId::parse("another-account").expect("account"),
            AgentHouseholdReadRequestV1 {
                subject: Some(AgentHouseholdSubjectV1::Self_),
                requested_projection: AgentHouseholdProjectionV1::Roster,
                expected_disclosure_generation: GenerationId::new(3),
                cursor: None,
                limit: 1,
            },
            CancellationToken::new(),
        )
        .await
        .expect_err("cross-account read fails closed");
    assert_eq!(error.code, "household_account_mismatch");

    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let error = controller
        .read(
            account(),
            AgentHouseholdReadRequestV1 {
                subject: Some(AgentHouseholdSubjectV1::Self_),
                requested_projection: AgentHouseholdProjectionV1::Roster,
                expected_disclosure_generation: GenerationId::new(3),
                cursor: None,
                limit: 1,
            },
            cancellation,
        )
        .await
        .expect_err("pre-dispatch cancellation fails closed");
    assert_eq!(error.code, "household_agent_cancelled_before_dispatch");
}
