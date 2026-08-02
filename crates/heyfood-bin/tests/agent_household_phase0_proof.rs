use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use heyfood_application::{
    AuthorizedAgentHouseholdPrepareV1, BoundAgentHouseholdDisclosureV1,
    BoundAgentHouseholdOutcomeReceiptV1, BoundAgentHouseholdProposalV1, BoundAgentHouseholdReadV1,
    BoundAgentHouseholdRosterAuthorityV1, BoxFuture, FrozenAgentHouseholdDisclosureV1,
    HouseholdAgentPhase0Port, HouseholdAgentPhase0Proof, PortError,
};
use heyfood_core::{
    AGENT_HOUSEHOLD_CONTRACT_VERSION, AccountId, AgentDisclosureDataClassV1,
    AgentDisclosureGrantSetV1, AgentDisclosureGrantStateV1, AgentDisclosureGrantSubjectV1,
    AgentDisclosureGrantV1, AgentDisclosureGrantingAuthorityV1, AgentDisclosurePurposeV1,
    AgentHouseholdChangeFieldV1, AgentHouseholdChangeV1, AgentHouseholdConsequenceV1,
    AgentHouseholdContextInputV1, AgentHouseholdMemberInputV1, AgentHouseholdMemberProjectionV1,
    AgentHouseholdOperationV1, AgentHouseholdOutcomeReceiptV1, AgentHouseholdPrepareRequestKindV1,
    AgentHouseholdPrepareRequestV1, AgentHouseholdProjectionV1, AgentHouseholdProposalIdV1,
    AgentHouseholdProposalPresentationV1, AgentHouseholdProposalRefInputV1,
    AgentHouseholdProposalStateV1, AgentHouseholdReadRequestKindV1, AgentHouseholdReadRequestV1,
    AgentHouseholdReadResultKindV1, AgentHouseholdReadSnapshotV1, AgentHouseholdRecoverabilityV1,
    AgentHouseholdSubjectV1, AgentMinimizedDeclaredProfileV1, CanonicalTimestampV1, DisplayName,
    GenerationId, HouseholdLifecycleV1, HouseholdProfileStateV1, HouseholdRevision, HouseholdScope,
    HouseholdSubjectId, MemberId, MinorStatusV1, RelationshipV1,
};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

const PROOF_MANIFEST: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/release-evidence/agent-household-phase0/phase0-proof-manifest.json"
));
const PROOF_MANIFEST_SHA256: &str =
    "9f9581eb5d142b4f5abd1a5a7535be9edaf0a628790cbcc84db06fa471353ace";

struct FixtureHouseholdAgentPort {
    account: AccountId,
    member: MemberId,
    proposal_ref: Mutex<Option<AgentHouseholdProposalIdV1>>,
    household_revision: AtomicU64,
    disclosure_generation: AtomicU64,
    disclosure_revoked: AtomicBool,
    disclosure_age_mode: AtomicU64,
    disclosure_revision: AtomicU64,
    disclosure_calls: AtomicU64,
    rotate_disclosure_after_first: AtomicBool,
    disclosure_wrong_account: AtomicBool,
    disclosure_observed_after_expiry: AtomicBool,
    expire_disclosure_after_first: AtomicBool,
    returned_member_override: Mutex<Option<MemberId>>,
    active_scope_override: Mutex<Option<HouseholdScope>>,
    authoritative_members: Mutex<Vec<MemberId>>,
    proposal_member_override: Mutex<Option<MemberId>>,
    status_accepts_any_requested_ref: AtomicBool,
    invalid_read_wire: AtomicBool,
    invalid_read_count_wire: AtomicBool,
    eligible_count_override: AtomicU64,
    invalid_proposal_wire: AtomicBool,
    proposal: Mutex<Option<StoredFixtureProposal>>,
}

#[derive(Clone)]
struct StoredFixtureProposal {
    presentation: AgentHouseholdProposalPresentationV1,
    frozen_disclosure: FrozenAgentHouseholdDisclosureV1,
}

