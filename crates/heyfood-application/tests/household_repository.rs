use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use heyfood_application::{
    AuthoritativeConsentStateV1, BoxFuture, CreateMemberWithDeclaredProfileV1, HouseholdCommit,
    HouseholdCommitOutcome, HouseholdContextErrorV1, HouseholdErase, HouseholdEraseOutcome,
    HouseholdInitialize, HouseholdLoad, HouseholdMutationAuthorityPort,
    HouseholdMutationAuthorityV1, HouseholdMutationPurposeV1, HouseholdProfileEligibilityV1,
    HouseholdProfileIneligibilityV1, HouseholdProfileOperationV1, HouseholdReadLeaseV1,
    HouseholdRepositoryPort, HouseholdRepositoryResolutionV1, HouseholdSession,
    NativeMemberAgeEvidenceV1, OwnerProfileRetryEligibilityV1, OwnerSyncIntentHandleV1,
    OwnerSyncTransitionEventV1, PortError, PreparedHouseholdTargetV1, SaveMemberDeclaredProfileV1,
    SaveOwnerProfileAndSyncIntentV1, SelectedHouseholdTargetV1, SelfOnlyHouseholdInitializationV1,
    TransitionOwnerSyncIntentV1, household_profile_eligibility_v1,
    owner_profile_action_eligibility_v1, resolve_household_commit_v1,
    resolve_household_initialize_v1, resolve_personalized_context_v1,
};
use heyfood_core::agent_household::HouseholdCommitEvidenceRepositoryAuthorityV1;
use heyfood_core::{
    AccountId, AgeEvidenceSourceV1, AgeEvidenceV1, AgentDisclosurePurposeV1,
    AgentHouseholdOperationV1, AgentHouseholdProjectionV1, AgentHouseholdProposalIdV1,
    AppliedCommitOutcomeV1, AppliedCommitRecordV1, CanonicalDateV1, CanonicalDigestV1,
    CanonicalJsonObjectV1, CanonicalTimestampV1, CommitId, ConsentVersionV1, DisplayName,
    GenerationId, HOUSEHOLD_STATE_SCHEMA_VERSION, HouseholdDeclaredProfileV1,
    HouseholdEffectFingerprintV1, HouseholdEffectV1, HouseholdLifecycleV1, HouseholdMemberV1,
    HouseholdOutboxId, HouseholdOutboxRecordV1, HouseholdOwnerV1, HouseholdProfileDocumentV1,
    HouseholdProfileOutboxEntryV1, HouseholdProfileRecordV1, HouseholdProfileStateV1,
    HouseholdRevision, HouseholdScope, HouseholdStateV1, HouseholdSubjectId,
    ImportedCompatibilityStateV1, LastDefiniteOwnerSyncErrorV1, LegacyRemoteProfileReferenceV1,
    LegacySourceIdentityV1, LocalHouseholdAuthoritySnapshotV1, LocalHouseholdFrozenCandidateV1,
    LocalHouseholdProposalAuthorityV1, LocalHouseholdProposalBindingV1,
    LocalHouseholdProposalJournalV1, MAX_HOUSEHOLD_MEMBERS, MigrationDispositionManifestV1,
    MigrationProvenanceV1, MinorStatusV1, OnboardingProfileInput, OutboxRevision,
    OwnerSyncIntentPhaseV1, OwnerSyncIntentV1, ProfileRevision, RelationshipSourceV1,
    RelationshipV1, RemoteProfileBaseV1, RemoteProfileExistenceV1, canonical_sha256_v1,
    classify_legacy_outbox_v1,
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

fn timestamp(second: u8) -> CanonicalTimestampV1 {
    CanonicalTimestampV1::parse(format!("2026-07-30T12:00:{second:02}.000Z")).unwrap()
}

fn account(value: &str) -> AccountId {
    AccountId::parse(value).unwrap()
}

fn empty_compatibility() -> ImportedCompatibilityStateV1 {
    ImportedCompatibilityStateV1 {
        fields: Vec::new(),
        legacy_python_applied_mutation_ids: Vec::new(),
        legacy_python_applied_mutation_ids_digest: None,
        legacy_remote_profile_references: Vec::new(),
        legacy_timestamp_provenance: Vec::new(),
    }
}

fn provenance(at: &CanonicalTimestampV1, commit_id: CommitId) -> MigrationProvenanceV1 {
    MigrationProvenanceV1 {
        source_identity: LegacySourceIdentityV1::NoSource {
            source_set_fingerprint: CanonicalDigestV1::from_bytes([7; 32]),
        },
        legacy_python_snapshot: None,
        migration_id: CommitId::new().as_uuid(),
        initialization_id: CommitId::new().as_uuid(),
        initial_commit_id: commit_id,
        migration_frozen_at: at.clone(),
    }
}

fn incomplete_owner(at: &CanonicalTimestampV1) -> HouseholdOwnerV1 {
    HouseholdOwnerV1 {
        display_name: DisplayName::parse("Owner").unwrap(),
        relationship: RelationshipV1::Self_,
        profile_state: HouseholdProfileStateV1::Incomplete,
        created_at: at.clone(),
        updated_at: at.clone(),
    }
}

fn empty_state(account_id: &str, commit_id: CommitId) -> HouseholdStateV1 {
    let at = timestamp(0);
    HouseholdStateV1 {
        schema_version: HOUSEHOLD_STATE_SCHEMA_VERSION,
        account_binding: account(account_id),
        revision: HouseholdRevision::new(1).unwrap(),
        owner: incomplete_owner(&at),
        active_scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
        members: Vec::new(),
        profiles: Vec::new(),
        outbox: Vec::new(),
        bounded_applied_commits: Vec::new(),
        imported_compatibility: empty_compatibility(),
        migration_dispositions: MigrationDispositionManifestV1 {
            dispositions: Vec::new(),
        },
        migration_provenance: provenance(&at, commit_id),
        updated_at: at,
    }
}

fn initialized_state(account_id: &str) -> HouseholdStateV1 {
    let commit_id = CommitId::new();
    let state = empty_state(account_id, commit_id);
    let command = HouseholdInitialize::new(
        account(account_id),
        commit_id,
        state,
        HouseholdEffectV1::Initialize,
        timestamp(0),
    )
    .unwrap();
    let HouseholdRepositoryResolutionV1::Write { state, .. } =
        resolve_household_initialize_v1(None, &command).unwrap()
    else {
        panic!("initialization must produce one write")
    };
    *state
}

fn declared_profile() -> OnboardingProfileInput {
    OnboardingProfileInput {
        diet_style_ids: vec!["vegan".to_owned()],
        avoid_ingredients: vec!["canary private ingredient".to_owned()],
        notes: Some("canary private notes".to_owned()),
        ..OnboardingProfileInput::default()
    }
}

fn native_profile_document() -> HouseholdProfileDocumentV1 {
    let input = declared_profile();
    HouseholdProfileDocumentV1::native(HouseholdDeclaredProfileV1 {
        diet_style_ids: input.diet_style_ids,
        custom_diet_styles: input.custom_diet_styles,
        allergy_ids: input.allergy_ids,
        custom_restrictions: input.custom_restrictions,
        health_condition_ids: input.health_condition_ids,
        custom_health_conditions: input.custom_health_conditions,
        avoid_ingredients: input.avoid_ingredients,
        activity_level: input.activity_level,
        cuisine_preferences: input.cuisine_preferences,
        custom_cuisines: input.custom_cuisines,
        severity_level: input.severity_level,
        notes: input.notes,
    })
    .unwrap()
}

fn atomic_create_candidate(
    current: &HouseholdStateV1,
    member_id: heyfood_core::MemberId,
    display_name: &str,
    committed_at: CanonicalTimestampV1,
) -> (HouseholdStateV1, HouseholdEffectV1) {
    let subject = HouseholdSubjectId::member(member_id.clone());
    let member = HouseholdMemberV1 {
        member_id,
        display_name: DisplayName::parse(display_name).unwrap(),
        relationship: RelationshipV1::Friend,
        relationship_source: RelationshipSourceV1::NativeDeclared,
        minor_status: MinorStatusV1::Adult,
        age_evidence: Some(AgeEvidenceV1 {
            date_of_birth: None,
            age_band: Some(heyfood_core::AgeBandV1::Age18Plus),
            source: AgeEvidenceSourceV1::NativeDeclared,
        }),
        minor_status_evaluated_on: CanonicalDateV1::parse("2026-07-30").unwrap(),
        lifecycle: HouseholdLifecycleV1::Active,
        profile_state: HouseholdProfileStateV1::LocalOnly,
        created_at: committed_at.clone(),
        updated_at: committed_at.clone(),
    };
    let profile = HouseholdProfileRecordV1 {
        subject: subject.clone(),
        profile_revision: ProfileRevision::new(1).unwrap(),
        document: native_profile_document(),
    };
    let selected_scope = HouseholdScope::Subject(subject);
    let mut candidate = current.clone();
    candidate.revision = current.revision.checked_next().unwrap();
    candidate.updated_at = committed_at;
    candidate.active_scope = selected_scope.clone();
    candidate.members.push(member.clone());
    candidate.profiles.push(profile.clone());
    let effect = HouseholdEffectV1::CreateMemberWithDeclaredProfile {
        member,
        profile,
        selected_scope,
    };
    (candidate, effect)
}

fn legacy_context_record(
    source_key: &str,
    marker: u8,
    restriction: &str,
) -> HouseholdOutboxRecordV1 {
    let payload =
        serde_json::to_vec(&serde_json::json!({"local_context":{"restrictions":[restriction]}}))
            .unwrap();
    let (outbox_id, legacy) = classify_legacy_outbox_v1(
        CanonicalDigestV1::from_bytes([marker; 32]),
        source_key,
        &payload,
        &timestamp(0),
    )
    .unwrap();
    HouseholdOutboxRecordV1 {
        outbox_id,
        outbox_revision: OutboxRevision::new(1).unwrap(),
        entry: HouseholdProfileOutboxEntryV1::Legacy {
            version: 1,
            target: legacy.target.clone(),
            legacy,
        },
    }
}

fn authority(
    second: u8,
    member_id: Option<heyfood_core::MemberId>,
) -> HouseholdMutationAuthorityV1 {
    HouseholdMutationAuthorityV1 {
        commit_id: CommitId::new(),
        frozen_commit_timestamp: timestamp(second),
        frozen_evaluation_date: CanonicalDateV1::parse("2026-07-30").unwrap(),
        member_id,
    }
}

#[derive(Default)]
struct MemoryHouseholdRepository {
    state: Mutex<Option<HouseholdStateV1>>,
    load_calls: AtomicUsize,
}

impl MemoryHouseholdRepository {
    fn with_state(state: HouseholdStateV1) -> Self {
        Self {
            state: Mutex::new(Some(state)),
            load_calls: AtomicUsize::new(0),
        }
    }

    async fn snapshot(&self) -> Option<HouseholdStateV1> {
        self.state.lock().await.clone()
    }
}

impl HouseholdRepositoryPort for MemoryHouseholdRepository {
    fn load<'a>(
        &'a self,
        account: &'a AccountId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<Option<HouseholdLoad>, PortError>> {
        Box::pin(async move {
            self.load_calls.fetch_add(1, Ordering::SeqCst);
            if cancellation.is_cancelled() {
                return Err(PortError::new(
                    "household_load_cancelled",
                    "household load cancelled",
                ));
            }
            let state = self.state.lock().await.clone();
            if let Some(state) = state {
                if &state.account_binding != account {
                    return Err(PortError::new(
                        "household_account_mismatch",
                        "household account mismatch",
                    ));
                }
                Ok(Some(HouseholdLoad::from_state(state)?))
            } else {
                Ok(None)
            }
        })
    }

    fn acquire_read_lease<'a>(
        &'a self,
        account: &'a AccountId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdReadLeaseV1, PortError>> {
        Box::pin(async move {
            let load = self
                .load(account, cancellation)
                .await?
                .ok_or_else(|| PortError::new("household_not_initialized", "missing state"))?;
            Ok(HouseholdReadLeaseV1::new(load, Box::new(())))
        })
    }

    fn initialize<'a>(
        &'a self,
        command: HouseholdInitialize,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdCommitOutcome, PortError>> {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(PortError::new(
                    "household_initialize_cancelled",
                    "household initialization cancelled",
                ));
            }
            let mut state = self.state.lock().await;
            if cancellation.is_cancelled() {
                return Err(PortError::new(
                    "household_initialize_cancelled",
                    "household initialization cancelled",
                ));
            }
            match resolve_household_initialize_v1(state.as_ref(), &command)? {
                HouseholdRepositoryResolutionV1::Replay(outcome) => Ok(outcome),
                HouseholdRepositoryResolutionV1::Write {
                    state: replacement,
                    outcome,
                } => {
                    *state = Some(*replacement);
                    Ok(outcome)
                }
            }
        })
    }

    fn commit<'a>(
        &'a self,
        command: HouseholdCommit,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdCommitOutcome, PortError>> {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(PortError::new(
                    "household_commit_cancelled",
                    "household commit cancelled",
                ));
            }
            let mut state = self.state.lock().await;
            if cancellation.is_cancelled() {
                return Err(PortError::new(
                    "household_commit_cancelled",
                    "household commit cancelled",
                ));
            }
            match resolve_household_commit_v1(state.as_ref(), &command)? {
                HouseholdRepositoryResolutionV1::Replay(outcome) => Ok(outcome),
                HouseholdRepositoryResolutionV1::Write {
                    state: replacement,
                    outcome,
                } => {
                    *state = Some(*replacement);
                    Ok(outcome)
                }
            }
        })
    }

    fn erase_account<'a>(
        &'a self,
        command: HouseholdErase,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdEraseOutcome, PortError>> {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(PortError::new(
                    "household_erase_cancelled",
                    "household erase cancelled",
                ));
            }
            let mut state = self.state.lock().await;
            if let Some(current) = state.as_ref()
                && (current.account_binding != command.account
                    || command
                        .expected_revision
                        .is_some_and(|revision| revision != current.revision))
            {
                return Err(PortError::new(
                    "household_revision_conflict",
                    "household erase authority is stale",
                ));
            }
            *state = None;
            Ok(HouseholdEraseOutcome {
                household_key_deleted: true,
                household_ciphertext_deleted: true,
                import_snapshot_deleted: true,
                legacy_source_retained: true,
                legacy_credentials_cleared: true,
                legacy_credentials_retained: false,
                local_credentials_cleared: true,
                outcome_uncertain: false,
            })
        })
    }
}

