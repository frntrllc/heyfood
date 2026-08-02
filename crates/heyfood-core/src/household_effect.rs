//! Normalized household effects and non-circular commit fingerprints.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{
    CommitId,
    household_canonical::{CanonicalDigestV1, canonical_sha256_v1},
    household_state::{
        CanonicalTimestampV1, HouseholdMemberV1, HouseholdOutboxId, HouseholdOutboxRecordV1,
        HouseholdProfileOutboxEntryV1, HouseholdProfileRecordV1, HouseholdProfileStateV1,
        HouseholdRevision, HouseholdScope, HouseholdStateError, HouseholdStateV1,
        HouseholdSubjectId, MemberId, OWNER_SYNC_OUTBOX_PREFIX, OwnerSyncIntentPhaseV1,
        ProfileDocumentProvenanceV1,
    },
};

pub const HOUSEHOLD_EFFECT_FINGERPRINT_CONTRACT: &str = "heyfood.household.effect.v1";

fn scope_kind(scope: &HouseholdScope) -> &'static str {
    match scope {
        HouseholdScope::Subject(HouseholdSubjectId::Self_) => "self",
        HouseholdScope::Subject(HouseholdSubjectId::Member(_)) => "member",
        HouseholdScope::Everyone => "everyone",
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExpectedHouseholdStateV1 {
    ExpectedAbsence,
    ExpectedRevision { revision: HouseholdRevision },
}

/// Complete normalized semantic effect. Values are serializable for the
/// fingerprint contract, while `Debug` deliberately omits sensitive payloads.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HouseholdEffectV1 {
    Initialize,
    SelectScope {
        scope: HouseholdScope,
    },
    AddMember {
        member: HouseholdMemberV1,
    },
    CreateMemberWithDeclaredProfile {
        member: HouseholdMemberV1,
        profile: HouseholdProfileRecordV1,
        selected_scope: HouseholdScope,
    },
    CreateMemberWithDeclaredProfileAndScope {
        member: HouseholdMemberV1,
        profile: HouseholdProfileRecordV1,
        previous_scope: HouseholdScope,
        resulting_scope: HouseholdScope,
    },
    ReplaceMember {
        member: HouseholdMemberV1,
    },
    ReplaceMemberAndDeclaredProfile {
        member: HouseholdMemberV1,
        profile: HouseholdProfileRecordV1,
    },
    ArchiveMember {
        member_id: MemberId,
    },
    ArchiveMemberAndSelectScope {
        member_id: MemberId,
        previous_scope: HouseholdScope,
        resulting_scope: HouseholdScope,
    },
    RestoreMember {
        member_id: MemberId,
    },
    SaveOwnerProfileAndOwnerSyncIntent {
        owner_profile: HouseholdProfileRecordV1,
        owner_sync_record: Box<HouseholdOutboxRecordV1>,
        replaced_outbox_id: Option<HouseholdOutboxId>,
    },
    UpsertProfile {
        profile: HouseholdProfileRecordV1,
    },
    RemoveProfile {
        subject: HouseholdSubjectId,
    },
    UpsertOutbox {
        record: HouseholdOutboxRecordV1,
    },
    RemoveOutbox {
        outbox_id: HouseholdOutboxId,
    },
    Migration,
    OwnerSyncTransition {
        outbox_id: HouseholdOutboxId,
        from_phase: OwnerSyncIntentPhaseV1,
        to_phase: Option<OwnerSyncIntentPhaseV1>,
        resulting_profile_state: HouseholdProfileStateV1,
    },
}

