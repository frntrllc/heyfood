//! Cancellable native-household repository contracts and live application use
//! cases.

use std::{fmt, sync::Arc};

use heyfood_core::{
    AccountId, AgeBandV1, AgeEvidenceSourceV1, AgeEvidenceV1, AppliedCommitOutcomeV1,
    AppliedCommitRecordV1, CanonicalDigestV1, CanonicalTimestampV1, CommitId, DisplayName,
    ExpectedHouseholdStateV1, HOUSEHOLD_STATE_SCHEMA_VERSION, HouseholdDeclaredProfileV1,
    HouseholdEffectFingerprintV1, HouseholdEffectV1, HouseholdLifecycleV1, HouseholdMemberV1,
    HouseholdOutboxId, HouseholdOutboxRecordV1, HouseholdOwnerV1, HouseholdProfileDocumentV1,
    HouseholdProfileOutboxEntryV1, HouseholdProfileRecordV1, HouseholdProfileStateV1,
    HouseholdRevision, HouseholdScope, HouseholdStateError, HouseholdStateV1, HouseholdSubjectId,
    ImportedCompatibilityStateV1, LastDefiniteOwnerSyncErrorV1, MAX_HOUSEHOLD_MEMBERS,
    MAX_HOUSEHOLD_PROFILES, MemberId, MigrationDispositionManifestV1, MigrationProvenanceV1,
    OnboardingProfileInput, OutboxRevision, OwnerSyncIntentPhaseV1, OwnerSyncIntentV1,
    ProfileRevision, RelationshipSourceV1, RelationshipV1, canonical_sha256_v1,
    derive_minor_status_v1, domain_hash_v1, effect_fingerprint_v1,
};
use tokio_util::sync::CancellationToken;

use crate::{
    HouseholdMutationAuthorityPort, HouseholdMutationAuthorityV1, HouseholdMutationPurposeV1,
    HouseholdRepositoryPort, PortError,
    household_context::{
        HouseholdContextErrorV1, HouseholdContextSnapshotV1, PreparedHouseholdTargetV1,
        resolve_personalized_context_v1, validate_scope_eligibility_v1,
    },
    household_profile_policy::{
        HouseholdProfileOperationV1, OwnerSyncIntentHandleV1, validate_d2_profile_policy_v1,
    },
};

const HOUSEHOLD_ACCOUNT_DIGEST_CONTRACT: &str = "heyfood.household.account-digest.v1";

#[derive(Clone, Eq, PartialEq)]
pub struct HouseholdLoad {
    pub state: HouseholdStateV1,
    pub state_digest: CanonicalDigestV1,
}

impl HouseholdLoad {
    pub fn from_state(state: HouseholdStateV1) -> Result<Self, PortError> {
        validate_d2_profile_policy_v1(&state).map_err(state_port_error)?;
        state.canonical_bytes().map_err(state_port_error)?;
        let state_digest = canonical_sha256_v1(&state)
            .map_err(HouseholdStateError::from)
            .map_err(state_port_error)?;
        Ok(Self {
            state,
            state_digest,
        })
    }
}

impl fmt::Debug for HouseholdLoad {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdLoad")
            .field("revision", &self.state.revision)
            .field("state_digest", &self.state_digest)
            .field("member_count", &self.state.members.len())
            .field("profile_count", &self.state.profiles.len())
            .field("outbox_count", &self.state.outbox.len())
            .finish()
    }
}

/// One authenticated household generation retained under the repository's
/// cross-process read lock. The opaque guard is deliberately unavailable to
/// consumers; keeping this value alive is the only authority to perform a
/// hosted operation against the accompanying state.
pub struct HouseholdReadLeaseV1 {
    load: HouseholdLoad,
    _retained_lock: Box<dyn Send + 'static>,
}

impl HouseholdReadLeaseV1 {
    #[must_use]
    pub fn new(load: HouseholdLoad, retained_lock: Box<dyn Send + 'static>) -> Self {
        Self {
            load,
            _retained_lock: retained_lock,
        }
    }

    #[must_use]
    pub const fn load(&self) -> &HouseholdLoad {
        &self.load
    }
}

impl fmt::Debug for HouseholdReadLeaseV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdReadLeaseV1")
            .field("revision", &self.load.state.revision)
            .field("state_digest", &self.load.state_digest)
            .field("retained_lock", &"[RETAINED]")
            .finish()
    }
}

/// Exact active household context plus the retained repository generation that
/// authorized it. Dropping this value releases the cross-process lock.
pub struct AuthorizedHostedContextV1 {
    snapshot: HouseholdContextSnapshotV1,
    _read_lease: HouseholdReadLeaseV1,
}

impl AuthorizedHostedContextV1 {
    #[must_use]
    pub const fn snapshot(&self) -> &HouseholdContextSnapshotV1 {
        &self.snapshot
    }

    /// The exact repository generation retained by this authorization. This is
    /// intentionally a borrowed view: callers cannot outlive or detach it from
    /// the cross-process read lease.
    #[must_use]
    pub const fn load(&self) -> &HouseholdLoad {
        self._read_lease.load()
    }
}

impl fmt::Debug for AuthorizedHostedContextV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthorizedHostedContextV1")
            .field("household_revision", &self.snapshot.household_revision)
            .field("scope_kind", &scope_kind(&self.snapshot.scope))
            .field("subject_count", &self.snapshot.subjects.len())
            .finish_non_exhaustive()
    }
}

/// Compatibility name for callers that deliberately require the owner-only
/// wrapper below. New household-aware callers should use
/// [`AuthorizedHostedContextV1`].
pub type AuthorizedOwnerHostedContextV1 = AuthorizedHostedContextV1;

#[derive(Clone, Eq, PartialEq)]
pub struct HouseholdInitialize {
    pub account: AccountId,
    pub expected_state: ExpectedHouseholdStateV1,
    pub commit_id: CommitId,
    pub claimed_effect_fingerprint: HouseholdEffectFingerprintV1,
    pub semantic_candidate_state: HouseholdStateV1,
    pub normalized_typed_effect: HouseholdEffectV1,
    pub frozen_commit_timestamp: CanonicalTimestampV1,
}

impl HouseholdInitialize {
    pub fn new(
        account: AccountId,
        commit_id: CommitId,
        semantic_candidate_state: HouseholdStateV1,
        normalized_typed_effect: HouseholdEffectV1,
        frozen_commit_timestamp: CanonicalTimestampV1,
    ) -> Result<Self, PortError> {
        if !matches!(
            normalized_typed_effect,
            HouseholdEffectV1::Initialize | HouseholdEffectV1::Migration
        ) {
            return Err(repository_error(
                "household_initialize_effect",
                "household initialization requires an initialize or migration effect",
            ));
        }
        let expected_state = ExpectedHouseholdStateV1::ExpectedAbsence;
        let claimed_effect_fingerprint = command_fingerprint(
            &account,
            commit_id,
            expected_state,
            &semantic_candidate_state,
            &normalized_typed_effect,
            &frozen_commit_timestamp,
        )?;
        Ok(Self {
            account,
            expected_state,
            commit_id,
            claimed_effect_fingerprint,
            semantic_candidate_state,
            normalized_typed_effect,
            frozen_commit_timestamp,
        })
    }
}