struct GeneratingMutationAuthority {
    calls: AtomicUsize,
}

impl HouseholdMutationAuthorityPort for GeneratingMutationAuthority {
    fn allocate(
        &self,
        purpose: HouseholdMutationPurposeV1,
    ) -> Result<HouseholdMutationAuthorityV1, PortError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(HouseholdMutationAuthorityV1 {
            commit_id: CommitId::new(),
            frozen_commit_timestamp: timestamp(59),
            frozen_evaluation_date: CanonicalDateV1::parse("2026-07-30").unwrap(),
            member_id: (purpose == HouseholdMutationPurposeV1::CreateMember)
                .then(heyfood_core::MemberId::new),
        })
    }
}

fn test_session<R>(account_id: AccountId, repository: Arc<R>) -> HouseholdSession
where
    R: HouseholdRepositoryPort + 'static,
{
    HouseholdSession::new(
        account_id,
        repository,
        Arc::new(GeneratingMutationAuthority {
            calls: AtomicUsize::new(0),
        }),
    )
}

struct FixedMutationAuthority {
    calls: AtomicUsize,
    allocations: StdMutex<VecDeque<(HouseholdMutationPurposeV1, HouseholdMutationAuthorityV1)>>,
}

impl FixedMutationAuthority {
    fn new(
        allocations: impl IntoIterator<
            Item = (HouseholdMutationPurposeV1, HouseholdMutationAuthorityV1),
        >,
    ) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            allocations: StdMutex::new(allocations.into_iter().collect()),
        }
    }
}

impl HouseholdMutationAuthorityPort for FixedMutationAuthority {
    fn allocate(
        &self,
        purpose: HouseholdMutationPurposeV1,
    ) -> Result<HouseholdMutationAuthorityV1, PortError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let (expected, authority) = self
            .allocations
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| PortError::new("test_authority_empty", "test authority is empty"))?;
        if expected != purpose {
            return Err(PortError::new(
                "test_authority_purpose",
                "test authority purpose mismatch",
            ));
        }
        Ok(authority)
    }
}

struct CommitThenUncertainRepository {
    state: Mutex<Option<HouseholdStateV1>>,
    commit_calls: AtomicUsize,
    commit_ids: StdMutex<Vec<CommitId>>,
}

impl CommitThenUncertainRepository {
    fn with_state(state: HouseholdStateV1) -> Self {
        Self {
            state: Mutex::new(Some(state)),
            commit_calls: AtomicUsize::new(0),
            commit_ids: StdMutex::new(Vec::new()),
        }
    }

    async fn snapshot(&self) -> HouseholdStateV1 {
        self.state.lock().await.clone().unwrap()
    }
}

impl HouseholdRepositoryPort for CommitThenUncertainRepository {
    fn load<'a>(
        &'a self,
        account: &'a AccountId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<Option<HouseholdLoad>, PortError>> {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(PortError::new(
                    "household_load_cancelled",
                    "household load cancelled",
                ));
            }
            let state = self.state.lock().await.clone();
            if state
                .as_ref()
                .is_some_and(|state| &state.account_binding != account)
            {
                return Err(PortError::new(
                    "household_account_mismatch",
                    "household account mismatch",
                ));
            }
            state.map(HouseholdLoad::from_state).transpose()
        })
    }

    fn acquire_read_lease<'a>(
        &'a self,
        account: &'a AccountId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdReadLeaseV1, PortError>> {
        Box::pin(async move {
            let load = self
                .load(account, cancellation)
                .await?
                .ok_or_else(|| PortError::new("household_not_initialized", "missing state"))?;
            Ok(HouseholdReadLeaseV1::new(load, Box::new(())))
        })
    }

    fn initialize<'a>(
        &'a self,
        _command: HouseholdInitialize,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdCommitOutcome, PortError>> {
        Box::pin(async {
            Err(PortError::new(
                "test_initialize_unavailable",
                "test repository is already initialized",
            ))
        })
    }

    fn commit<'a>(
        &'a self,
        command: HouseholdCommit,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdCommitOutcome, PortError>> {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(PortError::new(
                    "household_commit_cancelled",
                    "household commit cancelled",
                ));
            }
            self.commit_ids.lock().unwrap().push(command.commit_id);
            let call = self.commit_calls.fetch_add(1, Ordering::SeqCst);
            let mut state = self.state.lock().await;
            let outcome = match resolve_household_commit_v1(state.as_ref(), &command)? {
                HouseholdRepositoryResolutionV1::Replay(outcome) => outcome,
                HouseholdRepositoryResolutionV1::Write {
                    state: replacement,
                    outcome,
                } => {
                    *state = Some(*replacement);
                    outcome
                }
            };
            if call == 0 {
                Err(PortError::uncertain(
                    "test_commit_uncertain",
                    "test commit outcome is uncertain",
                ))
            } else {
                Ok(outcome)
            }
        })
    }

    fn erase_account<'a>(
        &'a self,
        _command: HouseholdErase,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdEraseOutcome, PortError>> {
        Box::pin(async {
            Err(PortError::new(
                "test_erase_unavailable",
                "test erase is unavailable",
            ))
        })
    }
}

fn as_object_safe(_port: Arc<dyn HouseholdRepositoryPort>) {}