impl fmt::Debug for HouseholdEffectV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Initialize => formatter.write_str("HouseholdEffectV1::Initialize"),
            Self::SelectScope { scope } => formatter
                .debug_struct("HouseholdEffectV1::SelectScope")
                .field(
                    "scope_kind",
                    &match scope {
                        HouseholdScope::Subject(HouseholdSubjectId::Self_) => "self",
                        HouseholdScope::Subject(HouseholdSubjectId::Member(_)) => "member",
                        HouseholdScope::Everyone => "everyone",
                    },
                )
                .finish(),
            Self::AddMember { member: _ } => formatter
                .debug_struct("HouseholdEffectV1::AddMember")
                .finish_non_exhaustive(),
            Self::CreateMemberWithDeclaredProfile {
                member: _,
                profile,
                selected_scope,
            } => formatter
                .debug_struct("HouseholdEffectV1::CreateMemberWithDeclaredProfile")
                .field("profile_revision", &profile.profile_revision)
                .field(
                    "selected_scope_kind",
                    &match selected_scope {
                        HouseholdScope::Subject(HouseholdSubjectId::Self_) => "self",
                        HouseholdScope::Subject(HouseholdSubjectId::Member(_)) => "member",
                        HouseholdScope::Everyone => "everyone",
                    },
                )
                .finish_non_exhaustive(),
            Self::CreateMemberWithDeclaredProfileAndScope {
                member: _,
                profile,
                previous_scope,
                resulting_scope,
            } => formatter
                .debug_struct("HouseholdEffectV1::CreateMemberWithDeclaredProfileAndScope")
                .field("profile_revision", &profile.profile_revision)
                .field("previous_scope_kind", &scope_kind(previous_scope))
                .field("resulting_scope_kind", &scope_kind(resulting_scope))
                .finish_non_exhaustive(),
            Self::ReplaceMember { member: _ } => formatter
                .debug_struct("HouseholdEffectV1::ReplaceMember")
                .finish_non_exhaustive(),
            Self::ReplaceMemberAndDeclaredProfile { member: _, profile } => formatter
                .debug_struct("HouseholdEffectV1::ReplaceMemberAndDeclaredProfile")
                .field("profile_revision", &profile.profile_revision)
                .finish_non_exhaustive(),
            Self::ArchiveMember { member_id: _ } => formatter
                .debug_struct("HouseholdEffectV1::ArchiveMember")
                .finish_non_exhaustive(),
            Self::ArchiveMemberAndSelectScope {
                member_id: _,
                previous_scope,
                resulting_scope,
            } => formatter
                .debug_struct("HouseholdEffectV1::ArchiveMemberAndSelectScope")
                .field("previous_scope_kind", &scope_kind(previous_scope))
                .field("resulting_scope_kind", &scope_kind(resulting_scope))
                .finish_non_exhaustive(),
            Self::RestoreMember { member_id: _ } => formatter
                .debug_struct("HouseholdEffectV1::RestoreMember")
                .finish_non_exhaustive(),
            Self::SaveOwnerProfileAndOwnerSyncIntent {
                owner_profile,
                owner_sync_record,
                replaced_outbox_id,
            } => formatter
                .debug_struct("HouseholdEffectV1::SaveOwnerProfileAndOwnerSyncIntent")
                .field("profile_revision", &owner_profile.profile_revision)
                .field("outbox_revision", &owner_sync_record.outbox_revision)
                .field("replaces_existing_intent", &replaced_outbox_id.is_some())
                .finish_non_exhaustive(),
            Self::UpsertProfile { profile } => formatter
                .debug_struct("HouseholdEffectV1::UpsertProfile")
                .field("profile_revision", &profile.profile_revision)
                .finish_non_exhaustive(),
            Self::RemoveProfile { subject: _ } => formatter
                .debug_struct("HouseholdEffectV1::RemoveProfile")
                .finish_non_exhaustive(),
            Self::UpsertOutbox { record } => formatter
                .debug_struct("HouseholdEffectV1::UpsertOutbox")
                .field("outbox_revision", &record.outbox_revision)
                .finish_non_exhaustive(),
            Self::RemoveOutbox { outbox_id: _ } => formatter
                .debug_struct("HouseholdEffectV1::RemoveOutbox")
                .finish_non_exhaustive(),
            Self::Migration => formatter.write_str("HouseholdEffectV1::Migration"),
            Self::OwnerSyncTransition {
                outbox_id: _,
                from_phase,
                to_phase,
                resulting_profile_state,
            } => formatter
                .debug_struct("HouseholdEffectV1::OwnerSyncTransition")
                .field("from_phase", from_phase)
                .field("to_phase", to_phase)
                .field("resulting_profile_state", resulting_profile_state)
                .finish(),
        }
    }
}