impl fmt::Debug for HouseholdInitialize {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdInitialize")
            .field("expected_state", &self.expected_state)
            .field("commit_id", &self.commit_id)
            .field(
                "claimed_effect_fingerprint",
                &self.claimed_effect_fingerprint,
            )
            .field(
                "resulting_revision",
                &self.semantic_candidate_state.revision,
            )
            .field("normalized_typed_effect", &self.normalized_typed_effect)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct HouseholdCommit {
    pub account: AccountId,
    pub expected_state: ExpectedHouseholdStateV1,
    pub commit_id: CommitId,
    pub claimed_effect_fingerprint: HouseholdEffectFingerprintV1,
    pub semantic_candidate_state: HouseholdStateV1,
    pub normalized_typed_effect: HouseholdEffectV1,
    pub frozen_commit_timestamp: CanonicalTimestampV1,
}

impl HouseholdCommit {
    pub fn new(
        account: AccountId,
        expected_revision: HouseholdRevision,
        commit_id: CommitId,
        semantic_candidate_state: HouseholdStateV1,
        normalized_typed_effect: HouseholdEffectV1,
        frozen_commit_timestamp: CanonicalTimestampV1,
    ) -> Result<Self, PortError> {
        if matches!(
            normalized_typed_effect,
            HouseholdEffectV1::Initialize | HouseholdEffectV1::Migration
        ) {
            return Err(repository_error(
                "household_commit_effect",
                "household commit cannot carry an initialization effect",
            ));
        }
        let expected_state = ExpectedHouseholdStateV1::ExpectedRevision {
            revision: expected_revision,
        };
        let claimed_effect_fingerprint = command_fingerprint(
            &account,
            commit_id,
            expected_state,
            &semantic_candidate_state,
            &normalized_typed_effect,
            &frozen_commit_timestamp,
        )?;
        Ok(Self {
            account,
            expected_state,
            commit_id,
            claimed_effect_fingerprint,
            semantic_candidate_state,
            normalized_typed_effect,
            frozen_commit_timestamp,
        })
    }
}

impl fmt::Debug for HouseholdCommit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdCommit")
            .field("expected_state", &self.expected_state)
            .field("commit_id", &self.commit_id)
            .field(
                "claimed_effect_fingerprint",
                &self.claimed_effect_fingerprint,
            )
            .field(
                "resulting_revision",
                &self.semantic_candidate_state.revision,
            )
            .field("normalized_typed_effect", &self.normalized_typed_effect)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HouseholdCommitOutcome {
    pub outcome: AppliedCommitOutcomeV1,
    pub resulting_revision: HouseholdRevision,
}

#[derive(Clone, Eq, PartialEq)]
pub enum HouseholdRepositoryResolutionV1 {
    Replay(HouseholdCommitOutcome),
    Write {
        state: Box<HouseholdStateV1>,
        outcome: HouseholdCommitOutcome,
    },
}

impl fmt::Debug for HouseholdRepositoryResolutionV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Replay(outcome) => formatter
                .debug_tuple("HouseholdRepositoryResolutionV1::Replay")
                .field(outcome)
                .finish(),
            Self::Write { state, outcome } => formatter
                .debug_struct("HouseholdRepositoryResolutionV1::Write")
                .field("outcome", outcome)
                .field("member_count", &state.members.len())
                .field("profile_count", &state.profiles.len())
                .field("outbox_count", &state.outbox.len())
                .finish(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HouseholdErase {
    pub account: AccountId,
    pub expected_revision: Option<HouseholdRevision>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HouseholdEraseOutcome {
    pub household_key_deleted: bool,
    pub household_ciphertext_deleted: bool,
    pub import_snapshot_deleted: bool,
    pub legacy_source_retained: bool,
    pub legacy_credentials_cleared: bool,
    pub legacy_credentials_retained: bool,
    pub local_credentials_cleared: bool,
    pub outcome_uncertain: bool,
}

/// Resolve an initialize command while the adapter holds its lifecycle/vault
/// lock. Existing commit IDs are inspected before expected-absence semantics.
pub fn resolve_household_initialize_v1(
    current: Option<&HouseholdStateV1>,
    command: &HouseholdInitialize,
) -> Result<HouseholdRepositoryResolutionV1, PortError> {
    if command.expected_state != ExpectedHouseholdStateV1::ExpectedAbsence
        || !matches!(
            command.normalized_typed_effect,
            HouseholdEffectV1::Initialize | HouseholdEffectV1::Migration
        )
    {
        return Err(repository_error(
            "household_initialize_shape",
            "household initialization command shape is invalid",
        ));
    }
    resolve_repository_write(
        current,
        RepositoryCommandView {
            account: &command.account,
            expected_state: command.expected_state,
            commit_id: command.commit_id,
            claimed_effect_fingerprint: command.claimed_effect_fingerprint,
            semantic_candidate_state: &command.semantic_candidate_state,
            normalized_typed_effect: &command.normalized_typed_effect,
            frozen_commit_timestamp: &command.frozen_commit_timestamp,
            new_outcome: AppliedCommitOutcomeV1::Initialized,
        },
    )
}

/// Resolve a normal commit while the adapter holds its lifecycle/vault lock.
/// Replay remains available after the ledger reaches its fixed capacity.
pub fn resolve_household_commit_v1(
    current: Option<&HouseholdStateV1>,
    command: &HouseholdCommit,
) -> Result<HouseholdRepositoryResolutionV1, PortError> {
    if !matches!(
        command.expected_state,
        ExpectedHouseholdStateV1::ExpectedRevision { .. }
    ) || matches!(
        command.normalized_typed_effect,
        HouseholdEffectV1::Initialize | HouseholdEffectV1::Migration
    ) {
        return Err(repository_error(
            "household_commit_shape",
            "household commit command shape is invalid",
        ));
    }
    resolve_repository_write(
        current,
        RepositoryCommandView {
            account: &command.account,
            expected_state: command.expected_state,
            commit_id: command.commit_id,
            claimed_effect_fingerprint: command.claimed_effect_fingerprint,
            semantic_candidate_state: &command.semantic_candidate_state,
            normalized_typed_effect: &command.normalized_typed_effect,
            frozen_commit_timestamp: &command.frozen_commit_timestamp,
            new_outcome: AppliedCommitOutcomeV1::Committed,
        },
    )
}

struct RepositoryCommandView<'a> {
    account: &'a AccountId,
    expected_state: ExpectedHouseholdStateV1,
    commit_id: CommitId,
    claimed_effect_fingerprint: HouseholdEffectFingerprintV1,
    semantic_candidate_state: &'a HouseholdStateV1,
    normalized_typed_effect: &'a HouseholdEffectV1,
    frozen_commit_timestamp: &'a CanonicalTimestampV1,
    new_outcome: AppliedCommitOutcomeV1,
}

fn resolve_repository_write(
    current: Option<&HouseholdStateV1>,
    command: RepositoryCommandView<'_>,
) -> Result<HouseholdRepositoryResolutionV1, PortError> {
    if let Some(current) = current {
        validate_d2_profile_policy_v1(current).map_err(state_port_error)?;
        if &current.account_binding != command.account {
            return Err(repository_error(
                "household_account_mismatch",
                "household state belongs to another account",
            ));
        }
    }
    validate_d2_profile_policy_v1(command.semantic_candidate_state).map_err(state_port_error)?;
    if &command.semantic_candidate_state.account_binding != command.account
        || command.semantic_candidate_state.updated_at != *command.frozen_commit_timestamp
    {
        return Err(repository_error(
            "household_account_or_time_mismatch",
            "household candidate account or frozen timestamp is invalid",
        ));
    }

    let recomputed = command_fingerprint(
        command.account,
        command.commit_id,
        command.expected_state,
        command.semantic_candidate_state,
        command.normalized_typed_effect,
        command.frozen_commit_timestamp,
    )?;
    if !constant_time_digest_eq(
        recomputed.as_digest().as_bytes(),
        command.claimed_effect_fingerprint.as_digest().as_bytes(),
    ) {
        return Err(repository_error(
            "household_effect_fingerprint_mismatch",
            "household effect fingerprint does not match the semantic candidate",
        ));
    }

    // Fixed replay ordering: inspect an existing commit before comparing the
    // command's expected absence/revision.
    if let Some(record) = current.and_then(|state| find_applied_commit(state, command.commit_id)) {
        if constant_time_digest_eq(
            record.fingerprint.as_bytes(),
            recomputed.as_digest().as_bytes(),
        ) {
            return Ok(HouseholdRepositoryResolutionV1::Replay(
                HouseholdCommitOutcome {
                    outcome: record.outcome,
                    resulting_revision: record.resulting_revision,
                },
            ));
        }
        return Err(repository_error(
            "household_commit_id_conflict",
            "household commit ID was already used for a different effect",
        ));
    }

    match (current, command.expected_state) {
        (None, ExpectedHouseholdStateV1::ExpectedAbsence) => {
            if !command
                .semantic_candidate_state
                .bounded_applied_commits
                .is_empty()
                || command
                    .semantic_candidate_state
                    .migration_provenance
                    .initial_commit_id
                    != command.commit_id
            {
                return Err(repository_error(
                    "household_initialize_ledger",
                    "household initialization candidate contains invalid commit provenance",
                ));
            }
            if !matches!(
                command.normalized_typed_effect,
                HouseholdEffectV1::Initialize | HouseholdEffectV1::Migration
            ) {
                return Err(repository_error(
                    "household_initialize_effect",
                    "household initialization effect is invalid",
                ));
            }
        }
        (Some(current), ExpectedHouseholdStateV1::ExpectedRevision { revision })
            if current.revision == revision =>
        {
            if current.bounded_applied_commits
                != command.semantic_candidate_state.bounded_applied_commits
            {
                return Err(repository_error(
                    "household_applied_commit_ledger_changed",
                    "household semantic candidate changed the applied-commit ledger",
                ));
            }
            validate_semantic_delta(
                current,
                command.semantic_candidate_state,
                command.normalized_typed_effect,
                command.frozen_commit_timestamp,
            )?;
        }
        _ => {
            return Err(repository_error(
                "household_revision_conflict",
                "household expected revision or absence no longer matches",
            ));
        }
    }

    command
        .semantic_candidate_state
        .ensure_commit_capacity()
        .map_err(state_port_error)?;
    let mut final_state = command.semantic_candidate_state.clone();
    let outcome = HouseholdCommitOutcome {
        outcome: command.new_outcome,
        resulting_revision: final_state.revision,
    };
    final_state
        .bounded_applied_commits
        .push(AppliedCommitRecordV1 {
            commit_id: command.commit_id,
            fingerprint: recomputed.as_digest(),
            resulting_revision: final_state.revision,
            outcome: command.new_outcome,
            committed_at: command.frozen_commit_timestamp.clone(),
        });
    final_state.bounded_applied_commits.sort_by(|left, right| {
        left.commit_id
            .as_uuid()
            .as_bytes()
            .cmp(right.commit_id.as_uuid().as_bytes())
    });
    validate_d2_profile_policy_v1(&final_state).map_err(state_port_error)?;
    final_state.canonical_bytes().map_err(state_port_error)?;
    Ok(HouseholdRepositoryResolutionV1::Write {
        state: Box::new(final_state),
        outcome,
    })
}

fn validate_semantic_delta(
    current: &HouseholdStateV1,
    candidate: &HouseholdStateV1,
    effect: &HouseholdEffectV1,
    committed_at: &CanonicalTimestampV1,
) -> Result<(), PortError> {
    let mut expected = current.clone();
    expected.revision = candidate.revision;
    expected.updated_at = committed_at.clone();
    match effect {
        HouseholdEffectV1::Initialize | HouseholdEffectV1::Migration => {
            return Err(repository_error(
                "household_commit_effect",
                "initialization effects require expected absence",
            ));
        }
        HouseholdEffectV1::SelectScope { scope } => {
            expected.active_scope = scope.clone();
        }
        HouseholdEffectV1::AddMember { member } => {
            if expected
                .members
                .iter()
                .any(|candidate| candidate.member_id == member.member_id)
            {
                return Err(repository_error(
                    "household_member_conflict",
                    "household member already exists",
                ));
            }
            expected.members.push(member.clone());
            sort_members(&mut expected);
        }
        HouseholdEffectV1::CreateMemberWithDeclaredProfile {
            member,
            profile,
            selected_scope,
        } => {
            let subject = HouseholdSubjectId::member(member.member_id.clone());
            if expected
                .members
                .iter()
                .any(|candidate| candidate.member_id == member.member_id)
                || expected
                    .profiles
                    .iter()
                    .any(|candidate| candidate.subject == subject)
            {
                return Err(repository_error(
                    "household_member_conflict",
                    "household member identity already exists",
                ));
            }
            if !member.member_id.is_native_uuid_v4()
                || member.relationship == RelationshipV1::Self_
                || member.relationship_source != RelationshipSourceV1::NativeDeclared
                || member.lifecycle != HouseholdLifecycleV1::Active
                || member.profile_state != HouseholdProfileStateV1::LocalOnly
                || member.created_at != *committed_at
                || member.updated_at != *committed_at
                || profile.subject != subject
                || profile.profile_revision.get() != 1
                || profile.document.provenance
                    != heyfood_core::ProfileDocumentProvenanceV1::NativeDeclared
                || selected_scope != &HouseholdScope::Subject(subject)
            {
                return Err(repository_error(
                    "household_member_create_invalid",
                    "atomic household member creation is invalid",
                ));
            }
            expected.members.push(member.clone());
            sort_members(&mut expected);
            upsert_profile(&mut expected, profile.clone());
            expected.active_scope = selected_scope.clone();
        }
        HouseholdEffectV1::CreateMemberWithDeclaredProfileAndScope {
            member,
            profile,
            previous_scope,
            resulting_scope,
        } => {
            if &expected.active_scope != previous_scope {
                return Err(repository_error(
                    "household_scope_conflict",
                    "household member creation used a stale prior scope",
                ));
            }
            let subject = HouseholdSubjectId::member(member.member_id.clone());
            if expected
                .members
                .iter()
                .any(|candidate| candidate.member_id == member.member_id)
                || expected
                    .profiles
                    .iter()
                    .any(|candidate| candidate.subject == subject)
                || !member.member_id.is_native_uuid_v4()
                || member.relationship == RelationshipV1::Self_
                || member.relationship_source != RelationshipSourceV1::NativeDeclared
                || member.lifecycle != HouseholdLifecycleV1::Active
                || member.profile_state != HouseholdProfileStateV1::LocalOnly
                || member.created_at != *committed_at
                || member.updated_at != *committed_at
                || profile.subject != subject
                || profile.profile_revision.get() != 1
                || profile.document.provenance
                    != heyfood_core::ProfileDocumentProvenanceV1::NativeDeclared
            {
                return Err(repository_error(
                    "household_member_create_invalid",
                    "atomic household member creation is invalid",
                ));
            }
            expected.members.push(member.clone());
            sort_members(&mut expected);
            upsert_profile(&mut expected, profile.clone());
            expected.active_scope = resulting_scope.clone();
        }
        HouseholdEffectV1::ReplaceMember { member } => {
            let existing = expected
                .members
                .iter_mut()
                .find(|candidate| candidate.member_id == member.member_id)
                .ok_or_else(|| {
                    repository_error("household_member_unknown", "household member is unknown")
                })?;
            *existing = member.clone();
            sort_members(&mut expected);
        }
        HouseholdEffectV1::ReplaceMemberAndDeclaredProfile { member, profile } => {
            let existing = expected
                .members
                .iter_mut()
                .find(|candidate| candidate.member_id == member.member_id)
                .ok_or_else(|| {
                    repository_error("household_member_unknown", "household member is unknown")
                })?;
            let subject = HouseholdSubjectId::member(member.member_id.clone());
            if profile.subject != subject {
                return Err(repository_error(
                    "household_profile_subject_mismatch",
                    "household profile belongs to another subject",
                ));
            }
            *existing = member.clone();
            sort_members(&mut expected);
            upsert_profile(&mut expected, profile.clone());
        }
        HouseholdEffectV1::ArchiveMember { member_id }
        | HouseholdEffectV1::RestoreMember { member_id } => {
            let member = expected
                .members
                .iter_mut()
                .find(|candidate| &candidate.member_id == member_id)
                .ok_or_else(|| {
                    repository_error("household_member_unknown", "household member is unknown")
                })?;
            member.lifecycle = if matches!(effect, HouseholdEffectV1::ArchiveMember { .. }) {
                HouseholdLifecycleV1::Archived
            } else {
                HouseholdLifecycleV1::Active
            };
            member.updated_at = committed_at.clone();
        }
        HouseholdEffectV1::ArchiveMemberAndSelectScope {
            member_id,
            previous_scope,
            resulting_scope,
        } => {
            if &expected.active_scope != previous_scope {
                return Err(repository_error(
                    "household_scope_conflict",
                    "household archive used a stale prior scope",
                ));
            }
            let member = expected
                .members
                .iter_mut()
                .find(|candidate| &candidate.member_id == member_id)
                .ok_or_else(|| {
                    repository_error("household_member_unknown", "household member is unknown")
                })?;
            member.lifecycle = HouseholdLifecycleV1::Archived;
            member.updated_at = committed_at.clone();
            expected.active_scope = resulting_scope.clone();
        }
        HouseholdEffectV1::SaveOwnerProfileAndOwnerSyncIntent {
            owner_profile,
            owner_sync_record,
            replaced_outbox_id,
        } => {
            validate_owner_profile_save_delta(
                &expected,
                owner_profile,
                owner_sync_record,
                replaced_outbox_id.as_ref(),
            )?;
            if let Some(replaced_outbox_id) = replaced_outbox_id {
                let old_index = expected
                    .outbox
                    .iter()
                    .position(|record| &record.outbox_id == replaced_outbox_id)
                    .ok_or_else(|| {
                        repository_error(
                            "owner_sync_replacement_missing",
                            "owner sync replacement target is unavailable",
                        )
                    })?;
                expected.outbox.remove(old_index);
            }
            upsert_profile(&mut expected, owner_profile.clone());
            upsert_outbox(&mut expected, owner_sync_record.as_ref().clone());
            expected.owner.profile_state = HouseholdProfileStateV1::PendingSync;
            expected.owner.updated_at = committed_at.clone();
        }
        HouseholdEffectV1::UpsertProfile { profile } => {
            upsert_profile(&mut expected, profile.clone());
            adopt_subject_profile_state(&mut expected, candidate, &profile.subject)?;
        }
        HouseholdEffectV1::RemoveProfile { subject } => {
            let index = expected
                .profiles
                .iter()
                .position(|profile| &profile.subject == subject)
                .ok_or_else(|| {
                    repository_error(
                        "household_profile_unknown",
                        "household profile is unavailable",
                    )
                })?;
            expected.profiles.remove(index);
            adopt_subject_profile_state(&mut expected, candidate, subject)?;
        }
        HouseholdEffectV1::UpsertOutbox { record } => {
            if matches!(
                record.entry,
                HouseholdProfileOutboxEntryV1::OwnerSync { .. }
            ) {
                return Err(repository_error(
                    "owner_sync_effect_required",
                    "owner sync records require the typed owner-sync effect",
                ));
            }
            let target = record.entry.target().clone();
            upsert_outbox(&mut expected, record.clone());
            adopt_subject_profile_state(&mut expected, candidate, &target)?;
        }
        HouseholdEffectV1::RemoveOutbox { outbox_id } => {
            let index = expected
                .outbox
                .iter()
                .position(|record| &record.outbox_id == outbox_id)
                .ok_or_else(|| {
                    repository_error(
                        "household_outbox_unknown",
                        "household outbox record is unavailable",
                    )
                })?;
            if matches!(
                expected.outbox[index].entry,
                HouseholdProfileOutboxEntryV1::OwnerSync { .. }
            ) {
                return Err(repository_error(
                    "owner_sync_effect_required",
                    "owner sync records require the typed owner-sync effect",
                ));
            }
            let target = expected.outbox[index].entry.target().clone();
            expected.outbox.remove(index);
            adopt_subject_profile_state(&mut expected, candidate, &target)?;
        }
        HouseholdEffectV1::OwnerSyncTransition {
            outbox_id,
            from_phase,
            to_phase,
            resulting_profile_state,
        } => {
            let old_index = expected
                .outbox
                .iter()
                .position(|record| &record.outbox_id == outbox_id)
                .ok_or_else(|| {
                    repository_error(
                        "owner_sync_intent_missing",
                        "owner sync intent is unavailable",
                    )
                })?;
            let HouseholdProfileOutboxEntryV1::OwnerSync {
                intent: old_intent, ..
            } = &expected.outbox[old_index].entry
            else {
                return Err(repository_error(
                    "owner_sync_intent_invalid",
                    "owner sync effect references a non-owner record",
                ));
            };
            if old_intent.phase != *from_phase {
                return Err(repository_error(
                    "owner_sync_transition_stale",
                    "owner sync source phase changed",
                ));
            }
            let old_intent = old_intent.clone();
            expected.owner.profile_state = *resulting_profile_state;
            expected.owner.updated_at = committed_at.clone();
            match to_phase {
                Some(to_phase) => {
                    let replacement = candidate
                        .outbox
                        .iter()
                        .find(|record| &record.outbox_id == outbox_id)
                        .ok_or_else(|| {
                            repository_error(
                                "owner_sync_intent_missing",
                                "owner sync replacement is unavailable",
                            )
                        })?
                        .clone();
                    let HouseholdProfileOutboxEntryV1::OwnerSync {
                        intent: replacement_intent,
                        ..
                    } = &replacement.entry
                    else {
                        return Err(repository_error(
                            "owner_sync_intent_invalid",
                            "owner sync replacement is invalid",
                        ));
                    };
                    if replacement_intent.phase != *to_phase {
                        return Err(repository_error(
                            "owner_sync_transition_invalid",
                            "owner sync replacement phase is invalid",
                        ));
                    }
                    validate_owner_intent_replacement(
                        &old_intent,
                        replacement_intent,
                        *resulting_profile_state,
                        committed_at,
                    )?;
                    expected.outbox[old_index] = replacement;
                }
                None => {
                    expected.outbox.remove(old_index);
                }
            }
        }
    }
    if &expected == candidate {
        Ok(())
    } else {
        Err(repository_error(
            "household_semantic_transition_mismatch",
            "household candidate contains changes outside its normalized effect",
        ))
    }
}

fn validate_owner_profile_save_delta(
    current: &HouseholdStateV1,
    owner_profile: &HouseholdProfileRecordV1,
    owner_sync_record: &HouseholdOutboxRecordV1,
    replaced_outbox_id: Option<&HouseholdOutboxId>,
) -> Result<(), PortError> {
    if owner_profile.subject != HouseholdSubjectId::self_() {
        return Err(repository_error(
            "owner_profile_subject_invalid",
            "owner profile save must target the authenticated owner",
        ));
    }
    let expected_profile_revision = current
        .profiles
        .iter()
        .find(|profile| profile.subject == HouseholdSubjectId::self_())
        .map_or_else(
            || ProfileRevision::new(1),
            |profile| profile.profile_revision.checked_next(),
        )
        .map_err(state_port_error)?;
    if owner_profile.profile_revision != expected_profile_revision {
        return Err(repository_error(
            "owner_profile_revision_conflict",
            "owner profile revision did not advance exactly once",
        ));
    }

    let existing_owner_sync = current.outbox.iter().find(|record| {
        matches!(
            record.entry,
            HouseholdProfileOutboxEntryV1::OwnerSync { .. }
        )
    });
    match (existing_owner_sync, replaced_outbox_id) {
        (None, None) => {}
        (Some(existing), Some(replaced)) if &existing.outbox_id == replaced => {
            let HouseholdProfileOutboxEntryV1::OwnerSync { intent, .. } = &existing.entry else {
                unreachable!("owner-sync match established above");
            };
            if !owner_sync_intent_can_be_replaced(intent.phase) {
                return Err(repository_error(
                    "owner_sync_replacement_blocked",
                    "owner sync intent must reconcile before a new owner save",
                ));
            }
        }
        (Some(_), None) => {
            return Err(repository_error(
                "owner_sync_replacement_required",
                "owner profile save must identify the existing owner sync intent",
            ));
        }
        (None, Some(_)) | (Some(_), Some(_)) => {
            return Err(repository_error(
                "owner_sync_replacement_mismatch",
                "owner sync replacement identity does not match current state",
            ));
        }
    }

    let HouseholdProfileOutboxEntryV1::OwnerSync {
        version,
        target,
        intent,
    } = &owner_sync_record.entry
    else {
        return Err(repository_error(
            "owner_sync_intent_invalid",
            "owner profile save requires an owner sync intent",
        ));
    };
    if *version != 1
        || target != &HouseholdSubjectId::self_()
        || intent.subject != HouseholdSubjectId::self_()
        || intent.phase != OwnerSyncIntentPhaseV1::NeedsConsentCheck
        || intent.intent_revision != 1
        || owner_sync_record.outbox_revision.get() != 1
    {
        return Err(repository_error(
            "owner_sync_intent_invalid",
            "new owner sync intent is not in its initial state",
        ));
    }
    Ok(())
}

fn owner_sync_intent_can_be_replaced(phase: OwnerSyncIntentPhaseV1) -> bool {
    matches!(
        phase,
        OwnerSyncIntentPhaseV1::NeedsConsentCheck
            | OwnerSyncIntentPhaseV1::NeedsRemoteBase
            | OwnerSyncIntentPhaseV1::ReadyToDispatch
            | OwnerSyncIntentPhaseV1::DefiniteFailure
            | OwnerSyncIntentPhaseV1::LocalOnlyNoConsent
    )
}

fn sort_members(state: &mut HouseholdStateV1) {
    state.members.sort_by(|left, right| {
        left.member_id
            .as_str()
            .as_bytes()
            .cmp(right.member_id.as_str().as_bytes())
    });
}

fn upsert_profile(state: &mut HouseholdStateV1, profile: heyfood_core::HouseholdProfileRecordV1) {
    if let Some(existing) = state
        .profiles
        .iter_mut()
        .find(|candidate| candidate.subject == profile.subject)
    {
        *existing = profile;
    } else {
        state.profiles.push(profile);
    }
    state
        .profiles
        .sort_by(|left, right| subject_cmp(&left.subject, &right.subject));
}

fn upsert_outbox(state: &mut HouseholdStateV1, record: HouseholdOutboxRecordV1) {
    if let Some(existing) = state
        .outbox
        .iter_mut()
        .find(|candidate| candidate.outbox_id == record.outbox_id)
    {
        *existing = record;
    } else {
        state.outbox.push(record);
    }
    state.outbox.sort_by(|left, right| {
        left.outbox_id
            .as_str()
            .as_bytes()
            .cmp(right.outbox_id.as_str().as_bytes())
    });
}

fn adopt_subject_profile_state(
    expected: &mut HouseholdStateV1,
    candidate: &HouseholdStateV1,
    subject: &HouseholdSubjectId,
) -> Result<(), PortError> {
    match subject {
        HouseholdSubjectId::Self_ => {
            expected.owner.profile_state = candidate.owner.profile_state;
            expected.owner.updated_at = candidate.owner.updated_at.clone();
        }
        HouseholdSubjectId::Member(member_id) => {
            let replacement = candidate
                .members
                .iter()
                .find(|member| &member.member_id == member_id)
                .ok_or_else(|| {
                    repository_error("household_member_unknown", "household member is unknown")
                })?;
            let member = expected
                .members
                .iter_mut()
                .find(|member| &member.member_id == member_id)
                .ok_or_else(|| {
                    repository_error("household_member_unknown", "household member is unknown")
                })?;
            member.profile_state = replacement.profile_state;
            member.updated_at = replacement.updated_at.clone();
        }
    }
    Ok(())
}

fn subject_cmp(left: &HouseholdSubjectId, right: &HouseholdSubjectId) -> std::cmp::Ordering {
    match (left, right) {
        (HouseholdSubjectId::Self_, HouseholdSubjectId::Self_) => std::cmp::Ordering::Equal,
        (HouseholdSubjectId::Self_, HouseholdSubjectId::Member(_)) => std::cmp::Ordering::Less,
        (HouseholdSubjectId::Member(_), HouseholdSubjectId::Self_) => std::cmp::Ordering::Greater,
        (HouseholdSubjectId::Member(left), HouseholdSubjectId::Member(right)) => {
            left.as_str().as_bytes().cmp(right.as_str().as_bytes())
        }
    }
}

fn find_applied_commit(
    state: &HouseholdStateV1,
    commit_id: CommitId,
) -> Option<&AppliedCommitRecordV1> {
    state
        .bounded_applied_commits
        .binary_search_by(|record| {
            record
                .commit_id
                .as_uuid()
                .as_bytes()
                .cmp(commit_id.as_uuid().as_bytes())
        })
        .ok()
        .map(|index| &state.bounded_applied_commits[index])
}

#[derive(Clone, Eq, PartialEq)]
pub struct SelfOnlyHouseholdInitializationV1 {
    pub owner: HouseholdOwnerV1,
    pub migration_provenance: MigrationProvenanceV1,
}

impl fmt::Debug for SelfOnlyHouseholdInitializationV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SelfOnlyHouseholdInitializationV1")
            .field("owner", &self.owner)
            .field(
                "source_identity",
                &self.migration_provenance.source_identity,
            )
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HouseholdOpenOutcomeV1 {
    Opened,
    Initialized,
}

#[derive(Clone, Eq, PartialEq)]
pub struct OpenHouseholdV1 {
    pub outcome: HouseholdOpenOutcomeV1,
    pub load: HouseholdLoad,
}

impl fmt::Debug for OpenHouseholdV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenHouseholdV1")
            .field("outcome", &self.outcome)
            .field("load", &self.load)
            .finish()
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum NativeMemberAgeEvidenceV1 {
    Under13,
    Age13_17,
    Age18Plus,
    Unknown,
}

#[derive(Clone, Eq, PartialEq)]
pub struct CreateMemberWithDeclaredProfileV1 {
    pub expected_household_revision: HouseholdRevision,
    pub display_name: DisplayName,
    pub relationship: RelationshipV1,
    pub age_evidence: NativeMemberAgeEvidenceV1,
    pub declared_profile: OnboardingProfileInput,
}

impl fmt::Debug for CreateMemberWithDeclaredProfileV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CreateMemberWithDeclaredProfileV1")
            .field(
                "expected_household_revision",
                &self.expected_household_revision,
            )
            .field("declared_profile", &self.declared_profile)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct CreatedMemberWithDeclaredProfileV1 {
    pub member_id: MemberId,
    pub resulting_household_revision: HouseholdRevision,
    pub active_scope: HouseholdScope,
    pub display_label: DisplayName,
}

impl fmt::Debug for CreatedMemberWithDeclaredProfileV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CreatedMemberWithDeclaredProfileV1")
            .field(
                "resulting_household_revision",
                &self.resulting_household_revision,
            )
            .field("active_scope_kind", &scope_kind(&self.active_scope))
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct SaveMemberDeclaredProfileV1 {
    pub expected_household_revision: HouseholdRevision,
    pub member_id: MemberId,
    pub expected_profile_revision: Option<ProfileRevision>,
    pub declared_profile: OnboardingProfileInput,
}

impl fmt::Debug for SaveMemberDeclaredProfileV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SaveMemberDeclaredProfileV1")
            .field(
                "expected_household_revision",
                &self.expected_household_revision,
            )
            .field("expected_profile_revision", &self.expected_profile_revision)
            .field("declared_profile", &self.declared_profile)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct SavedMemberDeclaredProfileV1 {
    pub member_id: MemberId,
    pub resulting_household_revision: HouseholdRevision,
    pub profile_revision: ProfileRevision,
    pub active_scope: HouseholdScope,
    pub display_label: DisplayName,
}

impl fmt::Debug for SavedMemberDeclaredProfileV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SavedMemberDeclaredProfileV1")
            .field(
                "resulting_household_revision",
                &self.resulting_household_revision,
            )
            .field("profile_revision", &self.profile_revision)
            .field("active_scope_kind", &scope_kind(&self.active_scope))
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub enum SelectedHouseholdTargetV1 {
    Me,
    Member {
        member_id: MemberId,
        display_label: DisplayName,
    },
    Everyone,
}

impl fmt::Debug for SelectedHouseholdTargetV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Me => "SelectedHouseholdTargetV1::Me",
            Self::Member { .. } => "SelectedHouseholdTargetV1::Member([REDACTED])",
            Self::Everyone => "SelectedHouseholdTargetV1::Everyone",
        })
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct SelectedHouseholdScopeV1 {
    pub resulting_household_revision: HouseholdRevision,
    pub active_scope: HouseholdScope,
    pub target: SelectedHouseholdTargetV1,
}

