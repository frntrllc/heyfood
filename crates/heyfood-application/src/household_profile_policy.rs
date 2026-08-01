//! D2 household profile policy.
//!
//! This module deliberately separates local personalization from hosted
//! profile-sync authority. In D2 every non-owner profile remains local-only;
//! a relationship or an inferred age never grants remote persistence.

use std::fmt;

use heyfood_core::{
    ConsentVersionV1, HouseholdLifecycleV1, HouseholdOutboxId, HouseholdProfileOutboxEntryV1,
    HouseholdProfileStateV1, HouseholdRevision, HouseholdStateError, HouseholdStateV1,
    HouseholdSubjectId, LastDefiniteOwnerSyncErrorV1, MinorStatusV1, OutboxRevision,
    OwnerSyncIntentPhaseV1, ProfileRevision,
};

/// The operation whose subject eligibility is being evaluated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HouseholdProfileOperationV1 {
    /// Build dietary context for a local or hosted evaluation.
    PersonalizedContext,
    /// Create or replace encrypted local profile material.
    LocalProfileWrite,
    /// Persist a profile to the hosted owner-profile service.
    PersistentProfileSync,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HouseholdProfileIneligibilityV1 {
    UnknownSubject,
    ArchivedSubject,
    ProfileIncomplete,
    ProfileConflicted,
    MinorPersistentSyncBlocked,
    UnknownAgePersistentSyncBlocked,
    NonOwnerPersistentSyncDeferred,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HouseholdProfileEligibilityV1 {
    Eligible,
    Ineligible(HouseholdProfileIneligibilityV1),
}

/// Apply the reviewed D2 local-only/minor policy to one typed subject.
pub fn household_profile_eligibility_v1(
    state: &HouseholdStateV1,
    subject: &HouseholdSubjectId,
    operation: HouseholdProfileOperationV1,
) -> HouseholdProfileEligibilityV1 {
    match subject {
        HouseholdSubjectId::Self_ => match operation {
            HouseholdProfileOperationV1::LocalProfileWrite => {
                HouseholdProfileEligibilityV1::Eligible
            }
            HouseholdProfileOperationV1::PersonalizedContext
            | HouseholdProfileOperationV1::PersistentProfileSync => {
                profile_state_eligibility(state.owner.profile_state)
            }
        },
        HouseholdSubjectId::Member(member_id) => {
            let Some(member) = state
                .members
                .iter()
                .find(|member| &member.member_id == member_id)
            else {
                return HouseholdProfileEligibilityV1::Ineligible(
                    HouseholdProfileIneligibilityV1::UnknownSubject,
                );
            };
            if member.lifecycle == HouseholdLifecycleV1::Archived {
                return HouseholdProfileEligibilityV1::Ineligible(
                    HouseholdProfileIneligibilityV1::ArchivedSubject,
                );
            }
            match operation {
                HouseholdProfileOperationV1::LocalProfileWrite => {
                    HouseholdProfileEligibilityV1::Eligible
                }
                HouseholdProfileOperationV1::PersonalizedContext => {
                    profile_state_eligibility(member.profile_state)
                }
                HouseholdProfileOperationV1::PersistentProfileSync => {
                    // The status-specific reasons are retained so a future,
                    // separately reviewed sync milestone cannot accidentally
                    // treat unknown/minor evidence as adult authority.
                    HouseholdProfileEligibilityV1::Ineligible(match member.minor_status {
                        MinorStatusV1::Minor => {
                            HouseholdProfileIneligibilityV1::MinorPersistentSyncBlocked
                        }
                        MinorStatusV1::Unknown => {
                            HouseholdProfileIneligibilityV1::UnknownAgePersistentSyncBlocked
                        }
                        MinorStatusV1::Adult => {
                            HouseholdProfileIneligibilityV1::NonOwnerPersistentSyncDeferred
                        }
                    })
                }
            }
        }
    }
}

fn profile_state_eligibility(state: HouseholdProfileStateV1) -> HouseholdProfileEligibilityV1 {
    match state {
        HouseholdProfileStateV1::Incomplete => HouseholdProfileEligibilityV1::Ineligible(
            HouseholdProfileIneligibilityV1::ProfileIncomplete,
        ),
        HouseholdProfileStateV1::Conflicted => HouseholdProfileEligibilityV1::Ineligible(
            HouseholdProfileIneligibilityV1::ProfileConflicted,
        ),
        HouseholdProfileStateV1::LocalOnly
        | HouseholdProfileStateV1::PendingSync
        | HouseholdProfileStateV1::Synced => HouseholdProfileEligibilityV1::Eligible,
    }
}

/// Re-check the D2 product policy after core structural validation.
///
/// Core already rejects pending/synced non-owner records. Keeping the
/// application assertion explicit prevents a later domain relaxation from
/// silently activating non-owner persistence in a D2 client.
pub fn validate_d2_profile_policy_v1(state: &HouseholdStateV1) -> Result<(), HouseholdStateError> {
    state.validate()?;
    if state.members.iter().any(|member| {
        matches!(
            member.profile_state,
            HouseholdProfileStateV1::PendingSync | HouseholdProfileStateV1::Synced
        )
    }) {
        return Err(HouseholdStateError::InvalidProfileDocument);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthoritativeConsentStateV1 {
    Active(ConsentVersionV1),
    Absent,
    Malformed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerProfileRetryUnavailableReasonV1 {
    ConsentRequired,
    NoIntent,
    DuplicateIntent,
    WrongSubjectOrId,
    StaleRevision,
    ConsentResponseMalformed,
    ConsentVersionChangedRequiresNewSave,
    ConsentRevokedRegrantRequired,
    DefiniteFailureRequiresNewSave,
    ConflictedRequiresResolution,
    ModeOrAccountIneligible,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerProfileRetryEligibilityV1 {
    StartLocalOnlyAfterConsent,
    ResumeNeedsConsentCheck,
    ResumeNeedsRemoteBase,
    ResumeReadyToDispatch,
    ReconcileDispatchingOutcomeUnknown,
    ReconcileOutcomeUncertain,
    Unavailable {
        reason: OwnerProfileRetryUnavailableReasonV1,
    },
}

/// The closed set of retry actions that may cross an execution boundary.
///
/// An unavailable eligibility can never be represented by this type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerProfileRetryActionV1 {
    StartLocalOnlyAfterConsent,
    ResumeNeedsConsentCheck,
    ResumeNeedsRemoteBase,
    ResumeReadyToDispatch,
    ReconcileDispatchingOutcomeUnknown,
    ReconcileOutcomeUncertain,
}

impl OwnerProfileRetryEligibilityV1 {
    #[must_use]
    pub const fn available_action(self) -> Option<OwnerProfileRetryActionV1> {
        match self {
            Self::StartLocalOnlyAfterConsent => {
                Some(OwnerProfileRetryActionV1::StartLocalOnlyAfterConsent)
            }
            Self::ResumeNeedsConsentCheck => {
                Some(OwnerProfileRetryActionV1::ResumeNeedsConsentCheck)
            }
            Self::ResumeNeedsRemoteBase => Some(OwnerProfileRetryActionV1::ResumeNeedsRemoteBase),
            Self::ResumeReadyToDispatch => Some(OwnerProfileRetryActionV1::ResumeReadyToDispatch),
            Self::ReconcileDispatchingOutcomeUnknown => {
                Some(OwnerProfileRetryActionV1::ReconcileDispatchingOutcomeUnknown)
            }
            Self::ReconcileOutcomeUncertain => {
                Some(OwnerProfileRetryActionV1::ReconcileOutcomeUncertain)
            }
            Self::Unavailable { .. } => None,
        }
    }
}

/// Opaque three-revision authority passed from a read-only Profile panel to a
/// later explicit retry use case. Every field is revalidated against a fresh
/// repository load before mutation.
#[derive(Clone, Eq, PartialEq)]
pub struct OwnerSyncIntentHandleV1 {
    pub outbox_id: HouseholdOutboxId,
    pub expected_household_revision: HouseholdRevision,
    pub expected_profile_revision: ProfileRevision,
    pub expected_outbox_revision: OutboxRevision,
}

impl fmt::Debug for OwnerSyncIntentHandleV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OwnerSyncIntentHandleV1")
            .field(
                "expected_household_revision",
                &self.expected_household_revision,
            )
            .field("expected_profile_revision", &self.expected_profile_revision)
            .field("expected_outbox_revision", &self.expected_outbox_revision)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnerProfileActionEligibilityV1 {
    pub active_consent_version: Option<ConsentVersionV1>,
    pub retry: OwnerProfileRetryEligibilityV1,
    pub intent: Option<OwnerSyncIntentHandleV1>,
}

/// Construct the pure, read-only owner action state. This function never
/// grants consent, changes an intent, or dispatches network work.
#[must_use]
pub fn owner_profile_action_eligibility_v1(
    state: &HouseholdStateV1,
    consent: AuthoritativeConsentStateV1,
) -> OwnerProfileActionEligibilityV1 {
    let active_consent_version = match consent {
        AuthoritativeConsentStateV1::Active(version) => Some(version),
        AuthoritativeConsentStateV1::Absent | AuthoritativeConsentStateV1::Malformed => None,
    };
    if matches!(consent, AuthoritativeConsentStateV1::Malformed) {
        return unavailable_owner_action(
            active_consent_version,
            OwnerProfileRetryUnavailableReasonV1::ConsentResponseMalformed,
        );
    }

    let mut owner_records = state.outbox.iter().filter_map(|record| {
        let HouseholdProfileOutboxEntryV1::OwnerSync {
            version,
            target,
            intent,
        } = &record.entry
        else {
            return None;
        };
        Some((record, *version, target, intent))
    });
    let Some((record, version, target, intent)) = owner_records.next() else {
        return unavailable_owner_action(
            active_consent_version,
            OwnerProfileRetryUnavailableReasonV1::NoIntent,
        );
    };
    if owner_records.next().is_some() {
        return unavailable_owner_action(
            active_consent_version,
            OwnerProfileRetryUnavailableReasonV1::DuplicateIntent,
        );
    }
    let expected_outbox_id = HouseholdOutboxId::owner_sync(intent.intent_id);
    if version != 1
        || target != &HouseholdSubjectId::self_()
        || intent.subject != HouseholdSubjectId::self_()
        || !matches!(
            expected_outbox_id.as_ref(),
            Ok(expected) if expected == &record.outbox_id
        )
    {
        return unavailable_owner_action(
            active_consent_version,
            OwnerProfileRetryUnavailableReasonV1::WrongSubjectOrId,
        );
    }
    let Some(profile) = state
        .profiles
        .iter()
        .find(|profile| profile.subject == HouseholdSubjectId::self_())
    else {
        return unavailable_owner_action(
            active_consent_version,
            OwnerProfileRetryUnavailableReasonV1::StaleRevision,
        );
    };
    if intent.local_profile_revision != profile.profile_revision.get()
        || record.outbox_revision.get() != intent.intent_revision
    {
        return unavailable_owner_action(
            active_consent_version,
            OwnerProfileRetryUnavailableReasonV1::StaleRevision,
        );
    }

    let retry = match intent.phase {
        OwnerSyncIntentPhaseV1::LocalOnlyNoConsent => {
            if active_consent_version.is_some() {
                OwnerProfileRetryEligibilityV1::StartLocalOnlyAfterConsent
            } else {
                OwnerProfileRetryEligibilityV1::Unavailable {
                    reason: OwnerProfileRetryUnavailableReasonV1::ConsentRequired,
                }
            }
        }
        OwnerSyncIntentPhaseV1::NeedsConsentCheck => {
            OwnerProfileRetryEligibilityV1::ResumeNeedsConsentCheck
        }
        OwnerSyncIntentPhaseV1::NeedsRemoteBase => {
            OwnerProfileRetryEligibilityV1::ResumeNeedsRemoteBase
        }
        OwnerSyncIntentPhaseV1::ReadyToDispatch => {
            OwnerProfileRetryEligibilityV1::ResumeReadyToDispatch
        }
        OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown => {
            OwnerProfileRetryEligibilityV1::ReconcileDispatchingOutcomeUnknown
        }
        OwnerSyncIntentPhaseV1::OutcomeUncertain => {
            OwnerProfileRetryEligibilityV1::ReconcileOutcomeUncertain
        }
        OwnerSyncIntentPhaseV1::DefiniteFailure => OwnerProfileRetryEligibilityV1::Unavailable {
            reason: match intent.last_definite_error {
                Some(LastDefiniteOwnerSyncErrorV1::ConsentVersionChangedRequiresNewSave) => {
                    OwnerProfileRetryUnavailableReasonV1::ConsentVersionChangedRequiresNewSave
                }
                Some(LastDefiniteOwnerSyncErrorV1::ConsentRevokedRegrantRequired) => {
                    OwnerProfileRetryUnavailableReasonV1::ConsentRevokedRegrantRequired
                }
                _ => OwnerProfileRetryUnavailableReasonV1::DefiniteFailureRequiresNewSave,
            },
        },
        OwnerSyncIntentPhaseV1::Conflicted => OwnerProfileRetryEligibilityV1::Unavailable {
            reason: OwnerProfileRetryUnavailableReasonV1::ConflictedRequiresResolution,
        },
    };
    let action = retry.available_action();
    OwnerProfileActionEligibilityV1 {
        active_consent_version,
        retry,
        intent: action.map(|_| OwnerSyncIntentHandleV1 {
            outbox_id: record.outbox_id.clone(),
            expected_household_revision: state.revision,
            expected_profile_revision: profile.profile_revision,
            expected_outbox_revision: record.outbox_revision,
        }),
    }
}

fn unavailable_owner_action(
    active_consent_version: Option<ConsentVersionV1>,
    reason: OwnerProfileRetryUnavailableReasonV1,
) -> OwnerProfileActionEligibilityV1 {
    OwnerProfileActionEligibilityV1 {
        active_consent_version,
        retry: OwnerProfileRetryEligibilityV1::Unavailable { reason },
        intent: None,
    }
}