impl HouseholdEffectV1 {
    fn validate_against(
        &self,
        expected_state: ExpectedHouseholdStateV1,
        candidate: &HouseholdStateV1,
    ) -> Result<(), HouseholdStateError> {
        match (expected_state, self) {
            (ExpectedHouseholdStateV1::ExpectedAbsence, Self::Initialize | Self::Migration)
                if candidate.revision.get() == 1 => {}
            (ExpectedHouseholdStateV1::ExpectedRevision { revision }, effect)
                if revision.checked_next()? == candidate.revision =>
            {
                match effect {
                    Self::Initialize | Self::Migration => {
                        return Err(HouseholdStateError::InvalidRevision);
                    }
                    Self::SelectScope { scope } if &candidate.active_scope == scope => {}
                    Self::AddMember { member } | Self::ReplaceMember { member }
                        if candidate.members.iter().any(|value| value == member) => {}
                    Self::CreateMemberWithDeclaredProfile {
                        member,
                        profile,
                        selected_scope,
                    } if validate_create_member_with_declared_profile(
                        candidate,
                        member,
                        profile,
                        selected_scope,
                        true,
                    )
                    .is_ok() => {}
                    Self::CreateMemberWithDeclaredProfileAndScope {
                        member,
                        profile,
                        previous_scope: _,
                        resulting_scope,
                    } if validate_create_member_with_declared_profile(
                        candidate,
                        member,
                        profile,
                        resulting_scope,
                        false,
                    )
                    .is_ok() => {}
                    Self::ReplaceMemberAndDeclaredProfile { member, profile }
                        if candidate.members.iter().any(|value| value == member)
                            && candidate.profiles.iter().any(|value| value == profile)
                            && profile.subject
                                == HouseholdSubjectId::member(member.member_id.clone()) => {}
                    Self::ArchiveMember { member_id }
                        if candidate.members.iter().any(|member| {
                            &member.member_id == member_id
                                && member.lifecycle
                                    == crate::household_state::HouseholdLifecycleV1::Archived
                        }) => {}
                    Self::ArchiveMemberAndSelectScope {
                        member_id,
                        previous_scope: _,
                        resulting_scope,
                    } if candidate.active_scope == *resulting_scope
                        && candidate.members.iter().any(|member| {
                            &member.member_id == member_id
                                && member.lifecycle
                                    == crate::household_state::HouseholdLifecycleV1::Archived
                        }) => {}
                    Self::RestoreMember { member_id }
                        if candidate.members.iter().any(|member| {
                            &member.member_id == member_id
                                && member.lifecycle
                                    == crate::household_state::HouseholdLifecycleV1::Active
                        }) => {}
                    Self::SaveOwnerProfileAndOwnerSyncIntent {
                        owner_profile,
                        owner_sync_record,
                        replaced_outbox_id,
                    } => validate_owner_profile_and_sync_intent_save(
                        candidate,
                        owner_profile,
                        owner_sync_record,
                        replaced_outbox_id.as_ref(),
                    )?,
                    Self::UpsertProfile { profile }
                        if candidate.profiles.iter().any(|value| value == profile) => {}
                    Self::RemoveProfile { subject }
                        if candidate
                            .profiles
                            .iter()
                            .all(|profile| &profile.subject != subject) => {}
                    Self::UpsertOutbox { record }
                        if candidate.outbox.iter().any(|value| value == record) => {}
                    Self::RemoveOutbox { outbox_id }
                        if candidate
                            .outbox
                            .iter()
                            .all(|record| &record.outbox_id != outbox_id) => {}
                    Self::OwnerSyncTransition {
                        outbox_id,
                        from_phase,
                        to_phase,
                        resulting_profile_state,
                    } if outbox_id.as_str().starts_with(OWNER_SYNC_OUTBOX_PREFIX)
                        && owner_sync_edge_is_legal(*from_phase, *to_phase)
                        && (to_phase.is_some()
                            || *resulting_profile_state == HouseholdProfileStateV1::Synced)
                        && candidate.owner.profile_state == *resulting_profile_state
                        && match to_phase {
                            Some(to_phase) => candidate.outbox.iter().any(|record| {
                                &record.outbox_id == outbox_id
                                    && matches!(
                                        &record.entry,
                                        HouseholdProfileOutboxEntryV1::OwnerSync {
                                            intent,
                                            ..
                                        } if intent.phase == *to_phase
                                    )
                            }),
                            None => candidate
                                .outbox
                                .iter()
                                .all(|record| &record.outbox_id != outbox_id),
                        } => {}
                    _ => return Err(HouseholdStateError::InvalidRevision),
                }
            }
            _ => return Err(HouseholdStateError::InvalidRevision),
        }
        Ok(())
    }
}