impl fmt::Debug for SelectedHouseholdScopeV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SelectedHouseholdScopeV1")
            .field(
                "resulting_household_revision",
                &self.resulting_household_revision,
            )
            .field("active_scope_kind", &scope_kind(&self.active_scope))
            .field("target", &self.target)
            .finish()
    }
}

/// A live account-bound repository handle. It intentionally caches neither
/// household state nor an imported Python snapshot.
#[derive(Clone)]
pub struct HouseholdSession {
    account: AccountId,
    repository: Arc<dyn HouseholdRepositoryPort>,
    mutation_authority: Arc<dyn HouseholdMutationAuthorityPort>,
}

impl HouseholdSession {
    #[must_use]
    pub fn new(
        account: AccountId,
        repository: Arc<dyn HouseholdRepositoryPort>,
        mutation_authority: Arc<dyn HouseholdMutationAuthorityPort>,
    ) -> Self {
        Self {
            account,
            repository,
            mutation_authority,
        }
    }

    #[must_use]
    pub fn account(&self) -> &AccountId {
        &self.account
    }

    pub async fn load(
        &self,
        cancellation: CancellationToken,
    ) -> Result<Option<HouseholdLoad>, PortError> {
        check_cancelled(&cancellation, "household_load_cancelled")?;
        let load = self.repository.load(&self.account, cancellation).await?;
        load.map(|load| {
            if load.state.account_binding != self.account {
                return Err(repository_error(
                    "household_account_mismatch",
                    "household repository returned another account",
                ));
            }
            HouseholdLoad::from_state(load.state)
        })
        .transpose()
    }

