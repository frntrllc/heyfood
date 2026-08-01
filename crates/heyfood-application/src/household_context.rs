//! Live, revision-bound household context construction.

use std::fmt;

use heyfood_core::{
    AccountId, HouseholdLifecycleV1, HouseholdProfileRecordV1, HouseholdRevision, HouseholdScope,
    HouseholdStateError, HouseholdStateV1, HouseholdSubjectId, ProfileRevision,
};
use serde_json::Value;

use crate::household_profile_policy::{
    HouseholdProfileEligibilityV1, HouseholdProfileIneligibilityV1, HouseholdProfileOperationV1,
    household_profile_eligibility_v1,
};

/// The reviewed target authority captured before a potentially mutating
/// dispatch. It is deliberately account- and revision-bound.
#[derive(Clone, Eq, PartialEq)]
pub struct PreparedHouseholdTargetV1 {
    pub account_binding: AccountId,
    pub household_revision: HouseholdRevision,
    pub scope: HouseholdScope,
}

impl PreparedHouseholdTargetV1 {
    pub fn from_active_scope(state: &HouseholdStateV1) -> Result<Self, HouseholdContextErrorV1> {
        state.validate().map_err(HouseholdContextErrorV1::Domain)?;
        validate_scope_eligibility_v1(
            state,
            &state.active_scope,
            HouseholdProfileOperationV1::PersonalizedContext,
        )?;
        Ok(Self {
            account_binding: state.account_binding.clone(),
            household_revision: state.revision,
            scope: state.active_scope.clone(),
        })
    }

    pub fn for_scope(
        state: &HouseholdStateV1,
        scope: HouseholdScope,
        operation: HouseholdProfileOperationV1,
    ) -> Result<Self, HouseholdContextErrorV1> {
        state.validate().map_err(HouseholdContextErrorV1::Domain)?;
        validate_scope_eligibility_v1(state, &scope, operation)?;
        Ok(Self {
            account_binding: state.account_binding.clone(),
            household_revision: state.revision,
            scope,
        })
    }

    /// Revalidate D0's stable target against a fresh live load. No caller may
    /// silently re-resolve a name or fall back to the owner.
    pub fn assert_current(
        &self,
        state: &HouseholdStateV1,
        operation: HouseholdProfileOperationV1,
    ) -> Result<(), HouseholdContextErrorV1> {
        state.validate().map_err(HouseholdContextErrorV1::Domain)?;
        if self.account_binding != state.account_binding {
            return Err(HouseholdContextErrorV1::AccountMismatch);
        }
        if self.household_revision != state.revision {
            return Err(HouseholdContextErrorV1::StaleRevision);
        }
        validate_scope_eligibility_v1(state, &self.scope, operation)
    }
}

impl fmt::Debug for PreparedHouseholdTargetV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedHouseholdTargetV1")
            .field("household_revision", &self.household_revision)
            .field("scope_kind", &scope_kind(&self.scope))
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct HouseholdSubjectContextV1 {
    pub subject: HouseholdSubjectId,
    pub profile_revision: ProfileRevision,
    pub effective_profile: Value,
}

impl fmt::Debug for HouseholdSubjectContextV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdSubjectContextV1")
            .field("subject_kind", &subject_kind(&self.subject))
            .field("profile_revision", &self.profile_revision)
            .field("effective_profile", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct HouseholdContextSnapshotV1 {
    pub household_revision: HouseholdRevision,
    pub scope: HouseholdScope,
    pub subjects: Vec<HouseholdSubjectContextV1>,
}

impl fmt::Debug for HouseholdContextSnapshotV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdContextSnapshotV1")
            .field("household_revision", &self.household_revision)
            .field("scope_kind", &scope_kind(&self.scope))
            .field("subject_count", &self.subjects.len())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HouseholdContextErrorV1 {
    AccountMismatch,
    StaleRevision,
    UnknownSubject,
    ArchivedSubject,
    ProfileIncomplete,
    ProfileConflicted,
    EveryoneRequiresTwoEligibleSubjects,
    Domain(HouseholdStateError),
}

impl HouseholdContextErrorV1 {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::AccountMismatch => "household_account_mismatch",
            Self::StaleRevision => "household_revision_stale",
            Self::UnknownSubject => "household_subject_unknown",
            Self::ArchivedSubject => "household_subject_archived",
            Self::ProfileIncomplete => "profile_incomplete",
            Self::ProfileConflicted => "profile_conflicted",
            Self::EveryoneRequiresTwoEligibleSubjects => {
                "household_everyone_requires_two_eligible_subjects"
            }
            Self::Domain(_) => "household_state_invalid",
        }
    }
}

impl fmt::Display for HouseholdContextErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::AccountMismatch => "household context belongs to another account",
            Self::StaleRevision => "household context revision changed",
            Self::UnknownSubject => "household subject is unknown",
            Self::ArchivedSubject => "household subject is archived",
            Self::ProfileIncomplete => "household profile is incomplete",
            Self::ProfileConflicted => "household profile requires conflict resolution",
            Self::EveryoneRequiresTwoEligibleSubjects => {
                "everyone context requires at least two eligible active subjects"
            }
            Self::Domain(_) => "household state is invalid",
        })
    }
}

impl std::error::Error for HouseholdContextErrorV1 {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Domain(error) => Some(error),
            _ => None,
        }
    }
}