fn validate_create_member_with_declared_profile(
    candidate: &HouseholdStateV1,
    member: &HouseholdMemberV1,
    profile: &HouseholdProfileRecordV1,
    selected_scope: &HouseholdScope,
    require_new_member_scope: bool,
) -> Result<(), HouseholdStateError> {
    let subject = HouseholdSubjectId::member(member.member_id.clone());
    if !member.member_id.is_native_uuid_v4()
        || member.relationship == crate::household_state::RelationshipV1::Self_
        || member.relationship_source
            != crate::household_state::RelationshipSourceV1::NativeDeclared
        || member.lifecycle != crate::household_state::HouseholdLifecycleV1::Active
        || member.profile_state != HouseholdProfileStateV1::LocalOnly
        || member.created_at != candidate.updated_at
        || member.updated_at != candidate.updated_at
        || profile.subject != subject
        || profile.profile_revision.get() != 1
        || profile.document.provenance != ProfileDocumentProvenanceV1::NativeDeclared
        || (require_new_member_scope && selected_scope != &HouseholdScope::Subject(subject.clone()))
        || candidate.active_scope != *selected_scope
        || !candidate.members.iter().any(|value| value == member)
        || !candidate.profiles.iter().any(|value| value == profile)
    {
        return Err(HouseholdStateError::InvalidProfileDocument);
    }
    Ok(())
}

fn validate_owner_profile_and_sync_intent_save(
    candidate: &HouseholdStateV1,
    owner_profile: &HouseholdProfileRecordV1,
    owner_sync_record: &HouseholdOutboxRecordV1,
    replaced_outbox_id: Option<&HouseholdOutboxId>,
) -> Result<(), HouseholdStateError> {
    let HouseholdProfileOutboxEntryV1::OwnerSync {
        version,
        target,
        intent,
    } = &owner_sync_record.entry
    else {
        return Err(HouseholdStateError::InvalidOwnerSyncIntent);
    };
    let effective_profile = owner_profile
        .document
        .effective_profile()?
        .ok_or(HouseholdStateError::InvalidOwnerSyncIntent)?;
    let expected_outbox_id = HouseholdOutboxId::owner_sync(intent.intent_id)?;
    if owner_profile.subject != HouseholdSubjectId::self_()
        || !candidate
            .profiles
            .iter()
            .any(|profile| profile == owner_profile)
        || !candidate
            .outbox
            .iter()
            .any(|record| record == owner_sync_record)
        || candidate.owner.profile_state != HouseholdProfileStateV1::PendingSync
        || *version != 1
        || target != &HouseholdSubjectId::self_()
        || intent.subject != HouseholdSubjectId::self_()
        || intent.phase != OwnerSyncIntentPhaseV1::NeedsConsentCheck
        || intent.local_household_revision != candidate.revision.get()
        || intent.local_profile_revision != owner_profile.profile_revision.get()
        || intent.local_profile_digest != canonical_sha256_v1(&effective_profile)?
        || intent.intent_revision != 1
        || intent.created_at != candidate.updated_at
        || intent.updated_at != candidate.updated_at
        || candidate.owner.updated_at != candidate.updated_at
        || owner_sync_record.outbox_revision.get() != 1
        || owner_sync_record.outbox_id != expected_outbox_id
    {
        return Err(HouseholdStateError::InvalidOwnerSyncIntent);
    }
    if let Some(replaced_outbox_id) = replaced_outbox_id
        && (!replaced_outbox_id
            .as_str()
            .starts_with(OWNER_SYNC_OUTBOX_PREFIX)
            || replaced_outbox_id == &owner_sync_record.outbox_id
            || candidate
                .outbox
                .iter()
                .any(|record| &record.outbox_id == replaced_outbox_id))
    {
        return Err(HouseholdStateError::InvalidOwnerSyncIntent);
    }
    Ok(())
}