    pub async fn load_required(
        &self,
        cancellation: CancellationToken,
    ) -> Result<HouseholdLoad, PortError> {
        self.load(cancellation).await?.ok_or_else(|| {
            repository_error(
                "household_not_initialized",
                "native household state is not initialized",
            )
        })
    }

    /// Acquire the exact live active context while retaining the repository's
    /// cross-process read lock. Callers must keep the returned value alive
    /// through every credential refresh and first hosted dispatch.
    pub async fn acquire_authorized_hosted_context(
        &self,
        cancellation: CancellationToken,
    ) -> Result<AuthorizedHostedContextV1, PortError> {
        check_cancelled(&cancellation, "household_hosted_context_cancelled")?;
        let read_lease = self
            .repository
            .acquire_read_lease(&self.account, cancellation)
            .await?;
        let load = read_lease.load();
        if load.state.account_binding != self.account {
            return Err(repository_error(
                "household_account_mismatch",
                "household repository returned another account",
            ));
        }
        let prepared = PreparedHouseholdTargetV1::from_active_scope(&load.state)
            .map_err(context_port_error)?;
        let snapshot =
            resolve_personalized_context_v1(&load.state, &prepared).map_err(context_port_error)?;
        if snapshot.household_revision != load.state.revision
            || snapshot.scope != prepared.scope
            || snapshot.subjects.is_empty()
        {
            return Err(repository_error(
                "household_hosted_context_invalid",
                "the authorized household context did not match the retained generation",
            ));
        }
        Ok(AuthorizedHostedContextV1 {
            snapshot,
            _read_lease: read_lease,
        })
    }

    /// Acquire one explicitly selected context from an exact previously loaded
    /// generation while retaining the repository's cross-process read lock.
    ///
    /// Human-reviewed one-shot operations use this seam when an explicit target
    /// may differ from the persisted active scope. The target is never resolved
    /// again after review: a change between the caller's load and lease
    /// acquisition is a typed stale-revision refusal, and the returned value
    /// keeps that exact generation locked through the first hosted dispatch.
    pub async fn acquire_authorized_hosted_context_for_scope(
        &self,
        expected_revision: HouseholdRevision,
        scope: HouseholdScope,
        cancellation: CancellationToken,
    ) -> Result<AuthorizedHostedContextV1, PortError> {
        check_cancelled(&cancellation, "household_hosted_context_cancelled")?;
        let read_lease = self
            .repository
            .acquire_read_lease(&self.account, cancellation)
            .await?;
        let load = read_lease.load();
        if load.state.account_binding != self.account {
            return Err(repository_error(
                "household_account_mismatch",
                "household repository returned another account",
            ));
        }
        if load.state.revision != expected_revision {
            return Err(repository_error(
                "household_revision_stale",
                "household context revision changed before authorization",
            ));
        }
        let prepared = PreparedHouseholdTargetV1::for_scope(
            &load.state,
            scope,
            HouseholdProfileOperationV1::PersonalizedContext,
        )
        .map_err(context_port_error)?;
        let snapshot =
            resolve_personalized_context_v1(&load.state, &prepared).map_err(context_port_error)?;
        if snapshot.household_revision != load.state.revision
            || snapshot.scope != prepared.scope
            || snapshot.subjects.is_empty()
        {
            return Err(repository_error(
                "household_hosted_context_invalid",
                "the authorized household context did not match the retained generation",
            ));
        }
        Ok(AuthorizedHostedContextV1 {
            snapshot,
            _read_lease: read_lease,
        })
    }

    /// Acquire the exact live owner context while retaining the repository's
    /// cross-process read lock. This compatibility wrapper intentionally
    /// rejects member and everyone scopes.
    pub async fn acquire_authorized_owner_hosted_context(
        &self,
        cancellation: CancellationToken,
    ) -> Result<AuthorizedOwnerHostedContextV1, PortError> {
        let authorized = self.acquire_authorized_hosted_context(cancellation).await?;
        let snapshot = authorized.snapshot();
        if snapshot.scope != HouseholdScope::Subject(HouseholdSubjectId::Self_)
            || snapshot.subjects.len() != 1
            || snapshot.subjects[0].subject != HouseholdSubjectId::Self_
        {
            return Err(repository_error(
                "household_hosted_context_not_authorized",
                "This owner-only operation requires the Me scope. Run /for me and try that operation again.",
            ));
        }
        Ok(authorized)
    }

    pub async fn open_or_initialize_self_only(
        &self,
        initialization: SelfOnlyHouseholdInitializationV1,
        cancellation: CancellationToken,
    ) -> Result<OpenHouseholdV1, PortError> {
        if let Some(load) = self.load(cancellation.clone()).await? {
            return Ok(OpenHouseholdV1 {
                outcome: HouseholdOpenOutcomeV1::Opened,
                load,
            });
        }
        check_cancelled(&cancellation, "household_initialize_cancelled")?;
        if initialization.owner.profile_state != HouseholdProfileStateV1::Incomplete
            || initialization.owner.created_at
                != initialization.migration_provenance.migration_frozen_at
            || initialization.owner.updated_at
                != initialization.migration_provenance.migration_frozen_at
        {
            return Err(repository_error(
                "household_self_only_invalid",
                "self-only household initialization is invalid",
            ));
        }
        let timestamp = initialization
            .migration_provenance
            .migration_frozen_at
            .clone();
        let commit_id = initialization.migration_provenance.initial_commit_id;
        let state = HouseholdStateV1 {
            schema_version: HOUSEHOLD_STATE_SCHEMA_VERSION,
            account_binding: self.account.clone(),
            revision: HouseholdRevision::new(1).map_err(state_port_error)?,
            owner: initialization.owner,
            active_scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
            members: Vec::new(),
            profiles: Vec::new(),
            outbox: Vec::new(),
            bounded_applied_commits: Vec::new(),
            imported_compatibility: ImportedCompatibilityStateV1 {
                fields: Vec::new(),
                legacy_python_applied_mutation_ids: Vec::new(),
                legacy_python_applied_mutation_ids_digest: None,
                legacy_remote_profile_references: Vec::new(),
                legacy_timestamp_provenance: Vec::new(),
            },
            migration_dispositions: MigrationDispositionManifestV1 {
                dispositions: Vec::new(),
            },
            migration_provenance: initialization.migration_provenance,
            updated_at: timestamp.clone(),
        };
        let command = HouseholdInitialize::new(
            self.account.clone(),
            commit_id,
            state,
            HouseholdEffectV1::Initialize,
            timestamp,
        )?;
        self.initialize(command, cancellation.clone()).await?;
        let load = self.load_required(cancellation).await?;
        Ok(OpenHouseholdV1 {
            outcome: HouseholdOpenOutcomeV1::Initialized,
            load,
        })
    }

    pub async fn initialize(
        &self,
        command: HouseholdInitialize,
        cancellation: CancellationToken,
    ) -> Result<HouseholdCommitOutcome, PortError> {
        check_account(&self.account, &command.account)?;
        check_cancelled(&cancellation, "household_initialize_cancelled")?;
        self.repository.initialize(command, cancellation).await
    }

    pub async fn commit(
        &self,
        command: HouseholdCommit,
        cancellation: CancellationToken,
    ) -> Result<HouseholdCommitOutcome, PortError> {
        check_account(&self.account, &command.account)?;
        check_cancelled(&cancellation, "household_commit_cancelled")?;
        self.repository.commit(command, cancellation).await
    }