/// Build a personalized context from the exact prepared target and live
/// revision. Missing member data is a typed refusal; owner data is never used
/// as a substitute.
pub fn resolve_personalized_context_v1(
    state: &HouseholdStateV1,
    prepared: &PreparedHouseholdTargetV1,
) -> Result<HouseholdContextSnapshotV1, HouseholdContextErrorV1> {
    prepared.assert_current(state, HouseholdProfileOperationV1::PersonalizedContext)?;
    let subjects = match &prepared.scope {
        HouseholdScope::Subject(subject) => vec![resolve_subject_context(state, subject)?],
        HouseholdScope::Everyone => {
            let mut contexts = Vec::new();
            if household_profile_eligibility_v1(
                state,
                &HouseholdSubjectId::self_(),
                HouseholdProfileOperationV1::PersonalizedContext,
            ) == HouseholdProfileEligibilityV1::Eligible
            {
                contexts.push(resolve_subject_context(
                    state,
                    &HouseholdSubjectId::self_(),
                )?);
            }
            for member in &state.members {
                if member.lifecycle != HouseholdLifecycleV1::Active {
                    continue;
                }
                let subject = HouseholdSubjectId::member(member.member_id.clone());
                if household_profile_eligibility_v1(
                    state,
                    &subject,
                    HouseholdProfileOperationV1::PersonalizedContext,
                ) == HouseholdProfileEligibilityV1::Eligible
                {
                    contexts.push(resolve_subject_context(state, &subject)?);
                }
            }
            if contexts.len() < 2 {
                return Err(HouseholdContextErrorV1::EveryoneRequiresTwoEligibleSubjects);
            }
            contexts
        }
    };
    Ok(HouseholdContextSnapshotV1 {
        household_revision: state.revision,
        scope: prepared.scope.clone(),
        subjects,
    })
}

pub fn validate_scope_eligibility_v1(
    state: &HouseholdStateV1,
    scope: &HouseholdScope,
    operation: HouseholdProfileOperationV1,
) -> Result<(), HouseholdContextErrorV1> {
    match scope {
        HouseholdScope::Subject(subject) => {
            map_eligibility(household_profile_eligibility_v1(state, subject, operation))
        }
        HouseholdScope::Everyone => {
            let mut eligible = usize::from(
                household_profile_eligibility_v1(state, &HouseholdSubjectId::self_(), operation)
                    == HouseholdProfileEligibilityV1::Eligible,
            );
            eligible += state
                .members
                .iter()
                .filter(|member| member.lifecycle == HouseholdLifecycleV1::Active)
                .filter(|member| {
                    household_profile_eligibility_v1(
                        state,
                        &HouseholdSubjectId::member(member.member_id.clone()),
                        operation,
                    ) == HouseholdProfileEligibilityV1::Eligible
                })
                .count();
            if eligible < 2 {
                Err(HouseholdContextErrorV1::EveryoneRequiresTwoEligibleSubjects)
            } else {
                Ok(())
            }
        }
    }
}

fn resolve_subject_context(
    state: &HouseholdStateV1,
    subject: &HouseholdSubjectId,
) -> Result<HouseholdSubjectContextV1, HouseholdContextErrorV1> {
    map_eligibility(household_profile_eligibility_v1(
        state,
        subject,
        HouseholdProfileOperationV1::PersonalizedContext,
    ))?;
    let profile =
        profile_for_subject(state, subject).ok_or(HouseholdContextErrorV1::ProfileIncomplete)?;
    let effective_profile = profile
        .document
        .effective_profile()
        .map_err(HouseholdContextErrorV1::Domain)?
        .ok_or(HouseholdContextErrorV1::ProfileIncomplete)?;
    Ok(HouseholdSubjectContextV1 {
        subject: subject.clone(),
        profile_revision: profile.profile_revision,
        effective_profile,
    })
}

fn profile_for_subject<'a>(
    state: &'a HouseholdStateV1,
    subject: &HouseholdSubjectId,
) -> Option<&'a HouseholdProfileRecordV1> {
    state
        .profiles
        .iter()
        .find(|profile| &profile.subject == subject)
}

fn map_eligibility(
    eligibility: HouseholdProfileEligibilityV1,
) -> Result<(), HouseholdContextErrorV1> {
    match eligibility {
        HouseholdProfileEligibilityV1::Eligible => Ok(()),
        HouseholdProfileEligibilityV1::Ineligible(reason) => Err(match reason {
            HouseholdProfileIneligibilityV1::UnknownSubject => {
                HouseholdContextErrorV1::UnknownSubject
            }
            HouseholdProfileIneligibilityV1::ArchivedSubject => {
                HouseholdContextErrorV1::ArchivedSubject
            }
            HouseholdProfileIneligibilityV1::ProfileIncomplete => {
                HouseholdContextErrorV1::ProfileIncomplete
            }
            HouseholdProfileIneligibilityV1::ProfileConflicted => {
                HouseholdContextErrorV1::ProfileConflicted
            }
            HouseholdProfileIneligibilityV1::MinorPersistentSyncBlocked
            | HouseholdProfileIneligibilityV1::UnknownAgePersistentSyncBlocked
            | HouseholdProfileIneligibilityV1::NonOwnerPersistentSyncDeferred => {
                HouseholdContextErrorV1::ProfileIncomplete
            }
        }),
    }
}

fn scope_kind(scope: &HouseholdScope) -> &'static str {
    match scope {
        HouseholdScope::Subject(subject) => subject_kind(subject),
        HouseholdScope::Everyone => "everyone",
    }
}

fn subject_kind(subject: &HouseholdSubjectId) -> &'static str {
    match subject {
        HouseholdSubjectId::Self_ => "self",
        HouseholdSubjectId::Member(_) => "member",
    }
}