fn owner_sync_edge_is_legal(
    from: OwnerSyncIntentPhaseV1,
    to: Option<OwnerSyncIntentPhaseV1>,
) -> bool {
    use OwnerSyncIntentPhaseV1::{
        Conflicted, DefiniteFailure, DispatchingOutcomeUnknown, LocalOnlyNoConsent,
        NeedsConsentCheck, NeedsRemoteBase, OutcomeUncertain, ReadyToDispatch,
    };
    matches!(
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
    )
}

#[derive(Clone, Eq, PartialEq, Serialize)]
pub struct HouseholdEffectFingerprintInputV1<'a> {
    pub contract: &'static str,
    pub account_digest: CanonicalDigestV1,
    pub commit_id: CommitId,
    pub expected_state: ExpectedHouseholdStateV1,
    pub resulting_revision: HouseholdRevision,
    pub frozen_commit_timestamp: &'a CanonicalTimestampV1,
    pub normalized_typed_effect: &'a HouseholdEffectV1,
    pub semantic_candidate_state_without_applied_commit_ledger: HouseholdStateV1,
}

impl fmt::Debug for HouseholdEffectFingerprintInputV1<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdEffectFingerprintInputV1")
            .field("contract", &self.contract)
            .field("account_digest", &self.account_digest)
            .field("expected_state", &self.expected_state)
            .field("resulting_revision", &self.resulting_revision)
            .field("normalized_typed_effect", &self.normalized_typed_effect)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HouseholdEffectFingerprintV1(CanonicalDigestV1);

impl HouseholdEffectFingerprintV1 {
    #[must_use]
    pub const fn from_digest(value: CanonicalDigestV1) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_digest(self) -> CanonicalDigestV1 {
        self.0
    }
}