    pub async fn erase_account(
        &self,
        expected_revision: Option<HouseholdRevision>,
        cancellation: CancellationToken,
    ) -> Result<HouseholdEraseOutcome, PortError> {
        check_cancelled(&cancellation, "household_erase_cancelled")?;
        self.repository
            .erase_account(
                HouseholdErase {
                    account: self.account.clone(),
                    expected_revision,
                },
                cancellation,
            )
            .await
    }

    pub async fn prepare_active_target(
        &self,
        cancellation: CancellationToken,
    ) -> Result<PreparedHouseholdTargetV1, PortError> {
        let load = self.load_required(cancellation).await?;
        PreparedHouseholdTargetV1::from_active_scope(&load.state).map_err(context_port_error)
    }

    pub async fn resolve_personalized_context(
        &self,
        prepared: &PreparedHouseholdTargetV1,
        cancellation: CancellationToken,
    ) -> Result<HouseholdContextSnapshotV1, PortError> {
        let load = self.load_required(cancellation).await?;
        resolve_personalized_context_v1(&load.state, prepared).map_err(context_port_error)
    }

    /// Atomically add one native member, their complete declared profile, and
    /// the selected member scope. The caller supplies no durable identity or
    /// time authority.
    pub async fn create_member_with_declared_profile(
        &self,
        create: CreateMemberWithDeclaredProfileV1,
        cancellation: CancellationToken,
    ) -> Result<CreatedMemberWithDeclaredProfileV1, PortError> {
        check_cancelled(&cancellation, "household_member_create_cancelled")?;
        let document = native_declared_profile_document(&create.declared_profile)?;
        if create.relationship == RelationshipV1::Self_ {
            return Err(repository_error(
                "household_member_relationship_invalid",
                "new household members require a non-self relationship",
            ));
        }

        let load = self.load_required(cancellation.clone()).await?;
        if load.state.revision != create.expected_household_revision {
            return Err(repository_error(
                "household_revision_conflict",
                "household member creation used a stale revision",
            ));
        }
        if load.state.members.len() >= MAX_HOUSEHOLD_MEMBERS
            || load.state.profiles.len() >= MAX_HOUSEHOLD_PROFILES
        {
            return Err(repository_error(
                "household_member_capacity",
                "household member capacity is exhausted",
            ));
        }
        load.state
            .ensure_commit_capacity()
            .map_err(state_port_error)?;
        let resulting_revision = load
            .state
            .revision
            .checked_next()
            .map_err(state_port_error)?;

        check_cancelled(&cancellation, "household_member_create_cancelled")?;
        let authority = self
            .allocate_mutation_authority(HouseholdMutationPurposeV1::CreateMember, &load.state)?;
        let commit_id = authority.commit_id;
        let member_id = authority.member_id.clone().ok_or_else(|| {
            repository_error(
                "household_mutation_authority_invalid",
                "create-member authority omitted its member identity",
            )
        })?;
        if load
            .state
            .members
            .iter()
            .any(|member| member.member_id == member_id)
        {
            return Err(repository_error(
                "household_member_conflict",
                "new household member identity already exists",
            ));
        }

        let age_evidence = native_age_evidence(create.age_evidence);
        let minor_status = derive_minor_status_v1(
            create.relationship,
            age_evidence.as_ref(),
            &authority.frozen_evaluation_date,
        )
        .map_err(state_port_error)?;
        let subject = HouseholdSubjectId::member(member_id.clone());
        let selected_scope = HouseholdScope::Subject(subject.clone());
        let member = HouseholdMemberV1 {
            member_id: member_id.clone(),
            display_name: create.display_name.clone(),
            relationship: create.relationship,
            relationship_source: RelationshipSourceV1::NativeDeclared,
            minor_status,
            age_evidence,
            minor_status_evaluated_on: authority.frozen_evaluation_date.clone(),
            lifecycle: HouseholdLifecycleV1::Active,
            profile_state: HouseholdProfileStateV1::LocalOnly,
            created_at: authority.frozen_commit_timestamp.clone(),
            updated_at: authority.frozen_commit_timestamp.clone(),
        };
        let profile = HouseholdProfileRecordV1 {
            subject,
            profile_revision: ProfileRevision::new(1).map_err(state_port_error)?,
            document,
        };
        let mut candidate = load.state;
        candidate.revision = resulting_revision;
        candidate.updated_at = authority.frozen_commit_timestamp.clone();
        candidate.active_scope = selected_scope.clone();
        candidate.members.push(member.clone());
        sort_members(&mut candidate);
        upsert_profile(&mut candidate, profile.clone());
        let effect = HouseholdEffectV1::CreateMemberWithDeclaredProfile {
            member: member.clone(),
            profile: profile.clone(),
            selected_scope: selected_scope.clone(),
        };
        let command = HouseholdCommit::new(
            self.account.clone(),
            create.expected_household_revision,
            commit_id,
            candidate,
            effect,
            authority.frozen_commit_timestamp,
        )?;
        check_cancelled(&cancellation, "household_member_create_cancelled")?;
        let (outcome, readback) = self.commit_and_reconcile(command, cancellation).await?;
        require_exact_committed_readback(&outcome, &readback, resulting_revision, commit_id)?;
        if readback.state.active_scope != selected_scope
            || !readback.state.members.iter().any(|value| value == &member)
            || !readback
                .state
                .profiles
                .iter()
                .any(|value| value == &profile)
        {
            return Err(household_outcome_uncertain());
        }
        Ok(CreatedMemberWithDeclaredProfileV1 {
            member_id,
            resulting_household_revision: resulting_revision,
            active_scope: selected_scope,
            display_label: create.display_name,
        })
    }

    /// Save a complete declared profile for one active non-owner member.
    /// This path never creates an outbox record.
    pub async fn save_member_declared_profile(
        &self,
        save: SaveMemberDeclaredProfileV1,
        cancellation: CancellationToken,
    ) -> Result<SavedMemberDeclaredProfileV1, PortError> {
        check_cancelled(&cancellation, "household_member_profile_cancelled")?;
        let document = native_declared_profile_document(&save.declared_profile)?;
        let load = self.load_required(cancellation.clone()).await?;
        if load.state.revision != save.expected_household_revision {
            return Err(repository_error(
                "household_revision_conflict",
                "household member profile save used a stale revision",
            ));
        }
        let member_index = load
            .state
            .members
            .iter()
            .position(|member| member.member_id == save.member_id)
            .ok_or_else(|| {
                repository_error(
                    "household_member_unknown",
                    "household member is unavailable",
                )
            })?;
        let member = &load.state.members[member_index];
        if member.lifecycle != HouseholdLifecycleV1::Active {
            return Err(repository_error(
                "household_member_archived",
                "archived household members cannot be onboarded",
            ));
        }
        match member.profile_state {
            HouseholdProfileStateV1::Incomplete | HouseholdProfileStateV1::LocalOnly => {}
            HouseholdProfileStateV1::Conflicted => {
                return Err(repository_error(
                    "household_member_conflict_resolution_required",
                    "household member profile requires conflict resolution",
                ));
            }
            HouseholdProfileStateV1::PendingSync | HouseholdProfileStateV1::Synced => {
                return Err(repository_error(
                    "household_member_profile_ineligible",
                    "household member profile state is not locally writable",
                ));
            }
        }
        let subject = HouseholdSubjectId::member(save.member_id.clone());
        let current_profile_revision = load
            .state
            .profiles
            .iter()
            .find(|profile| profile.subject == subject)
            .map(|profile| profile.profile_revision);
        if current_profile_revision != save.expected_profile_revision {
            return Err(repository_error(
                "household_member_profile_revision_conflict",
                "household member profile save used a stale profile revision",
            ));
        }
        if current_profile_revision.is_none() && load.state.profiles.len() >= MAX_HOUSEHOLD_PROFILES
        {
            return Err(repository_error(
                "household_profile_capacity",
                "household profile capacity is exhausted",
            ));
        }
        let profile_revision = current_profile_revision
            .map_or_else(|| ProfileRevision::new(1), ProfileRevision::checked_next)
            .map_err(state_port_error)?;
        load.state
            .ensure_commit_capacity()
            .map_err(state_port_error)?;
        let resulting_revision = load
            .state
            .revision
            .checked_next()
            .map_err(state_port_error)?;
        let display_label = member.display_name.clone();
        let active_scope = load.state.active_scope.clone();

        check_cancelled(&cancellation, "household_member_profile_cancelled")?;
        let authority = self.allocate_mutation_authority(
            HouseholdMutationPurposeV1::SaveMemberProfile,
            &load.state,
        )?;
        let commit_id = authority.commit_id;
        let profile = HouseholdProfileRecordV1 {
            subject,
            profile_revision,
            document,
        };
        let mut candidate = load.state;
        candidate.revision = resulting_revision;
        candidate.updated_at = authority.frozen_commit_timestamp.clone();
        candidate.members[member_index].profile_state = HouseholdProfileStateV1::LocalOnly;
        candidate.members[member_index].updated_at = authority.frozen_commit_timestamp.clone();
        upsert_profile(&mut candidate, profile.clone());
        let command = HouseholdCommit::new(
            self.account.clone(),
            save.expected_household_revision,
            commit_id,
            candidate,
            HouseholdEffectV1::UpsertProfile {
                profile: profile.clone(),
            },
            authority.frozen_commit_timestamp.clone(),
        )?;
        check_cancelled(&cancellation, "household_member_profile_cancelled")?;
        let (outcome, readback) = self.commit_and_reconcile(command, cancellation).await?;
        require_exact_committed_readback(&outcome, &readback, resulting_revision, commit_id)?;
        if readback.state.active_scope != active_scope
            || !readback
                .state
                .profiles
                .iter()
                .any(|value| value == &profile)
            || !readback.state.members.iter().any(|member| {
                member.member_id == save.member_id
                    && member.profile_state == HouseholdProfileStateV1::LocalOnly
                    && member.updated_at == authority.frozen_commit_timestamp
            })
        {
            return Err(household_outcome_uncertain());
        }
        Ok(SavedMemberDeclaredProfileV1 {
            member_id: save.member_id,
            resulting_household_revision: resulting_revision,
            profile_revision,
            active_scope,
            display_label,
        })
    }

    /// Persist one eligible live scope. The caller supplies neither commit
    /// identity nor time, and `Everyone` is represented without inventing a
    /// subject.
    pub async fn select_scope(
        &self,
        expected_revision: HouseholdRevision,
        scope: HouseholdScope,
        cancellation: CancellationToken,
    ) -> Result<SelectedHouseholdScopeV1, PortError> {
        check_cancelled(&cancellation, "household_scope_selection_cancelled")?;
        let load = self.load_required(cancellation.clone()).await?;
        if load.state.revision != expected_revision {
            return Err(repository_error(
                "household_revision_conflict",
                "household scope selection used a stale revision",
            ));
        }
        validate_scope_eligibility_v1(
            &load.state,
            &scope,
            HouseholdProfileOperationV1::PersonalizedContext,
        )
        .map_err(context_port_error)?;
        let target = selected_target(&load.state, &scope)?;
        load.state
            .ensure_commit_capacity()
            .map_err(state_port_error)?;
        let resulting_revision = load
            .state
            .revision
            .checked_next()
            .map_err(state_port_error)?;

        check_cancelled(&cancellation, "household_scope_selection_cancelled")?;
        let authority =
            self.allocate_mutation_authority(HouseholdMutationPurposeV1::SelectScope, &load.state)?;
        let commit_id = authority.commit_id;
        let mut candidate = load.state;
        candidate.revision = resulting_revision;
        candidate.active_scope = scope.clone();
        candidate.updated_at = authority.frozen_commit_timestamp.clone();
        let command = HouseholdCommit::new(
            self.account.clone(),
            expected_revision,
            commit_id,
            candidate,
            HouseholdEffectV1::SelectScope {
                scope: scope.clone(),
            },
            authority.frozen_commit_timestamp,
        )?;
        check_cancelled(&cancellation, "household_scope_selection_cancelled")?;
        let (outcome, readback) = self.commit_and_reconcile(command, cancellation).await?;
        require_exact_committed_readback(&outcome, &readback, resulting_revision, commit_id)?;
        if readback.state.active_scope != scope {
            return Err(household_outcome_uncertain());
        }
        Ok(SelectedHouseholdScopeV1 {
            resulting_household_revision: resulting_revision,
            active_scope: scope,
            target,
        })
    }

