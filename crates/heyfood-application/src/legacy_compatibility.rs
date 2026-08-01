//! Pure D2 native-household mode policy.
//!
//! Platform composition classifies account-bound artifacts while holding the
//! lifecycle/vault locks. This module consumes only a closed, already-verified
//! evidence class and makes the rollout decision without filesystem, broker,
//! migration, or network I/O.

use heyfood_core::NativeHouseholdRolloutV1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeHouseholdInitializationPhaseV1 {
    ReservedSource,
    ReadyToInitialize,
    UncommittedArtifacts,
    CommittedAwaitingFinalization,
}

/// Closed artifact classification produced after exact account/root/guard/key
/// and vault binding checks. A platform adapter must use `Contradictory` for
/// every unmatched, malformed, uncertain, or partially verified combination.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeHouseholdEvidenceV1 {
    NoNativeState,
    ResumableInitialization {
        phase: NativeHouseholdInitializationPhaseV1,
    },
    AbortingCleanup,
    ValidCommitted,
    RepairBlocked,
    PostLogout,
    Contradictory,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeHouseholdModeFactsV1 {
    pub teardown_journal_present: bool,
    pub evidence: NativeHouseholdEvidenceV1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeHouseholdCompletionModeV1 {
    NativeEnabled,
    NativeRollbackReadOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeHouseholdPostLogoutPolicyV1 {
    EphemeralSelfReadOnly,
    InitializeCleanSelfOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeHouseholdModeV1 {
    LegacyCompatibility,
    NativeEnable,
    ResumeNativeInitialization {
        phase: NativeHouseholdInitializationPhaseV1,
        completion: NativeHouseholdCompletionModeV1,
    },
    ResumeAbortingCleanup,
    NativeEnabled,
    NativeRollbackReadOnly,
    NativeRepairBlocked,
    PostLogoutClean {
        policy: NativeHouseholdPostLogoutPolicyV1,
    },
    ResumeTeardown,
}

impl NativeHouseholdModeV1 {
    /// Only the pre-provenance compatibility mode may read or recreate the
    /// released plaintext compatibility snapshot.
    #[must_use]
    pub const fn allows_released_compatibility_state(self) -> bool {
        matches!(self, Self::LegacyCompatibility)
    }

    /// Normal native household commits are legal only after exact committed
    /// provenance is open in the enabled rollout.
    #[must_use]
    pub const fn allows_native_household_commit(self) -> bool {
        matches!(self, Self::NativeEnabled)
    }

    /// These modes are startup/lifecycle actions rather than usable household
    /// sessions and must finish or fail closed before rendering account data.
    #[must_use]
    pub const fn requires_lifecycle_completion(self) -> bool {
        matches!(
            self,
            Self::NativeEnable
                | Self::ResumeNativeInitialization { .. }
                | Self::ResumeAbortingCleanup
                | Self::ResumeTeardown
        )
    }

    #[must_use]
    pub const fn is_native_read_only(self) -> bool {
        matches!(
            self,
            Self::NativeRollbackReadOnly
                | Self::NativeRepairBlocked
                | Self::PostLogoutClean {
                    policy: NativeHouseholdPostLogoutPolicyV1::EphemeralSelfReadOnly
                }
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeHouseholdModeErrorV1 {
    ContradictoryEvidence,
}

impl std::fmt::Display for NativeHouseholdModeErrorV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("native household state evidence is contradictory")
    }
}

impl std::error::Error for NativeHouseholdModeErrorV1 {}

/// Resolve the exact D2 rollout mode from evidence classified under the
/// account lifecycle lock. Teardown always resumes before ordinary mode
/// selection, and no existing native provenance can return to legacy import.
pub fn resolve_native_household_mode_v1(
    rollout: NativeHouseholdRolloutV1,
    facts: NativeHouseholdModeFactsV1,
) -> Result<NativeHouseholdModeV1, NativeHouseholdModeErrorV1> {
    if facts.teardown_journal_present {
        return Ok(NativeHouseholdModeV1::ResumeTeardown);
    }
    let enabled = rollout.is_enabled();
    match facts.evidence {
        NativeHouseholdEvidenceV1::NoNativeState if enabled => {
            Ok(NativeHouseholdModeV1::NativeEnable)
        }
        NativeHouseholdEvidenceV1::NoNativeState => Ok(NativeHouseholdModeV1::LegacyCompatibility),
        NativeHouseholdEvidenceV1::ResumableInitialization { phase } => {
            Ok(NativeHouseholdModeV1::ResumeNativeInitialization {
                phase,
                completion: if enabled {
                    NativeHouseholdCompletionModeV1::NativeEnabled
                } else {
                    NativeHouseholdCompletionModeV1::NativeRollbackReadOnly
                },
            })
        }
        NativeHouseholdEvidenceV1::AbortingCleanup => {
            Ok(NativeHouseholdModeV1::ResumeAbortingCleanup)
        }
        NativeHouseholdEvidenceV1::ValidCommitted if enabled => {
            Ok(NativeHouseholdModeV1::NativeEnabled)
        }
        NativeHouseholdEvidenceV1::ValidCommitted => {
            Ok(NativeHouseholdModeV1::NativeRollbackReadOnly)
        }
        NativeHouseholdEvidenceV1::RepairBlocked => Ok(NativeHouseholdModeV1::NativeRepairBlocked),
        NativeHouseholdEvidenceV1::PostLogout => Ok(NativeHouseholdModeV1::PostLogoutClean {
            policy: if enabled {
                NativeHouseholdPostLogoutPolicyV1::InitializeCleanSelfOnly
            } else {
                NativeHouseholdPostLogoutPolicyV1::EphemeralSelfReadOnly
            },
        }),
        NativeHouseholdEvidenceV1::Contradictory => {
            Err(NativeHouseholdModeErrorV1::ContradictoryEvidence)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(evidence: NativeHouseholdEvidenceV1) -> NativeHouseholdModeFactsV1 {
        NativeHouseholdModeFactsV1 {
            teardown_journal_present: false,
            evidence,
        }
    }

    #[test]
    fn only_absent_provenance_can_select_released_legacy_compatibility() {
        assert_eq!(
            resolve_native_household_mode_v1(
                NativeHouseholdRolloutV1::Disabled,
                facts(NativeHouseholdEvidenceV1::NoNativeState),
            )
            .unwrap(),
            NativeHouseholdModeV1::LegacyCompatibility
        );
        for evidence in [
            NativeHouseholdEvidenceV1::ResumableInitialization {
                phase: NativeHouseholdInitializationPhaseV1::ReservedSource,
            },
            NativeHouseholdEvidenceV1::AbortingCleanup,
            NativeHouseholdEvidenceV1::ValidCommitted,
            NativeHouseholdEvidenceV1::RepairBlocked,
            NativeHouseholdEvidenceV1::PostLogout,
        ] {
            let mode = resolve_native_household_mode_v1(
                NativeHouseholdRolloutV1::Disabled,
                facts(evidence),
            )
            .unwrap();
            assert_ne!(mode, NativeHouseholdModeV1::LegacyCompatibility);
            assert!(!mode.allows_released_compatibility_state());
        }
    }

    #[test]
    fn initialization_resumes_offline_toward_the_flag_selected_native_mode() {
        for phase in [
            NativeHouseholdInitializationPhaseV1::ReservedSource,
            NativeHouseholdInitializationPhaseV1::ReadyToInitialize,
            NativeHouseholdInitializationPhaseV1::UncommittedArtifacts,
            NativeHouseholdInitializationPhaseV1::CommittedAwaitingFinalization,
        ] {
            for (rollout, completion) in [
                (
                    NativeHouseholdRolloutV1::Disabled,
                    NativeHouseholdCompletionModeV1::NativeRollbackReadOnly,
                ),
                (
                    NativeHouseholdRolloutV1::Enabled,
                    NativeHouseholdCompletionModeV1::NativeEnabled,
                ),
            ] {
                let mode = resolve_native_household_mode_v1(
                    rollout,
                    facts(NativeHouseholdEvidenceV1::ResumableInitialization { phase }),
                )
                .unwrap();
                assert_eq!(
                    mode,
                    NativeHouseholdModeV1::ResumeNativeInitialization { phase, completion }
                );
                assert!(mode.requires_lifecycle_completion());
                assert!(!mode.allows_native_household_commit());
            }
        }
    }

    #[test]
    fn committed_repair_and_post_logout_modes_are_closed() {
        assert_eq!(
            resolve_native_household_mode_v1(
                NativeHouseholdRolloutV1::Enabled,
                facts(NativeHouseholdEvidenceV1::ValidCommitted),
            )
            .unwrap(),
            NativeHouseholdModeV1::NativeEnabled
        );
        assert_eq!(
            resolve_native_household_mode_v1(
                NativeHouseholdRolloutV1::Disabled,
                facts(NativeHouseholdEvidenceV1::ValidCommitted),
            )
            .unwrap(),
            NativeHouseholdModeV1::NativeRollbackReadOnly
        );
        assert_eq!(
            resolve_native_household_mode_v1(
                NativeHouseholdRolloutV1::Enabled,
                facts(NativeHouseholdEvidenceV1::RepairBlocked),
            )
            .unwrap(),
            NativeHouseholdModeV1::NativeRepairBlocked
        );
        assert_eq!(
            resolve_native_household_mode_v1(
                NativeHouseholdRolloutV1::Disabled,
                facts(NativeHouseholdEvidenceV1::PostLogout),
            )
            .unwrap(),
            NativeHouseholdModeV1::PostLogoutClean {
                policy: NativeHouseholdPostLogoutPolicyV1::EphemeralSelfReadOnly
            }
        );
        assert_eq!(
            resolve_native_household_mode_v1(
                NativeHouseholdRolloutV1::Enabled,
                facts(NativeHouseholdEvidenceV1::PostLogout),
            )
            .unwrap(),
            NativeHouseholdModeV1::PostLogoutClean {
                policy: NativeHouseholdPostLogoutPolicyV1::InitializeCleanSelfOnly
            }
        );
    }

    #[test]
    fn teardown_preempts_mode_selection_and_contradictions_fail_closed() {
        let teardown = NativeHouseholdModeFactsV1 {
            teardown_journal_present: true,
            evidence: NativeHouseholdEvidenceV1::Contradictory,
        };
        assert_eq!(
            resolve_native_household_mode_v1(NativeHouseholdRolloutV1::Enabled, teardown).unwrap(),
            NativeHouseholdModeV1::ResumeTeardown
        );
        assert_eq!(
            resolve_native_household_mode_v1(
                NativeHouseholdRolloutV1::Enabled,
                facts(NativeHouseholdEvidenceV1::Contradictory),
            ),
            Err(NativeHouseholdModeErrorV1::ContradictoryEvidence)
        );
    }

    #[test]
    fn write_and_legacy_authority_are_never_shared() {
        let modes = [
            NativeHouseholdModeV1::LegacyCompatibility,
            NativeHouseholdModeV1::NativeEnable,
            NativeHouseholdModeV1::ResumeAbortingCleanup,
            NativeHouseholdModeV1::NativeEnabled,
            NativeHouseholdModeV1::NativeRollbackReadOnly,
            NativeHouseholdModeV1::NativeRepairBlocked,
            NativeHouseholdModeV1::PostLogoutClean {
                policy: NativeHouseholdPostLogoutPolicyV1::InitializeCleanSelfOnly,
            },
            NativeHouseholdModeV1::ResumeTeardown,
        ];
        for mode in modes {
            assert!(
                !(mode.allows_native_household_commit()
                    && mode.allows_released_compatibility_state())
            );
        }
    }
}