pub fn effect_fingerprint_v1(
    account_digest: CanonicalDigestV1,
    commit_id: CommitId,
    expected_state: ExpectedHouseholdStateV1,
    resulting_revision: HouseholdRevision,
    frozen_commit_timestamp: &CanonicalTimestampV1,
    normalized_typed_effect: &HouseholdEffectV1,
    semantic_candidate_state: &HouseholdStateV1,
) -> Result<HouseholdEffectFingerprintV1, HouseholdStateError> {
    if semantic_candidate_state.revision != resulting_revision {
        return Err(HouseholdStateError::InvalidRevision);
    }
    semantic_candidate_state.validate()?;
    normalized_typed_effect.validate_against(expected_state, semantic_candidate_state)?;
    let mut without_ledger = semantic_candidate_state.clone();
    without_ledger.bounded_applied_commits.clear();
    let preimage = HouseholdEffectFingerprintInputV1 {
        contract: HOUSEHOLD_EFFECT_FINGERPRINT_CONTRACT,
        account_digest,
        commit_id,
        expected_state,
        resulting_revision,
        frozen_commit_timestamp,
        normalized_typed_effect,
        semantic_candidate_state_without_applied_commit_ledger: without_ledger,
    };
    canonical_sha256_v1(&preimage)
        .map(HouseholdEffectFingerprintV1)
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::household_state::*;
    use uuid::Uuid;

    fn state() -> HouseholdStateV1 {
        let timestamp = CanonicalTimestampV1::parse("2026-07-30T12:00:00.000Z").unwrap();
        HouseholdStateV1 {
            schema_version: HOUSEHOLD_STATE_SCHEMA_VERSION,
            account_binding: crate::AccountId::parse("acct_example_01").unwrap(),
            revision: HouseholdRevision::new(1).unwrap(),
            owner: HouseholdOwnerV1 {
                display_name: DisplayName::parse("Owner").unwrap(),
                relationship: RelationshipV1::Self_,
                profile_state: HouseholdProfileStateV1::Incomplete,
                created_at: timestamp.clone(),
                updated_at: timestamp.clone(),
            },
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
            migration_provenance: MigrationProvenanceV1 {
                source_identity: LegacySourceIdentityV1::NoSource {
                    source_set_fingerprint: CanonicalDigestV1::from_bytes([7; 32]),
                },
                legacy_python_snapshot: None,
                migration_id: Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap(),
                initialization_id: Uuid::parse_str("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb").unwrap(),
                initial_commit_id: CommitId::from_uuid(
                    Uuid::parse_str("cccccccc-cccc-4ccc-8ccc-cccccccccccc").unwrap(),
                ),
                migration_frozen_at: timestamp.clone(),
            },
            updated_at: timestamp,
        }
    }

    fn owner_save_candidate(
        restriction: &str,
        replaced_outbox_id: Option<HouseholdOutboxId>,
    ) -> (HouseholdStateV1, HouseholdEffectV1) {
        let mut candidate = state();
        candidate.revision = HouseholdRevision::new(2).unwrap();
        candidate.owner.profile_state = HouseholdProfileStateV1::PendingSync;
        let document = HouseholdProfileDocumentV1::legacy_projection(
            serde_json::to_string(&serde_json::json!({"restrictions":[restriction]}))
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
        let effective_profile = document.effective_profile().unwrap().unwrap();
        let owner_profile = HouseholdProfileRecordV1 {
            subject: HouseholdSubjectId::self_(),
            profile_revision: ProfileRevision::new(1).unwrap(),
            document,
        };
        let intent_id = Uuid::parse_str("eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee").unwrap();
        let intent = OwnerSyncIntentV1 {
            schema_version: 1,
            intent_id,
            intent_revision: 1,
            phase: OwnerSyncIntentPhaseV1::NeedsConsentCheck,
            subject: HouseholdSubjectId::self_(),
            local_household_revision: 2,
            local_profile_revision: 1,
            local_profile_digest: canonical_sha256_v1(&effective_profile).unwrap(),
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
            created_at: candidate.updated_at.clone(),
            updated_at: candidate.updated_at.clone(),
        };
        let owner_sync_record = HouseholdOutboxRecordV1 {
            outbox_id: HouseholdOutboxId::owner_sync(intent_id).unwrap(),
            outbox_revision: OutboxRevision::new(1).unwrap(),
            entry: HouseholdProfileOutboxEntryV1::OwnerSync {
                version: 1,
                target: HouseholdSubjectId::self_(),
                intent,
            },
        };
        candidate.profiles = vec![owner_profile.clone()];
        candidate.outbox = vec![owner_sync_record.clone()];
        let effect = HouseholdEffectV1::SaveOwnerProfileAndOwnerSyncIntent {
            owner_profile,
            owner_sync_record: Box::new(owner_sync_record),
            replaced_outbox_id,
        };
        (candidate, effect)
    }

    #[test]
    fn atomic_owner_profile_and_sync_intent_save_is_bound_redacted_and_fingerprinted() {
        let timestamp = CanonicalTimestampV1::parse("2026-07-30T12:00:00.000Z").unwrap();
        let account_digest = CanonicalDigestV1::from_bytes([1; 32]);
        let commit_id =
            CommitId::from_uuid(Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap());
        let expected_state = ExpectedHouseholdStateV1::ExpectedRevision {
            revision: HouseholdRevision::new(1).unwrap(),
        };
        let (candidate, effect) = owner_save_candidate("canary-profile-secret", None);
        let fingerprint = effect_fingerprint_v1(
            account_digest,
            commit_id,
            expected_state,
            HouseholdRevision::new(2).unwrap(),
            &timestamp,
            &effect,
            &candidate,
        )
        .unwrap();
        let rendered = format!("{effect:?}");
        assert!(!rendered.contains("canary-profile-secret"));
        assert!(!rendered.contains("eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee"));

        let golden: serde_json::Value = serde_json::from_str(include_str!(
            "../../../schemas/v1/household-canonical-v1.golden.json"
        ))
        .unwrap();
        let mut candidate_without_ledger = candidate.clone();
        candidate_without_ledger.bounded_applied_commits.clear();
        let preimage = HouseholdEffectFingerprintInputV1 {
            contract: HOUSEHOLD_EFFECT_FINGERPRINT_CONTRACT,
            account_digest,
            commit_id,
            expected_state,
            resulting_revision: HouseholdRevision::new(2).unwrap(),
            frozen_commit_timestamp: &timestamp,
            normalized_typed_effect: &effect,
            semantic_candidate_state_without_applied_commit_ledger: candidate_without_ledger,
        };
        let atomic_preimage =
            String::from_utf8(crate::to_canonical_bytes_v1(&preimage).unwrap()).unwrap();
        assert_eq!(
            atomic_preimage,
            golden["state"]["atomic_owner_save_effect_preimage_utf8"]
                .as_str()
                .unwrap()
        );
        assert_eq!(
            fingerprint.as_digest().to_lower_hex(),
            golden["state"]["atomic_owner_save_effect_fingerprint"]
                .as_str()
                .unwrap()
        );

        let (changed_candidate, changed_effect) =
            owner_save_candidate("different-profile-secret", None);
        let changed_profile_fingerprint = effect_fingerprint_v1(
            account_digest,
            commit_id,
            expected_state,
            HouseholdRevision::new(2).unwrap(),
            &timestamp,
            &changed_effect,
            &changed_candidate,
        )
        .unwrap();
        assert_ne!(fingerprint, changed_profile_fingerprint);

        let replacement = HouseholdOutboxId::owner_sync(
            Uuid::parse_str("ffffffff-ffff-4fff-8fff-ffffffffffff").unwrap(),
        )
        .unwrap();
        let (replacement_candidate, replacement_effect) =
            owner_save_candidate("canary-profile-secret", Some(replacement));
        let replacement_fingerprint = effect_fingerprint_v1(
            account_digest,
            commit_id,
            expected_state,
            HouseholdRevision::new(2).unwrap(),
            &timestamp,
            &replacement_effect,
            &replacement_candidate,
        )
        .unwrap();
        assert_ne!(fingerprint, replacement_fingerprint);
    }

    #[test]
    fn atomic_owner_save_rejects_revision_identity_and_digest_mismatches() {
        let timestamp = CanonicalTimestampV1::parse("2026-07-30T12:00:00.000Z").unwrap();
        let account_digest = CanonicalDigestV1::from_bytes([1; 32]);
        let commit_id =
            CommitId::from_uuid(Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap());
        let expected_state = ExpectedHouseholdStateV1::ExpectedRevision {
            revision: HouseholdRevision::new(1).unwrap(),
        };
        let (candidate, effect) = owner_save_candidate("profile-secret", None);
        let assert_rejected = |effect: &HouseholdEffectV1, candidate: &HouseholdStateV1| {
            assert_eq!(
                effect_fingerprint_v1(
                    account_digest,
                    commit_id,
                    expected_state,
                    HouseholdRevision::new(2).unwrap(),
                    &timestamp,
                    effect,
                    candidate,
                ),
                Err(HouseholdStateError::InvalidOwnerSyncIntent)
            );
        };

        let mut stale_revision_candidate = candidate.clone();
        let HouseholdProfileOutboxEntryV1::OwnerSync { intent, .. } =
            &mut stale_revision_candidate.outbox[0].entry
        else {
            unreachable!()
        };
        intent.local_household_revision = 1;
        let stale_revision_effect = HouseholdEffectV1::SaveOwnerProfileAndOwnerSyncIntent {
            owner_profile: stale_revision_candidate.profiles[0].clone(),
            owner_sync_record: Box::new(stale_revision_candidate.outbox[0].clone()),
            replaced_outbox_id: None,
        };
        assert_rejected(&stale_revision_effect, &stale_revision_candidate);

        let mut mismatched_profile_effect = effect.clone();
        let HouseholdEffectV1::SaveOwnerProfileAndOwnerSyncIntent { owner_profile, .. } =
            &mut mismatched_profile_effect
        else {
            unreachable!()
        };
        owner_profile.document = HouseholdProfileDocumentV1::legacy_projection(
            br#"{"restrictions":["different-secret"]}"#,
        )
        .unwrap();
        assert_rejected(&mismatched_profile_effect, &candidate);

        let HouseholdEffectV1::SaveOwnerProfileAndOwnerSyncIntent {
            owner_sync_record, ..
        } = &effect
        else {
            unreachable!()
        };
        let same_id_effect = HouseholdEffectV1::SaveOwnerProfileAndOwnerSyncIntent {
            owner_profile: candidate.profiles[0].clone(),
            owner_sync_record: owner_sync_record.clone(),
            replaced_outbox_id: Some(owner_sync_record.outbox_id.clone()),
        };
        assert_rejected(&same_id_effect, &candidate);
    }

    #[test]
    fn fingerprint_excludes_entire_applied_commit_ledger() {
        let state = state();
        let commit_id =
            CommitId::from_uuid(Uuid::parse_str("dddddddd-dddd-4ddd-8ddd-dddddddddddd").unwrap());
        let timestamp = CanonicalTimestampV1::parse("2026-07-30T12:00:00.000Z").unwrap();
        let account_digest = CanonicalDigestV1::from_bytes([1; 32]);
        let first = effect_fingerprint_v1(
            account_digest,
            commit_id,
            ExpectedHouseholdStateV1::ExpectedAbsence,
            HouseholdRevision::new(1).unwrap(),
            &timestamp,
            &HouseholdEffectV1::Initialize,
            &state,
        )
        .unwrap();
        let golden: serde_json::Value = serde_json::from_str(include_str!(
            "../../../schemas/v1/household-canonical-v1.golden.json"
        ))
        .unwrap();
        assert_eq!(
            first.as_digest().to_lower_hex(),
            golden["state"]["effect_fingerprint"].as_str().unwrap()
        );
        assert_eq!(
            String::from_utf8(state.canonical_bytes().unwrap()).unwrap(),
            golden["state"]["canonical_utf8"].as_str().unwrap()
        );
        assert_eq!(
            canonical_sha256_v1(&state).unwrap().to_lower_hex(),
            golden["state"]["sha256"].as_str().unwrap()
        );
        let preimage = HouseholdEffectFingerprintInputV1 {
            contract: HOUSEHOLD_EFFECT_FINGERPRINT_CONTRACT,
            account_digest,
            commit_id,
            expected_state: ExpectedHouseholdStateV1::ExpectedAbsence,
            resulting_revision: HouseholdRevision::new(1).unwrap(),
            frozen_commit_timestamp: &timestamp,
            normalized_typed_effect: &HouseholdEffectV1::Initialize,
            semantic_candidate_state_without_applied_commit_ledger: state.clone(),
        };
        assert_eq!(
            String::from_utf8(crate::to_canonical_bytes_v1(&preimage).unwrap()).unwrap(),
            golden["state"]["effect_preimage_utf8"].as_str().unwrap()
        );
        let mut with_ledger = state.clone();
        with_ledger
            .bounded_applied_commits
            .push(AppliedCommitRecordV1 {
                commit_id,
                fingerprint: CanonicalDigestV1::from_bytes([2; 32]),
                resulting_revision: HouseholdRevision::new(1).unwrap(),
                outcome: AppliedCommitOutcomeV1::Initialized,
                committed_at: timestamp.clone(),
            });
        let second = effect_fingerprint_v1(
            account_digest,
            commit_id,
            ExpectedHouseholdStateV1::ExpectedAbsence,
            HouseholdRevision::new(1).unwrap(),
            &timestamp,
            &HouseholdEffectV1::Initialize,
            &with_ledger,
        )
        .unwrap();
        assert_eq!(first, second);

        let mut revision_two = state;
        revision_two.revision = HouseholdRevision::new(2).unwrap();
        assert_eq!(
            effect_fingerprint_v1(
                account_digest,
                commit_id,
                ExpectedHouseholdStateV1::ExpectedRevision {
                    revision: HouseholdRevision::new(1).unwrap(),
                },
                HouseholdRevision::new(2).unwrap(),
                &timestamp,
                &HouseholdEffectV1::SelectScope {
                    scope: HouseholdScope::Everyone,
                },
                &revision_two,
            ),
            Err(HouseholdStateError::InvalidRevision)
        );
    }
}