#[tokio::test]
async fn self_only_initialization_is_live_account_bound_and_object_safe() {
    let repository = Arc::new(MemoryHouseholdRepository::default());
    as_object_safe(repository.clone());
    let session = test_session(account("account-a"), repository.clone());
    let at = timestamp(0);
    let commit_id = CommitId::new();
    let opened = session
        .open_or_initialize_self_only(
            SelfOnlyHouseholdInitializationV1 {
                owner: incomplete_owner(&at),
                migration_provenance: provenance(&at, commit_id),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(opened.load.state.revision.get(), 1);
    assert_eq!(opened.load.state.members.len(), 0);
    assert_eq!(opened.load.state.bounded_applied_commits.len(), 1);
    assert_eq!(
        opened.load.state.bounded_applied_commits[0].outcome,
        AppliedCommitOutcomeV1::Initialized
    );

    let second = session
        .open_or_initialize_self_only(
            SelfOnlyHouseholdInitializationV1 {
                owner: incomplete_owner(&at),
                migration_provenance: provenance(&at, CommitId::new()),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        second.outcome,
        heyfood_application::HouseholdOpenOutcomeV1::Opened
    );
    assert_eq!(repository.snapshot().await.unwrap().revision.get(), 1);
}

#[tokio::test]
async fn replay_precedes_revision_check_and_commit_id_reuse_conflicts() {
    let repository = Arc::new(MemoryHouseholdRepository::default());
    let session = test_session(account("account-a"), repository.clone());
    let initial_at = timestamp(0);
    let initial_commit = CommitId::new();
    session
        .open_or_initialize_self_only(
            SelfOnlyHouseholdInitializationV1 {
                owner: incomplete_owner(&initial_at),
                migration_provenance: provenance(&initial_at, initial_commit),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let initial = repository.snapshot().await.unwrap();
    let mut candidate = initial.clone();
    candidate.revision = candidate.revision.checked_next().unwrap();
    candidate.updated_at = timestamp(1);
    let commit_id = CommitId::new();
    let command = HouseholdCommit::new(
        account("account-a"),
        initial.revision,
        commit_id,
        candidate,
        HouseholdEffectV1::SelectScope {
            scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
        },
        timestamp(1),
    )
    .unwrap();
    let first = session
        .commit(command.clone(), CancellationToken::new())
        .await
        .unwrap();
    let replay = session
        .commit(command, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(first, replay);
    assert_eq!(repository.snapshot().await.unwrap().revision.get(), 2);

    let current = repository.snapshot().await.unwrap();
    let mut different_candidate = current.clone();
    different_candidate.revision = current.revision.checked_next().unwrap();
    different_candidate.updated_at = timestamp(2);
    let conflicting = HouseholdCommit::new(
        account("account-a"),
        current.revision,
        commit_id,
        different_candidate,
        HouseholdEffectV1::SelectScope {
            scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
        },
        timestamp(2),
    )
    .unwrap();
    let error = session
        .commit(conflicting, CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.code, "household_commit_id_conflict");
}

#[tokio::test]
async fn expected_revision_and_claimed_fingerprint_fail_closed() {
    let initial_commit = CommitId::new();
    let mut initial = empty_state("account-a", initial_commit);
    let initialization = HouseholdInitialize::new(
        account("account-a"),
        initial_commit,
        initial.clone(),
        HouseholdEffectV1::Initialize,
        timestamp(0),
    )
    .unwrap();
    let HouseholdRepositoryResolutionV1::Write {
        state: initialized, ..
    } = resolve_household_initialize_v1(None, &initialization).unwrap()
    else {
        panic!("expected write");
    };
    initial = *initialized;
    let repository = Arc::new(MemoryHouseholdRepository::with_state(initial.clone()));
    let session = test_session(account("account-a"), repository);

    let mut candidate = initial.clone();
    candidate.revision = candidate.revision.checked_next().unwrap();
    candidate.updated_at = timestamp(1);
    let mut command = HouseholdCommit::new(
        account("account-a"),
        initial.revision,
        CommitId::new(),
        candidate,
        HouseholdEffectV1::SelectScope {
            scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
        },
        timestamp(1),
    )
    .unwrap();
    command.claimed_effect_fingerprint = heyfood_core::HouseholdEffectFingerprintV1::from_digest(
        CanonicalDigestV1::from_bytes([0x55; 32]),
    );
    let error = session
        .commit(command, CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.code, "household_effect_fingerprint_mismatch");

    let mut stale_candidate = initial.clone();
    stale_candidate.revision = initial.revision.checked_next().unwrap();
    stale_candidate.updated_at = timestamp(2);
    let stale = HouseholdCommit::new(
        account("account-a"),
        initial.revision,
        CommitId::new(),
        stale_candidate,
        HouseholdEffectV1::SelectScope {
            scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
        },
        timestamp(2),
    )
    .unwrap();
    // Advance the repository with another exact low-level commit first.
    let mut advance_candidate = initial.clone();
    advance_candidate.revision = initial.revision.checked_next().unwrap();
    advance_candidate.updated_at = timestamp(1);
    let advance = HouseholdCommit::new(
        account("account-a"),
        initial.revision,
        CommitId::new(),
        advance_candidate,
        HouseholdEffectV1::SelectScope {
            scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
        },
        timestamp(1),
    )
    .unwrap();
    session
        .commit(advance, CancellationToken::new())
        .await
        .unwrap();
    let error = session
        .commit(stale, CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.code, "household_revision_conflict");
}

#[tokio::test]
async fn cancellation_is_explicit_and_does_not_reach_the_port() {
    let repository = Arc::new(MemoryHouseholdRepository::default());
    let session = test_session(account("account-a"), repository.clone());
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let error = session.load(cancellation).await.unwrap_err();
    assert_eq!(error.code, "household_load_cancelled");
    assert_eq!(repository.load_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn normalized_effect_rejects_unrelated_candidate_changes() {
    let initial_commit = CommitId::new();
    let initial = empty_state("account-a", initial_commit);
    let initialization = HouseholdInitialize::new(
        account("account-a"),
        initial_commit,
        initial,
        HouseholdEffectV1::Initialize,
        timestamp(0),
    )
    .unwrap();
    let HouseholdRepositoryResolutionV1::Write {
        state: initialized, ..
    } = resolve_household_initialize_v1(None, &initialization).unwrap()
    else {
        panic!("expected initialization write");
    };

    let mut candidate = (*initialized).clone();
    candidate.revision = candidate.revision.checked_next().unwrap();
    candidate.updated_at = timestamp(1);
    candidate.owner.display_name = DisplayName::parse("Changed Owner").unwrap();
    candidate.owner.updated_at = timestamp(1);
    let command = HouseholdCommit::new(
        account("account-a"),
        initialized.revision,
        CommitId::new(),
        candidate,
        HouseholdEffectV1::SelectScope {
            scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
        },
        timestamp(1),
    )
    .unwrap();

    let error = resolve_household_commit_v1(Some(&initialized), &command).unwrap_err();
    assert_eq!(error.code, "household_semantic_transition_mismatch");
}

fn legacy_profile() -> HouseholdProfileDocumentV1 {
    HouseholdProfileDocumentV1::legacy_projection(br#"{"restrictions":["peanut"]}"#).unwrap()
}

fn member(
    name: &str,
    status: MinorStatusV1,
    profile_state: HouseholdProfileStateV1,
) -> HouseholdMemberV1 {
    let evaluated_on = CanonicalDateV1::parse("2026-07-30").unwrap();
    let (relationship, source, evidence) = match status {
        MinorStatusV1::Minor => (
            RelationshipV1::Child,
            RelationshipSourceV1::NativeDeclared,
            None,
        ),
        MinorStatusV1::Adult => (
            RelationshipV1::Friend,
            RelationshipSourceV1::NativeDeclared,
            Some(AgeEvidenceV1 {
                date_of_birth: None,
                age_band: Some(heyfood_core::AgeBandV1::Age18Plus),
                source: AgeEvidenceSourceV1::NativeDeclared,
            }),
        ),
        MinorStatusV1::Unknown => (
            RelationshipV1::Other,
            RelationshipSourceV1::LegacyMissing,
            None,
        ),
    };
    HouseholdMemberV1 {
        member_id: heyfood_core::MemberId::new(),
        display_name: DisplayName::parse(name).unwrap(),
        relationship,
        relationship_source: source,
        minor_status: status,
        age_evidence: evidence,
        minor_status_evaluated_on: evaluated_on,
        lifecycle: HouseholdLifecycleV1::Active,
        profile_state,
        created_at: timestamp(0),
        updated_at: timestamp(0),
    }
}

fn state_with_profiles(
    member_status: MinorStatusV1,
    member_profile_state: HouseholdProfileStateV1,
    include_member_profile: bool,
) -> HouseholdStateV1 {
    let initial_commit = CommitId::new();
    let mut state = empty_state("account-a", initial_commit);
    state.owner.profile_state = HouseholdProfileStateV1::LocalOnly;
    let member = member("Member", member_status, member_profile_state);
    let member_subject = HouseholdSubjectId::member(member.member_id.clone());
    state.members.push(member);
    state.profiles.push(HouseholdProfileRecordV1 {
        subject: HouseholdSubjectId::self_(),
        profile_revision: ProfileRevision::new(1).unwrap(),
        document: legacy_profile(),
    });
    if include_member_profile {
        state.profiles.push(HouseholdProfileRecordV1 {
            subject: member_subject,
            profile_revision: ProfileRevision::new(1).unwrap(),
            document: legacy_profile(),
        });
    }
    state
}

#[test]
fn member_incomplete_context_never_falls_back_to_owner() {
    let mut state = state_with_profiles(
        MinorStatusV1::Unknown,
        HouseholdProfileStateV1::Incomplete,
        false,
    );
    let subject = HouseholdSubjectId::member(state.members[0].member_id.clone());
    state.active_scope = HouseholdScope::Subject(subject.clone());
    state
        .imported_compatibility
        .legacy_remote_profile_references = vec![LegacyRemoteProfileReferenceV1 {
        subject: subject.clone(),
        source_digest: CanonicalDigestV1::from_bytes([9; 32]),
    }];
    state.validate().unwrap();
    let prepared = PreparedHouseholdTargetV1 {
        account_binding: state.account_binding.clone(),
        household_revision: state.revision,
        scope: HouseholdScope::Subject(subject),
    };
    let error = resolve_personalized_context_v1(&state, &prepared).unwrap_err();
    assert_eq!(error, HouseholdContextErrorV1::ProfileIncomplete);
}

#[test]
fn everyone_context_requires_two_eligible_active_subjects() {
    let mut state = state_with_profiles(
        MinorStatusV1::Adult,
        HouseholdProfileStateV1::LocalOnly,
        true,
    );
    state.active_scope = HouseholdScope::Everyone;
    state.validate().unwrap();
    let prepared = PreparedHouseholdTargetV1::from_active_scope(&state).unwrap();
    let context = resolve_personalized_context_v1(&state, &prepared).unwrap();
    assert_eq!(context.subjects.len(), 2);

    state.members[0].lifecycle = HouseholdLifecycleV1::Archived;
    state.active_scope = HouseholdScope::Subject(HouseholdSubjectId::self_());
    state.validate().unwrap();
    let error = PreparedHouseholdTargetV1::for_scope(
        &state,
        HouseholdScope::Everyone,
        HouseholdProfileOperationV1::PersonalizedContext,
    )
    .unwrap_err();
    assert_eq!(
        error,
        HouseholdContextErrorV1::EveryoneRequiresTwoEligibleSubjects
    );
}

#[test]
fn non_owner_sync_is_local_only_and_minor_unknown_fail_closed() {
    for (minor_status, expected_reason) in [
        (
            MinorStatusV1::Minor,
            HouseholdProfileIneligibilityV1::MinorPersistentSyncBlocked,
        ),
        (
            MinorStatusV1::Unknown,
            HouseholdProfileIneligibilityV1::UnknownAgePersistentSyncBlocked,
        ),
        (
            MinorStatusV1::Adult,
            HouseholdProfileIneligibilityV1::NonOwnerPersistentSyncDeferred,
        ),
    ] {
        let state = state_with_profiles(minor_status, HouseholdProfileStateV1::LocalOnly, true);
        let subject = HouseholdSubjectId::member(state.members[0].member_id.clone());
        assert_eq!(
            household_profile_eligibility_v1(
                &state,
                &subject,
                HouseholdProfileOperationV1::PersistentProfileSync,
            ),
            HouseholdProfileEligibilityV1::Ineligible(expected_reason)
        );
        assert_eq!(
            household_profile_eligibility_v1(
                &state,
                &subject,
                HouseholdProfileOperationV1::PersonalizedContext,
            ),
            HouseholdProfileEligibilityV1::Eligible
        );
    }
}

fn owner_sync_state() -> (HouseholdStateV1, HouseholdOutboxId) {
    let initial_commit = CommitId::new();
    let mut state = empty_state("account-a", initial_commit);
    let profile = HouseholdProfileRecordV1 {
        subject: HouseholdSubjectId::self_(),
        profile_revision: ProfileRevision::new(1).unwrap(),
        document: legacy_profile(),
    };
    let intent = initial_owner_sync_intent(&profile, 1, timestamp(0));
    let outbox_id = HouseholdOutboxId::owner_sync(intent.intent_id).unwrap();
    state.owner.profile_state = HouseholdProfileStateV1::PendingSync;
    state.profiles.push(profile);
    state.outbox.push(HouseholdOutboxRecordV1 {
        outbox_id: outbox_id.clone(),
        outbox_revision: OutboxRevision::new(1).unwrap(),
        entry: HouseholdProfileOutboxEntryV1::OwnerSync {
            version: 1,
            target: HouseholdSubjectId::self_(),
            intent,
        },
    });
    state.validate().unwrap();
    (state, outbox_id)
}

#[tokio::test]
async fn authorized_owner_hosted_context_uses_exact_live_profile_for_all_ready_states() {
    let mut local_only = empty_state("account-a", CommitId::new());
    local_only.owner.profile_state = HouseholdProfileStateV1::LocalOnly;
    local_only.profiles.push(HouseholdProfileRecordV1 {
        subject: HouseholdSubjectId::self_(),
        profile_revision: ProfileRevision::new(1).unwrap(),
        document: native_profile_document(),
    });
    local_only.validate().unwrap();

    let (pending, _) = owner_sync_state();
    let mut synced = pending.clone();
    synced.owner.profile_state = HouseholdProfileStateV1::Synced;
    synced.outbox.clear();
    synced.validate().unwrap();

    for (state, expected_canary) in [
        (local_only, "canary private ingredient"),
        (pending, "peanut"),
        (synced, "peanut"),
    ] {
        let repository = Arc::new(MemoryHouseholdRepository::with_state(state));
        let session = test_session(account("account-a"), repository);
        let authorized = session
            .acquire_authorized_owner_hosted_context(CancellationToken::new())
            .await
            .unwrap();
        let snapshot = authorized.snapshot();
        assert_eq!(
            snapshot.scope,
            HouseholdScope::Subject(HouseholdSubjectId::self_())
        );
        assert_eq!(snapshot.subjects.len(), 1);
        assert_eq!(snapshot.subjects[0].subject, HouseholdSubjectId::self_());
        assert!(
            snapshot.subjects[0]
                .effective_profile
                .to_string()
                .contains(expected_canary)
        );
    }
}

#[tokio::test]
async fn authorized_owner_hosted_context_rejects_saved_member_and_everyone_scopes() {
    let mut state = state_with_profiles(
        MinorStatusV1::Adult,
        HouseholdProfileStateV1::LocalOnly,
        true,
    );
    for scope in [
        HouseholdScope::Subject(HouseholdSubjectId::member(
            state.members[0].member_id.clone(),
        )),
        HouseholdScope::Everyone,
    ] {
        state.active_scope = scope;
        state.validate().unwrap();
        let repository = Arc::new(MemoryHouseholdRepository::with_state(state.clone()));
        let session = test_session(account("account-a"), repository);
        let error = session
            .acquire_authorized_owner_hosted_context(CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(error.code, "household_hosted_context_not_authorized");
    }
}

#[tokio::test]
async fn authorized_hosted_context_retains_the_exact_member_and_everyone_scope() {
    let mut state = state_with_profiles(
        MinorStatusV1::Adult,
        HouseholdProfileStateV1::LocalOnly,
        true,
    );
    let member_subject = HouseholdSubjectId::member(state.members[0].member_id.clone());

    for (scope, expected_subjects) in [
        (HouseholdScope::Subject(member_subject.clone()), 1_usize),
        (HouseholdScope::Everyone, 2_usize),
    ] {
        state.active_scope = scope.clone();
        state.validate().unwrap();
        let repository = Arc::new(MemoryHouseholdRepository::with_state(state.clone()));
        let session = test_session(account("account-a"), repository);

        let authorized = session
            .acquire_authorized_hosted_context(CancellationToken::new())
            .await
            .unwrap();
        let snapshot = authorized.snapshot();
        assert_eq!(snapshot.scope, scope);
        assert_eq!(snapshot.household_revision, state.revision);
        assert_eq!(snapshot.subjects.len(), expected_subjects);
        if expected_subjects == 1 {
            assert_eq!(snapshot.subjects[0].subject, member_subject);
        } else {
            assert!(
                snapshot
                    .subjects
                    .iter()
                    .any(|subject| subject.subject == HouseholdSubjectId::self_())
            );
            assert!(
                snapshot
                    .subjects
                    .iter()
                    .any(|subject| subject.subject == member_subject)
            );
        }
    }
}

fn initial_owner_sync_intent(
    profile: &HouseholdProfileRecordV1,
    local_household_revision: u64,
    at: CanonicalTimestampV1,
) -> OwnerSyncIntentV1 {
    let effective = profile.document.effective_profile().unwrap().unwrap();
    let digest = canonical_sha256_v1(&effective).unwrap();
    let intent_id = CommitId::new().as_uuid();
    OwnerSyncIntentV1 {
        schema_version: 1,
        intent_id,
        intent_revision: 1,
        phase: OwnerSyncIntentPhaseV1::NeedsConsentCheck,
        subject: HouseholdSubjectId::self_(),
        local_household_revision,
        local_profile_revision: profile.profile_revision.get(),
        local_profile_digest: digest,
        remote_request_id: intent_id,
        consent_version: None,
        remote_base: None,
        expected_remote_version: None,
        request_method: None,
        request_path: None,
        request_body: None,
        request_body_digest: None,
        attempt_count: 0,
        last_definite_error: None,
        created_at: at.clone(),
        updated_at: at,
    }
}

fn frozen_owner_sync_state(
    phase: OwnerSyncIntentPhaseV1,
    attempt_count: u32,
    last_definite_error: Option<LastDefiniteOwnerSyncErrorV1>,
    profile_state: HouseholdProfileStateV1,
) -> (HouseholdStateV1, HouseholdOutboxId) {
    let (mut state, outbox_id) = owner_sync_state();
    let effective_profile = state.profiles[0]
        .document
        .effective_profile()
        .unwrap()
        .unwrap();
    let serde_json::Value::Object(request_map) = serde_json::json!({
        "member_id": "_self",
        "profile_data": effective_profile,
    }) else {
        unreachable!("request literal is an object");
    };
    let request_body = CanonicalJsonObjectV1::from_map(request_map, 524_288).unwrap();
    let request_body_digest = request_body.canonical_sha256();
    let HouseholdProfileOutboxEntryV1::OwnerSync { intent, .. } = &mut state.outbox[0].entry else {
        unreachable!("owner-sync fixture");
    };
    intent.intent_revision = 2;
    intent.phase = phase;
    intent.consent_version = Some(ConsentVersionV1::new(1).unwrap());
    intent.remote_base = Some(RemoteProfileBaseV1 {
        existence: RemoteProfileExistenceV1::Absent,
        version: None,
        profile_digest: None,
    });
    intent.request_method = Some("PUT".to_owned());
    intent.request_path = Some("/v1/profile/sync".to_owned());
    intent.request_body = Some(request_body);
    intent.request_body_digest = Some(request_body_digest);
    intent.attempt_count = attempt_count;
    intent.last_definite_error = last_definite_error;
    intent.updated_at = timestamp(1);
    state.revision = HouseholdRevision::new(2).unwrap();
    state.outbox[0].outbox_revision = OutboxRevision::new(2).unwrap();
    state.owner.profile_state = profile_state;
    state.owner.updated_at = timestamp(1);
    state.updated_at = timestamp(1);
    state.validate().unwrap();
    (state, outbox_id)
}

fn owner_sync_handle(state: &HouseholdStateV1) -> OwnerSyncIntentHandleV1 {
    OwnerSyncIntentHandleV1 {
        outbox_id: state.outbox[0].outbox_id.clone(),
        expected_household_revision: state.revision,
        expected_profile_revision: state.profiles[0].profile_revision,
        expected_outbox_revision: state.outbox[0].outbox_revision,
    }
}

fn owner_sync_intent(state: &HouseholdStateV1) -> &OwnerSyncIntentV1 {
    let HouseholdProfileOutboxEntryV1::OwnerSync { intent, .. } = &state.outbox[0].entry else {
        unreachable!("owner-sync fixture");
    };
    intent
}

#[tokio::test]
async fn owner_profile_save_atomically_commits_profile_and_fresh_intent() {
    let state = empty_state("account-a", CommitId::new());
    let repository = Arc::new(MemoryHouseholdRepository::with_state(state.clone()));
    let session = test_session(account("account-a"), repository.clone());
    let profile = HouseholdProfileRecordV1 {
        subject: HouseholdSubjectId::self_(),
        profile_revision: ProfileRevision::new(1).unwrap(),
        document: legacy_profile(),
    };
    let intent = initial_owner_sync_intent(&profile, 2, timestamp(1));
    let saved = session
        .save_owner_profile_and_sync_intent(
            SaveOwnerProfileAndSyncIntentV1 {
                expected_household_revision: state.revision,
                expected_profile_revision: None,
                replaced_intent: None,
                owner_profile: profile.clone(),
                owner_sync_intent: intent.clone(),
                commit_id: CommitId::new(),
                frozen_commit_timestamp: timestamp(1),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let current = repository.snapshot().await.unwrap();
    assert_eq!(saved.commit.resulting_revision, current.revision);
    assert_eq!(saved.handle.expected_household_revision, current.revision);
    assert_eq!(saved.handle.expected_profile_revision.get(), 1);
    assert_eq!(saved.handle.expected_outbox_revision.get(), 1);
    assert_eq!(
        current.owner.profile_state,
        HouseholdProfileStateV1::PendingSync
    );
    assert_eq!(current.profiles, vec![profile]);
    assert_eq!(current.outbox.len(), 1);
    let HouseholdProfileOutboxEntryV1::OwnerSync {
        intent: stored_intent,
        ..
    } = &current.outbox[0].entry
    else {
        panic!("expected owner sync intent");
    };
    assert_eq!(stored_intent, &intent);
    assert_eq!(current.outbox[0].outbox_id, saved.handle.outbox_id);
}

#[tokio::test]
async fn owner_profile_save_replaces_only_exact_replaceable_intent() {
    let state = empty_state("account-a", CommitId::new());
    let repository = Arc::new(MemoryHouseholdRepository::with_state(state.clone()));
    let session = test_session(account("account-a"), repository.clone());
    let first_profile = HouseholdProfileRecordV1 {
        subject: HouseholdSubjectId::self_(),
        profile_revision: ProfileRevision::new(1).unwrap(),
        document: legacy_profile(),
    };
    let first = session
        .save_owner_profile_and_sync_intent(
            SaveOwnerProfileAndSyncIntentV1 {
                expected_household_revision: state.revision,
                expected_profile_revision: None,
                replaced_intent: None,
                owner_profile: first_profile.clone(),
                owner_sync_intent: initial_owner_sync_intent(&first_profile, 2, timestamp(1)),
                commit_id: CommitId::new(),
                frozen_commit_timestamp: timestamp(1),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let after_first = repository.snapshot().await.unwrap();
    let second_profile = HouseholdProfileRecordV1 {
        subject: HouseholdSubjectId::self_(),
        profile_revision: ProfileRevision::new(2).unwrap(),
        document: HouseholdProfileDocumentV1::legacy_projection(br#"{"restrictions":["sesame"]}"#)
            .unwrap(),
    };
    let missing_authority_error = session
        .save_owner_profile_and_sync_intent(
            SaveOwnerProfileAndSyncIntentV1 {
                expected_household_revision: after_first.revision,
                expected_profile_revision: Some(ProfileRevision::new(1).unwrap()),
                replaced_intent: None,
                owner_profile: second_profile.clone(),
                owner_sync_intent: initial_owner_sync_intent(&second_profile, 3, timestamp(2)),
                commit_id: CommitId::new(),
                frozen_commit_timestamp: timestamp(2),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        missing_authority_error.code,
        "owner_sync_replacement_required"
    );
    assert_eq!(repository.snapshot().await.unwrap(), after_first);

    let second = session
        .save_owner_profile_and_sync_intent(
            SaveOwnerProfileAndSyncIntentV1 {
                expected_household_revision: after_first.revision,
                expected_profile_revision: Some(ProfileRevision::new(1).unwrap()),
                replaced_intent: Some(first.handle.clone()),
                owner_profile: second_profile.clone(),
                owner_sync_intent: initial_owner_sync_intent(&second_profile, 3, timestamp(2)),
                commit_id: CommitId::new(),
                frozen_commit_timestamp: timestamp(2),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let current = repository.snapshot().await.unwrap();
    assert_eq!(current.profiles, vec![second_profile]);
    assert_eq!(current.outbox.len(), 1);
    assert_ne!(current.outbox[0].outbox_id, first.handle.outbox_id);
    assert_eq!(current.outbox[0].outbox_id, second.handle.outbox_id);
}

#[tokio::test]
async fn owner_profile_save_cannot_replace_dispatch_authority() {
    let (state, _) = frozen_owner_sync_state(
        OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown,
        1,
        None,
        HouseholdProfileStateV1::PendingSync,
    );
    let eligibility = owner_profile_action_eligibility_v1(
        &state,
        AuthoritativeConsentStateV1::Active(ConsentVersionV1::new(1).unwrap()),
    );
    assert_eq!(
        eligibility.retry,
        OwnerProfileRetryEligibilityV1::ReconcileDispatchingOutcomeUnknown
    );
    let replacement_profile = HouseholdProfileRecordV1 {
        subject: HouseholdSubjectId::self_(),
        profile_revision: ProfileRevision::new(2).unwrap(),
        document: HouseholdProfileDocumentV1::legacy_projection(br#"{"restrictions":["sesame"]}"#)
            .unwrap(),
    };
    let repository = Arc::new(MemoryHouseholdRepository::with_state(state.clone()));
    let session = test_session(account("account-a"), repository.clone());
    let error = session
        .save_owner_profile_and_sync_intent(
            SaveOwnerProfileAndSyncIntentV1 {
                expected_household_revision: state.revision,
                expected_profile_revision: Some(ProfileRevision::new(1).unwrap()),
                replaced_intent: eligibility.intent,
                owner_profile: replacement_profile.clone(),
                owner_sync_intent: initial_owner_sync_intent(&replacement_profile, 2, timestamp(2)),
                commit_id: CommitId::new(),
                frozen_commit_timestamp: timestamp(2),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "owner_sync_replacement_blocked");
    assert_eq!(repository.snapshot().await.unwrap(), state);
}

#[test]
fn unavailable_owner_profile_actions_never_carry_retry_authority() {
    fn assert_unavailable_without_authority(
        state: &HouseholdStateV1,
        consent: AuthoritativeConsentStateV1,
    ) {
        let actions = owner_profile_action_eligibility_v1(state, consent);
        assert!(matches!(
            actions.retry,
            OwnerProfileRetryEligibilityV1::Unavailable { .. }
        ));
        assert!(actions.retry.available_action().is_none());
        assert!(actions.intent.is_none());
    }

    let (mut local_only, _) = owner_sync_state();
    let HouseholdProfileOutboxEntryV1::OwnerSync { intent, .. } = &mut local_only.outbox[0].entry
    else {
        unreachable!("owner-sync fixture");
    };
    intent.phase = OwnerSyncIntentPhaseV1::LocalOnlyNoConsent;
    intent.last_definite_error = Some(LastDefiniteOwnerSyncErrorV1::ConsentAbsent);
    local_only.owner.profile_state = HouseholdProfileStateV1::LocalOnly;
    local_only.validate().unwrap();
    assert_unavailable_without_authority(&local_only, AuthoritativeConsentStateV1::Absent);
}

#[tokio::test]
async fn owner_profile_save_blocks_uncertain_conflicted_and_wrong_authority() {
    for (phase, last_error, profile_state) in [
        (
            OwnerSyncIntentPhaseV1::OutcomeUncertain,
            None,
            HouseholdProfileStateV1::PendingSync,
        ),
        (
            OwnerSyncIntentPhaseV1::Conflicted,
            Some(LastDefiniteOwnerSyncErrorV1::VersionConflict),
            HouseholdProfileStateV1::Conflicted,
        ),
    ] {
        let (state, _) = frozen_owner_sync_state(phase, 1, last_error, profile_state);
        let replacement_profile = HouseholdProfileRecordV1 {
            subject: HouseholdSubjectId::self_(),
            profile_revision: ProfileRevision::new(2).unwrap(),
            document: HouseholdProfileDocumentV1::legacy_projection(
                br#"{"restrictions":["sesame"]}"#,
            )
            .unwrap(),
        };
        let repository = Arc::new(MemoryHouseholdRepository::with_state(state.clone()));
        let session = test_session(account("account-a"), repository.clone());
        let error = session
            .save_owner_profile_and_sync_intent(
                SaveOwnerProfileAndSyncIntentV1 {
                    expected_household_revision: state.revision,
                    expected_profile_revision: Some(ProfileRevision::new(1).unwrap()),
                    replaced_intent: Some(owner_sync_handle(&state)),
                    owner_profile: replacement_profile.clone(),
                    owner_sync_intent: initial_owner_sync_intent(
                        &replacement_profile,
                        2,
                        timestamp(2),
                    ),
                    commit_id: CommitId::new(),
                    frozen_commit_timestamp: timestamp(2),
                },
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "owner_sync_replacement_blocked");
        assert_eq!(repository.snapshot().await.unwrap(), state);
    }

    let (state, _) = owner_sync_state();
    let replacement_profile = HouseholdProfileRecordV1 {
        subject: HouseholdSubjectId::self_(),
        profile_revision: ProfileRevision::new(2).unwrap(),
        document: HouseholdProfileDocumentV1::legacy_projection(br#"{"restrictions":["sesame"]}"#)
            .unwrap(),
    };
    let mut wrong_handle = owner_sync_handle(&state);
    wrong_handle.outbox_id = HouseholdOutboxId::owner_sync(CommitId::new().as_uuid()).unwrap();
    let repository = Arc::new(MemoryHouseholdRepository::with_state(state.clone()));
    let session = test_session(account("account-a"), repository.clone());
    let error = session
        .save_owner_profile_and_sync_intent(
            SaveOwnerProfileAndSyncIntentV1 {
                expected_household_revision: state.revision,
                expected_profile_revision: Some(ProfileRevision::new(1).unwrap()),
                replaced_intent: Some(wrong_handle),
                owner_profile: replacement_profile.clone(),
                owner_sync_intent: initial_owner_sync_intent(&replacement_profile, 2, timestamp(2)),
                commit_id: CommitId::new(),
                frozen_commit_timestamp: timestamp(2),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "owner_sync_intent_invalid");
    assert_eq!(repository.snapshot().await.unwrap(), state);
}

#[test]
fn owner_profile_save_effect_names_exact_prior_intent_and_duplicates_fail_closed() {
    let (current, prior_outbox_id) = owner_sync_state();
    let owner_profile = HouseholdProfileRecordV1 {
        subject: HouseholdSubjectId::self_(),
        profile_revision: ProfileRevision::new(2).unwrap(),
        document: HouseholdProfileDocumentV1::legacy_projection(br#"{"restrictions":["sesame"]}"#)
            .unwrap(),
    };
    let owner_sync_intent = initial_owner_sync_intent(&owner_profile, 2, timestamp(1));
    let owner_sync_record = HouseholdOutboxRecordV1 {
        outbox_id: HouseholdOutboxId::owner_sync(owner_sync_intent.intent_id).unwrap(),
        outbox_revision: OutboxRevision::new(1).unwrap(),
        entry: HouseholdProfileOutboxEntryV1::OwnerSync {
            version: 1,
            target: HouseholdSubjectId::self_(),
            intent: owner_sync_intent,
        },
    };
    let mut candidate = current.clone();
    candidate.revision = candidate.revision.checked_next().unwrap();
    candidate.updated_at = timestamp(1);
    candidate.owner.updated_at = timestamp(1);
    candidate.profiles = vec![owner_profile.clone()];
    candidate.outbox = vec![owner_sync_record.clone()];
    candidate.validate().unwrap();
    let wrong_replaced_id = HouseholdOutboxId::owner_sync(CommitId::new().as_uuid()).unwrap();
    assert_ne!(wrong_replaced_id, prior_outbox_id);
    assert_ne!(wrong_replaced_id, owner_sync_record.outbox_id);
    let command = HouseholdCommit::new(
        account("account-a"),
        current.revision,
        CommitId::new(),
        candidate,
        HouseholdEffectV1::SaveOwnerProfileAndOwnerSyncIntent {
            owner_profile,
            owner_sync_record: Box::new(owner_sync_record),
            replaced_outbox_id: Some(wrong_replaced_id),
        },
        timestamp(1),
    )
    .unwrap();
    let error = resolve_household_commit_v1(Some(&current), &command).unwrap_err();
    assert_eq!(error.code, "owner_sync_replacement_mismatch");

    let (mut duplicate, _) = owner_sync_state();
    let duplicate_intent = initial_owner_sync_intent(
        &duplicate.profiles[0],
        duplicate.revision.get(),
        timestamp(0),
    );
    duplicate.outbox.push(HouseholdOutboxRecordV1 {
        outbox_id: HouseholdOutboxId::owner_sync(duplicate_intent.intent_id).unwrap(),
        outbox_revision: OutboxRevision::new(1).unwrap(),
        entry: HouseholdProfileOutboxEntryV1::OwnerSync {
            version: 1,
            target: HouseholdSubjectId::self_(),
            intent: duplicate_intent,
        },
    });
    duplicate.outbox.sort_by(|left, right| {
        left.outbox_id
            .as_str()
            .as_bytes()
            .cmp(right.outbox_id.as_str().as_bytes())
    });
    let error = HouseholdLoad::from_state(duplicate).unwrap_err();
    assert_eq!(error.code, "owner_sync_intent_invalid");
}

#[tokio::test]
async fn owner_sync_attempt_count_advances_exactly_once_only_for_dispatch() {
    let (ready, _) = frozen_owner_sync_state(
        OwnerSyncIntentPhaseV1::ReadyToDispatch,
        0,
        None,
        HouseholdProfileStateV1::PendingSync,
    );
    let repository = Arc::new(MemoryHouseholdRepository::with_state(ready.clone()));
    let session = test_session(account("account-a"), repository.clone());
    let handle = owner_sync_handle(&ready);
    let mut skipped_attempt = owner_sync_intent(&ready).clone();
    skipped_attempt.intent_revision = 3;
    skipped_attempt.phase = OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown;
    skipped_attempt.attempt_count = 2;
    skipped_attempt.updated_at = timestamp(2);
    let error = session
        .transition_owner_sync_intent(
            TransitionOwnerSyncIntentV1 {
                handle: handle.clone(),
                event: OwnerSyncTransitionEventV1::DispatchStarted,
                replacement: Some(skipped_attempt),
                resulting_profile_state: HouseholdProfileStateV1::PendingSync,
                commit_id: CommitId::new(),
                frozen_commit_timestamp: timestamp(2),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "owner_sync_attempt_count_invalid");
    assert_eq!(repository.snapshot().await.unwrap(), ready);

    let mut exact_attempt = owner_sync_intent(&ready).clone();
    exact_attempt.intent_revision = 3;
    exact_attempt.phase = OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown;
    exact_attempt.attempt_count = 1;
    exact_attempt.updated_at = timestamp(2);
    session
        .transition_owner_sync_intent(
            TransitionOwnerSyncIntentV1 {
                handle,
                event: OwnerSyncTransitionEventV1::DispatchStarted,
                replacement: Some(exact_attempt),
                resulting_profile_state: HouseholdProfileStateV1::PendingSync,
                commit_id: CommitId::new(),
                frozen_commit_timestamp: timestamp(2),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        owner_sync_intent(&repository.snapshot().await.unwrap()).attempt_count,
        1
    );

    let (dispatching, _) = frozen_owner_sync_state(
        OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown,
        1,
        None,
        HouseholdProfileStateV1::PendingSync,
    );
    let repository = Arc::new(MemoryHouseholdRepository::with_state(dispatching.clone()));
    let session = test_session(account("account-a"), repository.clone());
    let mut changed_outside_dispatch = owner_sync_intent(&dispatching).clone();
    changed_outside_dispatch.intent_revision = 3;
    changed_outside_dispatch.phase = OwnerSyncIntentPhaseV1::OutcomeUncertain;
    changed_outside_dispatch.attempt_count = 2;
    changed_outside_dispatch.updated_at = timestamp(2);
    let error = session
        .transition_owner_sync_intent(
            TransitionOwnerSyncIntentV1 {
                handle: owner_sync_handle(&dispatching),
                event: OwnerSyncTransitionEventV1::DispatchOutcomeUncertain,
                replacement: Some(changed_outside_dispatch),
                resulting_profile_state: HouseholdProfileStateV1::PendingSync,
                commit_id: CommitId::new(),
                frozen_commit_timestamp: timestamp(2),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "owner_sync_attempt_count_invalid");
    assert_eq!(repository.snapshot().await.unwrap(), dispatching);
}

#[tokio::test]
async fn owner_sync_revision_and_attempt_overflow_fail_closed() {
    let (attempt_max, _) = frozen_owner_sync_state(
        OwnerSyncIntentPhaseV1::ReadyToDispatch,
        u32::MAX,
        None,
        HouseholdProfileStateV1::PendingSync,
    );
    let repository = Arc::new(MemoryHouseholdRepository::with_state(attempt_max.clone()));
    let session = test_session(account("account-a"), repository.clone());
    let mut replacement = owner_sync_intent(&attempt_max).clone();
    replacement.intent_revision = 3;
    replacement.phase = OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown;
    replacement.updated_at = timestamp(2);
    let error = session
        .transition_owner_sync_intent(
            TransitionOwnerSyncIntentV1 {
                handle: owner_sync_handle(&attempt_max),
                event: OwnerSyncTransitionEventV1::DispatchStarted,
                replacement: Some(replacement),
                resulting_profile_state: HouseholdProfileStateV1::PendingSync,
                commit_id: CommitId::new(),
                frozen_commit_timestamp: timestamp(2),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "owner_sync_attempt_count_overflow");
    assert_eq!(repository.snapshot().await.unwrap(), attempt_max);

    let (mut revision_max, _) = frozen_owner_sync_state(
        OwnerSyncIntentPhaseV1::ReadyToDispatch,
        0,
        None,
        HouseholdProfileStateV1::PendingSync,
    );
    let HouseholdProfileOutboxEntryV1::OwnerSync { intent, .. } = &mut revision_max.outbox[0].entry
    else {
        unreachable!("owner-sync fixture");
    };
    intent.intent_revision = u64::MAX;
    revision_max.outbox[0].outbox_revision = OutboxRevision::new(u64::MAX).unwrap();
    revision_max.validate().unwrap();
    let repository = Arc::new(MemoryHouseholdRepository::with_state(revision_max.clone()));
    let session = test_session(account("account-a"), repository.clone());
    let mut replacement = owner_sync_intent(&revision_max).clone();
    replacement.phase = OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown;
    replacement.attempt_count = 1;
    replacement.updated_at = timestamp(2);
    let error = session
        .transition_owner_sync_intent(
            TransitionOwnerSyncIntentV1 {
                handle: owner_sync_handle(&revision_max),
                event: OwnerSyncTransitionEventV1::DispatchStarted,
                replacement: Some(replacement),
                resulting_profile_state: HouseholdProfileStateV1::PendingSync,
                commit_id: CommitId::new(),
                frozen_commit_timestamp: timestamp(2),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "owner_sync_revision_overflow");
    assert_eq!(repository.snapshot().await.unwrap(), revision_max);
}

#[tokio::test]
async fn owner_sync_definite_failure_is_source_and_event_specific() {
    let (ready, _) = frozen_owner_sync_state(
        OwnerSyncIntentPhaseV1::ReadyToDispatch,
        1,
        None,
        HouseholdProfileStateV1::PendingSync,
    );
    let repository = Arc::new(MemoryHouseholdRepository::with_state(ready.clone()));
    let session = test_session(account("account-a"), repository.clone());
    let mut fabricated_http_failure = owner_sync_intent(&ready).clone();
    fabricated_http_failure.intent_revision = 3;
    fabricated_http_failure.phase = OwnerSyncIntentPhaseV1::DefiniteFailure;
    fabricated_http_failure.last_definite_error = Some(LastDefiniteOwnerSyncErrorV1::Unauthorized);
    fabricated_http_failure.updated_at = timestamp(2);
    let error = session
        .transition_owner_sync_intent(
            TransitionOwnerSyncIntentV1 {
                handle: owner_sync_handle(&ready),
                event: OwnerSyncTransitionEventV1::DefiniteRemoteFailure,
                replacement: Some(fabricated_http_failure),
                resulting_profile_state: HouseholdProfileStateV1::PendingSync,
                commit_id: CommitId::new(),
                frozen_commit_timestamp: timestamp(2),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "owner_sync_transition_event_invalid");
    assert_eq!(repository.snapshot().await.unwrap(), ready);

    let (outcome_uncertain, _) = frozen_owner_sync_state(
        OwnerSyncIntentPhaseV1::OutcomeUncertain,
        1,
        None,
        HouseholdProfileStateV1::PendingSync,
    );
    let repository = Arc::new(MemoryHouseholdRepository::with_state(
        outcome_uncertain.clone(),
    ));
    let session = test_session(account("account-a"), repository.clone());
    let mut fabricated_reconciliation_failure = owner_sync_intent(&outcome_uncertain).clone();
    fabricated_reconciliation_failure.intent_revision = 3;
    fabricated_reconciliation_failure.phase = OwnerSyncIntentPhaseV1::DefiniteFailure;
    fabricated_reconciliation_failure.last_definite_error =
        Some(LastDefiniteOwnerSyncErrorV1::Unauthorized);
    fabricated_reconciliation_failure.updated_at = timestamp(2);
    let error = session
        .transition_owner_sync_intent(
            TransitionOwnerSyncIntentV1 {
                handle: owner_sync_handle(&outcome_uncertain),
                event: OwnerSyncTransitionEventV1::DefiniteRemoteFailure,
                replacement: Some(fabricated_reconciliation_failure),
                resulting_profile_state: HouseholdProfileStateV1::PendingSync,
                commit_id: CommitId::new(),
                frozen_commit_timestamp: timestamp(2),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "owner_sync_transition_event_invalid");
    assert_eq!(repository.snapshot().await.unwrap(), outcome_uncertain);

    let (dispatching, _) = frozen_owner_sync_state(
        OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown,
        1,
        None,
        HouseholdProfileStateV1::PendingSync,
    );
    let repository = Arc::new(MemoryHouseholdRepository::with_state(dispatching.clone()));
    let session = test_session(account("account-a"), repository.clone());
    let handle = owner_sync_handle(&dispatching);
    let mut definite_failure = owner_sync_intent(&dispatching).clone();
    definite_failure.intent_revision = 3;
    definite_failure.phase = OwnerSyncIntentPhaseV1::DefiniteFailure;
    definite_failure.last_definite_error = Some(LastDefiniteOwnerSyncErrorV1::Unauthorized);
    definite_failure.updated_at = timestamp(2);
    let error = session
        .transition_owner_sync_intent(
            TransitionOwnerSyncIntentV1 {
                handle: handle.clone(),
                event: OwnerSyncTransitionEventV1::ConsentRevoked,
                replacement: Some(definite_failure.clone()),
                resulting_profile_state: HouseholdProfileStateV1::PendingSync,
                commit_id: CommitId::new(),
                frozen_commit_timestamp: timestamp(2),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "owner_sync_transition_event_mismatch");
    assert_eq!(repository.snapshot().await.unwrap(), dispatching);

    session
        .transition_owner_sync_intent(
            TransitionOwnerSyncIntentV1 {
                handle,
                event: OwnerSyncTransitionEventV1::DefiniteRemoteFailure,
                replacement: Some(definite_failure),
                resulting_profile_state: HouseholdProfileStateV1::PendingSync,
                commit_id: CommitId::new(),
                frozen_commit_timestamp: timestamp(2),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let current = repository.snapshot().await.unwrap();
    assert_eq!(
        owner_sync_intent(&current).last_definite_error,
        Some(LastDefiniteOwnerSyncErrorV1::Unauthorized)
    );
}

#[tokio::test]
async fn owner_sync_transition_uses_three_revisions_and_preserves_profile_content() {
    let (state, outbox_id) = owner_sync_state();
    let repository = Arc::new(MemoryHouseholdRepository::with_state(state.clone()));
    let session = test_session(account("account-a"), repository.clone());
    let eligibility = owner_profile_action_eligibility_v1(
        &state,
        AuthoritativeConsentStateV1::Active(ConsentVersionV1::new(2).unwrap()),
    );
    assert_eq!(
        eligibility.retry,
        OwnerProfileRetryEligibilityV1::ResumeNeedsConsentCheck
    );
    let handle = eligibility.intent.unwrap();
    assert_eq!(handle.outbox_id, outbox_id);

    let old_intent = match &state.outbox[0].entry {
        HouseholdProfileOutboxEntryV1::OwnerSync { intent, .. } => intent,
        HouseholdProfileOutboxEntryV1::Legacy { .. } => unreachable!(),
    };
    let mut replacement = old_intent.clone();
    replacement.intent_revision = 2;
    replacement.phase = OwnerSyncIntentPhaseV1::NeedsRemoteBase;
    replacement.consent_version = Some(ConsentVersionV1::new(2).unwrap());
    replacement.updated_at = timestamp(1);
    session
        .transition_owner_sync_intent(
            TransitionOwnerSyncIntentV1 {
                handle,
                event: OwnerSyncTransitionEventV1::ActiveConsentObserved,
                replacement: Some(replacement),
                resulting_profile_state: HouseholdProfileStateV1::PendingSync,
                commit_id: CommitId::new(),
                frozen_commit_timestamp: timestamp(1),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let current = repository.snapshot().await.unwrap();
    assert_eq!(current.revision.get(), 2);
    assert_eq!(current.profiles[0], state.profiles[0]);
    assert_eq!(current.outbox[0].outbox_revision.get(), 2);
    let HouseholdProfileOutboxEntryV1::OwnerSync { intent, .. } = &current.outbox[0].entry else {
        panic!("expected owner intent");
    };
    assert_eq!(intent.intent_revision, 2);
    assert_eq!(intent.consent_version.unwrap().get(), 2);
}

#[tokio::test]
async fn owner_sync_rejects_illegal_edges_and_frozen_authority_changes() {
    let (state, _) = owner_sync_state();
    let repository = Arc::new(MemoryHouseholdRepository::with_state(state.clone()));
    let session = test_session(account("account-a"), repository.clone());
    let eligibility = owner_profile_action_eligibility_v1(
        &state,
        AuthoritativeConsentStateV1::Active(ConsentVersionV1::new(2).unwrap()),
    );
    let handle = eligibility.intent.unwrap();
    let old_intent = match &state.outbox[0].entry {
        HouseholdProfileOutboxEntryV1::OwnerSync { intent, .. } => intent,
        HouseholdProfileOutboxEntryV1::Legacy { .. } => unreachable!(),
    };
    let mut replacement = old_intent.clone();
    replacement.intent_revision = 2;
    replacement.phase = OwnerSyncIntentPhaseV1::OutcomeUncertain;
    replacement.consent_version = Some(ConsentVersionV1::new(2).unwrap());
    replacement.attempt_count = 1;
    replacement.last_definite_error = Some(LastDefiniteOwnerSyncErrorV1::VersionConflict);
    replacement.updated_at = timestamp(1);
    let error = session
        .transition_owner_sync_intent(
            TransitionOwnerSyncIntentV1 {
                handle,
                event: OwnerSyncTransitionEventV1::DispatchOutcomeUncertain,
                replacement: Some(replacement),
                resulting_profile_state: HouseholdProfileStateV1::PendingSync,
                commit_id: CommitId::new(),
                frozen_commit_timestamp: timestamp(1),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "owner_sync_transition_invalid");
    assert_eq!(repository.snapshot().await.unwrap(), state);
}

#[tokio::test]
async fn account_erasure_is_revision_bound_and_truthful() {
    let state = empty_state("account-a", CommitId::new());
    let repository = Arc::new(MemoryHouseholdRepository::with_state(state.clone()));
    let session = test_session(account("account-a"), repository.clone());
    let error = session
        .erase_account(
            Some(HouseholdRevision::new(2).unwrap()),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "household_revision_conflict");
    let outcome = session
        .erase_account(Some(state.revision), CancellationToken::new())
        .await
        .unwrap();
    assert!(outcome.household_key_deleted);
    assert!(outcome.household_ciphertext_deleted);
    assert!(outcome.local_credentials_cleared);
    assert!(repository.snapshot().await.is_none());
}

#[test]
fn applied_ledger_at_capacity_blocks_new_writes_but_replay_still_works() {
    let initial_commit = CommitId::new();
    let current = empty_state("account-a", initial_commit);
    let mut candidate = current.clone();
    candidate.revision = current.revision.checked_next().unwrap();
    candidate.updated_at = timestamp(1);
    let replay_id = CommitId::new();
    let replay_command = HouseholdCommit::new(
        account("account-a"),
        current.revision,
        replay_id,
        candidate.clone(),
        HouseholdEffectV1::SelectScope {
            scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
        },
        timestamp(1),
    )
    .unwrap();
    let replay_fingerprint = replay_command.claimed_effect_fingerprint.as_digest();

    let mut full = current.clone();
    full.revision = candidate.revision;
    full.updated_at = timestamp(1);
    full.bounded_applied_commits = (0..heyfood_core::MAX_APPLIED_COMMITS - 1)
        .map(|_| AppliedCommitRecordV1 {
            commit_id: CommitId::new(),
            fingerprint: CanonicalDigestV1::from_bytes([3; 32]),
            resulting_revision: current.revision,
            outcome: AppliedCommitOutcomeV1::Committed,
            committed_at: timestamp(0),
        })
        .collect();
    full.bounded_applied_commits.push(AppliedCommitRecordV1 {
        commit_id: replay_id,
        fingerprint: replay_fingerprint,
        resulting_revision: candidate.revision,
        outcome: AppliedCommitOutcomeV1::Committed,
        committed_at: timestamp(1),
    });
    full.bounded_applied_commits.sort_by(|left, right| {
        left.commit_id
            .as_uuid()
            .as_bytes()
            .cmp(right.commit_id.as_uuid().as_bytes())
    });
    full.validate().unwrap();
    assert!(matches!(
        resolve_household_commit_v1(Some(&full), &replay_command).unwrap(),
        HouseholdRepositoryResolutionV1::Replay(_)
    ));

    let mut new_candidate = full.clone();
    new_candidate.revision = full.revision.checked_next().unwrap();
    new_candidate.updated_at = timestamp(2);
    let new_command = HouseholdCommit::new(
        account("account-a"),
        full.revision,
        CommitId::new(),
        new_candidate,
        HouseholdEffectV1::SelectScope {
            scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
        },
        timestamp(2),
    )
    .unwrap();
    let error = resolve_household_commit_v1(Some(&full), &new_command).unwrap_err();
    assert_eq!(error.code, "household_applied_commit_ledger_full");
}

#[tokio::test]
async fn create_member_profile_and_selection_commit_as_one_local_only_delta() {
    let repository = Arc::new(MemoryHouseholdRepository::with_state(initialized_state(
        "account-a",
    )));
    let member_id = heyfood_core::MemberId::new();
    let mutation_authority = Arc::new(FixedMutationAuthority::new([(
        HouseholdMutationPurposeV1::CreateMember,
        authority(1, Some(member_id.clone())),
    )]));
    let session = HouseholdSession::new(
        account("account-a"),
        repository.clone(),
        mutation_authority.clone(),
    );
    let request = CreateMemberWithDeclaredProfileV1 {
        expected_household_revision: HouseholdRevision::new(1).unwrap(),
        display_name: DisplayName::parse("Canary household name").unwrap(),
        relationship: RelationshipV1::Child,
        age_evidence: NativeMemberAgeEvidenceV1::Age18Plus,
        declared_profile: declared_profile(),
    };
    let request_debug = format!("{request:?}");
    assert!(!request_debug.contains("Canary household name"));
    assert!(!request_debug.contains("Age18Plus"));
    assert!(!request_debug.contains("Child"));
    assert!(!request_debug.contains("canary private"));
    let created = session
        .create_member_with_declared_profile(request, CancellationToken::new())
        .await
        .unwrap();

    let state = repository.snapshot().await.unwrap();
    assert_eq!(mutation_authority.calls.load(Ordering::SeqCst), 1);
    assert_eq!(state.revision.get(), 2);
    assert_eq!(state.members.len(), 1);
    assert_eq!(state.profiles.len(), 1);
    assert!(state.outbox.is_empty());
    assert_eq!(state.members[0].member_id, member_id);
    assert_eq!(state.members[0].minor_status, MinorStatusV1::Minor);
    assert_eq!(
        state.members[0].relationship_source,
        RelationshipSourceV1::NativeDeclared
    );
    assert_eq!(
        state.members[0].profile_state,
        HouseholdProfileStateV1::LocalOnly
    );
    assert_eq!(state.profiles[0].profile_revision.get(), 1);
    assert_eq!(
        state.profiles[0].document.provenance,
        heyfood_core::ProfileDocumentProvenanceV1::NativeDeclared
    );
    assert_eq!(
        state.active_scope,
        HouseholdScope::Subject(HouseholdSubjectId::member(member_id.clone()))
    );
    assert_eq!(created.member_id, member_id);
    assert_eq!(created.resulting_household_revision.get(), 2);
    let debug = format!("{created:?}");
    assert!(!debug.contains(member_id.as_str()));
    assert!(!debug.contains("Canary household name"));
    assert!(!format!("{:?}", state.profiles[0]).contains("canary private"));
}

#[tokio::test]
async fn native_relationship_and_age_evidence_rederive_minor_status() {
    for (relationship, evidence, expected) in [
        (
            RelationshipV1::Child,
            NativeMemberAgeEvidenceV1::Age18Plus,
            MinorStatusV1::Minor,
        ),
        (
            RelationshipV1::Friend,
            NativeMemberAgeEvidenceV1::Under13,
            MinorStatusV1::Minor,
        ),
        (
            RelationshipV1::Friend,
            NativeMemberAgeEvidenceV1::Age13_17,
            MinorStatusV1::Minor,
        ),
        (
            RelationshipV1::Friend,
            NativeMemberAgeEvidenceV1::Age18Plus,
            MinorStatusV1::Adult,
        ),
        (
            RelationshipV1::Friend,
            NativeMemberAgeEvidenceV1::Unknown,
            MinorStatusV1::Unknown,
        ),
    ] {
        let repository = Arc::new(MemoryHouseholdRepository::with_state(initialized_state(
            "account-a",
        )));
        let member_id = heyfood_core::MemberId::new();
        let mutation_authority = Arc::new(FixedMutationAuthority::new([(
            HouseholdMutationPurposeV1::CreateMember,
            authority(1, Some(member_id)),
        )]));
        let session =
            HouseholdSession::new(account("account-a"), repository.clone(), mutation_authority);
        session
            .create_member_with_declared_profile(
                CreateMemberWithDeclaredProfileV1 {
                    expected_household_revision: HouseholdRevision::new(1).unwrap(),
                    display_name: DisplayName::parse("Repeated labels are allowed").unwrap(),
                    relationship,
                    age_evidence: evidence,
                    declared_profile: declared_profile(),
                },
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            repository.snapshot().await.unwrap().members[0].minor_status,
            expected
        );
    }
}

#[tokio::test]
async fn stale_invalid_cancelled_and_full_create_requests_allocate_no_authority() {
    let repository = Arc::new(MemoryHouseholdRepository::with_state(initialized_state(
        "account-a",
    )));
    let mutation_authority = Arc::new(FixedMutationAuthority::new([]));
    let session =
        HouseholdSession::new(account("account-a"), repository, mutation_authority.clone());
    let stale = session
        .create_member_with_declared_profile(
            CreateMemberWithDeclaredProfileV1 {
                expected_household_revision: HouseholdRevision::new(2).unwrap(),
                display_name: DisplayName::parse("Stale name").unwrap(),
                relationship: RelationshipV1::Friend,
                age_evidence: NativeMemberAgeEvidenceV1::Unknown,
                declared_profile: declared_profile(),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(stale.code, "household_revision_conflict");
    assert_eq!(mutation_authority.calls.load(Ordering::SeqCst), 0);

    let invalid_profile = session
        .create_member_with_declared_profile(
            CreateMemberWithDeclaredProfileV1 {
                expected_household_revision: HouseholdRevision::new(1).unwrap(),
                display_name: DisplayName::parse("Invalid profile").unwrap(),
                relationship: RelationshipV1::Friend,
                age_evidence: NativeMemberAgeEvidenceV1::Unknown,
                declared_profile: OnboardingProfileInput {
                    diet_style_ids: vec!["not-a-catalog-id".to_owned()],
                    ..OnboardingProfileInput::default()
                },
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(invalid_profile.code, "household_declared_profile_invalid");
    assert_eq!(mutation_authority.calls.load(Ordering::SeqCst), 0);

    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let cancelled = session
        .create_member_with_declared_profile(
            CreateMemberWithDeclaredProfileV1 {
                expected_household_revision: HouseholdRevision::new(1).unwrap(),
                display_name: DisplayName::parse("Cancelled name").unwrap(),
                relationship: RelationshipV1::Friend,
                age_evidence: NativeMemberAgeEvidenceV1::Unknown,
                declared_profile: declared_profile(),
            },
            cancellation,
        )
        .await
        .unwrap_err();
    assert_eq!(cancelled.code, "household_member_create_cancelled");
    assert_eq!(mutation_authority.calls.load(Ordering::SeqCst), 0);

    let mut full = initialized_state("account-a");
    full.members = (0..MAX_HOUSEHOLD_MEMBERS)
        .map(|index| {
            let mut value = member(
                &format!("Member {index}"),
                MinorStatusV1::Unknown,
                HouseholdProfileStateV1::Incomplete,
            );
            value.relationship_source = RelationshipSourceV1::NativeDeclared;
            value
        })
        .collect();
    full.members.sort_by(|left, right| {
        left.member_id
            .as_str()
            .as_bytes()
            .cmp(right.member_id.as_str().as_bytes())
    });
    full.validate().unwrap();
    let full_repository = Arc::new(MemoryHouseholdRepository::with_state(full));
    let full_authority = Arc::new(FixedMutationAuthority::new([]));
    let full_session = HouseholdSession::new(
        account("account-a"),
        full_repository,
        full_authority.clone(),
    );
    let full_error = full_session
        .create_member_with_declared_profile(
            CreateMemberWithDeclaredProfileV1 {
                expected_household_revision: HouseholdRevision::new(1).unwrap(),
                display_name: DisplayName::parse("One too many").unwrap(),
                relationship: RelationshipV1::Friend,
                age_evidence: NativeMemberAgeEvidenceV1::Unknown,
                declared_profile: declared_profile(),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(full_error.code, "household_member_capacity");
    assert_eq!(full_authority.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn post_dispatch_uncertainty_replays_exact_create_without_new_identity() {
    let repository = Arc::new(CommitThenUncertainRepository::with_state(
        initialized_state("account-a"),
    ));
    let member_id = heyfood_core::MemberId::new();
    let mutation_authority = Arc::new(FixedMutationAuthority::new([(
        HouseholdMutationPurposeV1::CreateMember,
        authority(1, Some(member_id.clone())),
    )]));
    let session = HouseholdSession::new(
        account("account-a"),
        repository.clone(),
        mutation_authority.clone(),
    );
    let created = session
        .create_member_with_declared_profile(
            CreateMemberWithDeclaredProfileV1 {
                expected_household_revision: HouseholdRevision::new(1).unwrap(),
                display_name: DisplayName::parse("Uncertain outcome").unwrap(),
                relationship: RelationshipV1::Friend,
                age_evidence: NativeMemberAgeEvidenceV1::Age18Plus,
                declared_profile: declared_profile(),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let state = repository.snapshot().await;
    let commit_ids = repository.commit_ids.lock().unwrap().clone();
    assert_eq!(repository.commit_calls.load(Ordering::SeqCst), 2);
    assert_eq!(commit_ids.len(), 2);
    assert_eq!(commit_ids[0], commit_ids[1]);
    assert_eq!(mutation_authority.calls.load(Ordering::SeqCst), 1);
    assert_eq!(state.revision.get(), 2);
    assert_eq!(state.members.len(), 1);
    assert_eq!(state.members[0].member_id, member_id);
    assert_eq!(created.member_id, member_id);
}

#[tokio::test]
async fn existing_member_profile_save_is_local_only_and_advances_exact_revisions() {
    let mut state = initialized_state("account-a");
    let member = member(
        "Existing member",
        MinorStatusV1::Adult,
        HouseholdProfileStateV1::Incomplete,
    );
    let member_id = member.member_id.clone();
    state.members.push(member);
    state.validate().unwrap();
    let repository = Arc::new(MemoryHouseholdRepository::with_state(state));
    let mutation_authority = Arc::new(FixedMutationAuthority::new([
        (
            HouseholdMutationPurposeV1::SaveMemberProfile,
            authority(1, None),
        ),
        (
            HouseholdMutationPurposeV1::SaveMemberProfile,
            authority(2, None),
        ),
    ]));
    let session = HouseholdSession::new(
        account("account-a"),
        repository.clone(),
        mutation_authority.clone(),
    );
    let first = session
        .save_member_declared_profile(
            SaveMemberDeclaredProfileV1 {
                expected_household_revision: HouseholdRevision::new(1).unwrap(),
                member_id: member_id.clone(),
                expected_profile_revision: None,
                declared_profile: declared_profile(),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(first.resulting_household_revision.get(), 2);
    assert_eq!(first.profile_revision.get(), 1);
    let stale_profile = session
        .save_member_declared_profile(
            SaveMemberDeclaredProfileV1 {
                expected_household_revision: first.resulting_household_revision,
                member_id: member_id.clone(),
                expected_profile_revision: None,
                declared_profile: declared_profile(),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        stale_profile.code,
        "household_member_profile_revision_conflict"
    );
    assert_eq!(mutation_authority.calls.load(Ordering::SeqCst), 1);
    let second = session
        .save_member_declared_profile(
            SaveMemberDeclaredProfileV1 {
                expected_household_revision: first.resulting_household_revision,
                member_id: member_id.clone(),
                expected_profile_revision: Some(first.profile_revision),
                declared_profile: OnboardingProfileInput {
                    diet_style_ids: vec!["vegetarian".to_owned()],
                    ..OnboardingProfileInput::default()
                },
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let state = repository.snapshot().await.unwrap();
    assert_eq!(second.resulting_household_revision.get(), 3);
    assert_eq!(second.profile_revision.get(), 2);
    assert_eq!(state.profiles.len(), 1);
    assert!(state.outbox.is_empty());
    assert_eq!(
        state.members[0].profile_state,
        HouseholdProfileStateV1::LocalOnly
    );
    assert_eq!(mutation_authority.calls.load(Ordering::SeqCst), 2);
}

fn apply_phase0_agent_effect(
    current: &HouseholdStateV1,
    candidate: HouseholdStateV1,
    effect: HouseholdEffectV1,
    committed_at: CanonicalTimestampV1,
) -> (HouseholdStateV1, HouseholdCommit) {
    let command = HouseholdCommit::new(
        current.account_binding.clone(),
        current.revision,
        CommitId::new(),
        candidate,
        effect,
        committed_at,
    )
    .expect("complete candidate freezes its fingerprint");
    let first = resolve_household_commit_v1(Some(current), &command).expect("first resolution");
    let crash_replay =
        resolve_household_commit_v1(Some(current), &command).expect("pre-persistence crash replay");
    assert_eq!(first, crash_replay);
    let HouseholdRepositoryResolutionV1::Write { state, outcome } = first else {
        panic!("new command must produce a write")
    };
    assert_eq!(outcome.resulting_revision, state.revision);
    assert!(state.bounded_applied_commits.iter().any(|record| {
        record.commit_id == command.commit_id
            && record.fingerprint == command.claimed_effect_fingerprint.as_digest()
    }));
    assert!(matches!(
        resolve_household_commit_v1(Some(&state), &command).expect("exact replay"),
        HouseholdRepositoryResolutionV1::Replay(_)
    ));
    (*state, command)
}

#[test]
fn phase0_agent_effects_execute_all_five_exact_once_repository_paths() {
    let mut state = initialized_state("phase0-five-effects");
    let original_scope = state.active_scope.clone();
    let member_id = heyfood_core::MemberId::new();

    let (mut add_candidate, legacy_add_effect) =
        atomic_create_candidate(&state, member_id.clone(), "Synthetic Member", timestamp(1));
    let HouseholdEffectV1::CreateMemberWithDeclaredProfile {
        member,
        profile,
        selected_scope: _,
    } = legacy_add_effect
    else {
        unreachable!()
    };
    add_candidate.active_scope = original_scope.clone();
    let add_effect = HouseholdEffectV1::CreateMemberWithDeclaredProfileAndScope {
        member: member.clone(),
        profile: profile.clone(),
        previous_scope: original_scope.clone(),
        resulting_scope: original_scope.clone(),
    };
    let add_commit_id = CommitId::new();
    let add_command = HouseholdCommit::new(
        state.account_binding.clone(),
        state.revision,
        add_commit_id,
        add_candidate,
        add_effect,
        timestamp(1),
    )
    .expect("complete add candidate freezes its fingerprint");
    let proposal_digest = CanonicalDigestV1::from_bytes([0x51; 32]);
    let proposal_ref = AgentHouseholdProposalIdV1::new();
    let repository_secret = [0x6b; 32];
    let repository_authority = HouseholdCommitEvidenceRepositoryAuthorityV1::from_repository_secret(
        state.account_binding.clone(),
        proposal_ref,
        add_commit_id,
        &repository_secret,
    );
    let commit_evidence = repository_authority.binding();
    let binding = LocalHouseholdProposalBindingV1::new(
        state.account_binding.clone(),
        proposal_ref,
        AgentHouseholdOperationV1::Add,
        GenerationId::new(3),
        CanonicalDigestV1::from_bytes([0x52; 32]),
        AgentDisclosurePurposeV1::HouseholdAgentProposalStatus,
        GenerationId::new(9),
        AgentHouseholdProjectionV1::ContentFree,
        state.revision,
        None,
        add_commit_id,
        commit_evidence.clone(),
        Some(member_id.clone()),
        original_scope.clone(),
        CanonicalDigestV1::from_bytes([0x53; 32]),
        CanonicalDigestV1::from_bytes([0x54; 32]),
        timestamp(0),
        timestamp(59),
    )
    .expect("closed proposal binding");
    let current_authority = LocalHouseholdAuthoritySnapshotV1::new(
        state.account_binding.clone(),
        GenerationId::new(3),
        CanonicalDigestV1::from_bytes([0x52; 32]),
        AgentDisclosurePurposeV1::HouseholdAgentProposalStatus,
        true,
        AgentHouseholdProjectionV1::Profile,
        GenerationId::new(9),
        state.revision,
        None,
        timestamp(1),
    );
    let frozen = LocalHouseholdFrozenCandidateV1::new(
        proposal_digest,
        add_command.claimed_effect_fingerprint,
        CanonicalDigestV1::from_bytes([0x55; 32]),
        CanonicalDigestV1::from_bytes([0x56; 32]),
        original_scope.clone(),
        false,
        timestamp(1),
    );
    let mut journal = LocalHouseholdProposalJournalV1::new(
        LocalHouseholdProposalAuthorityV1::awaiting_local_input(binding),
    )
    .expect("initial proposal journal");
    let intake_token = journal.cas_token();
    journal
        .freeze_for_review(&intake_token, &current_authority, frozen)
        .expect("intake and fingerprint freeze CAS");
    let review_token = journal.cas_token();
    journal
        .begin_commit(&review_token, &current_authority, proposal_digest)
        .expect("review-to-commit CAS");
    let crash_journal = journal
        .persisted_bytes()
        .expect("durable committing journal");

    let unapplied_proof = repository_authority
        .seal_unapplied_repository_observation(&commit_evidence, state.revision)
        .expect("unchanged authoritative repository proves no commit");
    let mut unapplied_recovered =
        LocalHouseholdProposalJournalV1::restore(&crash_journal).expect("journal restart");
    let uncertain_token = unapplied_recovered.cas_token();
    unapplied_recovered
        .mark_reconciliation_required(&uncertain_token)
        .expect("mark uncertain outcome");
    let reconciliation_token = unapplied_recovered.cas_token();
    unapplied_recovered
        .reconcile_unapplied_commit(&reconciliation_token, &unapplied_proof)
        .expect("repository-held authority proves the mutation was not applied");
    assert_eq!(
        unapplied_recovered.state(),
        heyfood_core::AgentHouseholdProposalStateV1::ProvenUncommitted
    );

    let HouseholdRepositoryResolutionV1::Write {
        state: after_add,
        outcome: add_outcome,
    } = resolve_household_commit_v1(Some(&state), &add_command).expect("repository write")
    else {
        panic!("new add command must write")
    };
    assert_eq!(add_outcome.resulting_revision, after_add.revision);
    let applied = after_add
        .bounded_applied_commits
        .iter()
        .find(|record| record.commit_id == add_commit_id)
        .expect("co-committed applied marker");
    assert_eq!(
        applied.fingerprint,
        add_command.claimed_effect_fingerprint.as_digest()
    );
    assert!(
        after_add
            .bounded_applied_commits
            .iter()
            .any(|record| record.commit_id == add_commit_id)
    );
    let mut recovered =
        LocalHouseholdProposalJournalV1::restore(&crash_journal).expect("journal restart");
    let committing_token = recovered.cas_token();
    let forged_secret = [0x7c; 32];
    let forged_authority = HouseholdCommitEvidenceRepositoryAuthorityV1::from_repository_secret(
        after_add.account_binding.clone(),
        proposal_ref,
        add_commit_id,
        &forged_secret,
    );
    let forged_binding = forged_authority.binding();
    let forged_proof = forged_authority
        .seal_applied_repository_observation(
            &forged_binding,
            HouseholdEffectFingerprintV1::from_digest(applied.fingerprint),
            applied.resulting_revision,
        )
        .expect("independent authority can seal but cannot match the journal");
    assert_eq!(
        recovered.reconcile_applied_commit(&committing_token, &forged_proof),
        Err(heyfood_core::AgentHouseholdContractErrorV1::AppliedCommitMismatch)
    );
    let applied_proof = repository_authority
        .seal_applied_repository_observation(
            &commit_evidence,
            HouseholdEffectFingerprintV1::from_digest(applied.fingerprint),
            applied.resulting_revision,
        )
        .expect("repository-held authority proves the applied commit");
    recovered
        .reconcile_applied_commit(&committing_token, &applied_proof)
        .expect("exact reviewed fingerprint reconciles after crash");
    assert_eq!(
        recovered.state(),
        heyfood_core::AgentHouseholdProposalStateV1::Committed
    );
    assert!(matches!(
        resolve_household_commit_v1(Some(&after_add), &add_command).expect("exact replay"),
        HouseholdRepositoryResolutionV1::Replay(_)
    ));
    assert_eq!(after_add.active_scope, original_scope);
    state = *after_add;

    let mut edited_member = member.clone();
    edited_member.display_name = DisplayName::parse("Synthetic Member Edited").unwrap();
    edited_member.updated_at = timestamp(2);
    let mut edited_profile = profile.clone();
    edited_profile.profile_revision = profile.profile_revision.checked_next().unwrap();
    let mut edited_declared = profile
        .document
        .declared_profile
        .clone()
        .expect("native declared profile");
    edited_declared
        .avoid_ingredients
        .push("second private ingredient".to_owned());
    edited_profile.document = HouseholdProfileDocumentV1::native(edited_declared).unwrap();
    let mut edit_candidate = state.clone();
    edit_candidate.revision = state.revision.checked_next().unwrap();
    edit_candidate.updated_at = timestamp(2);
    edit_candidate.members[0] = edited_member.clone();
    edit_candidate.profiles[0] = edited_profile.clone();
    let edit_effect = HouseholdEffectV1::ReplaceMemberAndDeclaredProfile {
        member: edited_member,
        profile: edited_profile,
    };
    let (after_edit, _) =
        apply_phase0_agent_effect(&state, edit_candidate, edit_effect, timestamp(2));
    state = after_edit;

    let member_scope = HouseholdScope::Subject(HouseholdSubjectId::member(member_id.clone()));
    let mut scope_candidate = state.clone();
    scope_candidate.revision = state.revision.checked_next().unwrap();
    scope_candidate.updated_at = timestamp(3);
    scope_candidate.active_scope = member_scope.clone();
    let (after_scope, _) = apply_phase0_agent_effect(
        &state,
        scope_candidate,
        HouseholdEffectV1::SelectScope {
            scope: member_scope.clone(),
        },
        timestamp(3),
    );
    state = after_scope;

    let self_scope = HouseholdScope::Subject(HouseholdSubjectId::self_());
    let mut archive_candidate = state.clone();
    archive_candidate.revision = state.revision.checked_next().unwrap();
    archive_candidate.updated_at = timestamp(4);
    archive_candidate.members[0].lifecycle = HouseholdLifecycleV1::Archived;
    archive_candidate.members[0].updated_at = timestamp(4);
    archive_candidate.active_scope = self_scope.clone();
    let archive_effect = HouseholdEffectV1::ArchiveMemberAndSelectScope {
        member_id: member_id.clone(),
        previous_scope: member_scope,
        resulting_scope: self_scope.clone(),
    };
    let (after_archive, _) =
        apply_phase0_agent_effect(&state, archive_candidate, archive_effect, timestamp(4));
    state = after_archive;

    let mut restore_candidate = state.clone();
    restore_candidate.revision = state.revision.checked_next().unwrap();
    restore_candidate.updated_at = timestamp(5);
    restore_candidate.members[0].lifecycle = HouseholdLifecycleV1::Active;
    restore_candidate.members[0].updated_at = timestamp(5);
    let (after_restore, _) = apply_phase0_agent_effect(
        &state,
        restore_candidate,
        HouseholdEffectV1::RestoreMember {
            member_id: member_id.clone(),
        },
        timestamp(5),
    );
    assert_eq!(after_restore.active_scope, self_scope);
    assert_eq!(after_restore.bounded_applied_commits.len(), 6);

    let conflicting = HouseholdCommit::new(
        state.account_binding.clone(),
        state.revision,
        add_command.commit_id,
        after_restore.clone(),
        HouseholdEffectV1::RestoreMember { member_id },
        timestamp(5),
    )
    .expect("different command can be constructed before ledger comparison");
    let conflict = resolve_household_commit_v1(Some(&after_restore), &conflicting)
        .expect_err("reused commit ID must conflict");
    assert_eq!(conflict.code, "household_commit_id_conflict");
}

#[tokio::test]
async fn everyone_selection_uses_a_closed_target_and_authority_shape_is_enforced() {
    let mut state = initialized_state("account-a");
    state.owner.profile_state = HouseholdProfileStateV1::LocalOnly;
    state.profiles.push(HouseholdProfileRecordV1 {
        subject: HouseholdSubjectId::self_(),
        profile_revision: ProfileRevision::new(1).unwrap(),
        document: legacy_profile(),
    });
    let mut eligible_member = member(
        "Eligible member",
        MinorStatusV1::Adult,
        HouseholdProfileStateV1::LocalOnly,
    );
    let member_subject = HouseholdSubjectId::member(eligible_member.member_id.clone());
    eligible_member.relationship_source = RelationshipSourceV1::NativeDeclared;
    state.members.push(eligible_member);
    state.profiles.push(HouseholdProfileRecordV1 {
        subject: member_subject,
        profile_revision: ProfileRevision::new(1).unwrap(),
        document: legacy_profile(),
    });
    state.validate().unwrap();
    let repository = Arc::new(MemoryHouseholdRepository::with_state(state));
    let mutation_authority = Arc::new(FixedMutationAuthority::new([(
        HouseholdMutationPurposeV1::SelectScope,
        authority(1, None),
    )]));
    let session =
        HouseholdSession::new(account("account-a"), repository.clone(), mutation_authority);
    let selected = session
        .select_scope(
            HouseholdRevision::new(1).unwrap(),
            HouseholdScope::Everyone,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(selected.active_scope, HouseholdScope::Everyone);
    assert_eq!(selected.target, SelectedHouseholdTargetV1::Everyone);
    assert_eq!(
        repository.snapshot().await.unwrap().active_scope,
        HouseholdScope::Everyone
    );

    let invalid_authority = Arc::new(FixedMutationAuthority::new([(
        HouseholdMutationPurposeV1::SelectScope,
        authority(2, Some(heyfood_core::MemberId::new())),
    )]));
    let invalid_session =
        HouseholdSession::new(account("account-a"), repository.clone(), invalid_authority);
    let error = invalid_session
        .select_scope(
            HouseholdRevision::new(2).unwrap(),
            HouseholdScope::Everyone,
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "household_mutation_authority_invalid");
    assert_eq!(repository.snapshot().await.unwrap().revision.get(), 2);
}

#[test]
fn atomic_create_delta_replay_and_fingerprint_mismatch_fail_closed() {
    let current = initialized_state("account-a");
    let member_id = heyfood_core::MemberId::new();
    let commit_id = CommitId::new();
    let (candidate, effect) =
        atomic_create_candidate(&current, member_id.clone(), "Exact draft", timestamp(1));
    let effect_debug = format!("{effect:?}");
    assert!(!effect_debug.contains("Exact draft"));
    assert!(!effect_debug.contains(member_id.as_str()));
    assert!(!effect_debug.contains("canary private"));
    let command = HouseholdCommit::new(
        account("account-a"),
        current.revision,
        commit_id,
        candidate,
        effect,
        timestamp(1),
    )
    .unwrap();
    let HouseholdRepositoryResolutionV1::Write {
        state: committed, ..
    } = resolve_household_commit_v1(Some(&current), &command).unwrap()
    else {
        panic!("first atomic create must write")
    };
    assert!(matches!(
        resolve_household_commit_v1(Some(&committed), &command).unwrap(),
        HouseholdRepositoryResolutionV1::Replay(_)
    ));

    let (different_candidate, different_effect) =
        atomic_create_candidate(&current, member_id, "Different draft", timestamp(2));
    let different = HouseholdCommit::new(
        account("account-a"),
        current.revision,
        commit_id,
        different_candidate,
        different_effect,
        timestamp(2),
    )
    .unwrap();
    assert_eq!(
        resolve_household_commit_v1(Some(&committed), &different)
            .unwrap_err()
            .code,
        "household_commit_id_conflict"
    );

    let (mut tampered_candidate, tampered_effect) = atomic_create_candidate(
        &current,
        heyfood_core::MemberId::new(),
        "Tampered delta",
        timestamp(1),
    );
    tampered_candidate.owner.display_name = DisplayName::parse("Changed owner").unwrap();
    let tampered = HouseholdCommit::new(
        account("account-a"),
        current.revision,
        CommitId::new(),
        tampered_candidate,
        tampered_effect,
        timestamp(1),
    )
    .unwrap();
    assert_eq!(
        resolve_household_commit_v1(Some(&current), &tampered)
            .unwrap_err()
            .code,
        "household_semantic_transition_mismatch"
    );
}

#[tokio::test]
async fn duplicate_labels_are_allowed_but_member_identity_collisions_are_rejected() {
    let repository = Arc::new(MemoryHouseholdRepository::with_state(initialized_state(
        "account-a",
    )));
    let first_id = heyfood_core::MemberId::new();
    let second_id = heyfood_core::MemberId::new();
    let mutation_authority = Arc::new(FixedMutationAuthority::new([
        (
            HouseholdMutationPurposeV1::CreateMember,
            authority(1, Some(first_id.clone())),
        ),
        (
            HouseholdMutationPurposeV1::CreateMember,
            authority(2, Some(second_id.clone())),
        ),
        (
            HouseholdMutationPurposeV1::CreateMember,
            authority(3, Some(first_id.clone())),
        ),
    ]));
    let session =
        HouseholdSession::new(account("account-a"), repository.clone(), mutation_authority);
    for (revision, expected_id) in [(1, first_id.clone()), (2, second_id.clone())] {
        let created = session
            .create_member_with_declared_profile(
                CreateMemberWithDeclaredProfileV1 {
                    expected_household_revision: HouseholdRevision::new(revision).unwrap(),
                    display_name: DisplayName::parse("Same display name").unwrap(),
                    relationship: RelationshipV1::Friend,
                    age_evidence: NativeMemberAgeEvidenceV1::Age18Plus,
                    declared_profile: declared_profile(),
                },
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(created.member_id, expected_id);
    }
    {
        let mut state = repository.state.lock().await;
        let state = state.as_mut().unwrap();
        state
            .members
            .iter_mut()
            .find(|member| member.member_id == first_id)
            .unwrap()
            .lifecycle = HouseholdLifecycleV1::Archived;
        state.validate().unwrap();
    }
    let collision = session
        .create_member_with_declared_profile(
            CreateMemberWithDeclaredProfileV1 {
                expected_household_revision: HouseholdRevision::new(3).unwrap(),
                display_name: DisplayName::parse("Third duplicate").unwrap(),
                relationship: RelationshipV1::Friend,
                age_evidence: NativeMemberAgeEvidenceV1::Age18Plus,
                declared_profile: declared_profile(),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(collision.code, "household_member_conflict");
    let state = repository.snapshot().await.unwrap();
    assert_eq!(state.revision.get(), 3);
    assert_eq!(state.members.len(), 2);
    assert_eq!(
        state
            .members
            .iter()
            .filter(|member| member.display_name.as_str() == "Same display name")
            .count(),
        2
    );
}

#[tokio::test]
async fn member_profile_save_rejects_conflicted_archived_unknown_and_owner_selectors() {
    let mut conflicted = initialized_state("account-a");
    let mut conflicted_member = member(
        "Conflicted member",
        MinorStatusV1::Adult,
        HouseholdProfileStateV1::Conflicted,
    );
    conflicted_member.relationship_source = RelationshipSourceV1::NativeDeclared;
    let member_id = conflicted_member.member_id.clone();
    let subject = HouseholdSubjectId::member(member_id.clone());
    conflicted.members.push(conflicted_member);
    conflicted.profiles.push(HouseholdProfileRecordV1 {
        subject,
        profile_revision: ProfileRevision::new(1).unwrap(),
        document: legacy_profile(),
    });
    conflicted.outbox = vec![
        legacy_context_record(member_id.as_str(), 3, "context-a"),
        legacy_context_record(member_id.as_str(), 4, "context-b"),
    ];
    conflicted.outbox.sort_by(|left, right| {
        left.outbox_id
            .as_str()
            .as_bytes()
            .cmp(right.outbox_id.as_str().as_bytes())
    });
    conflicted.validate().unwrap();
    let repository = Arc::new(MemoryHouseholdRepository::with_state(conflicted));
    let mutation_authority = Arc::new(FixedMutationAuthority::new([]));
    let session = HouseholdSession::new(
        account("account-a"),
        repository.clone(),
        mutation_authority.clone(),
    );
    let conflict = session
        .save_member_declared_profile(
            SaveMemberDeclaredProfileV1 {
                expected_household_revision: HouseholdRevision::new(1).unwrap(),
                member_id: member_id.clone(),
                expected_profile_revision: Some(ProfileRevision::new(1).unwrap()),
                declared_profile: declared_profile(),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        conflict.code,
        "household_member_conflict_resolution_required"
    );

    {
        let mut state = repository.state.lock().await;
        let state = state.as_mut().unwrap();
        state.outbox.clear();
        state.profiles.clear();
        state.members[0].profile_state = HouseholdProfileStateV1::Incomplete;
        state.members[0].lifecycle = HouseholdLifecycleV1::Archived;
        state.validate().unwrap();
    }
    let archived = session
        .save_member_declared_profile(
            SaveMemberDeclaredProfileV1 {
                expected_household_revision: HouseholdRevision::new(1).unwrap(),
                member_id,
                expected_profile_revision: None,
                declared_profile: declared_profile(),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(archived.code, "household_member_archived");
    assert!(heyfood_core::MemberId::parse_preserved("_self").is_err());
    for selector in [heyfood_core::MemberId::parse_preserved("unknown-member").unwrap()] {
        let unknown = session
            .save_member_declared_profile(
                SaveMemberDeclaredProfileV1 {
                    expected_household_revision: HouseholdRevision::new(1).unwrap(),
                    member_id: selector,
                    expected_profile_revision: None,
                    declared_profile: declared_profile(),
                },
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(unknown.code, "household_member_unknown");
    }
    assert_eq!(mutation_authority.calls.load(Ordering::SeqCst), 0);
    assert_eq!(repository.snapshot().await.unwrap().revision.get(), 1);
}