impl FixtureHouseholdAgentPort {
    fn new() -> Self {
        Self {
            account: account(),
            member: MemberId::parse_preserved("10000000-0000-4000-8000-000000000001")
                .expect("member"),
            proposal_ref: Mutex::new(None),
            household_revision: AtomicU64::new(7),
            disclosure_generation: AtomicU64::new(3),
            disclosure_revoked: AtomicBool::new(false),
            disclosure_age_mode: AtomicU64::new(0),
            disclosure_revision: AtomicU64::new(5),
            disclosure_calls: AtomicU64::new(0),
            rotate_disclosure_after_first: AtomicBool::new(false),
            disclosure_wrong_account: AtomicBool::new(false),
            disclosure_observed_after_expiry: AtomicBool::new(false),
            expire_disclosure_after_first: AtomicBool::new(false),
            returned_member_override: Mutex::new(None),
            active_scope_override: Mutex::new(None),
            authoritative_members: Mutex::new(vec![
                MemberId::parse_preserved("10000000-0000-4000-8000-000000000001").expect("member"),
            ]),
            proposal_member_override: Mutex::new(None),
            status_accepts_any_requested_ref: AtomicBool::new(false),
            invalid_read_wire: AtomicBool::new(false),
            invalid_read_count_wire: AtomicBool::new(false),
            eligible_count_override: AtomicU64::new(0),
            invalid_proposal_wire: AtomicBool::new(false),
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

    fn current_proposal_ref(&self) -> AgentHouseholdProposalIdV1 {
        self.proposal_ref
            .lock()
            .expect("proposal ref lock")
            .expect("prepared proposal ref")
    }

    fn presentation(
        &self,
        request: &AgentHouseholdPrepareRequestV1,
        proposal_ref: AgentHouseholdProposalIdV1,
    ) -> AgentHouseholdProposalPresentationV1 {
        AgentHouseholdProposalPresentationV1 {
            schema_version: AGENT_HOUSEHOLD_CONTRACT_VERSION,
            proposal_ref,
            operation: request.operation,
            state: AgentHouseholdProposalStateV1::AwaitingLocalInput,
            projection: request.requested_projection,
            disclosure_generation: self.current_generation(),
            affected_member_ref: match request.operation {
                AgentHouseholdOperationV1::Edit
                | AgentHouseholdOperationV1::Archive
                | AgentHouseholdOperationV1::Restore => Some(
                    self.proposal_member_override
                        .lock()
                        .expect("proposal override lock")
                        .clone()
                        .unwrap_or_else(|| self.member.clone()),
                ),
                AgentHouseholdOperationV1::Add | AgentHouseholdOperationV1::Scope => None,
            },
            affected_member_label: matches!(
                request.operation,
                AgentHouseholdOperationV1::Edit
                    | AgentHouseholdOperationV1::Archive
                    | AgentHouseholdOperationV1::Restore
            )
            .then(|| DisplayName::parse("Fixture Adult").expect("label")),
            changes: vec![AgentHouseholdChangeV1 {
                field: AgentHouseholdChangeFieldV1::Allergies,
                before: vec!["milk".to_owned()],
                after: vec!["milk".to_owned(), "egg".to_owned()],
            }],
            consequences: vec![AgentHouseholdConsequenceV1::ConversationContinuityReset],
            recoverability: AgentHouseholdRecoverabilityV1::EditableBeforeSave,
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
    fn eligible_roster(
        &self,
        account: AccountId,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<BoundAgentHouseholdRosterAuthorityV1, PortError>> {
        Box::pin(async move {
            if account != self.account {
                return Err(PortError::new(
                    "household_account_mismatch",
                    "fixture roster authority belongs to another account",
                ));
            }
            let mut eligible_subjects = vec![AgentDisclosureGrantSubjectV1::Self_];
            eligible_subjects.extend(
                self.authoritative_members
                    .lock()
                    .expect("authoritative members lock")
                    .iter()
                    .cloned()
                    .map(AgentDisclosureGrantSubjectV1::Member),
            );
            Ok(BoundAgentHouseholdRosterAuthorityV1 {
                account: self.account.clone(),
                household_revision: self.current_revision(),
                eligible_subjects,
            })
        })
    }

    fn disclosure(
        &self,
        account: AccountId,
        purpose: AgentDisclosurePurposeV1,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<BoundAgentHouseholdDisclosureV1, PortError>> {
        Box::pin(async move {
            if account != self.account {
                return Err(PortError::new(
                    "household_account_mismatch",
                    "fixture disclosure belongs to another account",
                ));
            }
            let generation = self.current_generation();
            let call = self.disclosure_calls.fetch_add(1, Ordering::SeqCst);
            let revision = self.disclosure_revision.load(Ordering::SeqCst)
                + u64::from(self.rotate_disclosure_after_first.load(Ordering::SeqCst) && call > 0);
            let (minor_status, data_classes, authority) =
                match self.disclosure_age_mode.load(Ordering::SeqCst) {
                    0 => (
                        MinorStatusV1::Adult,
                        vec![
                            AgentDisclosureDataClassV1::Roster,
                            AgentDisclosureDataClassV1::MinimizedDeclaredProfile,
                        ],
                        AgentDisclosureGrantingAuthorityV1::AccountOwnerAdultAuthorization,
                    ),
                    1 => (
                        MinorStatusV1::Minor,
                        vec![AgentDisclosureDataClassV1::Roster],
                        AgentDisclosureGrantingAuthorityV1::AuthorizedGuardianRosterOnly,
                    ),
                    _ => (
                        MinorStatusV1::Unknown,
                        vec![AgentDisclosureDataClassV1::Roster],
                        AgentDisclosureGrantingAuthorityV1::AuthorizedGuardianRosterOnly,
                    ),
                };
            let observed_at = if self.disclosure_observed_after_expiry.load(Ordering::SeqCst)
                || (self.expire_disclosure_after_first.load(Ordering::SeqCst) && call > 0)
            {
                timestamp("2026-08-02T12:21:00.000Z")
            } else {
                timestamp("2026-08-02T12:05:00.000Z")
            };
            let grants = if self.disclosure_revoked.load(Ordering::SeqCst) {
                Vec::new()
            } else {
                vec![
                    AgentDisclosureGrantV1::new(
                        self.account.clone(),
                        AgentDisclosureGrantSubjectV1::Member(self.member.clone()),
                        minor_status,
                        data_classes,
                        purpose,
                        authority,
                        revision,
                        generation,
                        AgentDisclosureGrantStateV1::Active,
                        timestamp("2026-08-02T11:59:00.000Z"),
                        Some(timestamp("2026-08-02T12:20:00.000Z")),
                    )
                    .expect("fixture disclosure"),
                ]
            };
            let mut bound = BoundAgentHouseholdDisclosureV1 {
                account: self.account.clone(),
                grants: AgentDisclosureGrantSetV1::new(
                    self.account.clone(),
                    generation,
                    purpose,
                    observed_at,
                    grants,
                )
                .expect("fixture grant set"),
            };
            if self.disclosure_wrong_account.load(Ordering::SeqCst) {
                bound.account = AccountId::parse("wrong-disclosure-account").expect("account");
            }
            Ok(bound)
        })
    }

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
            let resolved_subject = request
                .subject
                .clone()
                .unwrap_or(AgentHouseholdSubjectV1::Member(self.member.clone()));
            let projected_member = self
                .returned_member_override
                .lock()
                .expect("returned member override lock")
                .clone()
                .unwrap_or_else(|| self.member.clone());
            let mut members = match &resolved_subject {
                AgentHouseholdSubjectV1::Self_ => self
                    .returned_member_override
                    .lock()
                    .expect("returned member override lock")
                    .is_some()
                    .then(|| member_projection(projected_member.clone()))
                    .into_iter()
                    .collect(),
                AgentHouseholdSubjectV1::Member(_) | AgentHouseholdSubjectV1::Everyone => {
                    vec![member_projection(projected_member.clone())]
                }
            };
            if self.invalid_read_wire.load(Ordering::SeqCst)
                && let Some(profile) = members
                    .first_mut()
                    .and_then(|member| member.minimized_declared_profile.as_mut())
            {
                profile.allergies = vec!["milk".to_owned(), "milk".to_owned()];
            }
            let default_active_scope = if request.subject.is_none() {
                match &resolved_subject {
                    AgentHouseholdSubjectV1::Self_ => {
                        HouseholdScope::Subject(HouseholdSubjectId::self_())
                    }
                    AgentHouseholdSubjectV1::Member(member) => {
                        HouseholdScope::Subject(HouseholdSubjectId::member(member.clone()))
                    }
                    AgentHouseholdSubjectV1::Everyone => HouseholdScope::Everyone,
                }
            } else {
                HouseholdScope::Subject(HouseholdSubjectId::member(self.member.clone()))
            };
            let active_scope = self
                .active_scope_override
                .lock()
                .expect("active scope override lock")
                .clone()
                .unwrap_or(default_active_scope);
            let eligible_count_override = self.eligible_count_override.load(Ordering::SeqCst);
            Ok(BoundAgentHouseholdReadV1 {
                account: self.account.clone(),
                snapshot: AgentHouseholdReadSnapshotV1 {
                    schema_version: AGENT_HOUSEHOLD_CONTRACT_VERSION,
                    kind: AgentHouseholdReadResultKindV1::HouseholdReadResult,
                    projection: AgentHouseholdProjectionV1::Profile,
                    resolved_subject: Some(resolved_subject),
                    resolved_from_active_scope: request.subject.is_none(),
                    active_scope: Some(active_scope),
                    household_revision: self.current_revision(),
                    disclosure_generation: self.current_generation(),
                    eligible_member_count: if self.invalid_read_count_wire.load(Ordering::SeqCst) {
                        101
                    } else if eligible_count_override > 0 {
                        u16::try_from(eligible_count_override).expect("eligible count override")
                    } else {
                        2
                    },
                    restricted_member_count: 0,
                    members,
                    next_cursor: None,
                },
            })
        })
    }

    fn prepare(
        &self,
        account: AccountId,
        request: AuthorizedAgentHouseholdPrepareV1,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<BoundAgentHouseholdProposalV1, PortError>> {
        Box::pin(async move {
            if account != self.account {
                return Err(PortError::new(
                    "household_account_mismatch",
                    "fixture proposal belongs to another account",
                ));
            }
            if request.request.expected_household_revision != self.current_revision() {
                return Err(PortError::new(
                    "household_revision_stale",
                    "household revision changed",
                ));
            }
            let proposal_ref = request.prepared_disclosure.proposal_ref();
            *self.proposal_ref.lock().expect("proposal ref lock") = Some(proposal_ref);
            let mut presentation = self.presentation(&request.request, proposal_ref);
            if self.invalid_proposal_wire.load(Ordering::SeqCst) {
                presentation.changes[0].after = vec!["line\nfeed".to_owned()];
            }
            presentation = presentation.filtered_to(request.maximum_projection);
            let frozen_disclosure = request.prepared_disclosure.freeze();
            *self.proposal.lock().expect("proposal lock") = Some(StoredFixtureProposal {
                presentation: presentation.clone(),
                frozen_disclosure: frozen_disclosure.clone(),
            });
            Ok(BoundAgentHouseholdProposalV1 {
                account: self.account.clone(),
                presentation,
                frozen_disclosure,
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
            if account != self.account
                || (!self.status_accepts_any_requested_ref.load(Ordering::SeqCst)
                    && self
                        .proposal_ref
                        .lock()
                        .expect("proposal ref lock")
                        .as_ref()
                        != Some(&proposal_ref))
            {
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
            Ok(BoundAgentHouseholdProposalV1 {
                account: self.account.clone(),
                presentation: stored.presentation,
                frozen_disclosure: stored.frozen_disclosure,
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
            if account != self.account
                || self
                    .proposal_ref
                    .lock()
                    .expect("proposal ref lock")
                    .as_ref()
                    != Some(&proposal_ref)
            {
                return Err(PortError::new(
                    "household_account_mismatch",
                    "fixture cancellation is not account bound",
                ));
            }
            let before = self.current_revision();
            if let Some(proposal) = self.proposal.lock().expect("proposal lock").as_mut() {
                proposal.presentation.state = AgentHouseholdProposalStateV1::Cancelled;
                proposal.presentation.human_status =
                    proposal.presentation.state.human_status().to_owned();
            }
            Ok(BoundAgentHouseholdOutcomeReceiptV1 {
                account: self.account.clone(),
                receipt: AgentHouseholdOutcomeReceiptV1::cancelled(proposal_ref, before),
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

fn member_projection(member_ref: MemberId) -> AgentHouseholdMemberProjectionV1 {
    AgentHouseholdMemberProjectionV1 {
        member_ref,
        display_label: DisplayName::parse("Fixture Adult").expect("label"),
        relationship: RelationshipV1::Friend,
        lifecycle: HouseholdLifecycleV1::Active,
        profile_state: HouseholdProfileStateV1::LocalOnly,
        profile_schema_version: Some(1),
        profile_revision: heyfood_core::ProfileRevision::new(2).ok(),
        profile_complete: true,
        minimized_declared_profile: Some(AgentMinimizedDeclaredProfileV1 {
            diet_styles: vec!["vegan".to_owned()],
            allergies: vec!["milk".to_owned()],
            restrictions: Vec::new(),
            health_conditions: Vec::new(),
            avoid_ingredients: vec!["celery".to_owned()],
        }),
    }
}

fn member_read_request(projection: AgentHouseholdProjectionV1) -> AgentHouseholdReadRequestV1 {
    AgentHouseholdReadRequestV1 {
        schema_version: AGENT_HOUSEHOLD_CONTRACT_VERSION,
        kind: AgentHouseholdReadRequestKindV1::HouseholdReadRequest,
        subject: Some(AgentHouseholdSubjectV1::Member(
            MemberId::parse_preserved("10000000-0000-4000-8000-000000000001").expect("member"),
        )),
        requested_projection: projection,
        expected_disclosure_generation: GenerationId::new(3),
        cursor: None,
        limit: 10,
    }
}

fn edit_prepare_request() -> AgentHouseholdPrepareRequestV1 {
    AgentHouseholdPrepareRequestV1 {
        schema_version: AGENT_HOUSEHOLD_CONTRACT_VERSION,
        kind: AgentHouseholdPrepareRequestKindV1::PrepareHouseholdChange,
        operation: AgentHouseholdOperationV1::Edit,
        requested_projection: AgentHouseholdProjectionV1::Profile,
        expected_disclosure_generation: GenerationId::new(3),
        expected_household_revision: HouseholdRevision::new(7).expect("revision"),
        affected_member_ref: Some(
            MemberId::parse_preserved("10000000-0000-4000-8000-000000000001").expect("member"),
        ),
        bundled_scope: None,
    }
}

fn scope_prepare_request() -> AgentHouseholdPrepareRequestV1 {
    AgentHouseholdPrepareRequestV1 {
        schema_version: AGENT_HOUSEHOLD_CONTRACT_VERSION,
        kind: AgentHouseholdPrepareRequestKindV1::PrepareHouseholdChange,
        operation: AgentHouseholdOperationV1::Scope,
        requested_projection: AgentHouseholdProjectionV1::Profile,
        expected_disclosure_generation: GenerationId::new(3),
        expected_household_revision: HouseholdRevision::new(7).expect("revision"),
        affected_member_ref: None,
        bundled_scope: Some(HouseholdScope::Subject(HouseholdSubjectId::member(
            MemberId::parse_preserved("10000000-0000-4000-8000-000000000001").expect("member"),
        ))),
    }
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
                schema_version: AGENT_HOUSEHOLD_CONTRACT_VERSION,
                kind: AgentHouseholdReadRequestKindV1::HouseholdReadRequest,
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
                schema_version: AGENT_HOUSEHOLD_CONTRACT_VERSION,
                kind: AgentHouseholdPrepareRequestKindV1::PrepareHouseholdChange,
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
        .status(
            account(),
            port.current_proposal_ref(),
            CancellationToken::new(),
        )
        .await
        .expect("content-free status after revocation");
    assert_eq!(status.projection, AgentHouseholdProjectionV1::ContentFree);
    assert_eq!(status.state, AgentHouseholdProposalStateV1::Stale);
    assert!(status.affected_member_ref.is_none());
    assert!(status.affected_member_label.is_none());
    assert!(status.changes.is_empty());

    let receipt = controller
        .cancel(
            account(),
            port.current_proposal_ref(),
            CancellationToken::new(),
        )
        .await
        .expect("pre-dispatch cancel");
    assert!(receipt.known_no_household_mutation());
    assert_eq!(receipt.household_revision_before(), revision_before);
    assert_eq!(receipt.household_revision_after(), Some(revision_before));
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
                schema_version: AGENT_HOUSEHOLD_CONTRACT_VERSION,
                kind: AgentHouseholdPrepareRequestKindV1::PrepareHouseholdChange,
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
                schema_version: AGENT_HOUSEHOLD_CONTRACT_VERSION,
                kind: AgentHouseholdReadRequestKindV1::HouseholdReadRequest,
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
                schema_version: AGENT_HOUSEHOLD_CONTRACT_VERSION,
                kind: AgentHouseholdReadRequestKindV1::HouseholdReadRequest,
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

#[tokio::test]
async fn application_enforces_revoked_minor_unknown_and_cross_account_disclosure() {
    for age_mode in [1, 2] {
        let port = Arc::new(FixtureHouseholdAgentPort::new());
        port.disclosure_age_mode.store(age_mode, Ordering::SeqCst);
        let result = HouseholdAgentPhase0Proof::new(port)
            .read(
                account(),
                member_read_request(AgentHouseholdProjectionV1::Profile),
                CancellationToken::new(),
            )
            .await
            .expect("guardian authority permits roster only");
        assert_eq!(result.projection, AgentHouseholdProjectionV1::Roster);
        assert_eq!(result.members.len(), 1);
        assert!(result.members[0].minimized_declared_profile.is_none());
    }

    let revoked = Arc::new(FixtureHouseholdAgentPort::new());
    revoked.disclosure_revoked.store(true, Ordering::SeqCst);
    let result = HouseholdAgentPhase0Proof::new(revoked)
        .read(
            account(),
            member_read_request(AgentHouseholdProjectionV1::Profile),
            CancellationToken::new(),
        )
        .await
        .expect("revocation produces a content-free result");
    assert_eq!(result.projection, AgentHouseholdProjectionV1::ContentFree);
    assert!(result.resolved_subject.is_none());
    assert!(result.active_scope.is_none());
    assert!(result.members.is_empty());
    assert!(result.next_cursor.is_none());

    let wrong_account = Arc::new(FixtureHouseholdAgentPort::new());
    wrong_account
        .disclosure_wrong_account
        .store(true, Ordering::SeqCst);
    let error = HouseholdAgentPhase0Proof::new(wrong_account)
        .read(
            account(),
            member_read_request(AgentHouseholdProjectionV1::Profile),
            CancellationToken::new(),
        )
        .await
        .expect_err("cross-account disclosure fails closed");
    assert_eq!(error.code, "household_account_mismatch");
}

#[tokio::test]
async fn prepare_revalidates_the_exact_disclosure_revision_set_after_adapter_work() {
    for expires_without_digest_change in [false, true] {
        let port = Arc::new(FixtureHouseholdAgentPort::new());
        if expires_without_digest_change {
            port.expire_disclosure_after_first
                .store(true, Ordering::SeqCst);
        } else {
            port.rotate_disclosure_after_first
                .store(true, Ordering::SeqCst);
        }
        let result = HouseholdAgentPhase0Proof::new(port)
            .prepare(account(), edit_prepare_request(), CancellationToken::new())
            .await
            .expect("authority reduction returns a stale content-free proposal");
        assert_eq!(result.state, AgentHouseholdProposalStateV1::Stale);
        assert_eq!(result.projection, AgentHouseholdProjectionV1::ContentFree);
        assert!(result.affected_member_ref.is_none());
        assert!(result.affected_member_label.is_none());
        assert!(result.changes.is_empty());
        assert!(result.consequences.is_empty());
    }

    let scope_port = Arc::new(FixtureHouseholdAgentPort::new());
    scope_port
        .expire_disclosure_after_first
        .store(true, Ordering::SeqCst);
    let scope = HouseholdAgentPhase0Proof::new(scope_port)
        .prepare(account(), scope_prepare_request(), CancellationToken::new())
        .await
        .expect("content-free Scope still invalidates when subject authority expires");
    assert_eq!(scope.state, AgentHouseholdProposalStateV1::Stale);
    assert_eq!(scope.projection, AgentHouseholdProjectionV1::ContentFree);
    assert!(scope.affected_member_ref.is_none());
    assert!(scope.changes.is_empty());
}

#[tokio::test]
async fn application_rejects_authorized_a_with_returned_b_for_reads_and_prepare() {
    let other_member =
        MemberId::parse_preserved("10000000-0000-4000-8000-000000000002").expect("member B");

    let member_port = Arc::new(FixtureHouseholdAgentPort::new());
    *member_port
        .returned_member_override
        .lock()
        .expect("returned member override lock") = Some(other_member.clone());
    let error = HouseholdAgentPhase0Proof::new(member_port)
        .read(
            account(),
            member_read_request(AgentHouseholdProjectionV1::Profile),
            CancellationToken::new(),
        )
        .await
        .expect_err("member A authority cannot disclose member B");
    assert_eq!(error.code, "household_agent_subject_content_mismatch");

    let self_port = Arc::new(FixtureHouseholdAgentPort::new());
    *self_port
        .returned_member_override
        .lock()
        .expect("returned member override lock") = Some(other_member.clone());
    let error = HouseholdAgentPhase0Proof::new(self_port)
        .read(
            account(),
            AgentHouseholdReadRequestV1 {
                schema_version: AGENT_HOUSEHOLD_CONTRACT_VERSION,
                kind: AgentHouseholdReadRequestKindV1::HouseholdReadRequest,
                subject: Some(AgentHouseholdSubjectV1::Self_),
                requested_projection: AgentHouseholdProjectionV1::Profile,
                expected_disclosure_generation: GenerationId::new(3),
                cursor: None,
                limit: 1,
            },
            CancellationToken::new(),
        )
        .await
        .expect_err("self authority cannot disclose member B");
    assert_eq!(error.code, "household_agent_subject_content_mismatch");

    let proposal_port = Arc::new(FixtureHouseholdAgentPort::new());
    *proposal_port
        .proposal_member_override
        .lock()
        .expect("proposal override lock") = Some(other_member.clone());
    let error = HouseholdAgentPhase0Proof::new(proposal_port)
        .prepare(account(), edit_prepare_request(), CancellationToken::new())
        .await
        .expect_err("member A proposal cannot return member B");
    assert_eq!(error.code, "household_agent_subject_content_mismatch");

    let scope_port = Arc::new(FixtureHouseholdAgentPort::new());
    *scope_port
        .active_scope_override
        .lock()
        .expect("active scope override lock") = Some(HouseholdScope::Subject(
        HouseholdSubjectId::member(other_member),
    ));
    let result = HouseholdAgentPhase0Proof::new(scope_port)
        .read(
            account(),
            member_read_request(AgentHouseholdProjectionV1::Profile),
            CancellationToken::new(),
        )
        .await
        .expect("ungranted active-scope identity downgrades the complete result");
    assert_eq!(result.projection, AgentHouseholdProjectionV1::ContentFree);
    assert!(result.active_scope.is_none());
}

#[tokio::test]
async fn everyone_requires_the_independently_authoritative_complete_roster() {
    let valid_port = Arc::new(FixtureHouseholdAgentPort::new());
    HouseholdAgentPhase0Proof::new(valid_port)
        .read(
            account(),
            AgentHouseholdReadRequestV1 {
                schema_version: AGENT_HOUSEHOLD_CONTRACT_VERSION,
                kind: AgentHouseholdReadRequestKindV1::HouseholdReadRequest,
                subject: Some(AgentHouseholdSubjectV1::Everyone),
                requested_projection: AgentHouseholdProjectionV1::Profile,
                expected_disclosure_generation: GenerationId::new(3),
                cursor: None,
                limit: 10,
            },
            CancellationToken::new(),
        )
        .await
        .expect("complete authoritative Everyone roster");

    let omitted_member =
        MemberId::parse_preserved("10000000-0000-4000-8000-000000000002").expect("member B");
    let incomplete_port = Arc::new(FixtureHouseholdAgentPort::new());
    incomplete_port
        .authoritative_members
        .lock()
        .expect("authoritative members lock")
        .push(omitted_member.clone());
    let error = HouseholdAgentPhase0Proof::new(incomplete_port)
        .read(
            account(),
            AgentHouseholdReadRequestV1 {
                schema_version: AGENT_HOUSEHOLD_CONTRACT_VERSION,
                kind: AgentHouseholdReadRequestKindV1::HouseholdReadRequest,
                subject: Some(AgentHouseholdSubjectV1::Everyone),
                requested_projection: AgentHouseholdProjectionV1::Profile,
                expected_disclosure_generation: GenerationId::new(3),
                cursor: None,
                limit: 10,
            },
            CancellationToken::new(),
        )
        .await
        .expect_err("adapter cannot omit member B and decrement its own count");
    assert_eq!(error.code, "household_agent_everyone_incomplete");

    let identity_only_port = Arc::new(FixtureHouseholdAgentPort::new());
    identity_only_port
        .authoritative_members
        .lock()
        .expect("authoritative members lock")
        .push(omitted_member.clone());
    *identity_only_port
        .active_scope_override
        .lock()
        .expect("active scope override lock") = Some(HouseholdScope::Subject(
        HouseholdSubjectId::member(omitted_member),
    ));
    identity_only_port
        .eligible_count_override
        .store(3, Ordering::SeqCst);
    let error = HouseholdAgentPhase0Proof::new(identity_only_port)
        .read(
            account(),
            AgentHouseholdReadRequestV1 {
                schema_version: AGENT_HOUSEHOLD_CONTRACT_VERSION,
                kind: AgentHouseholdReadRequestKindV1::HouseholdReadRequest,
                subject: Some(AgentHouseholdSubjectV1::Everyone),
                requested_projection: AgentHouseholdProjectionV1::Profile,
                expected_disclosure_generation: GenerationId::new(3),
                cursor: None,
                limit: 10,
            },
            CancellationToken::new(),
        )
        .await
        .expect_err("active-scope identity cannot substitute for an omitted member projection");
    assert_eq!(error.code, "household_agent_everyone_incomplete");
}

#[tokio::test]
async fn status_invalidates_same_generation_revision_rotation_and_natural_expiry() {
    for expires in [false, true] {
        let port = Arc::new(FixtureHouseholdAgentPort::new());
        let controller = HouseholdAgentPhase0Proof::new(port.clone());
        controller
            .prepare(account(), edit_prepare_request(), CancellationToken::new())
            .await
            .expect("prepare");
        if expires {
            port.disclosure_observed_after_expiry
                .store(true, Ordering::SeqCst);
        } else {
            port.disclosure_revision.fetch_add(1, Ordering::SeqCst);
        }
        let status = controller
            .status(
                account(),
                port.current_proposal_ref(),
                CancellationToken::new(),
            )
            .await
            .expect("stale content-free status");
        assert_eq!(status.state, AgentHouseholdProposalStateV1::Stale);
        assert_eq!(status.projection, AgentHouseholdProjectionV1::ContentFree);
        assert!(status.affected_member_ref.is_none());
        assert!(status.changes.is_empty());
    }

    let scope_port = Arc::new(FixtureHouseholdAgentPort::new());
    let scope_controller = HouseholdAgentPhase0Proof::new(scope_port.clone());
    scope_controller
        .prepare(account(), scope_prepare_request(), CancellationToken::new())
        .await
        .expect("prepare content-free Scope proposal");
    scope_port
        .disclosure_observed_after_expiry
        .store(true, Ordering::SeqCst);
    let scope_status = scope_controller
        .status(
            account(),
            scope_port.current_proposal_ref(),
            CancellationToken::new(),
        )
        .await
        .expect("Scope status becomes stale after subject grant expiry");
    assert_eq!(scope_status.state, AgentHouseholdProposalStateV1::Stale);
    assert_eq!(
        scope_status.projection,
        AgentHouseholdProjectionV1::ContentFree
    );
    assert!(scope_status.changes.is_empty());
}

#[tokio::test]
async fn status_rejects_cross_proposal_and_cross_operation_frozen_authority() {
    let proposal_port = Arc::new(FixtureHouseholdAgentPort::new());
    let proposal_controller = HouseholdAgentPhase0Proof::new(proposal_port.clone());
    proposal_controller
        .prepare(account(), edit_prepare_request(), CancellationToken::new())
        .await
        .expect("prepare proposal A");
    let proposal_a = proposal_port.current_proposal_ref();
    proposal_controller
        .prepare(account(), edit_prepare_request(), CancellationToken::new())
        .await
        .expect("prepare proposal B");
    let proposal_b = proposal_port.current_proposal_ref();
    assert_ne!(proposal_a, proposal_b);
    proposal_port
        .status_accepts_any_requested_ref
        .store(true, Ordering::SeqCst);
    let error = proposal_controller
        .status(account(), proposal_a, CancellationToken::new())
        .await
        .expect_err("consistent proposal B result cannot authorize proposal A status");
    assert_eq!(error.code, "household_agent_proposal_mismatch");

    let operation_port = Arc::new(FixtureHouseholdAgentPort::new());
    let operation_controller = HouseholdAgentPhase0Proof::new(operation_port.clone());
    operation_controller
        .prepare(account(), edit_prepare_request(), CancellationToken::new())
        .await
        .expect("prepare edit");
    let edit_proposal = operation_port.current_proposal_ref();
    let mut archive_request = edit_prepare_request();
    archive_request.operation = AgentHouseholdOperationV1::Archive;
    operation_controller
        .prepare(account(), archive_request, CancellationToken::new())
        .await
        .expect("prepare archive");
    operation_port
        .status_accepts_any_requested_ref
        .store(true, Ordering::SeqCst);
    let error = operation_controller
        .status(account(), edit_proposal, CancellationToken::new())
        .await
        .expect_err("consistent archive result cannot authorize edit proposal status");
    assert_eq!(error.code, "household_agent_proposal_mismatch");
}

#[tokio::test]
async fn application_rejects_rust_values_that_closed_json_schemas_cannot_emit() {
    for cursor in [String::new(), "x".repeat(513)] {
        let mut request = member_read_request(AgentHouseholdProjectionV1::Profile);
        request.cursor = Some(cursor);
        let error = HouseholdAgentPhase0Proof::new(Arc::new(FixtureHouseholdAgentPort::new()))
            .read(account(), request, CancellationToken::new())
            .await
            .expect_err("schema-invalid cursor fails before adapter dispatch");
        assert_eq!(error.code, "household_agent_read_contract");
    }

    let read_port = Arc::new(FixtureHouseholdAgentPort::new());
    read_port.invalid_read_wire.store(true, Ordering::SeqCst);
    let error = HouseholdAgentPhase0Proof::new(read_port)
        .read(
            account(),
            member_read_request(AgentHouseholdProjectionV1::Profile),
            CancellationToken::new(),
        )
        .await
        .expect_err("duplicate profile values fail before disclosure");
    assert_eq!(error.code, "household_agent_read_contract");

    let count_port = Arc::new(FixtureHouseholdAgentPort::new());
    count_port
        .invalid_read_count_wire
        .store(true, Ordering::SeqCst);
    let error = HouseholdAgentPhase0Proof::new(count_port)
        .read(
            account(),
            member_read_request(AgentHouseholdProjectionV1::Profile),
            CancellationToken::new(),
        )
        .await
        .expect_err("Rust u16 values above the JSON schema bound fail closed");
    assert_eq!(error.code, "household_agent_read_contract");

    let proposal_port = Arc::new(FixtureHouseholdAgentPort::new());
    proposal_port
        .invalid_proposal_wire
        .store(true, Ordering::SeqCst);
    let error = HouseholdAgentPhase0Proof::new(proposal_port)
        .prepare(account(), edit_prepare_request(), CancellationToken::new())
        .await
        .expect_err("control-bearing proposal values fail before output");
    assert_eq!(error.code, "household_agent_proposal_contract");
}

#[tokio::test]
async fn scope_proposals_are_always_agent_visible_as_content_free_handoffs() {
    let port = Arc::new(FixtureHouseholdAgentPort::new());
    let result = HouseholdAgentPhase0Proof::new(port.clone())
        .prepare(
            account(),
            AgentHouseholdPrepareRequestV1 {
                schema_version: AGENT_HOUSEHOLD_CONTRACT_VERSION,
                kind: AgentHouseholdPrepareRequestKindV1::PrepareHouseholdChange,
                operation: AgentHouseholdOperationV1::Scope,
                requested_projection: AgentHouseholdProjectionV1::Profile,
                expected_disclosure_generation: GenerationId::new(3),
                expected_household_revision: port.current_revision(),
                affected_member_ref: None,
                bundled_scope: Some(HouseholdScope::Subject(HouseholdSubjectId::member(
                    port.member.clone(),
                ))),
            },
            CancellationToken::new(),
        )
        .await
        .expect("scope prepare");
    assert_eq!(result.projection, AgentHouseholdProjectionV1::ContentFree);
    assert!(result.affected_member_ref.is_none());
    assert!(result.affected_member_label.is_none());
    assert!(result.changes.is_empty());
    assert!(result.consequences.is_empty());
}

#[tokio::test]
async fn debug_views_never_emit_account_member_cursor_or_profile_content() {
    let port = Arc::new(FixtureHouseholdAgentPort::new());
    let mut request = member_read_request(AgentHouseholdProjectionV1::Profile);
    request.cursor = Some("cursor-secret-sentinel".to_owned());
    let raw = port
        .read(account(), request.clone(), CancellationToken::new())
        .await
        .expect("raw read");
    let disclosure = port
        .disclosure(
            account(),
            AgentDisclosurePurposeV1::HouseholdAgentRead,
            CancellationToken::new(),
        )
        .await
        .expect("disclosure");
    let debug = format!("{request:?} {raw:?} {disclosure:?}");
    for secret in [
        "phase0-household-account",
        "10000000-0000-4000-8000-000000000001",
        "cursor-secret-sentinel",
        "Fixture Adult",
        "vegan",
        "milk",
        "celery",
    ] {
        assert!(!debug.contains(secret), "debug leaked {secret}");
    }
}

#[test]
fn rust_wire_types_round_trip_every_closed_household_surface_fixture() {
    let cases = [
        (
            include_str!("../../../schemas/v1/agent-household-context-input.schema.json"),
            serde_json::to_value(
                serde_json::from_str::<AgentHouseholdContextInputV1>(include_str!(
                    "../../../fixtures/agent/household-phase0/context-input.json"
                ))
                .expect("typed context input"),
            )
            .expect("context serialization"),
        ),
        (
            include_str!("../../../schemas/v1/agent-household-member-input.schema.json"),
            serde_json::to_value(
                serde_json::from_str::<AgentHouseholdMemberInputV1>(include_str!(
                    "../../../fixtures/agent/household-phase0/member-input.json"
                ))
                .expect("typed member input"),
            )
            .expect("member serialization"),
        ),
        (
            include_str!("../../../schemas/v1/agent-household-read.schema.json"),
            serde_json::to_value(
                serde_json::from_str::<AgentHouseholdReadSnapshotV1>(include_str!(
                    "../../../fixtures/agent/household-phase0/read-result-profile.json"
                ))
                .expect("typed read result"),
            )
            .expect("read serialization"),
        ),
        (
            include_str!("../../../schemas/v1/agent-household-action.schema.json"),
            serde_json::to_value(
                serde_json::from_str::<AgentHouseholdPrepareRequestV1>(include_str!(
                    "../../../fixtures/agent/household-phase0/prepare-request.json"
                ))
                .expect("typed prepare input"),
            )
            .expect("prepare serialization"),
        ),
        (
            include_str!("../../../schemas/v1/agent-household-get-change-input.schema.json"),
            serde_json::to_value(
                serde_json::from_str::<AgentHouseholdProposalRefInputV1>(include_str!(
                    "../../../fixtures/agent/household-phase0/get-change-input.json"
                ))
                .expect("typed get input"),
            )
            .expect("get serialization"),
        ),
        (
            include_str!("../../../schemas/v1/agent-household-cancel-input.schema.json"),
            serde_json::to_value(
                serde_json::from_str::<AgentHouseholdProposalRefInputV1>(include_str!(
                    "../../../fixtures/agent/household-phase0/cancel-request.json"
                ))
                .expect("typed cancel input"),
            )
            .expect("cancel serialization"),
        ),
        (
            include_str!("../../../schemas/v1/agent-household-reconcile-input.schema.json"),
            serde_json::to_value(
                serde_json::from_str::<AgentHouseholdProposalRefInputV1>(include_str!(
                    "../../../fixtures/agent/household-phase0/reconcile-input.json"
                ))
                .expect("typed reconcile input"),
            )
            .expect("reconcile serialization"),
        ),
        (
            include_str!("../../../schemas/v1/agent-household-proposal-presentation.schema.json"),
            serde_json::to_value(
                serde_json::from_str::<AgentHouseholdProposalPresentationV1>(include_str!(
                    "../../../fixtures/agent/household-phase0/proposal-profile.json"
                ))
                .expect("typed proposal result"),
            )
            .expect("proposal serialization"),
        ),
        (
            include_str!("../../../schemas/v1/agent-household-outcome.schema.json"),
            serde_json::to_value(
                serde_json::from_str::<AgentHouseholdOutcomeReceiptV1>(include_str!(
                    "../../../fixtures/agent/household-phase0/cancel-outcome.json"
                ))
                .expect("typed outcome result"),
            )
            .expect("outcome serialization"),
        ),
    ];
    for (schema, value) in cases {
        let schema: serde_json::Value = serde_json::from_str(schema).expect("schema JSON");
        jsonschema::draft202012::validate(&schema, &value)
            .unwrap_or_else(|error| panic!("Rust wire value failed schema: {error}"));
    }
}

#[test]
fn closed_approval_and_scope_schemas_reject_illegal_authority_shapes() {
    let approval_schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../../schemas/v1/local-household-approval-protocol.schema.json"
    ))
    .expect("approval schema");
    jsonschema::draft202012::meta::validate(&approval_schema).expect("approval meta-schema");
    let approval_fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/agent/household-phase0/local-approval-lifecycle.json"
    ))
    .expect("approval fixture");
    jsonschema::draft202012::validate(&approval_schema, &approval_fixture)
        .expect("closed approval fixture");

    let mut terminal_revival = approval_fixture.clone();
    terminal_revival["legal_transitions"][0] = serde_json::json!(["committed", "prepared"]);
    assert!(jsonschema::draft202012::validate(&approval_schema, &terminal_revival).is_err());

    let mut duplicate_transition = approval_fixture.clone();
    duplicate_transition["legal_transitions"][0] =
        duplicate_transition["legal_transitions"][1].clone();
    assert!(jsonschema::draft202012::validate(&approval_schema, &duplicate_transition).is_err());

    let mut illegal_scenario_adjacency = approval_fixture;
    illegal_scenario_adjacency["scenarios"][0]["states"] =
        serde_json::json!(["prepared", "committed"]);
    assert!(
        jsonschema::draft202012::validate(&approval_schema, &illegal_scenario_adjacency).is_err()
    );

    let proposal_schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../../schemas/v1/agent-household-proposal-presentation.schema.json"
    ))
    .expect("proposal schema");
    jsonschema::draft202012::meta::validate(&proposal_schema).expect("proposal meta-schema");
    let mut leaked_scope: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/agent/household-phase0/proposal-profile.json"
    ))
    .expect("profile proposal fixture");
    leaked_scope["operation"] = serde_json::json!("scope");
    assert!(jsonschema::draft202012::validate(&proposal_schema, &leaked_scope).is_err());
    assert!(serde_json::from_value::<AgentHouseholdProposalPresentationV1>(leaked_scope).is_err());
}