    fn allocate_mutation_authority(
        &self,
        purpose: HouseholdMutationPurposeV1,
        state: &HouseholdStateV1,
    ) -> Result<HouseholdMutationAuthorityV1, PortError> {
        let authority = self.mutation_authority.allocate(purpose)?;
        validate_mutation_authority(&authority, purpose, state)?;
        Ok(authority)
    }

    async fn commit_and_reconcile(
        &self,
        command: HouseholdCommit,
        cancellation: CancellationToken,
    ) -> Result<(HouseholdCommitOutcome, HouseholdLoad), PortError> {
        check_account(&self.account, &command.account)?;
        check_cancelled(&cancellation, "household_commit_cancelled")?;
        let first = self.repository.commit(command.clone(), cancellation).await;
        let outcome = match first {
            Ok(outcome) => outcome,
            Err(error) if error.outcome_uncertain => self
                .repository
                .commit(command, CancellationToken::new())
                .await
                .map_err(|_| household_outcome_uncertain())?,
            Err(error) => return Err(error),
        };
        let readback = self
            .load_required(CancellationToken::new())
            .await
            .map_err(|_| household_outcome_uncertain())?;
        Ok((outcome, readback))
    }

    /// Apply one already-built strict migration candidate. Platform migration
    /// owns source discovery and phase-A/phase-B parsing; this use case only
    /// permits the exact account-bound, expected-absence repository write.
    pub async fn initialize_migration(
        &self,
        candidate: HouseholdStateV1,
        commit_id: CommitId,
        frozen_commit_timestamp: CanonicalTimestampV1,
        cancellation: CancellationToken,
    ) -> Result<HouseholdCommitOutcome, PortError> {
        let command = HouseholdInitialize::new(
            self.account.clone(),
            commit_id,
            candidate,
            HouseholdEffectV1::Migration,
            frozen_commit_timestamp,
        )?;
        self.initialize(command, cancellation).await
    }

    /// Atomically persist one owner profile-content revision and its new
    /// local-first sync intent. No network or consent mutation occurs here.
    pub async fn save_owner_profile_and_sync_intent(
        &self,
        save: SaveOwnerProfileAndSyncIntentV1,
        cancellation: CancellationToken,
    ) -> Result<SavedOwnerProfileAndSyncIntentV1, PortError> {
        let load = self.load_required(cancellation.clone()).await?;
        if load.state.revision != save.expected_household_revision {
            return Err(repository_error(
                "household_revision_conflict",
                "owner profile save used a stale household revision",
            ));
        }
        let current_profile_revision = load
            .state
            .profiles
            .iter()
            .find(|profile| profile.subject == HouseholdSubjectId::self_())
            .map(|profile| profile.profile_revision);
        if current_profile_revision != save.expected_profile_revision {
            return Err(repository_error(
                "owner_profile_revision_conflict",
                "owner profile save used a stale profile revision",
            ));
        }
        let expected_new_profile_revision = current_profile_revision
            .map_or_else(|| ProfileRevision::new(1), ProfileRevision::checked_next);
        if save.owner_profile.subject != HouseholdSubjectId::self_()
            || save.owner_profile.profile_revision
                != expected_new_profile_revision.map_err(state_port_error)?
        {
            return Err(repository_error(
                "owner_profile_revision_conflict",
                "owner profile save did not advance the owner profile exactly once",
            ));
        }

        let existing_owner_sync = load.state.outbox.iter().find(|record| {
            matches!(
                record.entry,
                HouseholdProfileOutboxEntryV1::OwnerSync { .. }
            )
        });
        let replaced_outbox_id = match (&save.replaced_intent, existing_owner_sync) {
            (None, None) => None,
            (Some(handle), Some(record)) => {
                if handle.expected_household_revision != save.expected_household_revision
                    || Some(handle.expected_profile_revision) != save.expected_profile_revision
                {
                    return Err(repository_error(
                        "owner_sync_revision_conflict",
                        "owner sync replacement authority is stale",
                    ));
                }
                handle
                    .assert_revisions(&load.state)
                    .map_err(state_port_error)?;
                let HouseholdProfileOutboxEntryV1::OwnerSync { intent, .. } = &record.entry else {
                    unreachable!("owner-sync match established above");
                };
                if record.outbox_id != handle.outbox_id
                    || !owner_sync_intent_can_be_replaced(intent.phase)
                {
                    return Err(repository_error(
                        "owner_sync_replacement_blocked",
                        "owner sync intent must reconcile before a new owner save",
                    ));
                }
                Some(record.outbox_id.clone())
            }
            (None, Some(record)) => {
                let HouseholdProfileOutboxEntryV1::OwnerSync { intent, .. } = &record.entry else {
                    unreachable!("owner-sync match established above");
                };
                let (code, message) = if owner_sync_intent_can_be_replaced(intent.phase) {
                    (
                        "owner_sync_replacement_required",
                        "owner profile save must carry the exact existing intent authority",
                    )
                } else {
                    (
                        "owner_sync_replacement_blocked",
                        "owner sync intent must reconcile before a new owner save",
                    )
                };
                return Err(repository_error(code, message));
            }
            (Some(_), None) => {
                return Err(repository_error(
                    "owner_sync_replacement_mismatch",
                    "owner sync replacement target is unavailable",
                ));
            }
        };

        if save.owner_sync_intent.created_at != save.frozen_commit_timestamp
            || save.owner_sync_intent.updated_at != save.frozen_commit_timestamp
        {
            return Err(repository_error(
                "owner_sync_timestamp_invalid",
                "new owner sync intent must use the frozen save timestamp",
            ));
        }
        let new_outbox_id = HouseholdOutboxId::owner_sync(save.owner_sync_intent.intent_id)
            .map_err(state_port_error)?;
        if replaced_outbox_id.as_ref() == Some(&new_outbox_id) {
            return Err(repository_error(
                "owner_sync_intent_id_reused",
                "a replacement owner save requires a fresh intent ID",
            ));
        }
        let new_outbox_revision = OutboxRevision::new(1).map_err(state_port_error)?;
        let owner_sync_record = HouseholdOutboxRecordV1 {
            outbox_id: new_outbox_id.clone(),
            outbox_revision: new_outbox_revision,
            entry: HouseholdProfileOutboxEntryV1::OwnerSync {
                version: 1,
                target: HouseholdSubjectId::self_(),
                intent: save.owner_sync_intent,
            },
        };

        let mut candidate = load.state;
        candidate.revision = candidate
            .revision
            .checked_next()
            .map_err(state_port_error)?;
        candidate.updated_at = save.frozen_commit_timestamp.clone();
        candidate.owner.profile_state = HouseholdProfileStateV1::PendingSync;
        candidate.owner.updated_at = save.frozen_commit_timestamp.clone();
        upsert_profile(&mut candidate, save.owner_profile.clone());
        if let Some(replaced_outbox_id) = &replaced_outbox_id {
            let index = candidate
                .outbox
                .iter()
                .position(|record| &record.outbox_id == replaced_outbox_id)
                .ok_or_else(|| {
                    repository_error(
                        "owner_sync_replacement_mismatch",
                        "owner sync replacement target is unavailable",
                    )
                })?;
            candidate.outbox.remove(index);
        }
        upsert_outbox(&mut candidate, owner_sync_record.clone());

        let resulting_revision = candidate.revision;
        let effect = HouseholdEffectV1::SaveOwnerProfileAndOwnerSyncIntent {
            owner_profile: save.owner_profile.clone(),
            owner_sync_record: Box::new(owner_sync_record),
            replaced_outbox_id,
        };
        let command = HouseholdCommit::new(
            self.account.clone(),
            save.expected_household_revision,
            save.commit_id,
            candidate,
            effect,
            save.frozen_commit_timestamp,
        )?;
        let commit = self.commit(command, cancellation).await?;
        Ok(SavedOwnerProfileAndSyncIntentV1 {
            commit,
            handle: OwnerSyncIntentHandleV1 {
                outbox_id: new_outbox_id,
                expected_household_revision: resulting_revision,
                expected_profile_revision: save.owner_profile.profile_revision,
                expected_outbox_revision: new_outbox_revision,
            },
        })
    }

    pub async fn transition_owner_sync_intent(
        &self,
        transition: TransitionOwnerSyncIntentV1,
        cancellation: CancellationToken,
    ) -> Result<HouseholdCommitOutcome, PortError> {
        let load = self.load_required(cancellation.clone()).await?;
        transition
            .handle
            .assert_revisions(&load.state)
            .map_err(state_port_error)?;
        let owner_profile_index = load
            .state
            .profiles
            .iter()
            .position(|profile| profile.subject == HouseholdSubjectId::self_())
            .ok_or_else(|| {
                repository_error(
                    "owner_sync_profile_missing",
                    "owner sync profile is unavailable",
                )
            })?;
        let outbox_index = load
            .state
            .outbox
            .iter()
            .position(|record| record.outbox_id == transition.handle.outbox_id)
            .ok_or_else(|| {
                repository_error(
                    "owner_sync_intent_missing",
                    "owner sync intent is unavailable",
                )
            })?;
        let old_record = &load.state.outbox[outbox_index];
        let HouseholdProfileOutboxEntryV1::OwnerSync {
            version,
            target,
            intent: old_intent,
        } = &old_record.entry
        else {
            return Err(repository_error(
                "owner_sync_intent_invalid",
                "owner sync handle does not reference an owner intent",
            ));
        };
        if *version != 1
            || target != &HouseholdSubjectId::self_()
            || old_record.outbox_revision != transition.handle.expected_outbox_revision
        {
            return Err(repository_error(
                "owner_sync_intent_invalid",
                "owner sync intent authority is invalid",
            ));
        }

        let to_phase = transition.replacement.as_ref().map(|intent| intent.phase);
        validate_owner_sync_transition_event(
            old_intent,
            transition.replacement.as_ref(),
            transition.resulting_profile_state,
            transition.event,
            &transition.frozen_commit_timestamp,
        )?;
        let mut candidate = load.state.clone();
        candidate.revision = candidate
            .revision
            .checked_next()
            .map_err(state_port_error)?;
        candidate.updated_at = transition.frozen_commit_timestamp.clone();
        candidate.owner.profile_state = transition.resulting_profile_state;
        candidate.owner.updated_at = transition.frozen_commit_timestamp.clone();

        match &transition.replacement {
            Some(replacement) => {
                let next_outbox_revision = old_record
                    .outbox_revision
                    .checked_next()
                    .map_err(state_port_error)?;
                if next_outbox_revision.get() != replacement.intent_revision {
                    return Err(repository_error(
                        "owner_sync_revision_conflict",
                        "owner sync intent and outbox revisions do not advance together",
                    ));
                }
                candidate.outbox[outbox_index] = HouseholdOutboxRecordV1 {
                    outbox_id: old_record.outbox_id.clone(),
                    outbox_revision: next_outbox_revision,
                    entry: HouseholdProfileOutboxEntryV1::OwnerSync {
                        version: 1,
                        target: HouseholdSubjectId::self_(),
                        intent: replacement.clone(),
                    },
                };
            }
            None => {
                if transition.resulting_profile_state != HouseholdProfileStateV1::Synced {
                    return Err(repository_error(
                        "owner_sync_completion_invalid",
                        "owner sync completion must mark the exact profile synced",
                    ));
                }
                candidate.outbox.remove(outbox_index);
            }
        }
        // Sync status never changes the declared/effective profile record or
        // its independent revision.
        if candidate.profiles[owner_profile_index] != load.state.profiles[owner_profile_index] {
            return Err(repository_error(
                "owner_sync_profile_revision_changed",
                "owner sync transition changed profile content",
            ));
        }
        let effect = HouseholdEffectV1::OwnerSyncTransition {
            outbox_id: transition.handle.outbox_id,
            from_phase: old_intent.phase,
            to_phase,
            resulting_profile_state: transition.resulting_profile_state,
        };
        let command = HouseholdCommit::new(
            self.account.clone(),
            transition.handle.expected_household_revision,
            transition.commit_id,
            candidate,
            effect,
            transition.frozen_commit_timestamp,
        )?;
        self.commit(command, cancellation).await
    }
}

impl fmt::Debug for HouseholdSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdSession")
            .field("account_bound", &true)
            .finish_non_exhaustive()
    }
}

impl OwnerSyncIntentHandleV1 {
    fn assert_revisions(&self, state: &HouseholdStateV1) -> Result<(), HouseholdStateError> {
        if state.revision != self.expected_household_revision {
            return Err(HouseholdStateError::InvalidRevision);
        }
        let profile = state
            .profiles
            .iter()
            .find(|profile| profile.subject == HouseholdSubjectId::self_())
            .ok_or(HouseholdStateError::InvalidOwnerSyncIntent)?;
        if profile.profile_revision != self.expected_profile_revision {
            return Err(HouseholdStateError::InvalidRevision);
        }
        let outbox = state
            .outbox
            .iter()
            .find(|record| record.outbox_id == self.outbox_id)
            .ok_or(HouseholdStateError::InvalidOwnerSyncIntent)?;
        if outbox.outbox_revision != self.expected_outbox_revision {
            return Err(HouseholdStateError::InvalidRevision);
        }
        Ok(())
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct SaveOwnerProfileAndSyncIntentV1 {
    pub expected_household_revision: HouseholdRevision,
    pub expected_profile_revision: Option<ProfileRevision>,
    pub replaced_intent: Option<OwnerSyncIntentHandleV1>,
    pub owner_profile: HouseholdProfileRecordV1,
    pub owner_sync_intent: OwnerSyncIntentV1,
    pub commit_id: CommitId,
    pub frozen_commit_timestamp: CanonicalTimestampV1,
}

impl fmt::Debug for SaveOwnerProfileAndSyncIntentV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SaveOwnerProfileAndSyncIntentV1")
            .field(
                "expected_household_revision",
                &self.expected_household_revision,
            )
            .field("expected_profile_revision", &self.expected_profile_revision)
            .field("replaces_existing_intent", &self.replaced_intent.is_some())
            .field("new_profile_revision", &self.owner_profile.profile_revision)
            .field("commit_id", &self.commit_id)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedOwnerProfileAndSyncIntentV1 {
    pub commit: HouseholdCommitOutcome,
    pub handle: OwnerSyncIntentHandleV1,
}

/// Closed mutating events in the reviewed owner-sync state machine.
///
/// Read failures and same-version consent revalidation that require no local
/// state change are deliberately absent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerSyncTransitionEventV1 {
    ActiveConsentObserved,
    AuthoritativeConsentAbsent,
    ConsentVersionUpdatedBeforeBase,
    RemoteBaseFrozen,
    DispatchStarted,
    PredispatchCancelled,
    DefiniteRemoteSuccess,
    DefiniteRemoteFailure,
    DispatchOutcomeUncertain,
    VersionConflictObserved,
    ReconciliationProvedApplied,
    ReconciliationFoundOldBase,
    ReconciliationConflicted,
    ReconciliationReadUnavailable,
    ConsentVersionChangedAfterFreeze,
    ConsentRevoked,
    ExplicitRetryAfterConsent,
}

#[derive(Clone, Eq, PartialEq)]
pub struct TransitionOwnerSyncIntentV1 {
    pub handle: OwnerSyncIntentHandleV1,
    pub event: OwnerSyncTransitionEventV1,
    pub replacement: Option<OwnerSyncIntentV1>,
    pub resulting_profile_state: HouseholdProfileStateV1,
    pub commit_id: CommitId,
    pub frozen_commit_timestamp: CanonicalTimestampV1,
}

impl fmt::Debug for TransitionOwnerSyncIntentV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TransitionOwnerSyncIntentV1")
            .field("handle", &self.handle)
            .field("event", &self.event)
            .field(
                "replacement_phase",
                &self.replacement.as_ref().map(|intent| intent.phase),
            )
            .field("resulting_profile_state", &self.resulting_profile_state)
            .field("commit_id", &self.commit_id)
            .finish_non_exhaustive()
    }
}

fn validate_owner_intent_replacement(
    previous: &OwnerSyncIntentV1,
    replacement: &OwnerSyncIntentV1,
    resulting_profile_state: HouseholdProfileStateV1,
    committed_at: &CanonicalTimestampV1,
) -> Result<(), PortError> {
    replacement.validate().map_err(state_port_error)?;
    if replacement.intent_revision
        != previous.intent_revision.checked_add(1).ok_or_else(|| {
            repository_error(
                "owner_sync_revision_overflow",
                "owner sync intent revision overflowed",
            )
        })?
        || replacement.intent_id != previous.intent_id
        || replacement.subject != previous.subject
        || replacement.local_household_revision != previous.local_household_revision
        || replacement.local_profile_revision != previous.local_profile_revision
        || replacement.local_profile_digest != previous.local_profile_digest
        || replacement.remote_request_id != previous.remote_request_id
        || replacement.created_at != previous.created_at
        || replacement.updated_at != *committed_at
    {
        return Err(repository_error(
            "owner_sync_frozen_authority_changed",
            "owner sync immutable authority changed",
        ));
    }

    let expected_attempt_count = if previous.phase == OwnerSyncIntentPhaseV1::ReadyToDispatch
        && replacement.phase == OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown
    {
        previous.attempt_count.checked_add(1).ok_or_else(|| {
            repository_error(
                "owner_sync_attempt_count_overflow",
                "owner sync dispatch attempt count overflowed",
            )
        })?
    } else {
        previous.attempt_count
    };
    if replacement.attempt_count != expected_attempt_count {
        return Err(repository_error(
            "owner_sync_attempt_count_invalid",
            "owner sync attempt count did not advance exactly for its event",
        ));
    }

    let previous_frozen = previous.remote_base.is_some()
        || previous.request_method.is_some()
        || previous.request_path.is_some()
        || previous.request_body.is_some()
        || previous.request_body_digest.is_some();
    if previous_frozen
        && (replacement.consent_version != previous.consent_version
            || replacement.remote_base != previous.remote_base
            || replacement.expected_remote_version != previous.expected_remote_version
            || replacement.request_method != previous.request_method
            || replacement.request_path != previous.request_path
            || replacement.request_body != previous.request_body
            || replacement.request_body_digest != previous.request_body_digest)
    {
        return Err(repository_error(
            "owner_sync_frozen_authority_changed",
            "owner sync frozen consent or request authority changed",
        ));
    }
    if !previous_frozen
        && previous.phase != OwnerSyncIntentPhaseV1::NeedsRemoteBase
        && replacement.remote_base.is_some()
    {
        return Err(repository_error(
            "owner_sync_base_freeze_invalid",
            "owner sync base can be frozen only from needs_remote_base",
        ));
    }
    if !OWNER_SYNC_REPLACEMENT_EVENTS.iter().any(|event| {
        replacement_event_matches(*event, previous, replacement, resulting_profile_state)
    }) {
        return Err(repository_error(
            "owner_sync_transition_event_invalid",
            "owner sync state change does not correspond to a legal event",
        ));
    }
    Ok(())
}

const OWNER_SYNC_REPLACEMENT_EVENTS: [OwnerSyncTransitionEventV1; 15] = [
    OwnerSyncTransitionEventV1::ActiveConsentObserved,
    OwnerSyncTransitionEventV1::AuthoritativeConsentAbsent,
    OwnerSyncTransitionEventV1::ConsentVersionUpdatedBeforeBase,
    OwnerSyncTransitionEventV1::RemoteBaseFrozen,
    OwnerSyncTransitionEventV1::DispatchStarted,
    OwnerSyncTransitionEventV1::PredispatchCancelled,
    OwnerSyncTransitionEventV1::DefiniteRemoteFailure,
    OwnerSyncTransitionEventV1::DispatchOutcomeUncertain,
    OwnerSyncTransitionEventV1::VersionConflictObserved,
    OwnerSyncTransitionEventV1::ReconciliationFoundOldBase,
    OwnerSyncTransitionEventV1::ReconciliationConflicted,
    OwnerSyncTransitionEventV1::ReconciliationReadUnavailable,
    OwnerSyncTransitionEventV1::ConsentVersionChangedAfterFreeze,
    OwnerSyncTransitionEventV1::ConsentRevoked,
    OwnerSyncTransitionEventV1::ExplicitRetryAfterConsent,
];

fn validate_owner_sync_transition_event(
    previous: &OwnerSyncIntentV1,
    replacement: Option<&OwnerSyncIntentV1>,
    resulting_profile_state: HouseholdProfileStateV1,
    event: OwnerSyncTransitionEventV1,
    committed_at: &CanonicalTimestampV1,
) -> Result<(), PortError> {
    validate_owner_sync_edge(previous.phase, replacement.map(|intent| intent.phase))?;
    match replacement {
        Some(replacement) => {
            validate_owner_intent_replacement(
                previous,
                replacement,
                resulting_profile_state,
                committed_at,
            )?;
            if replacement_event_matches(event, previous, replacement, resulting_profile_state) {
                Ok(())
            } else {
                Err(repository_error(
                    "owner_sync_transition_event_mismatch",
                    "owner sync event does not authorize the requested state change",
                ))
            }
        }
        None => {
            if resulting_profile_state != HouseholdProfileStateV1::Synced {
                return Err(repository_error(
                    "owner_sync_completion_invalid",
                    "owner sync completion must mark the exact profile synced",
                ));
            }
            let legal = match event {
                OwnerSyncTransitionEventV1::DefiniteRemoteSuccess => {
                    previous.phase == OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown
                }
                OwnerSyncTransitionEventV1::ReconciliationProvedApplied => matches!(
                    previous.phase,
                    OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown
                        | OwnerSyncIntentPhaseV1::OutcomeUncertain
                ),
                _ => false,
            };
            if legal {
                Ok(())
            } else {
                Err(repository_error(
                    "owner_sync_transition_event_mismatch",
                    "owner sync event cannot remove this intent",
                ))
            }
        }
    }
}

fn replacement_event_matches(
    event: OwnerSyncTransitionEventV1,
    previous: &OwnerSyncIntentV1,
    replacement: &OwnerSyncIntentV1,
    resulting_profile_state: HouseholdProfileStateV1,
) -> bool {
    use HouseholdProfileStateV1::{Conflicted, LocalOnly, PendingSync};
    use LastDefiniteOwnerSyncErrorV1::{
        ConsentAbsent, ConsentRevokedRegrantRequired, ConsentVersionChangedRequiresNewSave,
        Forbidden, NotFound, PredispatchCancelled, Unauthorized, Validation, VersionConflict,
    };
    use OwnerSyncIntentPhaseV1::{
        Conflicted as ConflictedPhase, DefiniteFailure, DispatchingOutcomeUnknown,
        LocalOnlyNoConsent, NeedsConsentCheck, NeedsRemoteBase, OutcomeUncertain, ReadyToDispatch,
    };

    match event {
        OwnerSyncTransitionEventV1::ActiveConsentObserved => {
            previous.phase == NeedsConsentCheck
                && replacement.phase == NeedsRemoteBase
                && replacement.consent_version.is_some()
                && resulting_profile_state == PendingSync
        }
        OwnerSyncTransitionEventV1::AuthoritativeConsentAbsent => {
            matches!(previous.phase, NeedsConsentCheck | NeedsRemoteBase)
                && replacement.phase == LocalOnlyNoConsent
                && replacement.last_definite_error == Some(ConsentAbsent)
                && resulting_profile_state == LocalOnly
        }
        OwnerSyncTransitionEventV1::ConsentVersionUpdatedBeforeBase => {
            previous.phase == NeedsRemoteBase
                && replacement.phase == NeedsRemoteBase
                && replacement.consent_version != previous.consent_version
                && resulting_profile_state == PendingSync
        }
        OwnerSyncTransitionEventV1::RemoteBaseFrozen => {
            previous.phase == NeedsRemoteBase
                && replacement.phase == ReadyToDispatch
                && replacement.consent_version == previous.consent_version
                && resulting_profile_state == PendingSync
        }
        OwnerSyncTransitionEventV1::DispatchStarted => {
            previous.phase == ReadyToDispatch
                && replacement.phase == DispatchingOutcomeUnknown
                && replacement.last_definite_error.is_none()
                && resulting_profile_state == PendingSync
        }
        OwnerSyncTransitionEventV1::PredispatchCancelled => {
            previous.phase == DispatchingOutcomeUnknown
                && replacement.phase == ReadyToDispatch
                && replacement.last_definite_error == Some(PredispatchCancelled)
                && resulting_profile_state == PendingSync
        }
        OwnerSyncTransitionEventV1::DefiniteRemoteFailure => {
            previous.phase == DispatchingOutcomeUnknown
                && replacement.phase == DefiniteFailure
                && matches!(
                    replacement.last_definite_error,
                    Some(Unauthorized | Forbidden | Validation | NotFound)
                )
                && resulting_profile_state == PendingSync
        }
        OwnerSyncTransitionEventV1::DispatchOutcomeUncertain => {
            previous.phase == DispatchingOutcomeUnknown
                && replacement.phase == OutcomeUncertain
                && replacement.last_definite_error.is_none()
                && resulting_profile_state == PendingSync
        }
        OwnerSyncTransitionEventV1::VersionConflictObserved => {
            previous.phase == DispatchingOutcomeUnknown
                && replacement.phase == OutcomeUncertain
                && replacement.last_definite_error == Some(VersionConflict)
                && resulting_profile_state == PendingSync
        }
        OwnerSyncTransitionEventV1::ReconciliationFoundOldBase => {
            matches!(previous.phase, DispatchingOutcomeUnknown | OutcomeUncertain)
                && replacement.phase == ReadyToDispatch
                && replacement.last_definite_error.is_none()
                && resulting_profile_state == PendingSync
        }
        OwnerSyncTransitionEventV1::ReconciliationConflicted => {
            matches!(previous.phase, DispatchingOutcomeUnknown | OutcomeUncertain)
                && replacement.phase == ConflictedPhase
                && replacement.last_definite_error == Some(VersionConflict)
                && resulting_profile_state == Conflicted
        }
        OwnerSyncTransitionEventV1::ReconciliationReadUnavailable => {
            ((previous.phase == DispatchingOutcomeUnknown
                && replacement.phase == OutcomeUncertain
                && replacement.last_definite_error.is_none())
                || (previous.phase == OutcomeUncertain
                    && replacement.phase == OutcomeUncertain
                    && replacement.last_definite_error == previous.last_definite_error))
                && resulting_profile_state == PendingSync
        }
        OwnerSyncTransitionEventV1::ConsentVersionChangedAfterFreeze => {
            matches!(
                previous.phase,
                ReadyToDispatch | DispatchingOutcomeUnknown | OutcomeUncertain
            ) && replacement.phase == DefiniteFailure
                && replacement.last_definite_error == Some(ConsentVersionChangedRequiresNewSave)
                && resulting_profile_state == PendingSync
        }
        OwnerSyncTransitionEventV1::ConsentRevoked => {
            matches!(
                previous.phase,
                ReadyToDispatch | DispatchingOutcomeUnknown | OutcomeUncertain
            ) && replacement.phase == DefiniteFailure
                && replacement.last_definite_error == Some(ConsentRevokedRegrantRequired)
                && resulting_profile_state == LocalOnly
        }
        OwnerSyncTransitionEventV1::ExplicitRetryAfterConsent => {
            previous.phase == LocalOnlyNoConsent
                && replacement.phase == NeedsConsentCheck
                && replacement.last_definite_error.is_none()
                && resulting_profile_state == PendingSync
        }
        OwnerSyncTransitionEventV1::DefiniteRemoteSuccess
        | OwnerSyncTransitionEventV1::ReconciliationProvedApplied => false,
    }
}

fn validate_owner_sync_edge(
    from: OwnerSyncIntentPhaseV1,
    to: Option<OwnerSyncIntentPhaseV1>,
) -> Result<(), PortError> {
    use OwnerSyncIntentPhaseV1::{
        Conflicted, DefiniteFailure, DispatchingOutcomeUnknown, LocalOnlyNoConsent,
        NeedsConsentCheck, NeedsRemoteBase, OutcomeUncertain, ReadyToDispatch,
    };
    let legal = matches!(
        (from, to),
        (
            NeedsConsentCheck,
            Some(NeedsRemoteBase | LocalOnlyNoConsent)
        ) | (
            NeedsRemoteBase,
            Some(NeedsRemoteBase | ReadyToDispatch | LocalOnlyNoConsent)
        ) | (
            ReadyToDispatch,
            Some(DispatchingOutcomeUnknown | DefiniteFailure)
        ) | (
            DispatchingOutcomeUnknown,
            Some(ReadyToDispatch | OutcomeUncertain | DefiniteFailure | Conflicted) | None
        ) | (
            OutcomeUncertain,
            Some(ReadyToDispatch | OutcomeUncertain | DefiniteFailure | Conflicted) | None
        ) | (LocalOnlyNoConsent, Some(NeedsConsentCheck))
    );
    if legal {
        Ok(())
    } else {
        Err(repository_error(
            "owner_sync_transition_invalid",
            "owner sync transition is not legal",
        ))
    }
}

fn native_declared_profile_document(
    input: &OnboardingProfileInput,
) -> Result<HouseholdProfileDocumentV1, PortError> {
    input.profile_data().map_err(|_| {
        repository_error(
            "household_declared_profile_invalid",
            "declared household profile is invalid",
        )
    })?;
    HouseholdProfileDocumentV1::native(HouseholdDeclaredProfileV1 {
        diet_style_ids: input.diet_style_ids.clone(),
        custom_diet_styles: input.custom_diet_styles.clone(),
        allergy_ids: input.allergy_ids.clone(),
        custom_restrictions: input.custom_restrictions.clone(),
        health_condition_ids: input.health_condition_ids.clone(),
        custom_health_conditions: input.custom_health_conditions.clone(),
        avoid_ingredients: input.avoid_ingredients.clone(),
        activity_level: input.activity_level.clone(),
        cuisine_preferences: input.cuisine_preferences.clone(),
        custom_cuisines: input.custom_cuisines.clone(),
        severity_level: input.severity_level,
        notes: input.notes.clone(),
    })
    .map_err(|_| {
        repository_error(
            "household_declared_profile_invalid",
            "declared household profile is invalid",
        )
    })
}

fn native_age_evidence(value: NativeMemberAgeEvidenceV1) -> Option<AgeEvidenceV1> {
    let age_band = match value {
        NativeMemberAgeEvidenceV1::Under13 => AgeBandV1::Under13,
        NativeMemberAgeEvidenceV1::Age13_17 => AgeBandV1::Age13_17,
        NativeMemberAgeEvidenceV1::Age18Plus => AgeBandV1::Age18Plus,
        NativeMemberAgeEvidenceV1::Unknown => return None,
    };
    Some(AgeEvidenceV1 {
        date_of_birth: None,
        age_band: Some(age_band),
        source: AgeEvidenceSourceV1::NativeDeclared,
    })
}

fn selected_target(
    state: &HouseholdStateV1,
    scope: &HouseholdScope,
) -> Result<SelectedHouseholdTargetV1, PortError> {
    match scope {
        HouseholdScope::Subject(HouseholdSubjectId::Self_) => Ok(SelectedHouseholdTargetV1::Me),
        HouseholdScope::Subject(HouseholdSubjectId::Member(member_id)) => {
            let member = state
                .members
                .iter()
                .find(|member| &member.member_id == member_id)
                .ok_or_else(|| {
                    repository_error(
                        "household_member_unknown",
                        "household member is unavailable",
                    )
                })?;
            if member.lifecycle != HouseholdLifecycleV1::Active {
                return Err(repository_error(
                    "household_member_archived",
                    "archived household members cannot be selected",
                ));
            }
            Ok(SelectedHouseholdTargetV1::Member {
                member_id: member_id.clone(),
                display_label: member.display_name.clone(),
            })
        }
        HouseholdScope::Everyone => Ok(SelectedHouseholdTargetV1::Everyone),
    }
}

fn validate_mutation_authority(
    authority: &HouseholdMutationAuthorityV1,
    purpose: HouseholdMutationPurposeV1,
    state: &HouseholdStateV1,
) -> Result<(), PortError> {
    let commit_bytes = authority.commit_id.as_uuid().into_bytes();
    let commit_is_uuid_v4 =
        commit_bytes[6] >> 4 == 4 && commit_bytes[8] & 0b1100_0000 == 0b1000_0000;
    let member_shape_is_valid = match (purpose, authority.member_id.as_ref()) {
        (HouseholdMutationPurposeV1::CreateMember, Some(member_id)) => {
            member_id.is_native_uuid_v4()
        }
        (
            HouseholdMutationPurposeV1::SaveMemberProfile | HouseholdMutationPurposeV1::SelectScope,
            None,
        ) => true,
        _ => false,
    };
    let timestamp_date = authority
        .frozen_commit_timestamp
        .as_str()
        .get(..10)
        .unwrap_or_default();
    if !commit_is_uuid_v4
        || !member_shape_is_valid
        || timestamp_date != authority.frozen_evaluation_date.as_str()
        || authority.frozen_commit_timestamp < state.updated_at
    {
        return Err(repository_error(
            "household_mutation_authority_invalid",
            "household mutation authority is invalid",
        ));
    }
    Ok(())
}

fn require_exact_committed_readback(
    outcome: &HouseholdCommitOutcome,
    readback: &HouseholdLoad,
    resulting_revision: HouseholdRevision,
    commit_id: CommitId,
) -> Result<(), PortError> {
    if outcome.outcome != AppliedCommitOutcomeV1::Committed
        || outcome.resulting_revision != resulting_revision
        || readback.state.revision != resulting_revision
        || !readback.state.bounded_applied_commits.iter().any(|record| {
            record.commit_id == commit_id
                && record.outcome == AppliedCommitOutcomeV1::Committed
                && record.resulting_revision == resulting_revision
        })
    {
        return Err(household_outcome_uncertain());
    }
    Ok(())
}

fn household_outcome_uncertain() -> PortError {
    PortError::uncertain(
        "household_mutation_outcome_uncertain",
        "household mutation outcome requires reconciliation",
    )
}

fn scope_kind(scope: &HouseholdScope) -> &'static str {
    match scope {
        HouseholdScope::Subject(HouseholdSubjectId::Self_) => "self",
        HouseholdScope::Subject(HouseholdSubjectId::Member(_)) => "member",
        HouseholdScope::Everyone => "everyone",
    }
}

fn command_fingerprint(
    account: &AccountId,
    commit_id: CommitId,
    expected_state: ExpectedHouseholdStateV1,
    candidate: &HouseholdStateV1,
    effect: &HouseholdEffectV1,
    committed_at: &CanonicalTimestampV1,
) -> Result<HouseholdEffectFingerprintV1, PortError> {
    let account_digest = domain_hash_v1(
        HOUSEHOLD_ACCOUNT_DIGEST_CONTRACT,
        &[account.as_str().as_bytes()],
    )
    .map_err(|_| {
        repository_error(
            "household_account_digest",
            "household account digest could not be constructed",
        )
    })?;
    effect_fingerprint_v1(
        account_digest,
        commit_id,
        expected_state,
        candidate.revision,
        committed_at,
        effect,
        candidate,
    )
    .map_err(state_port_error)
}

fn constant_time_digest_eq(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right.iter())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn check_account(expected: &AccountId, actual: &AccountId) -> Result<(), PortError> {
    if expected == actual {
        Ok(())
    } else {
        Err(repository_error(
            "household_account_mismatch",
            "household command belongs to another account",
        ))
    }
}

fn check_cancelled(cancellation: &CancellationToken, code: &'static str) -> Result<(), PortError> {
    if cancellation.is_cancelled() {
        Err(repository_error(code, "household operation was cancelled"))
    } else {
        Ok(())
    }
}

fn context_port_error(error: HouseholdContextErrorV1) -> PortError {
    repository_error(error.code(), error.to_string())
}

fn state_port_error(error: HouseholdStateError) -> PortError {
    let code = match error {
        HouseholdStateError::AppliedCommitLedgerFull => "household_applied_commit_ledger_full",
        HouseholdStateError::RevisionOverflow => "household_revision_overflow",
        HouseholdStateError::InvalidRevision => "household_revision_invalid",
        HouseholdStateError::InvalidOwnerSyncIntent => "owner_sync_intent_invalid",
        _ => "household_state_invalid",
    };
    repository_error(code, error.to_string())
}

fn repository_error(code: &'static str, message: impl Into<String>) -> PortError {
    PortError::new(code, message)
}
