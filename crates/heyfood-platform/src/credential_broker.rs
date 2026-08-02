//! Killable native-keyring brokers.
//!
//! Secrets cross process boundaries only through inherited anonymous pipes.
//! The household contracts in this module remain available without the
//! `native-credentials` feature so vault-format and in-memory store tests never
//! acquire an accidental file-key fallback.

use std::collections::HashMap;
use std::fmt;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use heyfood_application::{BoxFuture, PortError};
use heyfood_core::{
    CanonicalDigestV1, CanonicalTimestampV1, CommitId, CompatibilityJsonLimitsV1,
    LegacyPythonSnapshotProvenanceV1, parse_bounded_typed_json_v1, to_canonical_bytes_v1,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::household_vault::{
    HouseholdAccountSlotV1, HouseholdLifecycleLease, HouseholdVaultLease,
};

/// The released per-operation broker document ceiling.
pub const MAX_BROKER_DOCUMENT_BYTES: usize = 16 * 1024;
/// The one larger response allowed exclusively for legacy-household loading.
pub const MAX_LEGACY_HOUSEHOLD_BROKER_RESPONSE_BYTES: usize = (4 * 1024 * 1024) + (64 * 1024);
pub const HOUSEHOLD_KEYRING_SERVICE_V1: &str = "ai.frntr.heyfood.household.v1";
pub const LEGACY_PYTHON_KEYRING_SERVICE: &str = "heyfood-cli";
const COMMIT_EVIDENCE_ROOT_DERIVATION_V1: &[u8] = b"heyfood.household.commit-evidence.root.v1";
const COMMIT_EVIDENCE_PROPOSAL_REF_HASH_V1: &[u8] =
    b"heyfood.household.commit-evidence.proposal-ref.v1";
pub(crate) const COMMIT_EVIDENCE_RETENTION_SECONDS: u64 = 30 * 24 * 60 * 60;
const MAX_HOUSEHOLD_COMMIT_EVIDENCE_RECORDS: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HouseholdBrokerOperationV1 {
    SecureStoreProbe,
    KeyLoad,
    KeyInitialize,
    KeyAbortInitialization,
    KeyReplace,
    KeyDelete,
    KeyVerifyAbsent,
    MigrationGuardLoad,
    MigrationGuardCompareExchange,
    LegacyPythonHouseholdProbe,
    LegacyPythonHouseholdLoad,
    LegacyPythonCredentialsScrubAndVerify,
}

impl HouseholdBrokerOperationV1 {
    #[must_use]
    pub const fn action(self) -> &'static str {
        match self {
            Self::SecureStoreProbe => "household-secure-store-probe",
            Self::KeyLoad => "household-key-load",
            Self::KeyInitialize => "household-key-initialize",
            Self::KeyAbortInitialization => "household-key-abort-initialization",
            Self::KeyReplace => "household-key-replace",
            Self::KeyDelete => "household-key-delete",
            Self::KeyVerifyAbsent => "household-key-verify-absent",
            Self::MigrationGuardLoad => "household-migration-guard-load",
            Self::MigrationGuardCompareExchange => "household-migration-guard-compare-exchange",
            Self::LegacyPythonHouseholdProbe => "legacy-python-household-probe",
            Self::LegacyPythonHouseholdLoad => "legacy-python-household-load",
            Self::LegacyPythonCredentialsScrubAndVerify => {
                "legacy-python-credentials-scrub-and-verify"
            }
        }
    }

    #[must_use]
    pub const fn response_limit(self) -> usize {
        match self {
            Self::LegacyPythonHouseholdLoad => MAX_LEGACY_HOUSEHOLD_BROKER_RESPONSE_BYTES,
            _ => MAX_BROKER_DOCUMENT_BYTES,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HouseholdKeyringLocatorsV1 {
    pub service: &'static str,
    pub key_bundle_username: String,
    pub migration_guard_username: String,
}

impl HouseholdKeyringLocatorsV1 {
    pub fn from_account_slot(slot: &HouseholdAccountSlotV1) -> Result<Self, PortError> {
        let locator = hex_digest(slot.account_locator_digest());
        let value = Self {
            service: HOUSEHOLD_KEYRING_SERVICE_V1,
            key_bundle_username: format!("key-{locator}"),
            migration_guard_username: format!("migration-guard-{locator}"),
        };
        if value.key_bundle_username.len() != 68 || value.migration_guard_username.len() != 80 {
            return Err(PortError::new(
                "household_keyring_locator",
                "household keyring locator is invalid",
            ));
        }
        Ok(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyPythonKeyringLocatorV1 {
    pub service: &'static str,
    pub username: String,
}

impl LegacyPythonKeyringLocatorV1 {
    pub fn from_resolved_config_path_bytes(path: &[u8]) -> Result<Self, PortError> {
        if path.is_empty() || path.first() != Some(&b'/') || path.last() == Some(&b'/') {
            return Err(PortError::new(
                "legacy_python_keyring_locator",
                "legacy Python config locator is invalid",
            ));
        }
        let digest = Sha256::digest(path);
        let username = format!("config-{}", hex_prefix(&digest, 20));
        if username.len() != 27 {
            return Err(PortError::new(
                "legacy_python_keyring_locator",
                "legacy Python keyring locator is invalid",
            ));
        }
        Ok(Self {
            service: LEGACY_PYTHON_KEYRING_SERVICE,
            username,
        })
    }

    /// Canonical identity of the exact historical service/username pair.
    ///
    /// The raw path and keyring target never need to survive migration. This
    /// digest is safe to retain in the migration guard and source manifest.
    pub fn locator_digest(&self) -> Result<CanonicalDigestV1, PortError> {
        #[derive(Serialize)]
        #[serde(deny_unknown_fields)]
        struct LocatorIdentity<'a> {
            service: &'a str,
            username: &'a str,
        }

        let canonical = to_canonical_bytes_v1(&LocatorIdentity {
            service: self.service,
            username: &self.username,
        })
        .map_err(|_| {
            PortError::new(
                "legacy_python_keyring_locator",
                "legacy Python keyring locator is invalid",
            )
        })?;
        Ok(CanonicalDigestV1::from_bytes(
            Sha256::digest(canonical).into(),
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct KeyId(Uuid);

impl KeyId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for KeyId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct KeyBundleRevision(u64);

impl KeyBundleRevision {
    pub fn new(value: u64) -> Result<Self, PortError> {
        if value == 0 {
            return Err(PortError::new(
                "household_key_bundle_revision",
                "household key-bundle revision must be positive",
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn checked_next(self) -> Result<Self, PortError> {
        self.0
            .checked_add(1)
            .ok_or_else(|| {
                PortError::new(
                    "household_key_bundle_revision",
                    "household key-bundle revision is exhausted",
                )
            })
            .and_then(Self::new)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HouseholdKeyBundlePhase {
    Initializing,
    Stable,
    Rewriting,
}

#[derive(Clone, Eq, PartialEq)]
pub struct HouseholdKeyMaterial {
    bytes: Zeroizing<[u8; 32]>,
}

impl HouseholdKeyMaterial {
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self {
            bytes: Zeroizing::new(bytes),
        }
    }

    pub fn generate() -> Result<Self, PortError> {
        use chacha20poly1305::aead::Generate as _;

        <[u8; 32]>::try_generate()
            .map(|bytes| Self {
                bytes: Zeroizing::new(bytes),
            })
            .map_err(|_| {
                PortError::new(
                    "household_key_generation",
                    "household key material could not be generated",
                )
            })
    }

    pub(crate) fn expose(&self) -> &[u8; 32] {
        &self.bytes
    }

    #[cfg(feature = "native-credentials")]
    fn from_zeroizing(bytes: Zeroizing<[u8; 32]>) -> Self {
        Self { bytes }
    }
}

impl fmt::Debug for HouseholdKeyMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HouseholdKeyMaterial([REDACTED])")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct HouseholdKeyBundle {
    pub account_digest: [u8; 32],
    pub native_root_instance_digest: [u8; 32],
    pub account_locator_digest: [u8; 32],
    pub revision: KeyBundleRevision,
    pub active_key_id: KeyId,
    pub active_key: HouseholdKeyMaterial,
    commit_evidence_key: HouseholdKeyMaterial,
    commit_evidence_records: Vec<HouseholdCommitEvidenceRecordV1>,
    pub previous_key: Option<(KeyId, HouseholdKeyMaterial)>,
    pub initialization_id: Option<Uuid>,
    pub initial_commit_id: Option<Uuid>,
    pub initial_effect_fingerprint: Option<[u8; 32]>,
    pub initial_state_digest: Option<[u8; 32]>,
    pub rotation_id: Option<Uuid>,
    pub phase: HouseholdKeyBundlePhase,
}

impl HouseholdKeyBundle {
    #[allow(clippy::too_many_arguments)]
    pub fn initializing(
        slot: &HouseholdAccountSlotV1,
        revision: KeyBundleRevision,
        active_key_id: KeyId,
        active_key: HouseholdKeyMaterial,
        initialization_id: Uuid,
        initial_commit_id: Uuid,
        initial_effect_fingerprint: [u8; 32],
        initial_state_digest: [u8; 32],
    ) -> Self {
        let commit_evidence_key = derive_commit_evidence_root_key(&active_key);
        Self {
            account_digest: slot.account_digest(),
            native_root_instance_digest: slot.native_root_instance_digest(),
            account_locator_digest: slot.account_locator_digest(),
            revision,
            active_key_id,
            active_key,
            commit_evidence_key,
            commit_evidence_records: Vec::new(),
            previous_key: None,
            initialization_id: Some(initialization_id),
            initial_commit_id: Some(initial_commit_id),
            initial_effect_fingerprint: Some(initial_effect_fingerprint),
            initial_state_digest: Some(initial_state_digest),
            rotation_id: None,
            phase: HouseholdKeyBundlePhase::Initializing,
        }
    }

    pub fn stable(
        slot: &HouseholdAccountSlotV1,
        revision: KeyBundleRevision,
        active_key_id: KeyId,
        active_key: HouseholdKeyMaterial,
    ) -> Self {
        let commit_evidence_key = derive_commit_evidence_root_key(&active_key);
        Self {
            account_digest: slot.account_digest(),
            native_root_instance_digest: slot.native_root_instance_digest(),
            account_locator_digest: slot.account_locator_digest(),
            revision,
            active_key_id,
            active_key,
            commit_evidence_key,
            commit_evidence_records: Vec::new(),
            previous_key: None,
            initialization_id: None,
            initial_commit_id: None,
            initial_effect_fingerprint: None,
            initial_state_digest: None,
            rotation_id: None,
            phase: HouseholdKeyBundlePhase::Stable,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn rewriting(
        slot: &HouseholdAccountSlotV1,
        revision: KeyBundleRevision,
        active_key_id: KeyId,
        active_key: HouseholdKeyMaterial,
        previous: &Self,
        rotation_id: Uuid,
    ) -> Result<Self, PortError> {
        previous.validate_for(slot)?;
        if previous.phase != HouseholdKeyBundlePhase::Stable
            || revision.get() != previous.revision.checked_next()?.get()
        {
            return Err(PortError::new(
                "household_key_bundle_invalid",
                "household key rotation requires the exact stable predecessor",
            ));
        }
        Ok(Self {
            account_digest: slot.account_digest(),
            native_root_instance_digest: slot.native_root_instance_digest(),
            account_locator_digest: slot.account_locator_digest(),
            revision,
            active_key_id,
            active_key,
            commit_evidence_key: previous.commit_evidence_key.clone(),
            commit_evidence_records: previous.commit_evidence_records.clone(),
            previous_key: Some((previous.active_key_id, previous.active_key.clone())),
            initialization_id: None,
            initial_commit_id: None,
            initial_effect_fingerprint: None,
            initial_state_digest: None,
            rotation_id: Some(rotation_id),
            phase: HouseholdKeyBundlePhase::Rewriting,
        })
    }

    /// Finalize initialization or rotation without replacing the durable,
    /// non-rotating authority used for commit reconciliation.
    #[allow(clippy::too_many_arguments)]
    pub fn stabilized(
        &self,
        slot: &HouseholdAccountSlotV1,
        revision: KeyBundleRevision,
    ) -> Result<Self, PortError> {
        self.validate_for(slot)?;
        if !matches!(
            self.phase,
            HouseholdKeyBundlePhase::Initializing | HouseholdKeyBundlePhase::Rewriting
        ) || revision.get() != self.revision.checked_next()?.get()
        {
            return Err(PortError::new(
                "household_key_bundle_invalid",
                "household key stabilization requires the exact non-stable predecessor",
            ));
        }
        Ok(Self {
            account_digest: slot.account_digest(),
            native_root_instance_digest: slot.native_root_instance_digest(),
            account_locator_digest: slot.account_locator_digest(),
            revision,
            active_key_id: self.active_key_id,
            active_key: self.active_key.clone(),
            commit_evidence_key: self.commit_evidence_key.clone(),
            commit_evidence_records: self.commit_evidence_records.clone(),
            previous_key: None,
            initialization_id: None,
            initial_commit_id: None,
            initial_effect_fingerprint: None,
            initial_state_digest: None,
            rotation_id: None,
            phase: HouseholdKeyBundlePhase::Stable,
        })
    }

    pub(crate) fn reserve_commit_evidence(
        &self,
        proposal_ref: Uuid,
        commit_id: CommitId,
        now_unix_seconds: u64,
        applied_commit_ids: &[CommitId],
    ) -> Result<Self, PortError> {
        if self.phase != HouseholdKeyBundlePhase::Stable {
            return Err(PortError::new(
                "household_key_phase",
                "commit evidence reservation requires a stable household key bundle",
            ));
        }
        let mut replacement = self.clone();
        replacement.commit_evidence_records.retain(|record| {
            record.expires_at_unix_seconds > now_unix_seconds
                && !applied_commit_ids.contains(&record.commit_id)
        });
        if let Some(state) =
            replacement.commit_evidence_record(proposal_ref, commit_id, now_unix_seconds)
        {
            return match state {
                HouseholdCommitEvidenceStateV1::Reserved => {
                    if replacement == *self {
                        Ok(self.clone())
                    } else {
                        replacement.revision = self.revision.checked_next()?;
                        Ok(replacement)
                    }
                }
                HouseholdCommitEvidenceStateV1::Denied => {
                    Err(commit_evidence_record_mismatch_error())
                }
            };
        }
        let proposal_ref_hash = commit_evidence_proposal_ref_hash(proposal_ref);
        if replacement.commit_evidence_records.iter().any(|record| {
            record.proposal_ref_hash == proposal_ref_hash || record.commit_id == commit_id
        }) {
            return Err(PortError::new(
                "household_commit_evidence_conflict",
                "household commit evidence identity is already reserved",
            ));
        }
        if replacement.commit_evidence_records.len() >= MAX_HOUSEHOLD_COMMIT_EVIDENCE_RECORDS {
            return Err(PortError::new(
                "household_commit_evidence_capacity",
                "household commit evidence ledger is full",
            ));
        }
        replacement.revision = self.revision.checked_next()?;
        replacement
            .commit_evidence_records
            .push(HouseholdCommitEvidenceRecordV1 {
                proposal_ref_hash,
                commit_id,
                state: HouseholdCommitEvidenceStateV1::Reserved,
                expires_at_unix_seconds: now_unix_seconds
                    .checked_add(COMMIT_EVIDENCE_RETENTION_SECONDS)
                    .ok_or_else(commit_evidence_retention_error)?,
            });
        replacement.commit_evidence_records.sort_by(|left, right| {
            left.commit_id
                .as_uuid()
                .as_bytes()
                .cmp(right.commit_id.as_uuid().as_bytes())
        });
        Ok(replacement)
    }

    pub(crate) fn deny_reserved_commit(
        &self,
        proposal_ref: Uuid,
        commit_id: CommitId,
        now_unix_seconds: u64,
    ) -> Result<Self, PortError> {
        let proposal_ref_hash = commit_evidence_proposal_ref_hash(proposal_ref);
        let Some(index) = self.commit_evidence_records.iter().position(|record| {
            record.proposal_ref_hash == proposal_ref_hash
                && record.commit_id == commit_id
                && record.expires_at_unix_seconds > now_unix_seconds
        }) else {
            return Err(commit_evidence_record_mismatch_error());
        };
        if self.commit_evidence_records[index].state == HouseholdCommitEvidenceStateV1::Denied {
            return Ok(self.clone());
        }
        let mut replacement = self.clone();
        replacement.revision = self.revision.checked_next()?;
        replacement.commit_evidence_records[index].state = HouseholdCommitEvidenceStateV1::Denied;
        Ok(replacement)
    }

    pub(crate) fn commit_evidence_key(&self) -> &HouseholdKeyMaterial {
        &self.commit_evidence_key
    }

    pub(crate) fn commit_evidence_record(
        &self,
        proposal_ref: Uuid,
        commit_id: CommitId,
        now_unix_seconds: u64,
    ) -> Option<HouseholdCommitEvidenceStateV1> {
        let proposal_ref_hash = commit_evidence_proposal_ref_hash(proposal_ref);
        self.commit_evidence_records
            .iter()
            .find(|record| {
                record.proposal_ref_hash == proposal_ref_hash
                    && record.commit_id == commit_id
                    && record.expires_at_unix_seconds > now_unix_seconds
            })
            .map(|record| record.state)
    }

    pub(crate) fn denies_commit(&self, commit_id: CommitId, now_unix_seconds: u64) -> bool {
        self.commit_evidence_records.iter().any(|record| {
            record.commit_id == commit_id
                && record.state == HouseholdCommitEvidenceStateV1::Denied
                && record.expires_at_unix_seconds > now_unix_seconds
        })
    }

    pub(crate) fn release_reserved_commit(
        &self,
        proposal_ref: Uuid,
        commit_id: CommitId,
        now_unix_seconds: u64,
    ) -> Result<Self, PortError> {
        if self.commit_evidence_record(proposal_ref, commit_id, now_unix_seconds)
            != Some(HouseholdCommitEvidenceStateV1::Reserved)
        {
            return Err(commit_evidence_record_mismatch_error());
        }
        let proposal_ref_hash = commit_evidence_proposal_ref_hash(proposal_ref);
        let mut replacement = self.clone();
        replacement.revision = self.revision.checked_next()?;
        replacement.commit_evidence_records.retain(|record| {
            record.proposal_ref_hash != proposal_ref_hash || record.commit_id != commit_id
        });
        Ok(replacement)
    }

    pub fn validate_for(&self, slot: &HouseholdAccountSlotV1) -> Result<(), PortError> {
        if self.account_digest != slot.account_digest()
            || self.native_root_instance_digest != slot.native_root_instance_digest()
            || self.account_locator_digest != slot.account_locator_digest()
        {
            return Err(PortError::new(
                "household_key_account_mismatch",
                "household key bundle does not match the requested account slot",
            ));
        }
        if self.active_key_id.as_uuid().is_nil()
            || self
                .previous_key
                .as_ref()
                .is_some_and(|(key_id, _)| key_id.as_uuid().is_nil())
        {
            return Err(PortError::new(
                "household_key_bundle_invalid",
                "household key bundle contains an invalid key ID",
            ));
        }
        if self.commit_evidence_records.len() > MAX_HOUSEHOLD_COMMIT_EVIDENCE_RECORDS
            || self.commit_evidence_records.windows(2).any(|pair| {
                pair[0].commit_id.as_uuid().as_bytes() >= pair[1].commit_id.as_uuid().as_bytes()
            })
            || self.commit_evidence_records.iter().any(|record| {
                record.proposal_ref_hash == [0_u8; 32]
                    || record.commit_id.as_uuid().is_nil()
                    || record.expires_at_unix_seconds == 0
            })
            || self
                .commit_evidence_records
                .iter()
                .enumerate()
                .any(|(index, record)| {
                    self.commit_evidence_records[index + 1..]
                        .iter()
                        .any(|later| later.proposal_ref_hash == record.proposal_ref_hash)
                })
        {
            return Err(PortError::new(
                "household_key_bundle_invalid",
                "household key bundle contains an invalid commit evidence ledger",
            ));
        }
        match self.phase {
            HouseholdKeyBundlePhase::Initializing => {
                if self.initialization_id.is_none()
                    || self.initial_commit_id.is_none()
                    || self.initial_effect_fingerprint.is_none()
                    || self.initial_state_digest.is_none()
                    || self.rotation_id.is_some()
                    || self.previous_key.is_some()
                    || self.initialization_id.is_some_and(|value| value.is_nil())
                    || self.initial_commit_id.is_some_and(|value| value.is_nil())
                {
                    return Err(PortError::new(
                        "household_key_bundle_invalid",
                        "household initializing key bundle is invalid",
                    ));
                }
            }
            HouseholdKeyBundlePhase::Stable => {
                if self.initialization_id.is_some()
                    || self.initial_commit_id.is_some()
                    || self.initial_effect_fingerprint.is_some()
                    || self.initial_state_digest.is_some()
                    || self.rotation_id.is_some()
                    || self.previous_key.is_some()
                {
                    return Err(PortError::new(
                        "household_key_bundle_invalid",
                        "household stable key bundle is invalid",
                    ));
                }
            }
            HouseholdKeyBundlePhase::Rewriting => {
                if self.rotation_id.is_none()
                    || self.previous_key.is_none()
                    || self.initialization_id.is_some()
                    || self.initial_commit_id.is_some()
                    || self.initial_effect_fingerprint.is_some()
                    || self.initial_state_digest.is_some()
                    || self.rotation_id.is_some_and(|value| value.is_nil())
                {
                    return Err(PortError::new(
                        "household_key_bundle_invalid",
                        "household rewriting key bundle is invalid",
                    ));
                }
            }
        }
        if self
            .previous_key
            .as_ref()
            .is_some_and(|(key_id, _)| *key_id == self.active_key_id)
        {
            return Err(PortError::new(
                "household_key_bundle_invalid",
                "household active and previous key IDs must differ",
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_initial_for(
        &self,
        slot: &HouseholdAccountSlotV1,
        guard: &HouseholdMigrationGuardDocument,
    ) -> Result<(), PortError> {
        self.validate_for(slot)?;
        guard.validate_for(slot)?;
        if self.phase != HouseholdKeyBundlePhase::Initializing || self.revision.get() != 1 {
            return Err(PortError::new(
                "household_key_bundle_invalid",
                "initial household key bundle must be initializing at revision one",
            ));
        }
        if guard.state() != HouseholdMigrationGuardStateV1::Initializing
            || guard.initialization_phase()
                != Some(HouseholdMigrationInitializationPhaseV1::ReadyToInitialize)
            || self.initialization_id != Some(guard.initialization_id())
            || self.initial_commit_id != Some(guard.initial_commit_id())
            || self.initial_effect_fingerprint != guard.initial_effect_fingerprint()
            || self.initial_state_digest != guard.initial_state_digest()
        {
            return Err(PortError::new(
                "household_key_guard_mismatch",
                "household key initialization does not match the ready migration guard",
            ));
        }
        Ok(())
    }

    pub(crate) fn key_for(&self, key_id: KeyId) -> Option<&HouseholdKeyMaterial> {
        if self.active_key_id == key_id {
            Some(&self.active_key)
        } else {
            self.previous_key
                .as_ref()
                .filter(|(candidate, _)| *candidate == key_id)
                .map(|(_, key)| key)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HouseholdCommitEvidenceStateV1 {
    Reserved,
    Denied,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HouseholdCommitEvidenceRecordV1 {
    proposal_ref_hash: [u8; 32],
    commit_id: CommitId,
    state: HouseholdCommitEvidenceStateV1,
    expires_at_unix_seconds: u64,
}

fn commit_evidence_proposal_ref_hash(proposal_ref: Uuid) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(COMMIT_EVIDENCE_PROPOSAL_REF_HASH_V1);
    hasher.update(proposal_ref.as_bytes());
    hasher.finalize().into()
}

fn commit_evidence_record_mismatch_error() -> PortError {
    PortError::new(
        "household_commit_evidence_mismatch",
        "household commit evidence did not match the authoritative repository",
    )
}

fn commit_evidence_retention_error() -> PortError {
    PortError::new(
        "household_commit_evidence_retention",
        "household commit evidence retention could not be bounded",
    )
}

fn derive_commit_evidence_root_key(active_key: &HouseholdKeyMaterial) -> HouseholdKeyMaterial {
    let mut hasher = Sha256::new();
    hasher.update(COMMIT_EVIDENCE_ROOT_DERIVATION_V1);
    hasher.update(active_key.expose());
    let mut bytes = [0_u8; 32];
    bytes.copy_from_slice(&hasher.finalize());
    HouseholdKeyMaterial::from_bytes(bytes)
}

impl fmt::Debug for HouseholdKeyBundle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdKeyBundle")
            .field("account_digest", &hex_digest(self.account_digest))
            .field(
                "native_root_instance_digest",
                &hex_digest(self.native_root_instance_digest),
            )
            .field(
                "account_locator_digest",
                &hex_digest(self.account_locator_digest),
            )
            .field("revision", &self.revision)
            .field("active_key_id", &self.active_key_id)
            .field("has_previous_key", &self.previous_key.is_some())
            .field("phase", &self.phase)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyStoreExpectation {
    Absent,
}

pub trait HouseholdKeyStore: Send + Sync {
    fn load<'a>(
        &'a self,
        lifecycle_lease: &'a HouseholdLifecycleLease,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<Option<HouseholdKeyBundle>, PortError>>;

    fn initialize<'a>(
        &'a self,
        vault_lease: &'a mut HouseholdVaultLease,
        expected: KeyStoreExpectation,
        expected_guard: HouseholdMigrationGuardDocument,
        bundle: HouseholdKeyBundle,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<(), PortError>>;

    fn compare_exchange<'a>(
        &'a self,
        vault_lease: &'a mut HouseholdVaultLease,
        expected: KeyBundleRevision,
        replacement: HouseholdKeyBundle,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<(), PortError>>;

    fn delete_and_verify<'a>(
        &'a self,
        vault_lease: &'a mut HouseholdVaultLease,
        expected_revision: KeyBundleRevision,
        expected_key_id: KeyId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<(), PortError>>;

    fn abort_initialization_and_verify<'a>(
        &'a self,
        vault_lease: &'a mut HouseholdVaultLease,
        expected_revision: KeyBundleRevision,
        expected_initialization_id: Uuid,
        expected_aborting_guard: HouseholdMigrationGuardDocument,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<(), PortError>>;
}

const MIGRATION_GUARD_SCHEMA_VERSION: u16 = 1;
const MIGRATION_GUARD_LIMITS: CompatibilityJsonLimitsV1 = CompatibilityJsonLimitsV1 {
    maximum_bytes: MAX_BROKER_DOCUMENT_BYTES,
    maximum_depth: 4,
    maximum_object_keys: 20,
    maximum_array_entries: 0,
    maximum_nodes: 64,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HouseholdMigrationGuardStateV1 {
    Initializing,
    Aborting,
    Migrated,
    InitializedNoSource,
    BlockedRepair,
    BlockedAfterLogout,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HouseholdMigrationInitializationPhaseV1 {
    ReservedSource,
    ReadyToInitialize,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HouseholdMigrationCleanupPhaseV1 {
    CleanupPending,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HouseholdMigrationRepairFailureCategoryV1 {
    SourceChanged,
    SemanticValidation,
    CanonicalConstruction,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HouseholdMigrationPresentSourceKindV1 {
    LegacyPythonSourceBundleV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HouseholdMigrationSourceIdentityV1 {
    Present {
        source_kind: HouseholdMigrationPresentSourceKindV1,
        source_digest: CanonicalDigestV1,
    },
    NoSource {
        source_set_fingerprint: CanonicalDigestV1,
    },
}

impl HouseholdMigrationSourceIdentityV1 {
    #[must_use]
    pub fn present(source_digest: [u8; 32]) -> Self {
        Self::Present {
            source_kind: HouseholdMigrationPresentSourceKindV1::LegacyPythonSourceBundleV1,
            source_digest: CanonicalDigestV1::from_bytes(source_digest),
        }
    }

    #[must_use]
    pub fn no_source(source_set_fingerprint: [u8; 32]) -> Self {
        Self::NoSource {
            source_set_fingerprint: CanonicalDigestV1::from_bytes(source_set_fingerprint),
        }
    }
}

/// Typed, canonical, account/root/source-bound migration guard.
///
/// The secure-store representation is accepted only when its bytes are the
/// exact Canonical Bytes v1 encoding of this closed schema. In particular, an
/// incomplete JSON object cannot become a guard merely by carrying a revision
/// and the three account-slot digests.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HouseholdMigrationGuardDocument {
    schema_version: u16,
    account_digest: CanonicalDigestV1,
    native_root_instance_digest: CanonicalDigestV1,
    account_locator_digest: CanonicalDigestV1,
    guard_revision: u64,
    state: HouseholdMigrationGuardStateV1,
    source_identity: HouseholdMigrationSourceIdentityV1,
    legacy_python_snapshot: Option<LegacyPythonSnapshotProvenanceV1>,
    migration_id: Uuid,
    initialization_id: Uuid,
    initialization_phase: Option<HouseholdMigrationInitializationPhaseV1>,
    migration_frozen_at: CanonicalTimestampV1,
    initial_commit_id: Uuid,
    initial_effect_fingerprint: Option<CanonicalDigestV1>,
    initial_state_digest: Option<CanonicalDigestV1>,
    cleanup_phase: Option<HouseholdMigrationCleanupPhaseV1>,
    repair_failure_category: Option<HouseholdMigrationRepairFailureCategoryV1>,
}

impl HouseholdMigrationGuardDocument {
    #[allow(clippy::too_many_arguments)]
    pub fn initializing_reserved(
        slot: &HouseholdAccountSlotV1,
        source_identity: HouseholdMigrationSourceIdentityV1,
        migration_id: Uuid,
        initialization_id: Uuid,
        initial_commit_id: Uuid,
        migration_frozen_at: CanonicalTimestampV1,
    ) -> Result<Self, PortError> {
        Self::initializing_reserved_with_snapshot(
            slot,
            source_identity,
            None,
            migration_id,
            initialization_id,
            initial_commit_id,
            migration_frozen_at,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn initializing_reserved_with_snapshot(
        slot: &HouseholdAccountSlotV1,
        source_identity: HouseholdMigrationSourceIdentityV1,
        legacy_python_snapshot: Option<LegacyPythonSnapshotProvenanceV1>,
        migration_id: Uuid,
        initialization_id: Uuid,
        initial_commit_id: Uuid,
        migration_frozen_at: CanonicalTimestampV1,
    ) -> Result<Self, PortError> {
        let guard = Self {
            schema_version: MIGRATION_GUARD_SCHEMA_VERSION,
            account_digest: CanonicalDigestV1::from_bytes(slot.account_digest()),
            native_root_instance_digest: CanonicalDigestV1::from_bytes(
                slot.native_root_instance_digest(),
            ),
            account_locator_digest: CanonicalDigestV1::from_bytes(slot.account_locator_digest()),
            guard_revision: 1,
            state: HouseholdMigrationGuardStateV1::Initializing,
            source_identity,
            legacy_python_snapshot,
            migration_id,
            initialization_id,
            initialization_phase: Some(HouseholdMigrationInitializationPhaseV1::ReservedSource),
            migration_frozen_at,
            initial_commit_id,
            initial_effect_fingerprint: None,
            initial_state_digest: None,
            cleanup_phase: None,
            repair_failure_category: None,
        };
        guard.validate_for(slot)?;
        Ok(guard)
    }

    pub fn ready_to_initialize(
        &self,
        initial_effect_fingerprint: [u8; 32],
        initial_state_digest: [u8; 32],
    ) -> Result<Self, PortError> {
        if self.state != HouseholdMigrationGuardStateV1::Initializing
            || self.initialization_phase
                != Some(HouseholdMigrationInitializationPhaseV1::ReservedSource)
        {
            return Err(migration_guard_transition_error());
        }
        let mut replacement = self.clone();
        replacement.guard_revision = checked_guard_revision(self.guard_revision)?;
        replacement.initialization_phase =
            Some(HouseholdMigrationInitializationPhaseV1::ReadyToInitialize);
        replacement.initial_effect_fingerprint =
            Some(CanonicalDigestV1::from_bytes(initial_effect_fingerprint));
        replacement.initial_state_digest =
            Some(CanonicalDigestV1::from_bytes(initial_state_digest));
        replacement.validate_transition_from(self)?;
        Ok(replacement)
    }

    pub fn begin_aborting(
        &self,
        failure: HouseholdMigrationRepairFailureCategoryV1,
    ) -> Result<Self, PortError> {
        if self.state != HouseholdMigrationGuardStateV1::Initializing {
            return Err(migration_guard_transition_error());
        }
        let mut replacement = self.clone();
        replacement.guard_revision = checked_guard_revision(self.guard_revision)?;
        replacement.state = HouseholdMigrationGuardStateV1::Aborting;
        replacement.cleanup_phase = Some(HouseholdMigrationCleanupPhaseV1::CleanupPending);
        replacement.repair_failure_category = Some(failure);
        replacement.validate_transition_from(self)?;
        Ok(replacement)
    }

    pub(crate) fn blocked_repair_after_cleanup(&self) -> Result<Self, PortError> {
        if self.state != HouseholdMigrationGuardStateV1::Aborting {
            return Err(migration_guard_transition_error());
        }
        let mut replacement = self.clone();
        replacement.guard_revision = checked_guard_revision(self.guard_revision)?;
        replacement.state = HouseholdMigrationGuardStateV1::BlockedRepair;
        replacement.initialization_phase = None;
        replacement.cleanup_phase = None;
        replacement.validate_transition_from(self)?;
        Ok(replacement)
    }

    pub fn complete_initialization(&self) -> Result<Self, PortError> {
        if self.state != HouseholdMigrationGuardStateV1::Initializing
            || self.initialization_phase
                != Some(HouseholdMigrationInitializationPhaseV1::ReadyToInitialize)
        {
            return Err(migration_guard_transition_error());
        }
        let mut replacement = self.clone();
        replacement.guard_revision = checked_guard_revision(self.guard_revision)?;
        replacement.state = match self.source_identity {
            HouseholdMigrationSourceIdentityV1::Present { .. } => {
                HouseholdMigrationGuardStateV1::Migrated
            }
            HouseholdMigrationSourceIdentityV1::NoSource { .. } => {
                HouseholdMigrationGuardStateV1::InitializedNoSource
            }
        };
        replacement.initialization_phase = None;
        replacement.validate_transition_from(self)?;
        Ok(replacement)
    }

    pub fn blocked_after_logout(&self) -> Result<Self, PortError> {
        if !matches!(
            self.state,
            HouseholdMigrationGuardStateV1::Migrated
                | HouseholdMigrationGuardStateV1::InitializedNoSource
                | HouseholdMigrationGuardStateV1::BlockedRepair
        ) {
            return Err(migration_guard_transition_error());
        }
        let mut replacement = self.clone();
        replacement.guard_revision = checked_guard_revision(self.guard_revision)?;
        replacement.state = HouseholdMigrationGuardStateV1::BlockedAfterLogout;
        replacement.validate_transition_from(self)?;
        Ok(replacement)
    }

    pub fn from_canonical_bytes(
        slot: &HouseholdAccountSlotV1,
        bytes: &[u8],
    ) -> Result<Self, PortError> {
        if bytes.is_empty() || bytes.len() > MAX_BROKER_DOCUMENT_BYTES {
            return Err(migration_guard_quarantine_error());
        }
        let value = parse_bounded_typed_json_v1(bytes, MIGRATION_GUARD_LIMITS)
            .map_err(|_| migration_guard_quarantine_error())?;
        let guard: Self =
            serde_json::from_value(value).map_err(|_| migration_guard_quarantine_error())?;
        guard
            .validate_for(slot)
            .map_err(|_| migration_guard_quarantine_error())?;
        let canonical = guard
            .canonical_bytes()
            .map_err(|_| migration_guard_quarantine_error())?;
        if canonical.as_slice() != bytes {
            return Err(migration_guard_quarantine_error());
        }
        Ok(guard)
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, PortError> {
        self.validate_shape()?;
        let bytes = to_canonical_bytes_v1(self).map_err(|_| migration_guard_invalid_error())?;
        if bytes.is_empty() || bytes.len() > MAX_BROKER_DOCUMENT_BYTES {
            return Err(migration_guard_invalid_error());
        }
        Ok(bytes)
    }

    pub fn validate_for(&self, slot: &HouseholdAccountSlotV1) -> Result<(), PortError> {
        self.validate_shape()?;
        if self.account_digest.as_bytes() != &slot.account_digest()
            || self.native_root_instance_digest.as_bytes() != &slot.native_root_instance_digest()
            || self.account_locator_digest.as_bytes() != &slot.account_locator_digest()
        {
            return Err(PortError::new(
                "household_migration_guard_account_mismatch",
                "household migration guard does not match the requested account slot",
            ));
        }
        Ok(())
    }

    fn validate_shape(&self) -> Result<(), PortError> {
        if self.schema_version != MIGRATION_GUARD_SCHEMA_VERSION
            || self.guard_revision == 0
            || !uuid_is_canonical_v4(self.migration_id)
            || !uuid_is_canonical_v4(self.initialization_id)
            || !uuid_is_canonical_v4(self.initial_commit_id)
        {
            return Err(migration_guard_invalid_error());
        }
        let ready_tuple =
            self.initial_effect_fingerprint.is_some() && self.initial_state_digest.is_some();
        if self.initial_effect_fingerprint.is_some() != self.initial_state_digest.is_some() {
            return Err(migration_guard_invalid_error());
        }
        if matches!(
            (&self.source_identity, &self.legacy_python_snapshot),
            (HouseholdMigrationSourceIdentityV1::NoSource { .. }, Some(_))
        ) {
            return Err(migration_guard_invalid_error());
        }
        let valid = match self.state {
            HouseholdMigrationGuardStateV1::Initializing => {
                self.cleanup_phase.is_none()
                    && self.repair_failure_category.is_none()
                    && match self.initialization_phase {
                        Some(HouseholdMigrationInitializationPhaseV1::ReservedSource) => {
                            !ready_tuple
                        }
                        Some(HouseholdMigrationInitializationPhaseV1::ReadyToInitialize) => {
                            ready_tuple
                        }
                        None => false,
                    }
            }
            HouseholdMigrationGuardStateV1::Aborting => {
                self.cleanup_phase == Some(HouseholdMigrationCleanupPhaseV1::CleanupPending)
                    && self.repair_failure_category.is_some()
                    && match self.initialization_phase {
                        Some(HouseholdMigrationInitializationPhaseV1::ReservedSource) => {
                            !ready_tuple
                        }
                        Some(HouseholdMigrationInitializationPhaseV1::ReadyToInitialize) => {
                            ready_tuple
                        }
                        None => false,
                    }
            }
            HouseholdMigrationGuardStateV1::Migrated => {
                matches!(
                    self.source_identity,
                    HouseholdMigrationSourceIdentityV1::Present { .. }
                ) && self.initialization_phase.is_none()
                    && self.cleanup_phase.is_none()
                    && self.repair_failure_category.is_none()
                    && ready_tuple
            }
            HouseholdMigrationGuardStateV1::InitializedNoSource => {
                matches!(
                    self.source_identity,
                    HouseholdMigrationSourceIdentityV1::NoSource { .. }
                ) && self.initialization_phase.is_none()
                    && self.cleanup_phase.is_none()
                    && self.repair_failure_category.is_none()
                    && ready_tuple
            }
            HouseholdMigrationGuardStateV1::BlockedRepair => {
                self.initialization_phase.is_none()
                    && self.cleanup_phase.is_none()
                    && self.repair_failure_category.is_some()
            }
            HouseholdMigrationGuardStateV1::BlockedAfterLogout => {
                self.initialization_phase.is_none()
                    && self.cleanup_phase.is_none()
                    && (self.repair_failure_category.is_some() || ready_tuple)
            }
        };
        if !valid {
            return Err(migration_guard_invalid_error());
        }
        Ok(())
    }

    fn validate_transition_from(&self, current: &Self) -> Result<(), PortError> {
        self.validate_shape()?;
        current.validate_shape()?;
        if self.guard_revision != checked_guard_revision(current.guard_revision)?
            || self.schema_version != current.schema_version
            || self.account_digest != current.account_digest
            || self.native_root_instance_digest != current.native_root_instance_digest
            || self.account_locator_digest != current.account_locator_digest
            || self.source_identity != current.source_identity
            || self.legacy_python_snapshot != current.legacy_python_snapshot
            || self.migration_id != current.migration_id
            || self.initialization_id != current.initialization_id
            || self.migration_frozen_at != current.migration_frozen_at
            || self.initial_commit_id != current.initial_commit_id
        {
            return Err(migration_guard_transition_error());
        }

        let transition_valid = match (current.state, self.state) {
            (
                HouseholdMigrationGuardStateV1::Initializing,
                HouseholdMigrationGuardStateV1::Initializing,
            ) => {
                current.initialization_phase
                    == Some(HouseholdMigrationInitializationPhaseV1::ReservedSource)
                    && self.initialization_phase
                        == Some(HouseholdMigrationInitializationPhaseV1::ReadyToInitialize)
                    && current.initial_effect_fingerprint.is_none()
                    && current.initial_state_digest.is_none()
                    && self.initial_effect_fingerprint.is_some()
                    && self.initial_state_digest.is_some()
                    && self.cleanup_phase.is_none()
                    && self.repair_failure_category.is_none()
            }
            (
                HouseholdMigrationGuardStateV1::Initializing,
                HouseholdMigrationGuardStateV1::Aborting,
            ) => {
                self.initialization_phase == current.initialization_phase
                    && self.initial_effect_fingerprint == current.initial_effect_fingerprint
                    && self.initial_state_digest == current.initial_state_digest
                    && self.cleanup_phase == Some(HouseholdMigrationCleanupPhaseV1::CleanupPending)
                    && self.repair_failure_category.is_some()
            }
            (
                HouseholdMigrationGuardStateV1::Initializing,
                HouseholdMigrationGuardStateV1::Migrated
                | HouseholdMigrationGuardStateV1::InitializedNoSource,
            ) => {
                current.initialization_phase
                    == Some(HouseholdMigrationInitializationPhaseV1::ReadyToInitialize)
                    && self.initialization_phase.is_none()
                    && self.initial_effect_fingerprint == current.initial_effect_fingerprint
                    && self.initial_state_digest == current.initial_state_digest
                    && self.cleanup_phase.is_none()
                    && self.repair_failure_category.is_none()
            }
            (
                HouseholdMigrationGuardStateV1::Aborting,
                HouseholdMigrationGuardStateV1::BlockedRepair,
            ) => {
                self.initialization_phase.is_none()
                    && self.initial_effect_fingerprint == current.initial_effect_fingerprint
                    && self.initial_state_digest == current.initial_state_digest
                    && self.cleanup_phase.is_none()
                    && self.repair_failure_category == current.repair_failure_category
            }
            (
                HouseholdMigrationGuardStateV1::Migrated
                | HouseholdMigrationGuardStateV1::InitializedNoSource
                | HouseholdMigrationGuardStateV1::BlockedRepair,
                HouseholdMigrationGuardStateV1::BlockedAfterLogout,
            ) => {
                self.initialization_phase == current.initialization_phase
                    && self.initial_effect_fingerprint == current.initial_effect_fingerprint
                    && self.initial_state_digest == current.initial_state_digest
                    && self.cleanup_phase == current.cleanup_phase
                    && self.repair_failure_category == current.repair_failure_category
            }
            _ => false,
        };
        if !transition_valid {
            return Err(migration_guard_transition_error());
        }
        Ok(())
    }

    #[must_use]
    pub const fn guard_revision(&self) -> u64 {
        self.guard_revision
    }

    #[must_use]
    pub const fn state(&self) -> HouseholdMigrationGuardStateV1 {
        self.state
    }

    #[must_use]
    pub const fn initialization_phase(&self) -> Option<HouseholdMigrationInitializationPhaseV1> {
        self.initialization_phase
    }

    #[must_use]
    pub fn source_identity(&self) -> &HouseholdMigrationSourceIdentityV1 {
        &self.source_identity
    }

    #[must_use]
    pub fn legacy_python_snapshot(&self) -> Option<&LegacyPythonSnapshotProvenanceV1> {
        self.legacy_python_snapshot.as_ref()
    }

    #[must_use]
    pub const fn migration_id(&self) -> Uuid {
        self.migration_id
    }

    #[must_use]
    pub const fn initialization_id(&self) -> Uuid {
        self.initialization_id
    }

    #[must_use]
    pub fn migration_frozen_at(&self) -> &CanonicalTimestampV1 {
        &self.migration_frozen_at
    }

    #[must_use]
    pub const fn initial_commit_id(&self) -> Uuid {
        self.initial_commit_id
    }

    #[must_use]
    pub fn initial_effect_fingerprint(&self) -> Option<[u8; 32]> {
        self.initial_effect_fingerprint
            .as_ref()
            .map(|digest| *digest.as_bytes())
    }

    #[must_use]
    pub fn initial_state_digest(&self) -> Option<[u8; 32]> {
        self.initial_state_digest
            .as_ref()
            .map(|digest| *digest.as_bytes())
    }

    #[must_use]
    pub const fn repair_failure_category(
        &self,
    ) -> Option<HouseholdMigrationRepairFailureCategoryV1> {
        self.repair_failure_category
    }
}

impl fmt::Debug for HouseholdMigrationGuardDocument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdMigrationGuardDocument")
            .field("account_digest", &self.account_digest)
            .field("guard_revision", &self.guard_revision)
            .field("state", &self.state)
            .field("initialization_phase", &self.initialization_phase)
            .field("cleanup_phase", &self.cleanup_phase)
            .field("repair_failure_category", &self.repair_failure_category)
            .finish_non_exhaustive()
    }
}

fn uuid_is_canonical_v4(value: Uuid) -> bool {
    !value.is_nil() && value.get_version_num() == 4
}

fn checked_guard_revision(value: u64) -> Result<u64, PortError> {
    value.checked_add(1).ok_or_else(|| {
        PortError::new(
            "household_migration_guard_revision",
            "household migration guard revision is exhausted",
        )
    })
}

fn migration_guard_invalid_error() -> PortError {
    PortError::new(
        "household_migration_guard_invalid",
        "household migration guard document is invalid",
    )
}

fn migration_guard_quarantine_error() -> PortError {
    PortError::new(
        "household_migration_guard_quarantined",
        "household migration guard is incomplete, noncanonical, or invalid",
    )
}

fn migration_guard_transition_error() -> PortError {
    PortError::new(
        "household_migration_guard_transition",
        "household migration guard transition is invalid",
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MigrationGuardExpectation {
    Absent,
    Revision(u64),
}

pub trait HouseholdMigrationGuardStore: Send + Sync {
    fn load<'a>(
        &'a self,
        lifecycle_lease: &'a HouseholdLifecycleLease,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<Option<HouseholdMigrationGuardDocument>, PortError>>;

    fn compare_exchange<'a>(
        &'a self,
        vault_lease: &'a mut HouseholdVaultLease,
        expected: MigrationGuardExpectation,
        replacement: Option<HouseholdMigrationGuardDocument>,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<(), PortError>>;
}

pub trait HouseholdSecureStore: HouseholdKeyStore + HouseholdMigrationGuardStore {}

impl<T> HouseholdSecureStore for T where T: HouseholdKeyStore + HouseholdMigrationGuardStore {}

pub fn open_production_household_secure_store(
    paths: &crate::NativePaths,
    deadline: Duration,
) -> Result<Arc<dyn HouseholdSecureStore>, PortError> {
    #[cfg(feature = "native-credentials")]
    {
        native::HouseholdKeyBroker::from_native_paths(paths, deadline)
            .map(|broker| Arc::new(broker) as Arc<dyn HouseholdSecureStore>)
    }
    #[cfg(not(feature = "native-credentials"))]
    {
        let _ = (paths, deadline);
        Err(PortError::new(
            "household_secure_store_unavailable",
            "native household secure storage is unavailable in this build",
        ))
    }
}

#[derive(Clone, Default)]
pub struct InMemoryHouseholdSecureStore {
    keys: Arc<Mutex<HashMap<[u8; 32], HouseholdKeyBundle>>>,
    guards: Arc<Mutex<HashMap<[u8; 32], HouseholdMigrationGuardDocument>>>,
    #[cfg(test)]
    guard_cas_uncertain_after_commit: Arc<AtomicBool>,
    #[cfg(test)]
    key_abort_uncertain_after_delete: Arc<AtomicBool>,
}

impl fmt::Debug for InMemoryHouseholdSecureStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InMemoryHouseholdSecureStore")
            .finish_non_exhaustive()
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
impl InMemoryHouseholdSecureStore {
    pub(crate) fn inject_next_guard_cas_uncertain_after_commit(&self) {
        self.guard_cas_uncertain_after_commit
            .store(true, Ordering::SeqCst);
    }

    pub(crate) fn inject_next_key_abort_uncertain_after_delete(&self) {
        self.key_abort_uncertain_after_delete
            .store(true, Ordering::SeqCst);
    }
}

fn check_cancellation(cancellation: &CancellationToken) -> Result<(), PortError> {
    if cancellation.is_cancelled() {
        Err(PortError::new(
            "household_operation_cancelled",
            "household secure-store operation was cancelled",
        ))
    } else {
        Ok(())
    }
}

fn lifecycle_account_slot(
    lifecycle_lease: &HouseholdLifecycleLease,
) -> Result<HouseholdAccountSlotV1, PortError> {
    let account_slot = lifecycle_lease.account_slot().clone();
    lifecycle_lease.validate_for(&account_slot)?;
    Ok(account_slot)
}

fn vault_account_slot(
    vault_lease: &HouseholdVaultLease,
) -> Result<HouseholdAccountSlotV1, PortError> {
    let account_slot = vault_lease.account_slot().clone();
    vault_lease.validate_for(&account_slot)?;
    Ok(account_slot)
}

fn verify_vault_lease_after_mutation(
    vault_lease: &HouseholdVaultLease,
    account_slot: &HouseholdAccountSlotV1,
) -> Result<(), PortError> {
    vault_lease.validate_for(account_slot).map_err(|_| {
        PortError::uncertain(
            "household_vault_lease",
            "household secure-store mutation requires reconciliation",
        )
    })
}

impl HouseholdKeyStore for InMemoryHouseholdSecureStore {
    fn load<'a>(
        &'a self,
        lifecycle_lease: &'a HouseholdLifecycleLease,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<Option<HouseholdKeyBundle>, PortError>> {
        Box::pin(async move {
            check_cancellation(&cancellation)?;
            let account_slot = lifecycle_account_slot(lifecycle_lease)?;
            let result = self
                .keys
                .lock()
                .map_err(|_| {
                    PortError::new(
                        "household_secure_store",
                        "household secure store is unavailable",
                    )
                })?
                .get(&account_slot.account_locator_digest())
                .cloned();
            if let Some(bundle) = &result {
                bundle.validate_for(&account_slot)?;
            }
            check_cancellation(&cancellation)?;
            lifecycle_lease.validate_for(&account_slot)?;
            Ok(result)
        })
    }

    fn initialize<'a>(
        &'a self,
        vault_lease: &'a mut HouseholdVaultLease,
        _expected: KeyStoreExpectation,
        expected_guard: HouseholdMigrationGuardDocument,
        bundle: HouseholdKeyBundle,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<(), PortError>> {
        Box::pin(async move {
            let _operation = vault_lease.acquire_operation(&cancellation).await?;
            check_cancellation(&cancellation)?;
            let account_slot = vault_account_slot(vault_lease)?;
            expected_guard.validate_for(&account_slot)?;
            let stored_guard = self
                .guards
                .lock()
                .map_err(|_| {
                    PortError::new(
                        "household_secure_store",
                        "household secure store is unavailable",
                    )
                })?
                .get(&account_slot.account_locator_digest())
                .cloned();
            if stored_guard.as_ref() != Some(&expected_guard) {
                return Err(PortError::new(
                    "household_key_guard_mismatch",
                    "household key initialization requires the exact ready migration guard",
                ));
            }
            bundle.validate_initial_for(&account_slot, &expected_guard)?;
            let mut keys = self.keys.lock().map_err(|_| {
                PortError::new(
                    "household_secure_store",
                    "household secure store is unavailable",
                )
            })?;
            if keys.contains_key(&account_slot.account_locator_digest()) {
                return Err(PortError::new(
                    "household_key_exists",
                    "household key bundle already exists",
                ));
            }
            keys.insert(account_slot.account_locator_digest(), bundle);
            verify_vault_lease_after_mutation(vault_lease, &account_slot)?;
            Ok(())
        })
    }

    fn compare_exchange<'a>(
        &'a self,
        vault_lease: &'a mut HouseholdVaultLease,
        expected: KeyBundleRevision,
        replacement: HouseholdKeyBundle,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<(), PortError>> {
        Box::pin(async move {
            let _operation = vault_lease.acquire_operation(&cancellation).await?;
            check_cancellation(&cancellation)?;
            let account_slot = vault_account_slot(vault_lease)?;
            replacement.validate_for(&account_slot)?;
            if replacement.revision != expected.checked_next()? {
                return Err(PortError::new(
                    "household_key_cas",
                    "household key replacement revision is invalid",
                ));
            }
            let mut keys = self.keys.lock().map_err(|_| {
                PortError::new(
                    "household_secure_store",
                    "household secure store is unavailable",
                )
            })?;
            let current = keys
                .get(&account_slot.account_locator_digest())
                .ok_or_else(|| {
                    PortError::new("household_key_not_found", "household key bundle is absent")
                })?;
            current.validate_for(&account_slot)?;
            if current.revision != expected {
                return Err(PortError::new(
                    "household_key_cas",
                    "household key bundle changed concurrently",
                ));
            }
            keys.insert(account_slot.account_locator_digest(), replacement);
            verify_vault_lease_after_mutation(vault_lease, &account_slot)?;
            Ok(())
        })
    }

    fn delete_and_verify<'a>(
        &'a self,
        vault_lease: &'a mut HouseholdVaultLease,
        expected_revision: KeyBundleRevision,
        expected_key_id: KeyId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<(), PortError>> {
        Box::pin(async move {
            let _operation = vault_lease.acquire_operation(&cancellation).await?;
            check_cancellation(&cancellation)?;
            let account_slot = vault_account_slot(vault_lease)?;
            let mut keys = self.keys.lock().map_err(|_| {
                PortError::new(
                    "household_secure_store",
                    "household secure store is unavailable",
                )
            })?;
            let current = keys
                .get(&account_slot.account_locator_digest())
                .ok_or_else(|| {
                    PortError::new("household_key_not_found", "household key bundle is absent")
                })?;
            current.validate_for(&account_slot)?;
            if current.revision != expected_revision || current.active_key_id != expected_key_id {
                return Err(PortError::new(
                    "household_key_cas",
                    "household key bundle changed concurrently",
                ));
            }
            keys.remove(&account_slot.account_locator_digest());
            if keys.contains_key(&account_slot.account_locator_digest()) {
                return Err(PortError::uncertain(
                    "household_key_delete",
                    "household key deletion could not be verified",
                ));
            }
            verify_vault_lease_after_mutation(vault_lease, &account_slot)?;
            Ok(())
        })
    }

    fn abort_initialization_and_verify<'a>(
        &'a self,
        vault_lease: &'a mut HouseholdVaultLease,
        expected_revision: KeyBundleRevision,
        expected_initialization_id: Uuid,
        expected_aborting_guard: HouseholdMigrationGuardDocument,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<(), PortError>> {
        Box::pin(async move {
            let _operation = vault_lease.acquire_operation(&cancellation).await?;
            check_cancellation(&cancellation)?;
            let account_slot = vault_account_slot(vault_lease)?;
            expected_aborting_guard.validate_for(&account_slot)?;
            if expected_aborting_guard.state() != HouseholdMigrationGuardStateV1::Aborting
                || expected_aborting_guard.initialization_id() != expected_initialization_id
            {
                return Err(PortError::new(
                    "household_key_abort_guard",
                    "household key abort requires the exact cleanup-pending migration guard",
                ));
            }
            let stored_guard = self
                .guards
                .lock()
                .map_err(|_| {
                    PortError::new(
                        "household_secure_store",
                        "household secure store is unavailable",
                    )
                })?
                .get(&account_slot.account_locator_digest())
                .cloned();
            if stored_guard.as_ref() != Some(&expected_aborting_guard) {
                return Err(PortError::new(
                    "household_key_abort_guard",
                    "household key abort requires the authoritative cleanup-pending guard",
                ));
            }
            let mut keys = self.keys.lock().map_err(|_| {
                PortError::new(
                    "household_secure_store",
                    "household secure store is unavailable",
                )
            })?;
            let current = keys
                .get(&account_slot.account_locator_digest())
                .ok_or_else(|| {
                    PortError::new("household_key_not_found", "household key bundle is absent")
                })?;
            current.validate_for(&account_slot)?;
            if current.revision != expected_revision
                || current.phase != HouseholdKeyBundlePhase::Initializing
                || current.initialization_id != Some(expected_initialization_id)
            {
                return Err(PortError::new(
                    "household_key_abort_cas",
                    "household initializing key bundle changed concurrently",
                ));
            }
            keys.remove(&account_slot.account_locator_digest());
            if keys.contains_key(&account_slot.account_locator_digest()) {
                return Err(PortError::uncertain(
                    "household_key_abort",
                    "household key initialization abort could not be verified",
                ));
            }
            verify_vault_lease_after_mutation(vault_lease, &account_slot)?;
            #[cfg(test)]
            if self
                .key_abort_uncertain_after_delete
                .swap(false, Ordering::SeqCst)
            {
                return Err(PortError::uncertain(
                    "household_key_abort",
                    "household key initialization abort result was lost",
                ));
            }
            Ok(())
        })
    }
}

impl HouseholdMigrationGuardStore for InMemoryHouseholdSecureStore {
    fn load<'a>(
        &'a self,
        lifecycle_lease: &'a HouseholdLifecycleLease,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<Option<HouseholdMigrationGuardDocument>, PortError>> {
        Box::pin(async move {
            check_cancellation(&cancellation)?;
            let account_slot = lifecycle_account_slot(lifecycle_lease)?;
            let result = self
                .guards
                .lock()
                .map_err(|_| {
                    PortError::new(
                        "household_secure_store",
                        "household secure store is unavailable",
                    )
                })?
                .get(&account_slot.account_locator_digest())
                .cloned();
            if let Some(guard) = &result {
                guard.validate_for(&account_slot)?;
            }
            lifecycle_lease.validate_for(&account_slot)?;
            Ok(result)
        })
    }

    fn compare_exchange<'a>(
        &'a self,
        vault_lease: &'a mut HouseholdVaultLease,
        expected: MigrationGuardExpectation,
        replacement: Option<HouseholdMigrationGuardDocument>,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<(), PortError>> {
        Box::pin(async move {
            let _operation = vault_lease.acquire_operation(&cancellation).await?;
            check_cancellation(&cancellation)?;
            let account_slot = vault_account_slot(vault_lease)?;
            if let Some(replacement) = &replacement {
                replacement.validate_for(&account_slot)?;
                if expected == MigrationGuardExpectation::Absent
                    && (replacement.guard_revision() != 1
                        || replacement.state() != HouseholdMigrationGuardStateV1::Initializing
                        || replacement.initialization_phase()
                            != Some(HouseholdMigrationInitializationPhaseV1::ReservedSource))
                {
                    return Err(PortError::new(
                        "household_migration_guard_revision",
                        "initial household migration guard must be reserved at revision one",
                    ));
                }
            } else {
                return Err(PortError::new(
                    "household_migration_guard_delete_forbidden",
                    "household migration guards are retained for the account lifetime",
                ));
            }
            let mut guards = self.guards.lock().map_err(|_| {
                PortError::new(
                    "household_secure_store",
                    "household secure store is unavailable",
                )
            })?;
            let current = guards.get(&account_slot.account_locator_digest()).cloned();
            let matches = match expected {
                MigrationGuardExpectation::Absent => current.is_none(),
                MigrationGuardExpectation::Revision(revision) => current
                    .as_ref()
                    .is_some_and(|guard| guard.guard_revision() == revision),
            };
            if !matches {
                return Err(PortError::new(
                    "household_migration_guard_cas",
                    "household migration guard changed concurrently",
                ));
            }
            if let (MigrationGuardExpectation::Revision(_), Some(replacement), Some(current)) =
                (expected, &replacement, &current)
            {
                replacement.validate_transition_from(current)?;
            }
            let guard = replacement.expect("guard deletion was rejected");
            guards.insert(account_slot.account_locator_digest(), guard);
            verify_vault_lease_after_mutation(vault_lease, &account_slot)?;
            #[cfg(test)]
            if self
                .guard_cas_uncertain_after_commit
                .swap(false, Ordering::SeqCst)
            {
                return Err(PortError::uncertain(
                    "household_migration_guard_cas",
                    "household migration guard CAS result was lost",
                ));
            }
            Ok(())
        })
    }
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    let bytes = bytes.as_ref();
    hex_prefix(bytes, bytes.len() * 2)
}

fn hex_prefix(bytes: &[u8], hexadecimal_characters: usize) -> String {
    use std::fmt::Write as _;

    let byte_count = hexadecimal_characters.div_ceil(2).min(bytes.len());
    let mut output = String::with_capacity(byte_count * 2);
    for byte in &bytes[..byte_count] {
        let _ = write!(output, "{byte:02x}");
    }
    output.truncate(hexadecimal_characters);
    output
}

#[cfg(feature = "native-credentials")]
fn decode_lower_hex_32(value: &str) -> Result<[u8; 32], PortError> {
    if value.len() != 64
        || value
            .as_bytes()
            .iter()
            .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(byte))
    {
        return Err(PortError::new(
            "household_broker_document",
            "household broker document is invalid",
        ));
    }
    let mut output = [0_u8; 32];
    for (index, byte) in output.iter_mut().enumerate() {
        let start = index * 2;
        *byte = u8::from_str_radix(&value[start..start + 2], 16).map_err(|_| {
            PortError::new(
                "household_broker_document",
                "household broker document is invalid",
            )
        })?;
    }
    Ok(output)
}

#[cfg(feature = "native-credentials")]
mod native {

    use std::ffi::OsStr;
    use std::io::{Read, Write};
    use std::path::{Path, PathBuf};
    use std::process::{ExitCode, Stdio};
    use std::time::{Duration, Instant};

    use heyfood_application::{BoxFuture, CredentialCommit, CredentialPort, PortError};
    use heyfood_core::{
        AccountId, CommitId, CompatibilityJsonLimitsV1, CredentialVersion, SessionCredentials,
        parse_bounded_json_object_v1, to_canonical_bytes_v1,
    };
    use serde::{Deserialize, Serialize};
    use serde_json::{Map, Value};
    use sha2::{Digest as _, Sha256};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::process::Command;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;
    use zeroize::{Zeroize as _, Zeroizing};

    use crate::AuthorizationSessionStore;
    use crate::household_vault::{
        HouseholdAccountSlotV1, HouseholdLifecycleLease, HouseholdVaultLease,
        household_native_root_instance_digest_v1,
    };
    use crate::persistence::{AuthorizationSessionStage, CredentialState};
    use crate::python_import::{
        LegacyPythonConfigKindV1, LegacyPythonCredentialSourceLeaseV1,
        LegacyPythonKeyringProbeOutcomeV1,
    };

    const BROKER_MODE: &str = "__heyfood_credential_broker";
    use super::{
        HOUSEHOLD_KEYRING_SERVICE_V1, HouseholdBrokerOperationV1, HouseholdCommitEvidenceRecordV1,
        HouseholdCommitEvidenceStateV1, HouseholdKeyBundle, HouseholdKeyBundlePhase,
        HouseholdKeyMaterial, HouseholdKeyStore, HouseholdKeyringLocatorsV1,
        HouseholdMigrationGuardDocument, HouseholdMigrationGuardStateV1,
        HouseholdMigrationGuardStore, HouseholdMigrationInitializationPhaseV1, KeyBundleRevision,
        KeyId, KeyStoreExpectation, LegacyPythonKeyringLocatorV1, MAX_BROKER_DOCUMENT_BYTES,
        MAX_LEGACY_HOUSEHOLD_BROKER_RESPONSE_BYTES, MigrationGuardExpectation, decode_lower_hex_32,
        derive_commit_evidence_root_key, hex_digest, lifecycle_account_slot,
        migration_guard_invalid_error, migration_guard_quarantine_error, vault_account_slot,
        verify_vault_lease_after_mutation,
    };

    #[derive(Clone, Debug)]
    pub struct CredentialBrokerStore {
        executable: PathBuf,
        root: PathBuf,
        deadline: Duration,
    }

    impl CredentialBrokerStore {
        pub fn open(root: impl Into<PathBuf>, deadline: Duration) -> Result<Self, PortError> {
            if deadline.is_zero() || deadline > Duration::from_secs(30) {
                return Err(PortError::new(
                    "credential_broker_deadline",
                    "credential broker deadline must be between 1ns and 30s",
                ));
            }
            let executable = std::env::current_exe().map_err(|error| {
                PortError::new("credential_broker_executable", error.to_string())
            })?;
            if !executable.is_file() {
                return Err(PortError::new(
                    "credential_broker_executable",
                    "current executable is not a regular file",
                ));
            }
            Ok(Self {
                executable,
                root: root.into(),
                deadline,
            })
        }

        pub async fn initialize(&self, credentials: SessionCredentials) -> Result<(), PortError> {
            let input = CredentialState::new(credentials).encode();
            self.request("initialize", input, true).await.map(|_| ())
        }

        pub async fn delete(&self) -> Result<(), PortError> {
            self.request("delete", Vec::new(), true).await.map(|_| ())
        }

        async fn request(
            &self,
            action: &'static str,
            input: Vec<u8>,
            outcome_uncertain: bool,
        ) -> Result<Vec<u8>, PortError> {
            if input.len() > MAX_BROKER_DOCUMENT_BYTES {
                return Err(PortError::new(
                    "credential_broker_size",
                    "credential broker input exceeds its limit",
                ));
            }
            let child = Command::new(&self.executable)
                .arg(BROKER_MODE)
                .arg(action)
                .arg(&self.root)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|error| PortError::new("credential_broker_spawn", error.to_string()))?;
            run_bounded_child(child, input, self.deadline, outcome_uncertain).await
        }

        /// Synchronous bounded request used by the cross-store authorization
        /// transaction. The native keyring call still happens only in the broker
        /// child; this process waits for at most the configured deadline and then
        /// kills and reaps the child.
        fn request_blocking(
            &self,
            action: &'static str,
            input: Vec<u8>,
            outcome_uncertain: bool,
        ) -> Result<Vec<u8>, PortError> {
            if input.len() > MAX_BROKER_DOCUMENT_BYTES {
                return Err(PortError::new(
                    "credential_broker_size",
                    "credential broker input exceeds its limit",
                ));
            }
            let child = std::process::Command::new(&self.executable)
                .arg(BROKER_MODE)
                .arg(action)
                .arg(&self.root)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|error| PortError::new("credential_broker_spawn", error.to_string()))?;
            run_bounded_child_blocking(child, input, self.deadline, outcome_uncertain)
        }

        pub fn reconciliation_required(&self) -> Result<bool, PortError> {
            match self
                .request_blocking("reconciliation", Vec::new(), false)?
                .as_slice()
            {
                b"0\n" => Ok(false),
                b"1\n" => Ok(true),
                _ => Err(PortError::new(
                    "credential_broker_response",
                    "native credential broker returned an invalid reconciliation status",
                )),
            }
        }
    }

    /// Closed result of the purpose-limited historical keyring probe.
    ///
    /// A present result carries an unforgeable load authority bound to the
    /// exact account slot, frozen config target, locator digest, and probe
    /// operation. The authority contains no raw account ID or credential.
    #[derive(Clone, Debug)]
    pub enum LegacyPythonHouseholdProbeResultV1 {
        AuthoritativeMissing,
        Present(LegacyPythonHouseholdLoadAuthorityV1),
        Unavailable,
    }

    #[derive(Clone)]
    pub struct LegacyPythonHouseholdLoadAuthorityV1 {
        account_slot: HouseholdAccountSlotV1,
        config_kind: LegacyPythonConfigKindV1,
        resolved_config_path: PathBuf,
        locator_digest: [u8; 32],
    }

    impl std::fmt::Debug for LegacyPythonHouseholdLoadAuthorityV1 {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter
                .debug_struct("LegacyPythonHouseholdLoadAuthorityV1")
                .field("config_kind", &self.config_kind)
                .field("account_locator_digest", &"[REDACTED]")
                .field("legacy_locator_digest", &"[REDACTED]")
                .finish_non_exhaustive()
        }
    }

    /// Content-free result for the logout-only historical credential probe.
    ///
    /// Present entries carry a single-use-style authority that binds a later
    /// scrub to the exact document and retained noncredential projection seen
    /// by the broker. No credential value crosses the child-process boundary.
    #[derive(Clone, Debug)]
    pub enum LegacyPythonCredentialProbeResultV1 {
        AuthoritativeMissing { noncredential_digest: [u8; 32] },
        Present(Box<LegacyPythonCredentialScrubAuthorityV1>),
        Unavailable,
        Malformed,
    }

    #[derive(Clone)]
    pub struct LegacyPythonCredentialScrubAuthorityV1 {
        account_slot: HouseholdAccountSlotV1,
        config_kind: LegacyPythonConfigKindV1,
        resolved_config_path: PathBuf,
        locator_digest: [u8; 32],
        document_digest: [u8; 32],
        noncredential_digest: [u8; 32],
        credentials_present: bool,
    }

    impl LegacyPythonCredentialScrubAuthorityV1 {
        #[must_use]
        pub const fn noncredential_digest(&self) -> [u8; 32] {
            self.noncredential_digest
        }

        #[must_use]
        pub const fn credentials_present(&self) -> bool {
            self.credentials_present
        }
    }

    impl std::fmt::Debug for LegacyPythonCredentialScrubAuthorityV1 {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter
                .debug_struct("LegacyPythonCredentialScrubAuthorityV1")
                .field("config_kind", &self.config_kind)
                .field("credentials_present", &self.credentials_present)
                .field("account_locator_digest", &"[REDACTED]")
                .field("legacy_locator_digest", &"[REDACTED]")
                .field("document_digest", &"[REDACTED]")
                .field("noncredential_digest", &"[REDACTED]")
                .finish_non_exhaustive()
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum LegacyPythonCredentialScrubResultV1 {
        VerifiedAbsent { noncredential_digest: [u8; 32] },
        VerifiedScrubbed { noncredential_digest: [u8; 32] },
        Changed,
        Unavailable,
        Malformed,
    }

    #[derive(Clone)]
    pub struct HouseholdKeyBroker {
        executable: PathBuf,
        root: PathBuf,
        expected_native_root_instance_digest: [u8; 32],
        deadline: Duration,
    }

    impl std::fmt::Debug for HouseholdKeyBroker {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter
                .debug_struct("HouseholdKeyBroker")
                .field("deadline", &self.deadline)
                .finish_non_exhaustive()
        }
    }

    impl HouseholdKeyBroker {
        pub fn from_native_paths(
            paths: &crate::NativePaths,
            deadline: Duration,
        ) -> Result<Self, PortError> {
            Self::open(paths.data_dir().to_owned(), deadline)
        }

        fn open(root: impl Into<PathBuf>, deadline: Duration) -> Result<Self, PortError> {
            if deadline.is_zero() || deadline > Duration::from_secs(30) {
                return Err(PortError::new(
                    "household_broker_deadline",
                    "household broker deadline must be between 1ns and 30s",
                ));
            }
            let root = root.into();
            if !root.is_absolute() {
                return Err(PortError::new(
                    "household_broker_root",
                    "household broker root must be absolute",
                ));
            }
            let expected_native_root_instance_digest =
                household_native_root_instance_digest_v1(&root)?;
            let executable = std::env::current_exe().map_err(|_| {
                PortError::new(
                    "household_broker_executable",
                    "household broker executable is unavailable",
                )
            })?;
            if !executable.is_file() {
                return Err(PortError::new(
                    "household_broker_executable",
                    "household broker executable is unavailable",
                ));
            }
            Ok(Self {
                executable,
                root,
                expected_native_root_instance_digest,
                deadline,
            })
        }

        pub async fn probe(&self, cancellation: CancellationToken) -> Result<(), PortError> {
            self.request(
                HouseholdBrokerOperationV1::SecureStoreProbe,
                Zeroizing::new(Vec::new()),
                cancellation,
                false,
            )
            .await
            .map(|_| ())
        }

        /// Inspect one of the two frozen historical Python keyring targets
        /// without returning any household or credential value.
        pub async fn legacy_python_household_probe(
            &self,
            lifecycle_lease: &HouseholdLifecycleLease,
            config_kind: LegacyPythonConfigKindV1,
            resolved_config_path: &Path,
            cancellation: CancellationToken,
        ) -> Result<LegacyPythonHouseholdProbeResultV1, PortError> {
            let account_slot = lifecycle_account_slot(lifecycle_lease)?;
            self.validate_slot(&account_slot)?;
            let target =
                LegacyPythonTargetWire::new(config_kind, resolved_config_path, &account_slot)?;
            let request = LegacyPythonBrokerRequestWire::new(
                HouseholdBrokerOperationV1::LegacyPythonHouseholdProbe,
                &account_slot,
                target.clone(),
            );
            let response = decode_legacy_python_response(
                &self
                    .request(
                        HouseholdBrokerOperationV1::LegacyPythonHouseholdProbe,
                        encode_legacy_python_request(&request)?,
                        cancellation,
                        false,
                    )
                    .await?,
            )?;
            validate_legacy_python_response(
                &response,
                HouseholdBrokerOperationV1::LegacyPythonHouseholdProbe,
                &account_slot,
                &target,
            )?;
            let result = match response.status {
                LegacyPythonBrokerStatusWire::AuthoritativeMissing => {
                    require_legacy_payload_absent(&response)?;
                    LegacyPythonHouseholdProbeResultV1::AuthoritativeMissing
                }
                LegacyPythonBrokerStatusWire::PresentHousehold
                | LegacyPythonBrokerStatusWire::PresentNoHousehold => {
                    require_legacy_payload_absent(&response)?;
                    LegacyPythonHouseholdProbeResultV1::Present(
                        LegacyPythonHouseholdLoadAuthorityV1 {
                            account_slot: account_slot.clone(),
                            config_kind,
                            resolved_config_path: resolved_config_path.to_owned(),
                            locator_digest: decode_lower_hex_32(&target.locator_digest)?,
                        },
                    )
                }
                LegacyPythonBrokerStatusWire::Unavailable => {
                    require_legacy_payload_absent(&response)?;
                    LegacyPythonHouseholdProbeResultV1::Unavailable
                }
                status => return Err(legacy_python_status_error(status)),
            };
            lifecycle_lease.validate_for(&account_slot)?;
            Ok(result)
        }

        /// Load only the three historical household objects authorized by a
        /// successful probe. The raw keyring document and every credential or
        /// unknown field remain confined to the broker child.
        pub async fn legacy_python_household_load(
            &self,
            lifecycle_lease: &HouseholdLifecycleLease,
            authority: &LegacyPythonHouseholdLoadAuthorityV1,
            cancellation: CancellationToken,
        ) -> Result<Vec<u8>, PortError> {
            let account_slot = lifecycle_account_slot(lifecycle_lease)?;
            self.validate_slot(&account_slot)?;
            if account_slot != authority.account_slot {
                return Err(PortError::new(
                    "legacy_python_broker_binding",
                    "historical Python keyring load authority does not match the account",
                ));
            }
            let target = LegacyPythonTargetWire::new(
                authority.config_kind,
                &authority.resolved_config_path,
                &account_slot,
            )?;
            if decode_lower_hex_32(&target.locator_digest)? != authority.locator_digest {
                return Err(PortError::new(
                    "legacy_python_broker_binding",
                    "historical Python keyring load authority changed",
                ));
            }
            let request = LegacyPythonBrokerRequestWire::new(
                HouseholdBrokerOperationV1::LegacyPythonHouseholdLoad,
                &account_slot,
                target.clone(),
            );
            let response = decode_legacy_python_response(
                &self
                    .request(
                        HouseholdBrokerOperationV1::LegacyPythonHouseholdLoad,
                        encode_legacy_python_request(&request)?,
                        cancellation,
                        false,
                    )
                    .await?,
            )?;
            validate_legacy_python_response(
                &response,
                HouseholdBrokerOperationV1::LegacyPythonHouseholdLoad,
                &account_slot,
                &target,
            )?;
            let result = match response.status {
                LegacyPythonBrokerStatusWire::PresentHousehold
                | LegacyPythonBrokerStatusWire::PresentNoHousehold => response
                    .payload
                    .as_ref()
                    .ok_or_else(household_document_error)?
                    .canonical_household_document(),
                LegacyPythonBrokerStatusWire::AuthoritativeMissing => Err(PortError::new(
                    "legacy_python_source_changed",
                    "historical Python keyring source changed after its authoritative probe",
                )),
                LegacyPythonBrokerStatusWire::Unavailable => Err(PortError::new(
                    "legacy_python_source_probe_unavailable",
                    "historical Python keyring target became unavailable during load",
                )),
                status => Err(legacy_python_status_error(status)),
            };
            lifecycle_lease.validate_for(&account_slot)?;
            result
        }

        /// Produce the exact raw outcome accepted by the migration converter.
        /// Present sources always cross both independently bounded operations.
        pub async fn legacy_python_household_probe_and_load(
            &self,
            lifecycle_lease: &HouseholdLifecycleLease,
            config_kind: LegacyPythonConfigKindV1,
            resolved_config_path: &Path,
            cancellation: CancellationToken,
        ) -> Result<LegacyPythonKeyringProbeOutcomeV1, PortError> {
            match self
                .legacy_python_household_probe(
                    lifecycle_lease,
                    config_kind,
                    resolved_config_path,
                    cancellation.child_token(),
                )
                .await?
            {
                LegacyPythonHouseholdProbeResultV1::AuthoritativeMissing => {
                    Ok(LegacyPythonKeyringProbeOutcomeV1::AuthoritativeMissing)
                }
                LegacyPythonHouseholdProbeResultV1::Unavailable => {
                    Ok(LegacyPythonKeyringProbeOutcomeV1::Unavailable)
                }
                LegacyPythonHouseholdProbeResultV1::Present(authority) => self
                    .legacy_python_household_load(lifecycle_lease, &authority, cancellation)
                    .await
                    .map(LegacyPythonKeyringProbeOutcomeV1::Present),
            }
        }

        /// Probe one frozen historical keyring credential target without
        /// returning its values. The retained projection digest excludes only
        /// the exact released Python credential fields.
        pub async fn legacy_python_credentials_probe(
            &self,
            lifecycle_lease: &HouseholdLifecycleLease,
            config_kind: LegacyPythonConfigKindV1,
            resolved_config_path: &Path,
            cancellation: CancellationToken,
        ) -> Result<LegacyPythonCredentialProbeResultV1, PortError> {
            let account_slot = lifecycle_account_slot(lifecycle_lease)?;
            let result = self
                .legacy_python_credentials_probe_for_slot(
                    &account_slot,
                    config_kind,
                    resolved_config_path,
                    cancellation,
                )
                .await?;
            lifecycle_lease.validate_for(&account_slot)?;
            Ok(result)
        }

        pub(crate) async fn legacy_python_credentials_probe_with_source_lease(
            &self,
            source_lease: &LegacyPythonCredentialSourceLeaseV1,
            config_kind: LegacyPythonConfigKindV1,
            resolved_config_path: &Path,
            cancellation: CancellationToken,
        ) -> Result<LegacyPythonCredentialProbeResultV1, PortError> {
            let account_slot = source_lease
                .account_slot_for_target(config_kind, resolved_config_path)?
                .clone();
            let result = self
                .legacy_python_credentials_probe_for_slot(
                    &account_slot,
                    config_kind,
                    resolved_config_path,
                    cancellation,
                )
                .await?;
            if source_lease.account_slot_for_target(config_kind, resolved_config_path)?
                != &account_slot
            {
                return Err(PortError::new(
                    "legacy_python_credential_binding",
                    "historical credential source authority changed",
                ));
            }
            Ok(result)
        }

        async fn legacy_python_credentials_probe_for_slot(
            &self,
            account_slot: &HouseholdAccountSlotV1,
            config_kind: LegacyPythonConfigKindV1,
            resolved_config_path: &Path,
            cancellation: CancellationToken,
        ) -> Result<LegacyPythonCredentialProbeResultV1, PortError> {
            self.validate_slot(account_slot)?;
            let target =
                LegacyPythonTargetWire::new(config_kind, resolved_config_path, account_slot)?;
            let request = LegacyPythonCredentialRequestWire::probe(account_slot, target.clone());
            let response = decode_legacy_python_credential_response(
                &self
                    .request(
                        HouseholdBrokerOperationV1::LegacyPythonCredentialsScrubAndVerify,
                        encode_legacy_python_credential_request(&request)?,
                        cancellation,
                        false,
                    )
                    .await?,
            )?;
            validate_legacy_python_credential_response(&response, &request, account_slot, &target)?;
            let result = match response.status {
                LegacyPythonCredentialStatusWire::AuthoritativeMissing => {
                    LegacyPythonCredentialProbeResultV1::AuthoritativeMissing {
                        noncredential_digest: decode_lower_hex_32(
                            response
                                .noncredential_digest
                                .as_deref()
                                .ok_or_else(household_document_error)?,
                        )?,
                    }
                }
                LegacyPythonCredentialStatusWire::PresentCredentials
                | LegacyPythonCredentialStatusWire::PresentNoCredentials => {
                    let credentials_present =
                        response.status == LegacyPythonCredentialStatusWire::PresentCredentials;
                    LegacyPythonCredentialProbeResultV1::Present(Box::new(
                        LegacyPythonCredentialScrubAuthorityV1 {
                            account_slot: account_slot.clone(),
                            config_kind,
                            resolved_config_path: resolved_config_path.to_owned(),
                            locator_digest: decode_lower_hex_32(&target.locator_digest)?,
                            document_digest: decode_lower_hex_32(
                                response
                                    .document_digest
                                    .as_deref()
                                    .ok_or_else(household_document_error)?,
                            )?,
                            noncredential_digest: decode_lower_hex_32(
                                response
                                    .noncredential_digest
                                    .as_deref()
                                    .ok_or_else(household_document_error)?,
                            )?,
                            credentials_present,
                        },
                    ))
                }
                LegacyPythonCredentialStatusWire::Unavailable => {
                    LegacyPythonCredentialProbeResultV1::Unavailable
                }
                LegacyPythonCredentialStatusWire::Malformed
                | LegacyPythonCredentialStatusWire::Oversized => {
                    LegacyPythonCredentialProbeResultV1::Malformed
                }
                _ => return Err(household_document_error()),
            };
            Ok(result)
        }

        /// Scrub and verify the exact document authorized by the immediately
        /// preceding content-free probe. A changed document is not retried or
        /// silently rebound.
        pub async fn legacy_python_credentials_scrub_and_verify(
            &self,
            lifecycle_lease: &HouseholdLifecycleLease,
            authority: &LegacyPythonCredentialScrubAuthorityV1,
            cancellation: CancellationToken,
        ) -> Result<LegacyPythonCredentialScrubResultV1, PortError> {
            let account_slot = lifecycle_account_slot(lifecycle_lease)?;
            let result = self
                .legacy_python_credentials_scrub_and_verify_for_slot(
                    &account_slot,
                    authority,
                    cancellation,
                )
                .await?;
            lifecycle_lease.validate_for(&account_slot)?;
            Ok(result)
        }

        pub(crate) async fn legacy_python_credentials_scrub_and_verify_with_source_lease(
            &self,
            source_lease: &LegacyPythonCredentialSourceLeaseV1,
            authority: &LegacyPythonCredentialScrubAuthorityV1,
            cancellation: CancellationToken,
        ) -> Result<LegacyPythonCredentialScrubResultV1, PortError> {
            let account_slot = source_lease
                .account_slot_for_target(authority.config_kind, &authority.resolved_config_path)?
                .clone();
            let result = self
                .legacy_python_credentials_scrub_and_verify_for_slot(
                    &account_slot,
                    authority,
                    cancellation,
                )
                .await?;
            if source_lease
                .account_slot_for_target(authority.config_kind, &authority.resolved_config_path)?
                != &account_slot
            {
                return Err(PortError::new(
                    "legacy_python_credential_binding",
                    "historical credential source authority changed",
                ));
            }
            Ok(result)
        }

        async fn legacy_python_credentials_scrub_and_verify_for_slot(
            &self,
            account_slot: &HouseholdAccountSlotV1,
            authority: &LegacyPythonCredentialScrubAuthorityV1,
            cancellation: CancellationToken,
        ) -> Result<LegacyPythonCredentialScrubResultV1, PortError> {
            self.validate_slot(account_slot)?;
            if *account_slot != authority.account_slot {
                return Err(PortError::new(
                    "legacy_python_credential_binding",
                    "historical credential scrub authority does not match the account",
                ));
            }
            let target = LegacyPythonTargetWire::new(
                authority.config_kind,
                &authority.resolved_config_path,
                account_slot,
            )?;
            if decode_lower_hex_32(&target.locator_digest)? != authority.locator_digest {
                return Err(PortError::new(
                    "legacy_python_credential_binding",
                    "historical credential scrub authority changed",
                ));
            }
            let request = LegacyPythonCredentialRequestWire::scrub(
                account_slot,
                target.clone(),
                authority.document_digest,
                authority.noncredential_digest,
            );
            let response = decode_legacy_python_credential_response(
                &self
                    .request(
                        HouseholdBrokerOperationV1::LegacyPythonCredentialsScrubAndVerify,
                        encode_legacy_python_credential_request(&request)?,
                        cancellation,
                        true,
                    )
                    .await?,
            )?;
            validate_legacy_python_credential_response(&response, &request, account_slot, &target)?;
            let digest = || {
                decode_lower_hex_32(
                    response
                        .noncredential_digest
                        .as_deref()
                        .ok_or_else(household_document_error)?,
                )
            };
            let result = match response.status {
                LegacyPythonCredentialStatusWire::AuthoritativeMissing
                | LegacyPythonCredentialStatusWire::VerifiedAbsent => {
                    LegacyPythonCredentialScrubResultV1::VerifiedAbsent {
                        noncredential_digest: digest()?,
                    }
                }
                LegacyPythonCredentialStatusWire::VerifiedScrubbed => {
                    LegacyPythonCredentialScrubResultV1::VerifiedScrubbed {
                        noncredential_digest: digest()?,
                    }
                }
                LegacyPythonCredentialStatusWire::Changed => {
                    LegacyPythonCredentialScrubResultV1::Changed
                }
                LegacyPythonCredentialStatusWire::Unavailable => {
                    LegacyPythonCredentialScrubResultV1::Unavailable
                }
                LegacyPythonCredentialStatusWire::Malformed
                | LegacyPythonCredentialStatusWire::Oversized => {
                    LegacyPythonCredentialScrubResultV1::Malformed
                }
                _ => return Err(household_document_error()),
            };
            Ok(result)
        }

        fn validate_slot(&self, account_slot: &HouseholdAccountSlotV1) -> Result<(), PortError> {
            if account_slot.native_root_instance_digest()
                != self.expected_native_root_instance_digest
            {
                return Err(PortError::new(
                    "household_broker_root_mismatch",
                    "household account slot does not match the broker root",
                ));
            }
            Ok(())
        }

        async fn request(
            &self,
            operation: HouseholdBrokerOperationV1,
            input: Zeroizing<Vec<u8>>,
            cancellation: CancellationToken,
            outcome_uncertain: bool,
        ) -> Result<Zeroizing<Vec<u8>>, PortError> {
            if cancellation.is_cancelled() {
                return Err(household_cancelled_error());
            }
            if input.len() > MAX_BROKER_DOCUMENT_BYTES {
                return Err(PortError::new(
                    "household_broker_size",
                    "household broker input exceeds its limit",
                ));
            }
            let child = Command::new(&self.executable)
                .arg(BROKER_MODE)
                .arg(operation.action())
                .arg(&self.root)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|_| {
                    PortError::new(
                        "household_broker_spawn",
                        "household broker could not be started",
                    )
                })?;
            run_household_bounded_child(
                child,
                input,
                self.deadline,
                cancellation,
                operation.response_limit(),
                outcome_uncertain,
            )
            .await
        }
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct HouseholdSlotWire {
        account_digest: String,
        account_locator_digest: String,
        directory_name: String,
        native_root_instance_digest: String,
    }

    impl HouseholdSlotWire {
        fn from_slot(slot: &HouseholdAccountSlotV1) -> Self {
            Self {
                account_digest: hex_digest(slot.account_digest()),
                account_locator_digest: hex_digest(slot.account_locator_digest()),
                directory_name: slot.directory_name().to_owned(),
                native_root_instance_digest: hex_digest(slot.native_root_instance_digest()),
            }
        }

        fn decode(&self) -> Result<HouseholdAccountSlotV1, PortError> {
            HouseholdAccountSlotV1::from_components(
                decode_lower_hex_32(&self.account_digest)?,
                decode_lower_hex_32(&self.native_root_instance_digest)?,
                decode_lower_hex_32(&self.account_locator_digest)?,
                self.directory_name.clone(),
            )
        }
    }

    #[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
    #[serde(deny_unknown_fields)]
    struct LegacyPythonTargetWire {
        config_kind: String,
        locator_digest: String,
        resolved_config_path: String,
    }

    impl LegacyPythonTargetWire {
        fn new(
            config_kind: LegacyPythonConfigKindV1,
            resolved_config_path: &Path,
            account_slot: &HouseholdAccountSlotV1,
        ) -> Result<Self, PortError> {
            let resolved_config_path = resolved_config_path.to_str().ok_or_else(|| {
                PortError::new(
                    "legacy_python_keyring_locator",
                    "historical Python keyring path is not valid UTF-8",
                )
            })?;
            validate_frozen_legacy_config_path(config_kind, Path::new(resolved_config_path))?;
            let locator = LegacyPythonKeyringLocatorV1::from_resolved_config_path_bytes(
                resolved_config_path.as_bytes(),
            )?;
            let value = Self {
                config_kind: legacy_config_kind_name(config_kind).to_owned(),
                locator_digest: locator.locator_digest()?.to_lower_hex(),
                resolved_config_path: resolved_config_path.to_owned(),
            };
            value.validate(account_slot)?;
            Ok(value)
        }

        fn validate(
            &self,
            _account_slot: &HouseholdAccountSlotV1,
        ) -> Result<LegacyPythonConfigKindV1, PortError> {
            let config_kind = decode_legacy_config_kind(&self.config_kind)?;
            validate_frozen_legacy_config_path(config_kind, Path::new(&self.resolved_config_path))?;
            let locator = LegacyPythonKeyringLocatorV1::from_resolved_config_path_bytes(
                self.resolved_config_path.as_bytes(),
            )?;
            if locator.locator_digest()?.as_bytes() != &decode_lower_hex_32(&self.locator_digest)? {
                return Err(PortError::new(
                    "legacy_python_broker_binding",
                    "historical Python keyring locator binding is invalid",
                ));
            }
            Ok(config_kind)
        }
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct LegacyPythonBrokerRequestWire {
        operation: String,
        schema_version: u16,
        slot: HouseholdSlotWire,
        target: LegacyPythonTargetWire,
    }

    impl LegacyPythonBrokerRequestWire {
        fn new(
            operation: HouseholdBrokerOperationV1,
            account_slot: &HouseholdAccountSlotV1,
            target: LegacyPythonTargetWire,
        ) -> Self {
            Self {
                operation: operation.action().to_owned(),
                schema_version: 1,
                slot: HouseholdSlotWire::from_slot(account_slot),
                target,
            }
        }

        fn validate(
            &self,
            expected_operation: HouseholdBrokerOperationV1,
            native_root: &Path,
        ) -> Result<(HouseholdAccountSlotV1, LegacyPythonConfigKindV1), PortError> {
            if self.schema_version != 1 || self.operation != expected_operation.action() {
                return Err(PortError::new(
                    "legacy_python_broker_binding",
                    "historical Python keyring operation binding is invalid",
                ));
            }
            let slot = self.slot.decode()?;
            if slot.native_root_instance_digest()
                != household_native_root_instance_digest_v1(native_root)?
            {
                return Err(PortError::new(
                    "household_broker_root_mismatch",
                    "household account slot does not match the broker root",
                ));
            }
            let config_kind = self.target.validate(&slot)?;
            Ok((slot, config_kind))
        }
    }

    #[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
    #[serde(rename_all = "snake_case")]
    enum LegacyPythonBrokerStatusWire {
        AuthoritativeMissing,
        PresentHousehold,
        PresentNoHousehold,
        Unavailable,
        MalformedJson,
        OversizedEntry,
        InvalidHouseholdSubdocument,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct LegacyPythonHouseholdPayloadWire {
        #[serde(rename = "household.local_profiles")]
        local_profiles: Option<Map<String, Value>>,
        local_profiles_digest: Option<String>,
        payload_digest: String,
        #[serde(rename = "household.profile_outbox")]
        profile_outbox: Option<Map<String, Value>>,
        profile_outbox_digest: Option<String>,
        #[serde(rename = "household.state")]
        state: Option<Map<String, Value>>,
        state_digest: Option<String>,
    }

    impl LegacyPythonHouseholdPayloadWire {
        fn from_objects(
            state: Option<Map<String, Value>>,
            local_profiles: Option<Map<String, Value>>,
            profile_outbox: Option<Map<String, Value>>,
        ) -> Result<Self, PortError> {
            let mut value = Self {
                state_digest: canonical_optional_object_digest(state.as_ref())?,
                local_profiles_digest: canonical_optional_object_digest(local_profiles.as_ref())?,
                profile_outbox_digest: canonical_optional_object_digest(profile_outbox.as_ref())?,
                state,
                local_profiles,
                profile_outbox,
                payload_digest: String::new(),
            };
            value.payload_digest = hex_digest(Sha256::digest(
                value.canonical_household_document_without_digest_validation()?,
            ));
            Ok(value)
        }

        fn canonical_household_document(&self) -> Result<Vec<u8>, PortError> {
            if canonical_optional_object_digest(self.state.as_ref())? != self.state_digest
                || canonical_optional_object_digest(self.local_profiles.as_ref())?
                    != self.local_profiles_digest
                || canonical_optional_object_digest(self.profile_outbox.as_ref())?
                    != self.profile_outbox_digest
            {
                return Err(household_document_error());
            }
            let canonical = self.canonical_household_document_without_digest_validation()?;
            let actual_digest: [u8; 32] = Sha256::digest(&canonical).into();
            if decode_lower_hex_32(&self.payload_digest)? != actual_digest {
                return Err(household_document_error());
            }
            Ok(canonical)
        }

        fn canonical_household_document_without_digest_validation(
            &self,
        ) -> Result<Vec<u8>, PortError> {
            let mut object = Map::new();
            if let Some(value) = &self.state {
                object.insert("household.state".to_owned(), Value::Object(value.clone()));
            }
            if let Some(value) = &self.local_profiles {
                object.insert(
                    "household.local_profiles".to_owned(),
                    Value::Object(value.clone()),
                );
            }
            if let Some(value) = &self.profile_outbox {
                object.insert(
                    "household.profile_outbox".to_owned(),
                    Value::Object(value.clone()),
                );
            }
            let canonical = to_canonical_bytes_v1(&Value::Object(object)).map_err(|_| {
                PortError::new(
                    "legacy_python_keyring_format",
                    "historical Python keyring household state is invalid",
                )
            })?;
            if canonical.len() > 4 * 1024 * 1024 {
                return Err(PortError::new(
                    "legacy_python_keyring_size",
                    "historical Python keyring household state exceeds its limit",
                ));
            }
            Ok(canonical)
        }

        fn has_household_data(&self) -> bool {
            self.state.is_some() || self.local_profiles.is_some() || self.profile_outbox.is_some()
        }
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct LegacyPythonBrokerResponseWire {
        operation: String,
        payload: Option<LegacyPythonHouseholdPayloadWire>,
        schema_version: u16,
        slot: HouseholdSlotWire,
        status: LegacyPythonBrokerStatusWire,
        target: LegacyPythonTargetWire,
    }

    impl LegacyPythonBrokerResponseWire {
        fn status(
            request: &LegacyPythonBrokerRequestWire,
            status: LegacyPythonBrokerStatusWire,
            payload: Option<LegacyPythonHouseholdPayloadWire>,
        ) -> Self {
            Self {
                operation: request.operation.clone(),
                payload,
                schema_version: 1,
                slot: request.slot.clone(),
                status,
                target: request.target.clone(),
            }
        }
    }

    #[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
    #[serde(rename_all = "snake_case")]
    enum LegacyPythonCredentialActionWire {
        Probe,
        ScrubAndVerify,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct LegacyPythonCredentialRequestWire {
        action: LegacyPythonCredentialActionWire,
        expected_document_digest: Option<String>,
        expected_noncredential_digest: Option<String>,
        operation: String,
        schema_version: u16,
        slot: HouseholdSlotWire,
        target: LegacyPythonTargetWire,
    }

    impl LegacyPythonCredentialRequestWire {
        fn probe(account_slot: &HouseholdAccountSlotV1, target: LegacyPythonTargetWire) -> Self {
            Self {
                action: LegacyPythonCredentialActionWire::Probe,
                expected_document_digest: None,
                expected_noncredential_digest: None,
                operation: HouseholdBrokerOperationV1::LegacyPythonCredentialsScrubAndVerify
                    .action()
                    .to_owned(),
                schema_version: 1,
                slot: HouseholdSlotWire::from_slot(account_slot),
                target,
            }
        }

        fn scrub(
            account_slot: &HouseholdAccountSlotV1,
            target: LegacyPythonTargetWire,
            expected_document_digest: [u8; 32],
            expected_noncredential_digest: [u8; 32],
        ) -> Self {
            Self {
                action: LegacyPythonCredentialActionWire::ScrubAndVerify,
                expected_document_digest: Some(hex_digest(expected_document_digest)),
                expected_noncredential_digest: Some(hex_digest(expected_noncredential_digest)),
                operation: HouseholdBrokerOperationV1::LegacyPythonCredentialsScrubAndVerify
                    .action()
                    .to_owned(),
                schema_version: 1,
                slot: HouseholdSlotWire::from_slot(account_slot),
                target,
            }
        }

        fn validate(
            &self,
            native_root: &Path,
        ) -> Result<(HouseholdAccountSlotV1, LegacyPythonConfigKindV1), PortError> {
            if self.schema_version != 1
                || self.operation
                    != HouseholdBrokerOperationV1::LegacyPythonCredentialsScrubAndVerify.action()
            {
                return Err(PortError::new(
                    "legacy_python_credential_binding",
                    "historical credential operation binding is invalid",
                ));
            }
            match self.action {
                LegacyPythonCredentialActionWire::Probe
                    if self.expected_document_digest.is_none()
                        && self.expected_noncredential_digest.is_none() => {}
                LegacyPythonCredentialActionWire::ScrubAndVerify
                    if self.expected_document_digest.is_some()
                        && self.expected_noncredential_digest.is_some() =>
                {
                    decode_lower_hex_32(
                        self.expected_document_digest
                            .as_deref()
                            .ok_or_else(household_document_error)?,
                    )?;
                    decode_lower_hex_32(
                        self.expected_noncredential_digest
                            .as_deref()
                            .ok_or_else(household_document_error)?,
                    )?;
                }
                _ => return Err(household_document_error()),
            }
            let slot = self.slot.decode()?;
            if slot.native_root_instance_digest()
                != household_native_root_instance_digest_v1(native_root)?
            {
                return Err(PortError::new(
                    "household_broker_root_mismatch",
                    "household account slot does not match the broker root",
                ));
            }
            let config_kind = self.target.validate(&slot)?;
            Ok((slot, config_kind))
        }
    }

    #[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
    #[serde(rename_all = "snake_case")]
    enum LegacyPythonCredentialStatusWire {
        AuthoritativeMissing,
        PresentCredentials,
        PresentNoCredentials,
        VerifiedAbsent,
        VerifiedScrubbed,
        Changed,
        Unavailable,
        Malformed,
        Oversized,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct LegacyPythonCredentialResponseWire {
        action: LegacyPythonCredentialActionWire,
        document_digest: Option<String>,
        noncredential_digest: Option<String>,
        operation: String,
        schema_version: u16,
        slot: HouseholdSlotWire,
        status: LegacyPythonCredentialStatusWire,
        target: LegacyPythonTargetWire,
    }

    impl LegacyPythonCredentialResponseWire {
        fn new(
            request: &LegacyPythonCredentialRequestWire,
            status: LegacyPythonCredentialStatusWire,
            document_digest: Option<[u8; 32]>,
            noncredential_digest: Option<[u8; 32]>,
        ) -> Self {
            Self {
                action: request.action,
                document_digest: document_digest.map(hex_digest),
                noncredential_digest: noncredential_digest.map(hex_digest),
                operation: request.operation.clone(),
                schema_version: 1,
                slot: request.slot.clone(),
                status,
                target: request.target.clone(),
            }
        }
    }

    #[derive(Clone, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct PreviousKeyWire {
        key: Zeroizing<String>,
        key_id: Uuid,
    }

    #[derive(Clone, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct HouseholdCommitEvidenceRecordWire {
        #[serde(rename = "c")]
        commit_id: Uuid,
        #[serde(rename = "p")]
        proposal_ref_hash: String,
        #[serde(rename = "s")]
        state: String,
        #[serde(rename = "x")]
        expires_at_unix_seconds: u64,
    }

    #[derive(Clone, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct HouseholdKeyBundleWire {
        account_digest: String,
        account_locator_digest: String,
        active_key: Zeroizing<String>,
        active_key_id: Uuid,
        commit_evidence_key: Option<Zeroizing<String>>,
        #[serde(default)]
        commit_evidence_records: Vec<HouseholdCommitEvidenceRecordWire>,
        initial_commit_id: Option<Uuid>,
        initial_effect_fingerprint: Option<String>,
        initial_state_digest: Option<String>,
        initialization_id: Option<Uuid>,
        native_root_instance_digest: String,
        phase: String,
        previous_key: Option<PreviousKeyWire>,
        revision: u64,
        rotation_id: Option<Uuid>,
        schema_version: u16,
    }

    impl HouseholdKeyBundleWire {
        fn from_bundle(bundle: &HouseholdKeyBundle) -> Self {
            Self {
                account_digest: hex_digest(bundle.account_digest),
                account_locator_digest: hex_digest(bundle.account_locator_digest),
                active_key: Zeroizing::new(hex_digest(bundle.active_key.expose())),
                active_key_id: bundle.active_key_id.as_uuid(),
                commit_evidence_key: Some(Zeroizing::new(hex_digest(
                    bundle.commit_evidence_key.expose(),
                ))),
                commit_evidence_records: bundle
                    .commit_evidence_records
                    .iter()
                    .map(|record| HouseholdCommitEvidenceRecordWire {
                        commit_id: record.commit_id.as_uuid(),
                        proposal_ref_hash: hex_digest(record.proposal_ref_hash),
                        state: match record.state {
                            HouseholdCommitEvidenceStateV1::Reserved => "reserved",
                            HouseholdCommitEvidenceStateV1::Denied => "denied",
                        }
                        .to_owned(),
                        expires_at_unix_seconds: record.expires_at_unix_seconds,
                    })
                    .collect(),
                initial_commit_id: bundle.initial_commit_id,
                initial_effect_fingerprint: bundle.initial_effect_fingerprint.map(hex_digest),
                initial_state_digest: bundle.initial_state_digest.map(hex_digest),
                initialization_id: bundle.initialization_id,
                native_root_instance_digest: hex_digest(bundle.native_root_instance_digest),
                phase: match bundle.phase {
                    HouseholdKeyBundlePhase::Initializing => "initializing",
                    HouseholdKeyBundlePhase::Stable => "stable",
                    HouseholdKeyBundlePhase::Rewriting => "rewriting",
                }
                .to_owned(),
                previous_key: bundle
                    .previous_key
                    .as_ref()
                    .map(|(key_id, key)| PreviousKeyWire {
                        key: Zeroizing::new(hex_digest(key.expose())),
                        key_id: key_id.as_uuid(),
                    }),
                revision: bundle.revision.get(),
                rotation_id: bundle.rotation_id,
                schema_version: 2,
            }
        }

        fn decode(
            &self,
            account_slot: &HouseholdAccountSlotV1,
        ) -> Result<HouseholdKeyBundle, PortError> {
            if !matches!(self.schema_version, 1 | 2) {
                return Err(household_document_error());
            }
            let phase = match self.phase.as_str() {
                "initializing" => HouseholdKeyBundlePhase::Initializing,
                "stable" => HouseholdKeyBundlePhase::Stable,
                "rewriting" => HouseholdKeyBundlePhase::Rewriting,
                _ => return Err(household_document_error()),
            };
            let active_key = decode_household_key_material(&self.active_key)?;
            let commit_evidence_key = match (
                self.schema_version,
                self.commit_evidence_key.as_ref(),
                self.commit_evidence_records.is_empty(),
            ) {
                (1, None, true) => derive_commit_evidence_root_key(&active_key),
                (2, Some(value), _) => decode_household_key_material(value)?,
                _ => return Err(household_document_error()),
            };
            let bundle = HouseholdKeyBundle {
                account_digest: decode_lower_hex_32(&self.account_digest)?,
                native_root_instance_digest: decode_lower_hex_32(
                    &self.native_root_instance_digest,
                )?,
                account_locator_digest: decode_lower_hex_32(&self.account_locator_digest)?,
                revision: KeyBundleRevision::new(self.revision)?,
                active_key_id: KeyId::from_uuid(self.active_key_id),
                active_key,
                commit_evidence_key,
                commit_evidence_records: self
                    .commit_evidence_records
                    .iter()
                    .map(|record| {
                        let state = match record.state.as_str() {
                            "reserved" => HouseholdCommitEvidenceStateV1::Reserved,
                            "denied" => HouseholdCommitEvidenceStateV1::Denied,
                            _ => return Err(household_document_error()),
                        };
                        Ok(HouseholdCommitEvidenceRecordV1 {
                            proposal_ref_hash: decode_lower_hex_32(&record.proposal_ref_hash)?,
                            commit_id: CommitId::from_uuid(record.commit_id),
                            state,
                            expires_at_unix_seconds: record.expires_at_unix_seconds,
                        })
                    })
                    .collect::<Result<Vec<_>, PortError>>()?,
                previous_key: self
                    .previous_key
                    .as_ref()
                    .map(|previous| {
                        decode_household_key_material(&previous.key)
                            .map(|key| (KeyId::from_uuid(previous.key_id), key))
                    })
                    .transpose()?,
                initialization_id: self.initialization_id,
                initial_commit_id: self.initial_commit_id,
                initial_effect_fingerprint: self
                    .initial_effect_fingerprint
                    .as_deref()
                    .map(decode_lower_hex_32)
                    .transpose()?,
                initial_state_digest: self
                    .initial_state_digest
                    .as_deref()
                    .map(decode_lower_hex_32)
                    .transpose()?,
                rotation_id: self.rotation_id,
                phase,
            };
            bundle.validate_for(account_slot)?;
            Ok(bundle)
        }
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct HouseholdMigrationGuardWire {
        account_digest: String,
        account_locator_digest: String,
        canonical_document: String,
        guard_revision: u64,
        native_root_instance_digest: String,
        schema_version: u16,
    }

    impl HouseholdMigrationGuardWire {
        fn from_guard(guard: &HouseholdMigrationGuardDocument) -> Result<Self, PortError> {
            let canonical_document = String::from_utf8(guard.canonical_bytes()?)
                .map_err(|_| migration_guard_invalid_error())?;
            Ok(Self {
                account_digest: guard.account_digest.to_lower_hex(),
                account_locator_digest: guard.account_locator_digest.to_lower_hex(),
                canonical_document,
                guard_revision: guard.guard_revision(),
                native_root_instance_digest: guard.native_root_instance_digest.to_lower_hex(),
                schema_version: 1,
            })
        }

        fn decode(
            &self,
            account_slot: &HouseholdAccountSlotV1,
        ) -> Result<HouseholdMigrationGuardDocument, PortError> {
            if self.schema_version != 1 {
                return Err(household_document_error());
            }
            let guard = HouseholdMigrationGuardDocument::from_canonical_bytes(
                account_slot,
                self.canonical_document.as_bytes(),
            )?;
            if guard.account_digest.as_bytes() != &decode_lower_hex_32(&self.account_digest)?
                || guard.native_root_instance_digest.as_bytes()
                    != &decode_lower_hex_32(&self.native_root_instance_digest)?
                || guard.account_locator_digest.as_bytes()
                    != &decode_lower_hex_32(&self.account_locator_digest)?
                || guard.guard_revision() != self.guard_revision
            {
                return Err(migration_guard_quarantine_error());
            }
            Ok(guard)
        }
    }

    #[derive(Clone, Copy, Debug, Deserialize, Serialize)]
    #[serde(rename_all = "snake_case")]
    enum KeyExpectationWire {
        Absent,
        Revision,
    }

    #[derive(Clone, Copy, Debug, Deserialize, Serialize)]
    #[serde(rename_all = "snake_case")]
    enum GuardExpectationWire {
        Absent,
        Revision,
    }

    #[derive(Clone, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct HouseholdBrokerRequestWire {
        abort_initialization_id: Option<Uuid>,
        bundle: Option<HouseholdKeyBundleWire>,
        delete_guard: bool,
        expected_key_id: Option<Uuid>,
        expected_revision: Option<u64>,
        guard: Option<HouseholdMigrationGuardWire>,
        guard_expectation: Option<GuardExpectationWire>,
        key_expectation: Option<KeyExpectationWire>,
        slot: HouseholdSlotWire,
    }

    impl HouseholdBrokerRequestWire {
        fn load(slot: &HouseholdAccountSlotV1) -> Self {
            Self {
                abort_initialization_id: None,
                bundle: None,
                delete_guard: false,
                expected_key_id: None,
                expected_revision: None,
                guard: None,
                guard_expectation: None,
                key_expectation: None,
                slot: HouseholdSlotWire::from_slot(slot),
            }
        }
    }

    #[derive(Clone, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct HouseholdBrokerResponseWire {
        bundle: Option<HouseholdKeyBundleWire>,
        guard: Option<HouseholdMigrationGuardWire>,
        status: String,
    }

    fn encode_household_request(
        request: &HouseholdBrokerRequestWire,
    ) -> Result<Zeroizing<Vec<u8>>, PortError> {
        let encoded =
            Zeroizing::new(serde_json::to_vec(request).map_err(|_| household_document_error())?);
        if encoded.len() > MAX_BROKER_DOCUMENT_BYTES {
            return Err(PortError::new(
                "household_broker_size",
                "household broker request exceeds its limit",
            ));
        }
        Ok(encoded)
    }

    fn decode_household_response(bytes: &[u8]) -> Result<HouseholdBrokerResponseWire, PortError> {
        if bytes.is_empty() || bytes.len() > MAX_BROKER_DOCUMENT_BYTES {
            return Err(household_document_error());
        }
        serde_json::from_slice(bytes).map_err(|_| household_document_error())
    }

    fn encode_legacy_python_request(
        request: &LegacyPythonBrokerRequestWire,
    ) -> Result<Zeroizing<Vec<u8>>, PortError> {
        let encoded =
            Zeroizing::new(serde_json::to_vec(request).map_err(|_| household_document_error())?);
        if encoded.len() > MAX_BROKER_DOCUMENT_BYTES {
            return Err(PortError::new(
                "household_broker_size",
                "historical Python keyring broker request exceeds its limit",
            ));
        }
        Ok(encoded)
    }

    fn decode_legacy_python_response(
        bytes: &[u8],
    ) -> Result<LegacyPythonBrokerResponseWire, PortError> {
        if bytes.is_empty() || bytes.len() > MAX_LEGACY_HOUSEHOLD_BROKER_RESPONSE_BYTES {
            return Err(household_document_error());
        }
        serde_json::from_slice(bytes).map_err(|_| household_document_error())
    }

    fn encode_legacy_python_credential_request(
        request: &LegacyPythonCredentialRequestWire,
    ) -> Result<Zeroizing<Vec<u8>>, PortError> {
        let encoded =
            Zeroizing::new(serde_json::to_vec(request).map_err(|_| household_document_error())?);
        if encoded.len() > MAX_BROKER_DOCUMENT_BYTES {
            return Err(PortError::new(
                "household_broker_size",
                "historical Python credential broker request exceeds its limit",
            ));
        }
        Ok(encoded)
    }

    fn decode_legacy_python_credential_response(
        bytes: &[u8],
    ) -> Result<LegacyPythonCredentialResponseWire, PortError> {
        if bytes.is_empty() || bytes.len() > MAX_BROKER_DOCUMENT_BYTES {
            return Err(household_document_error());
        }
        serde_json::from_slice(bytes).map_err(|_| household_document_error())
    }

    fn validate_legacy_python_credential_response(
        response: &LegacyPythonCredentialResponseWire,
        request: &LegacyPythonCredentialRequestWire,
        account_slot: &HouseholdAccountSlotV1,
        target: &LegacyPythonTargetWire,
    ) -> Result<(), PortError> {
        if response.schema_version != 1
            || response.operation
                != HouseholdBrokerOperationV1::LegacyPythonCredentialsScrubAndVerify.action()
            || response.action != request.action
            || response.slot.decode()? != *account_slot
            || response.target != *target
        {
            return Err(PortError::new(
                "legacy_python_credential_binding",
                "historical Python credential response binding is invalid",
            ));
        }
        response.target.validate(account_slot)?;
        match response.status {
            LegacyPythonCredentialStatusWire::AuthoritativeMissing
            | LegacyPythonCredentialStatusWire::VerifiedAbsent => {
                if response.document_digest.is_some() || response.noncredential_digest.is_none() {
                    return Err(household_document_error());
                }
            }
            LegacyPythonCredentialStatusWire::PresentCredentials
            | LegacyPythonCredentialStatusWire::PresentNoCredentials => {
                if response.document_digest.is_none()
                    || response.noncredential_digest.is_none()
                    || response.action != LegacyPythonCredentialActionWire::Probe
                {
                    return Err(household_document_error());
                }
            }
            LegacyPythonCredentialStatusWire::VerifiedScrubbed => {
                if response.document_digest.is_some()
                    || response.noncredential_digest.is_none()
                    || response.action != LegacyPythonCredentialActionWire::ScrubAndVerify
                {
                    return Err(household_document_error());
                }
            }
            LegacyPythonCredentialStatusWire::Changed
            | LegacyPythonCredentialStatusWire::Unavailable
            | LegacyPythonCredentialStatusWire::Malformed
            | LegacyPythonCredentialStatusWire::Oversized => {
                if response.document_digest.is_some() || response.noncredential_digest.is_some() {
                    return Err(household_document_error());
                }
            }
        }
        if let Some(value) = response.document_digest.as_deref() {
            decode_lower_hex_32(value)?;
        }
        if let Some(value) = response.noncredential_digest.as_deref() {
            decode_lower_hex_32(value)?;
        }
        Ok(())
    }

    fn validate_legacy_python_response(
        response: &LegacyPythonBrokerResponseWire,
        operation: HouseholdBrokerOperationV1,
        account_slot: &HouseholdAccountSlotV1,
        target: &LegacyPythonTargetWire,
    ) -> Result<(), PortError> {
        if response.schema_version != 1
            || response.operation != operation.action()
            || response.slot.decode()? != *account_slot
            || response.target != *target
        {
            return Err(PortError::new(
                "legacy_python_broker_binding",
                "historical Python keyring response binding is invalid",
            ));
        }
        response.target.validate(account_slot)?;
        Ok(())
    }

    fn require_legacy_payload_absent(
        response: &LegacyPythonBrokerResponseWire,
    ) -> Result<(), PortError> {
        if response.payload.is_some() {
            Err(household_document_error())
        } else {
            Ok(())
        }
    }

    fn legacy_python_status_error(status: LegacyPythonBrokerStatusWire) -> PortError {
        match status {
            LegacyPythonBrokerStatusWire::MalformedJson => PortError::new(
                "legacy_python_keyring_malformed",
                "historical Python keyring entry is malformed",
            ),
            LegacyPythonBrokerStatusWire::OversizedEntry => PortError::new(
                "legacy_python_keyring_size",
                "historical Python keyring entry exceeds its limit",
            ),
            LegacyPythonBrokerStatusWire::InvalidHouseholdSubdocument => PortError::new(
                "legacy_python_keyring_format",
                "historical Python keyring household state is invalid",
            ),
            _ => household_document_error(),
        }
    }

    fn legacy_config_kind_name(kind: LegacyPythonConfigKindV1) -> &'static str {
        match kind {
            LegacyPythonConfigKindV1::Current => "current",
            LegacyPythonConfigKindV1::Legacy => "legacy",
        }
    }

    fn decode_legacy_config_kind(value: &str) -> Result<LegacyPythonConfigKindV1, PortError> {
        match value {
            "current" => Ok(LegacyPythonConfigKindV1::Current),
            "legacy" => Ok(LegacyPythonConfigKindV1::Legacy),
            _ => Err(PortError::new(
                "legacy_python_broker_binding",
                "historical Python keyring target binding is invalid",
            )),
        }
    }

    fn validate_frozen_legacy_config_path(
        kind: LegacyPythonConfigKindV1,
        path: &Path,
    ) -> Result<(), PortError> {
        use std::path::Component;

        if !path.is_absolute()
            || path.file_name() != Some(OsStr::new("config.json"))
            || path.parent().and_then(Path::file_name)
                != Some(OsStr::new(match kind {
                    LegacyPythonConfigKindV1::Current => "heyfood",
                    LegacyPythonConfigKindV1::Legacy => "hellofood",
                }))
            || path
                .components()
                .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        {
            return Err(PortError::new(
                "legacy_python_broker_binding",
                "historical Python keyring target is not one of the frozen config locators",
            ));
        }
        Ok(())
    }

    fn canonical_optional_object_digest(
        value: Option<&Map<String, Value>>,
    ) -> Result<Option<String>, PortError> {
        value
            .map(|value| {
                to_canonical_bytes_v1(&Value::Object(value.clone()))
                    .map(|canonical| hex_digest(Sha256::digest(canonical)))
                    .map_err(|_| {
                        PortError::new(
                            "legacy_python_keyring_format",
                            "historical Python keyring household state is invalid",
                        )
                    })
            })
            .transpose()
    }

    fn classify_legacy_python_keyring_bytes(
        bytes: &[u8],
    ) -> Result<
        (
            LegacyPythonBrokerStatusWire,
            Option<LegacyPythonHouseholdPayloadWire>,
        ),
        PortError,
    > {
        if bytes.len() > 4 * 1024 * 1024 {
            return Ok((LegacyPythonBrokerStatusWire::OversizedEntry, None));
        }
        let mut object = match parse_bounded_json_object_v1(
            bytes,
            CompatibilityJsonLimitsV1::MIGRATION_CANDIDATE,
        ) {
            Ok(object) => object,
            Err(_) => return Ok((LegacyPythonBrokerStatusWire::MalformedJson, None)),
        };
        let extract = |object: &mut Map<String, Value>,
                       key: &str|
         -> Result<Option<Map<String, Value>>, ()> {
            object
                .remove(key)
                .map(|value| value.as_object().cloned().ok_or(()))
                .transpose()
        };
        let state = match extract(&mut object, "household.state") {
            Ok(value) => value,
            Err(()) => {
                return Ok((
                    LegacyPythonBrokerStatusWire::InvalidHouseholdSubdocument,
                    None,
                ));
            }
        };
        let local_profiles = match extract(&mut object, "household.local_profiles") {
            Ok(value) => value,
            Err(()) => {
                return Ok((
                    LegacyPythonBrokerStatusWire::InvalidHouseholdSubdocument,
                    None,
                ));
            }
        };
        let profile_outbox = match extract(&mut object, "household.profile_outbox") {
            Ok(value) => value,
            Err(()) => {
                return Ok((
                    LegacyPythonBrokerStatusWire::InvalidHouseholdSubdocument,
                    None,
                ));
            }
        };
        let payload =
            LegacyPythonHouseholdPayloadWire::from_objects(state, local_profiles, profile_outbox)?;
        let status = if payload.has_household_data() {
            LegacyPythonBrokerStatusWire::PresentHousehold
        } else {
            LegacyPythonBrokerStatusWire::PresentNoHousehold
        };
        Ok((status, Some(payload)))
    }

    struct LegacyPythonCredentialProjectionV1 {
        canonical_noncredential: Zeroizing<Vec<u8>>,
        document_digest: [u8; 32],
        noncredential_digest: [u8; 32],
        credentials_present: bool,
    }

    fn classify_legacy_python_credential_bytes(
        bytes: &[u8],
    ) -> Result<
        Result<LegacyPythonCredentialProjectionV1, LegacyPythonCredentialStatusWire>,
        PortError,
    > {
        const FLAT_CREDENTIAL_FIELDS: [&str; 5] = [
            "api_key",
            "oauth.access_token",
            "oauth.refresh_token",
            "session.access_token",
            "session.refresh_token",
        ];

        if bytes.len() > 4 * 1024 * 1024 {
            return Ok(Err(LegacyPythonCredentialStatusWire::Oversized));
        }
        let mut object = match parse_bounded_json_object_v1(
            bytes,
            CompatibilityJsonLimitsV1::MIGRATION_CANDIDATE,
        ) {
            Ok(object) => object,
            Err(_) => return Ok(Err(LegacyPythonCredentialStatusWire::Malformed)),
        };
        let mut credentials_present = false;
        for field in FLAT_CREDENTIAL_FIELDS {
            if let Some(value) = object.remove(field) {
                if !value.is_string() {
                    return Ok(Err(LegacyPythonCredentialStatusWire::Malformed));
                }
                credentials_present = true;
            }
        }
        // Some pre-release stores serialized the two bundles rather than the
        // released flat keyring names. Scrub only their exact token members,
        // retaining client IDs, user IDs, expiry fields, and unknown members.
        for bundle_name in ["oauth", "session"] {
            let Some(bundle_value) = object.get_mut(bundle_name) else {
                continue;
            };
            let Some(bundle) = bundle_value.as_object_mut() else {
                continue;
            };
            for token_name in ["access_token", "refresh_token"] {
                if let Some(value) = bundle.remove(token_name) {
                    if !value.is_string() {
                        return Ok(Err(LegacyPythonCredentialStatusWire::Malformed));
                    }
                    credentials_present = true;
                }
            }
        }
        let canonical_noncredential = Zeroizing::new(
            to_canonical_bytes_v1(&Value::Object(object))
                .map_err(|_| household_document_error())?,
        );
        Ok(Ok(LegacyPythonCredentialProjectionV1 {
            document_digest: Sha256::digest(bytes).into(),
            noncredential_digest: Sha256::digest(&canonical_noncredential).into(),
            canonical_noncredential,
            credentials_present,
        }))
    }

    fn missing_legacy_credential_noncredential_digest() -> Result<[u8; 32], PortError> {
        #[derive(Serialize)]
        struct Missing<'a> {
            state: &'a str,
        }
        let canonical = to_canonical_bytes_v1(&Missing {
            state: "authoritative_missing",
        })
        .map_err(|_| household_document_error())?;
        Ok(Sha256::digest(canonical).into())
    }

    fn decode_household_key_material(value: &str) -> Result<HouseholdKeyMaterial, PortError> {
        if value.len() != 64
            || value
                .as_bytes()
                .iter()
                .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(byte))
        {
            return Err(household_document_error());
        }
        let mut output = Zeroizing::new([0_u8; 32]);
        for (index, byte) in output.iter_mut().enumerate() {
            let start = index * 2;
            *byte = u8::from_str_radix(&value[start..start + 2], 16)
                .map_err(|_| household_document_error())?;
        }
        Ok(HouseholdKeyMaterial::from_zeroizing(output))
    }

    impl HouseholdKeyStore for HouseholdKeyBroker {
        fn load<'a>(
            &'a self,
            lifecycle_lease: &'a HouseholdLifecycleLease,
            cancellation: CancellationToken,
        ) -> BoxFuture<'a, Result<Option<HouseholdKeyBundle>, PortError>> {
            Box::pin(async move {
                let account_slot = lifecycle_account_slot(lifecycle_lease)?;
                self.validate_slot(&account_slot)?;
                let request =
                    encode_household_request(&HouseholdBrokerRequestWire::load(&account_slot))?;
                let response = decode_household_response(
                    &self
                        .request(
                            HouseholdBrokerOperationV1::KeyLoad,
                            request,
                            cancellation,
                            false,
                        )
                        .await?,
                )?;
                let result = match (response.status.as_str(), response.bundle, response.guard) {
                    ("not_found", None, None) => Ok(None),
                    ("ok", Some(bundle), None) => bundle.decode(&account_slot).map(Some),
                    _ => Err(household_document_error()),
                };
                lifecycle_lease.validate_for(&account_slot)?;
                result
            })
        }

        fn initialize<'a>(
            &'a self,
            vault_lease: &'a mut HouseholdVaultLease,
            _expected: KeyStoreExpectation,
            expected_guard: HouseholdMigrationGuardDocument,
            bundle: HouseholdKeyBundle,
            cancellation: CancellationToken,
        ) -> BoxFuture<'a, Result<(), PortError>> {
            Box::pin(async move {
                let _operation = vault_lease.acquire_operation(&cancellation).await?;
                let account_slot = vault_account_slot(vault_lease)?;
                self.validate_slot(&account_slot)?;
                bundle.validate_initial_for(&account_slot, &expected_guard)?;
                let mut request = HouseholdBrokerRequestWire::load(&account_slot);
                request.key_expectation = Some(KeyExpectationWire::Absent);
                request.bundle = Some(HouseholdKeyBundleWire::from_bundle(&bundle));
                request.guard = Some(HouseholdMigrationGuardWire::from_guard(&expected_guard)?);
                let result = self
                    .request(
                        HouseholdBrokerOperationV1::KeyInitialize,
                        encode_household_request(&request)?,
                        cancellation,
                        true,
                    )
                    .await
                    .map(|_| ());
                verify_vault_lease_after_mutation(vault_lease, &account_slot)?;
                result
            })
        }

        fn compare_exchange<'a>(
            &'a self,
            vault_lease: &'a mut HouseholdVaultLease,
            expected: KeyBundleRevision,
            replacement: HouseholdKeyBundle,
            cancellation: CancellationToken,
        ) -> BoxFuture<'a, Result<(), PortError>> {
            Box::pin(async move {
                let _operation = vault_lease.acquire_operation(&cancellation).await?;
                let account_slot = vault_account_slot(vault_lease)?;
                self.validate_slot(&account_slot)?;
                replacement.validate_for(&account_slot)?;
                if replacement.revision != expected.checked_next()? {
                    return Err(PortError::new(
                        "household_key_cas",
                        "household key replacement revision is invalid",
                    ));
                }
                let mut request = HouseholdBrokerRequestWire::load(&account_slot);
                request.key_expectation = Some(KeyExpectationWire::Revision);
                request.expected_revision = Some(expected.get());
                request.bundle = Some(HouseholdKeyBundleWire::from_bundle(&replacement));
                let result = self
                    .request(
                        HouseholdBrokerOperationV1::KeyReplace,
                        encode_household_request(&request)?,
                        cancellation,
                        true,
                    )
                    .await
                    .map(|_| ());
                verify_vault_lease_after_mutation(vault_lease, &account_slot)?;
                result
            })
        }

        fn delete_and_verify<'a>(
            &'a self,
            vault_lease: &'a mut HouseholdVaultLease,
            expected_revision: KeyBundleRevision,
            expected_key_id: KeyId,
            cancellation: CancellationToken,
        ) -> BoxFuture<'a, Result<(), PortError>> {
            Box::pin(async move {
                let _operation = vault_lease.acquire_operation(&cancellation).await?;
                let account_slot = vault_account_slot(vault_lease)?;
                self.validate_slot(&account_slot)?;
                let mut request = HouseholdBrokerRequestWire::load(&account_slot);
                request.expected_revision = Some(expected_revision.get());
                request.expected_key_id = Some(expected_key_id.as_uuid());
                let result = self
                    .request(
                        HouseholdBrokerOperationV1::KeyDelete,
                        encode_household_request(&request)?,
                        cancellation,
                        true,
                    )
                    .await
                    .map(|_| ());
                verify_vault_lease_after_mutation(vault_lease, &account_slot)?;
                result
            })
        }

        fn abort_initialization_and_verify<'a>(
            &'a self,
            vault_lease: &'a mut HouseholdVaultLease,
            expected_revision: KeyBundleRevision,
            expected_initialization_id: Uuid,
            expected_aborting_guard: HouseholdMigrationGuardDocument,
            cancellation: CancellationToken,
        ) -> BoxFuture<'a, Result<(), PortError>> {
            Box::pin(async move {
                let _operation = vault_lease.acquire_operation(&cancellation).await?;
                let account_slot = vault_account_slot(vault_lease)?;
                self.validate_slot(&account_slot)?;
                let mut request = HouseholdBrokerRequestWire::load(&account_slot);
                request.expected_revision = Some(expected_revision.get());
                request.abort_initialization_id = Some(expected_initialization_id);
                request.guard = Some(HouseholdMigrationGuardWire::from_guard(
                    &expected_aborting_guard,
                )?);
                let result = self
                    .request(
                        HouseholdBrokerOperationV1::KeyAbortInitialization,
                        encode_household_request(&request)?,
                        cancellation,
                        true,
                    )
                    .await
                    .map(|_| ());
                verify_vault_lease_after_mutation(vault_lease, &account_slot)?;
                result
            })
        }
    }

    impl HouseholdMigrationGuardStore for HouseholdKeyBroker {
        fn load<'a>(
            &'a self,
            lifecycle_lease: &'a HouseholdLifecycleLease,
            cancellation: CancellationToken,
        ) -> BoxFuture<'a, Result<Option<HouseholdMigrationGuardDocument>, PortError>> {
            Box::pin(async move {
                let account_slot = lifecycle_account_slot(lifecycle_lease)?;
                self.validate_slot(&account_slot)?;
                let request =
                    encode_household_request(&HouseholdBrokerRequestWire::load(&account_slot))?;
                let response = decode_household_response(
                    &self
                        .request(
                            HouseholdBrokerOperationV1::MigrationGuardLoad,
                            request,
                            cancellation,
                            false,
                        )
                        .await?,
                )?;
                let result = match (response.status.as_str(), response.bundle, response.guard) {
                    ("not_found", None, None) => Ok(None),
                    ("ok", None, Some(guard)) => guard.decode(&account_slot).map(Some),
                    _ => Err(household_document_error()),
                };
                lifecycle_lease.validate_for(&account_slot)?;
                result
            })
        }

        fn compare_exchange<'a>(
            &'a self,
            vault_lease: &'a mut HouseholdVaultLease,
            expected: MigrationGuardExpectation,
            replacement: Option<HouseholdMigrationGuardDocument>,
            cancellation: CancellationToken,
        ) -> BoxFuture<'a, Result<(), PortError>> {
            Box::pin(async move {
                let _operation = vault_lease.acquire_operation(&cancellation).await?;
                let account_slot = vault_account_slot(vault_lease)?;
                self.validate_slot(&account_slot)?;
                if let Some(replacement) = &replacement {
                    replacement.validate_for(&account_slot)?;
                    if expected == MigrationGuardExpectation::Absent
                        && (replacement.guard_revision() != 1
                            || replacement.state() != HouseholdMigrationGuardStateV1::Initializing
                            || replacement.initialization_phase()
                                != Some(HouseholdMigrationInitializationPhaseV1::ReservedSource))
                    {
                        return Err(PortError::new(
                            "household_migration_guard_revision",
                            "initial household migration guard must be reserved at revision one",
                        ));
                    }
                } else {
                    return Err(PortError::new(
                        "household_migration_guard_delete_forbidden",
                        "household migration guards are retained for the account lifetime",
                    ));
                }
                let mut request = HouseholdBrokerRequestWire::load(&account_slot);
                match expected {
                    MigrationGuardExpectation::Absent => {
                        request.guard_expectation = Some(GuardExpectationWire::Absent);
                    }
                    MigrationGuardExpectation::Revision(revision) => {
                        request.guard_expectation = Some(GuardExpectationWire::Revision);
                        request.expected_revision = Some(revision);
                    }
                }
                request.delete_guard = false;
                request.guard = replacement
                    .as_ref()
                    .map(HouseholdMigrationGuardWire::from_guard)
                    .transpose()?;
                let result = self
                    .request(
                        HouseholdBrokerOperationV1::MigrationGuardCompareExchange,
                        encode_household_request(&request)?,
                        cancellation,
                        true,
                    )
                    .await
                    .map(|_| ());
                verify_vault_lease_after_mutation(vault_lease, &account_slot)?;
                result
            })
        }
    }

    struct KillReapGuard(Option<std::process::Child>);

    impl KillReapGuard {
        fn child_mut(&mut self) -> &mut std::process::Child {
            self.0.as_mut().expect("broker child is present")
        }

        fn kill_and_reap(&mut self) -> std::io::Result<()> {
            if let Some(mut child) = self.0.take() {
                let _ = child.kill();
                child.wait().map(|_| ())
            } else {
                Ok(())
            }
        }

        fn disarm(&mut self) {
            self.0 = None;
        }
    }

    impl Drop for KillReapGuard {
        fn drop(&mut self) {
            let _ = self.kill_and_reap();
        }
    }

    fn run_bounded_child_blocking(
        child: std::process::Child,
        input: Vec<u8>,
        deadline: Duration,
        outcome_uncertain: bool,
    ) -> Result<Vec<u8>, PortError> {
        let started = Instant::now();
        let mut child = KillReapGuard(Some(child));
        let mut stdin =
            child.child_mut().stdin.take().ok_or_else(|| {
                PortError::new("credential_broker_pipe", "broker stdin is missing")
            })?;
        let stdout =
            child.child_mut().stdout.take().ok_or_else(|| {
                PortError::new("credential_broker_pipe", "broker stdout is missing")
            })?;
        let writer = std::thread::spawn(move || {
            let result = stdin.write_all(&input);
            drop(stdin);
            result
        });
        let reader = std::thread::spawn(move || {
            let mut output = Vec::new();
            stdout
                .take((MAX_BROKER_DOCUMENT_BYTES + 1) as u64)
                .read_to_end(&mut output)
                .map(|_| output)
        });
        let status = loop {
            match child.child_mut().try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(None) => {
                    child.kill_and_reap().map_err(|_| {
                        broker_error(
                            outcome_uncertain,
                            "credential_broker_reap",
                            "native credential broker could not be terminated",
                        )
                    })?;
                    let _ = writer.join();
                    let _ = reader.join();
                    return Err(broker_error(
                        outcome_uncertain,
                        "credential_broker_timeout",
                        "native credential operation exceeded its deadline",
                    ));
                }
                Err(error) => {
                    let _ = child.kill_and_reap();
                    let _ = writer.join();
                    let _ = reader.join();
                    return Err(PortError::new("credential_broker_wait", error.to_string()));
                }
            }
        };
        child.disarm();
        writer
            .join()
            .map_err(|_| PortError::new("credential_broker_pipe", "broker writer panicked"))?
            .map_err(|error| PortError::new("credential_broker_pipe", error.to_string()))?;
        let output = reader
            .join()
            .map_err(|_| PortError::new("credential_broker_pipe", "broker reader panicked"))?
            .map_err(|error| PortError::new("credential_broker_pipe", error.to_string()))?;
        if !status.success() {
            return Err(broker_error(
                outcome_uncertain,
                "credential_broker_failed",
                "native credential operation failed",
            ));
        }
        if output.len() > MAX_BROKER_DOCUMENT_BYTES {
            return Err(PortError::new(
                "credential_broker_size",
                "credential broker output exceeds its limit",
            ));
        }
        Ok(output)
    }

    async fn run_bounded_child(
        mut child: tokio::process::Child,
        input: Vec<u8>,
        deadline: Duration,
        outcome_uncertain: bool,
    ) -> Result<Vec<u8>, PortError> {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| PortError::new("credential_broker_pipe", "broker stdin is missing"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| PortError::new("credential_broker_pipe", "broker stdout is missing"))?;
        let operation = async {
            stdin
                .write_all(&input)
                .await
                .map_err(|error| PortError::new("credential_broker_pipe", error.to_string()))?;
            stdin
                .shutdown()
                .await
                .map_err(|error| PortError::new("credential_broker_pipe", error.to_string()))?;
            drop(stdin);
            let mut output = Vec::new();
            let mut stdout = stdout.take((MAX_BROKER_DOCUMENT_BYTES + 1) as u64);
            let (status, _) = tokio::try_join!(child.wait(), stdout.read_to_end(&mut output),)
                .map_err(|error| PortError::new("credential_broker_wait", error.to_string()))?;
            Ok::<_, PortError>((status, output))
        };
        let (status, output) = match tokio::time::timeout(deadline, operation).await {
            Ok(result) => result?,
            Err(_) => {
                // Dropping a `kill_on_drop` child requests termination but does not
                // guarantee that the OS process has been reaped before this method
                // returns. Explicitly kill and await it so a timed-out keyring
                // prompt cannot survive as a live or zombie broker.
                child.kill().await.map_err(|_| {
                    broker_error(
                        outcome_uncertain,
                        "credential_broker_reap",
                        "native credential broker could not be terminated",
                    )
                })?;
                return Err(broker_error(
                    outcome_uncertain,
                    "credential_broker_timeout",
                    "native credential operation exceeded its deadline",
                ));
            }
        };
        if !status.success() {
            return Err(broker_error(
                outcome_uncertain,
                "credential_broker_failed",
                "native credential operation failed",
            ));
        }
        if output.len() > MAX_BROKER_DOCUMENT_BYTES {
            return Err(PortError::new(
                "credential_broker_size",
                "credential broker output exceeds its limit",
            ));
        }
        Ok(output)
    }

    async fn run_household_bounded_child(
        mut child: tokio::process::Child,
        input: Zeroizing<Vec<u8>>,
        deadline: Duration,
        cancellation: CancellationToken,
        response_limit: usize,
        outcome_uncertain: bool,
    ) -> Result<Zeroizing<Vec<u8>>, PortError> {
        let mut stdin = child.stdin.take().ok_or_else(|| {
            PortError::new("household_broker_pipe", "household broker pipe is missing")
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            PortError::new("household_broker_pipe", "household broker pipe is missing")
        })?;
        let mut operation = Box::pin(async {
            stdin.write_all(&input).await.map_err(|_| {
                PortError::new(
                    "household_broker_pipe",
                    "household broker input could not be written",
                )
            })?;
            stdin.shutdown().await.map_err(|_| {
                PortError::new(
                    "household_broker_pipe",
                    "household broker input could not be closed",
                )
            })?;
            drop(stdin);
            let mut output = Zeroizing::new(Vec::new());
            let mut stdout = stdout.take((response_limit + 1) as u64);
            let (status, _) = tokio::try_join!(child.wait(), stdout.read_to_end(&mut output),)
                .map_err(|_| {
                    PortError::new("household_broker_wait", "household broker did not complete")
                })?;
            Ok::<_, PortError>((status, output))
        });
        enum Completion {
            Operation(Result<(std::process::ExitStatus, Zeroizing<Vec<u8>>), PortError>),
            Cancelled,
            TimedOut,
        }
        let completion = tokio::select! {
            result = &mut operation => Completion::Operation(result),
            () = cancellation.cancelled() => Completion::Cancelled,
            () = tokio::time::sleep(deadline) => Completion::TimedOut,
        };
        match completion {
            Completion::Operation(Ok((status, output))) => {
                if !status.success() {
                    return Err(broker_error(
                        outcome_uncertain,
                        "household_broker_failed",
                        "household secure-store operation failed",
                    ));
                }
                if output.len() > response_limit {
                    return Err(PortError::new(
                        "household_broker_size",
                        "household broker output exceeds its operation limit",
                    ));
                }
                Ok(output)
            }
            Completion::Operation(Err(error)) => {
                drop(operation);
                child.kill().await.map_err(|_| {
                    broker_error(
                        outcome_uncertain,
                        "household_broker_reap",
                        "household broker could not be terminated",
                    )
                })?;
                child.wait().await.map_err(|_| {
                    broker_error(
                        outcome_uncertain,
                        "household_broker_reap",
                        "household broker could not be reaped",
                    )
                })?;
                Err(error)
            }
            Completion::Cancelled => {
                drop(operation);
                child.kill().await.map_err(|_| {
                    broker_error(
                        outcome_uncertain,
                        "household_broker_reap",
                        "household broker could not be terminated",
                    )
                })?;
                child.wait().await.map_err(|_| {
                    broker_error(
                        outcome_uncertain,
                        "household_broker_reap",
                        "household broker could not be reaped",
                    )
                })?;
                Err(broker_error(
                    outcome_uncertain,
                    "household_broker_cancelled",
                    "household secure-store operation was cancelled",
                ))
            }
            Completion::TimedOut => {
                drop(operation);
                child.kill().await.map_err(|_| {
                    broker_error(
                        outcome_uncertain,
                        "household_broker_reap",
                        "household broker could not be terminated",
                    )
                })?;
                child.wait().await.map_err(|_| {
                    broker_error(
                        outcome_uncertain,
                        "household_broker_reap",
                        "household broker could not be reaped",
                    )
                })?;
                Err(broker_error(
                    outcome_uncertain,
                    "household_broker_timeout",
                    "household secure-store operation exceeded its deadline",
                ))
            }
        }
    }

    fn broker_error(
        outcome_uncertain: bool,
        code: &'static str,
        message: &'static str,
    ) -> PortError {
        if outcome_uncertain {
            PortError::uncertain(code, message)
        } else {
            PortError::new(code, message)
        }
    }

    fn household_cancelled_error() -> PortError {
        PortError::new(
            "household_broker_cancelled",
            "household secure-store operation was cancelled",
        )
    }

    fn household_document_error() -> PortError {
        PortError::new(
            "household_broker_document",
            "household broker document is invalid",
        )
    }

    impl CredentialPort for CredentialBrokerStore {
        fn load(&self) -> BoxFuture<'_, Result<Option<SessionCredentials>, PortError>> {
            Box::pin(async move {
                let output = self.request("load", Vec::new(), false).await?;
                if output.is_empty() {
                    Ok(None)
                } else {
                    CredentialState::decode(&output)
                        .map(|state| Some(state.credentials))
                        .map_err(|_| {
                            PortError::new(
                                "credential_broker_response",
                                "native credential broker returned an invalid document",
                            )
                        })
                }
            })
        }

        fn commit(&self, commit: CredentialCommit) -> BoxFuture<'_, Result<(), PortError>> {
            Box::pin(async move {
                let mut input = format!(
                    "expected={}\ncommit={}\n",
                    commit.expected_version.get(),
                    commit.commit_id.as_uuid()
                )
                .into_bytes();
                input.extend_from_slice(&CredentialState::new(commit.credentials).encode());
                self.request("commit", input, true).await.map(|_| ())
            })
        }

        fn mark_reconciliation_required(
            &self,
            commit_id: CommitId,
        ) -> BoxFuture<'_, Result<(), PortError>> {
            Box::pin(async move {
                self.request(
                    "mark",
                    format!("{}\n", commit_id.as_uuid()).into_bytes(),
                    false,
                )
                .await
                .map(|_| ())
            })
        }

        fn clear_reconciliation_required(
            &self,
            commit_id: CommitId,
        ) -> BoxFuture<'_, Result<(), PortError>> {
            Box::pin(async move {
                self.request(
                    "clear",
                    format!("{}\n", commit_id.as_uuid()).into_bytes(),
                    false,
                )
                .await
                .map(|_| ())
            })
        }
    }

    impl AuthorizationSessionStore for CredentialBrokerStore {
        fn initialize_authorized_session(
            &self,
            credentials: &SessionCredentials,
        ) -> Result<(), PortError> {
            self.request_blocking(
                "initialize",
                CredentialState::new(credentials.clone()).encode(),
                true,
            )
            .map(|_| ())
        }

        fn load_authorized_session(&self) -> Result<Option<SessionCredentials>, PortError> {
            let output = self.request_blocking("load", Vec::new(), false)?;
            if output.is_empty() {
                Ok(None)
            } else {
                CredentialState::decode(&output).map(|state| Some(state.credentials))
            }
        }

        fn replace_authorized_session(
            &self,
            credentials: &SessionCredentials,
        ) -> Result<(), PortError> {
            self.request_blocking(
                "replace",
                CredentialState::new(credentials.clone()).encode(),
                true,
            )
            .map(|_| ())
        }

        fn stage_authorized_session(
            &self,
            client_transaction_id: &str,
            previous: &SessionCredentials,
            replacement: &SessionCredentials,
        ) -> Result<(), PortError> {
            self.request_blocking(
                "stage",
                AuthorizationSessionStage {
                    client_transaction_id: client_transaction_id.to_owned(),
                    previous: previous.clone(),
                    replacement: replacement.clone(),
                }
                .encode(),
                true,
            )
            .map(|_| ())
        }

        fn verify_staged_authorized_session(
            &self,
            client_transaction_id: &str,
            previous: &SessionCredentials,
            replacement: &SessionCredentials,
        ) -> Result<(), PortError> {
            self.request_blocking(
                "verify-stage",
                AuthorizationSessionStage {
                    client_transaction_id: client_transaction_id.to_owned(),
                    previous: previous.clone(),
                    replacement: replacement.clone(),
                }
                .encode(),
                false,
            )
            .map(|_| ())
        }

        fn clear_staged_authorized_session(
            &self,
            client_transaction_id: &str,
            expected_replacement: &SessionCredentials,
        ) -> Result<(), PortError> {
            let mut input = format!("client_transaction={client_transaction_id}\n").into_bytes();
            input.extend_from_slice(&CredentialState::new(expected_replacement.clone()).encode());
            self.request_blocking("clear-stage", input, true)
                .map(|_| ())
        }

        fn delete_authorized_session(&self) -> Result<(), PortError> {
            self.request_blocking("delete", Vec::new(), true)
                .map(|_| ())
        }

        fn delete_authorized_session_for_logout(
            &self,
            expected: Option<&SessionCredentials>,
        ) -> Result<(), PortError> {
            let input = expected
                .cloned()
                .map_or_else(Vec::new, |value| CredentialState::new(value).encode());
            self.request_blocking("logout-delete-exact", input, true)
                .map(|_| ())
        }

        fn delete_authorized_session_after_preflight_failure(
            &self,
            expected_account: &AccountId,
        ) -> Result<(), PortError> {
            self.request_blocking(
                "logout-delete-account",
                format!("{}\n", expected_account.as_str()).into_bytes(),
                true,
            )
            .map(|_| ())
        }
    }

    /// Handle the broker mode before any terminal/tracing initialization. Returns
    /// `None` for every ordinary invocation.
    pub fn run_credential_broker_if_requested() -> Option<ExitCode> {
        let mut arguments = std::env::args_os().skip(1);
        if arguments.next().as_deref() != Some(OsStr::new(BROKER_MODE)) {
            return None;
        }
        let Some(action) = arguments.next().and_then(|value| value.into_string().ok()) else {
            return Some(ExitCode::from(2));
        };
        let Some(root) = arguments.next().map(PathBuf::from) else {
            return Some(ExitCode::from(2));
        };
        if arguments.next().is_some() {
            return Some(ExitCode::from(2));
        }
        if verify_broker_parent().is_err() {
            return Some(ExitCode::from(2));
        }
        Some(match run_broker_action(&action, &root) {
            Ok(mut output) => {
                let written = std::io::stdout().write_all(&output).is_ok();
                output.zeroize();
                if written {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                }
            }
            Err(_) => ExitCode::FAILURE,
        })
    }

    /// Prevent the hidden broker mode from becoming a confused-deputy credential
    /// oracle. Only the exact running heyfood executable may be the broker's
    /// immediate parent; shells, test runners, and unrelated processes fail before
    /// stdin or native credential storage is touched.
    fn verify_broker_parent() -> Result<(), PortError> {
        use sysinfo::{Pid, ProcessesToUpdate, System};

        let current_pid = Pid::from_u32(std::process::id());
        let mut system = System::new();
        system.refresh_processes(ProcessesToUpdate::Some(&[current_pid]), true);
        let parent_pid = system
            .process(current_pid)
            .and_then(sysinfo::Process::parent)
            .ok_or_else(|| {
                PortError::new(
                    "credential_broker_parent",
                    "credential broker parent identity is unavailable",
                )
            })?;
        system.refresh_processes(ProcessesToUpdate::Some(&[parent_pid]), true);
        let parent_executable = system
            .process(parent_pid)
            .and_then(sysinfo::Process::exe)
            .ok_or_else(|| {
                PortError::new(
                    "credential_broker_parent",
                    "credential broker parent executable is unavailable",
                )
            })?
            .canonicalize()
            .map_err(|error| PortError::new("credential_broker_parent", error.to_string()))?;
        let broker_executable = std::env::current_exe()
            .and_then(std::fs::canonicalize)
            .map_err(|error| PortError::new("credential_broker_parent", error.to_string()))?;
        if parent_executable != broker_executable {
            return Err(PortError::new(
                "credential_broker_parent",
                "credential broker was not launched by the running heyfood executable",
            ));
        }
        Ok(())
    }

    fn run_broker_action(action: &str, root: &Path) -> Result<Vec<u8>, PortError> {
        let mut input = Zeroizing::new(Vec::new());
        std::io::stdin()
            .take((MAX_BROKER_DOCUMENT_BYTES + 1) as u64)
            .read_to_end(&mut input)
            .map_err(|error| PortError::new("credential_broker_read", error.to_string()))?;
        if input.len() > MAX_BROKER_DOCUMENT_BYTES {
            return Err(PortError::new(
                "credential_broker_size",
                "credential broker input exceeds its limit",
            ));
        }

        if action.starts_with("household-") || action.starts_with("legacy-python-") {
            return run_household_broker_action(action, &input, root);
        }

        #[cfg(windows)]
        let store = crate::persistence::WindowsCredentialStore::open(root)?;
        #[cfg(not(windows))]
        let store = crate::persistence::KeyringCredentialStore::open(root)?;

        match action {
            "load" if input.is_empty() => Ok(store
                .broker_load()?
                .map_or_else(Vec::new, |value| CredentialState::new(value).encode())),
            "initialize" => {
                store.initialize(&CredentialState::decode(&input)?.credentials)?;
                Ok(Vec::new())
            }
            "replace" => {
                store.replace_authorized_session(&CredentialState::decode(&input)?.credentials)?;
                Ok(Vec::new())
            }
            "stage" => {
                let stage = AuthorizationSessionStage::decode(&input)?;
                store.stage_authorized_session(
                    &stage.client_transaction_id,
                    &stage.previous,
                    &stage.replacement,
                )?;
                Ok(Vec::new())
            }
            "verify-stage" => {
                let stage = AuthorizationSessionStage::decode(&input)?;
                store.verify_staged_authorized_session(
                    &stage.client_transaction_id,
                    &stage.previous,
                    &stage.replacement,
                )?;
                Ok(Vec::new())
            }
            "clear-stage" => {
                store.clear_staged_authorized_session(
                    required_field(&input, "client_transaction")?,
                    &CredentialState::decode(&input)?.credentials,
                )?;
                Ok(Vec::new())
            }
            "reconciliation" if input.is_empty() => Ok(if store.reconciliation_required()? {
                b"1\n".to_vec()
            } else {
                b"0\n".to_vec()
            }),
            "commit" => {
                let expected = required_field(&input, "expected")?
                    .parse::<u64>()
                    .map(CredentialVersion::new)
                    .map_err(|_| {
                        PortError::new("credential_broker_request", "invalid expected version")
                    })?;
                let commit_id = parse_commit_id(required_field(&input, "commit")?)?;
                let credentials = CredentialState::decode(&input)?.credentials;
                store.broker_commit(CredentialCommit {
                    commit_id,
                    expected_version: expected,
                    credentials,
                })?;
                Ok(Vec::new())
            }
            "mark" => {
                store.broker_mark(parse_commit_id(trimmed_input(&input)?)?)?;
                Ok(Vec::new())
            }
            "clear" => {
                store.broker_clear(parse_commit_id(trimmed_input(&input)?)?)?;
                Ok(Vec::new())
            }
            "delete" if input.is_empty() => {
                store.delete()?;
                Ok(Vec::new())
            }
            "logout-delete-exact" => {
                let expected = if input.is_empty() {
                    None
                } else {
                    Some(CredentialState::decode(&input)?.credentials)
                };
                store.delete_authorized_session_for_logout(expected.as_ref())?;
                Ok(Vec::new())
            }
            "logout-delete-account" => {
                let account = AccountId::parse(trimmed_input(&input)?).map_err(|_| {
                    PortError::new(
                        "credential_broker_request",
                        "invalid logout account binding",
                    )
                })?;
                store.delete_authorized_session_after_preflight_failure(&account)?;
                Ok(Vec::new())
            }
            _ => Err(PortError::new(
                "credential_broker_request",
                "invalid credential broker request",
            )),
        }
    }

    fn run_household_broker_action(
        action: &str,
        input: &[u8],
        root: &Path,
    ) -> Result<Vec<u8>, PortError> {
        if action == HouseholdBrokerOperationV1::SecureStoreProbe.action() {
            if !input.is_empty() {
                return Err(household_document_error());
            }
            let probe = keyring::Entry::new(HOUSEHOLD_KEYRING_SERVICE_V1, "secure-store-probe-v1")
                .map_err(|_| household_secure_store_error())?;
            match probe.get_secret() {
                Ok(mut value) => {
                    value.zeroize();
                    return Ok(Vec::new());
                }
                Err(keyring::Error::NoEntry) => return Ok(Vec::new()),
                Err(_) => return Err(household_secure_store_error()),
            }
        }
        if action == HouseholdBrokerOperationV1::LegacyPythonCredentialsScrubAndVerify.action() {
            let request: LegacyPythonCredentialRequestWire =
                serde_json::from_slice(input).map_err(|_| household_document_error())?;
            let (_slot, _config_kind) = request.validate(root)?;
            let locator = LegacyPythonKeyringLocatorV1::from_resolved_config_path_bytes(
                request.target.resolved_config_path.as_bytes(),
            )?;
            let entry = keyring::Entry::new(locator.service, &locator.username)
                .map_err(|_| household_secure_store_error())?;
            let response = match request.action {
                LegacyPythonCredentialActionWire::Probe => match entry.get_secret() {
                    Err(keyring::Error::NoEntry) => LegacyPythonCredentialResponseWire::new(
                        &request,
                        LegacyPythonCredentialStatusWire::AuthoritativeMissing,
                        None,
                        Some(missing_legacy_credential_noncredential_digest()?),
                    ),
                    Err(_) => LegacyPythonCredentialResponseWire::new(
                        &request,
                        LegacyPythonCredentialStatusWire::Unavailable,
                        None,
                        None,
                    ),
                    Ok(bytes) => {
                        let bytes = Zeroizing::new(bytes);
                        match classify_legacy_python_credential_bytes(&bytes)? {
                            Err(status) => LegacyPythonCredentialResponseWire::new(
                                &request, status, None, None,
                            ),
                            Ok(projection) => LegacyPythonCredentialResponseWire::new(
                                &request,
                                if projection.credentials_present {
                                    LegacyPythonCredentialStatusWire::PresentCredentials
                                } else {
                                    LegacyPythonCredentialStatusWire::PresentNoCredentials
                                },
                                Some(projection.document_digest),
                                Some(projection.noncredential_digest),
                            ),
                        }
                    }
                },
                LegacyPythonCredentialActionWire::ScrubAndVerify => match entry.get_secret() {
                    Err(keyring::Error::NoEntry) => LegacyPythonCredentialResponseWire::new(
                        &request,
                        LegacyPythonCredentialStatusWire::AuthoritativeMissing,
                        None,
                        Some(missing_legacy_credential_noncredential_digest()?),
                    ),
                    Err(_) => LegacyPythonCredentialResponseWire::new(
                        &request,
                        LegacyPythonCredentialStatusWire::Unavailable,
                        None,
                        None,
                    ),
                    Ok(bytes) => {
                        let bytes = Zeroizing::new(bytes);
                        match classify_legacy_python_credential_bytes(&bytes)? {
                            Err(status) => LegacyPythonCredentialResponseWire::new(
                                &request, status, None, None,
                            ),
                            Ok(projection) => {
                                let expected_document = decode_lower_hex_32(
                                    request
                                        .expected_document_digest
                                        .as_deref()
                                        .ok_or_else(household_document_error)?,
                                )?;
                                let expected_noncredential = decode_lower_hex_32(
                                    request
                                        .expected_noncredential_digest
                                        .as_deref()
                                        .ok_or_else(household_document_error)?,
                                )?;
                                if projection.document_digest != expected_document
                                    || projection.noncredential_digest != expected_noncredential
                                {
                                    LegacyPythonCredentialResponseWire::new(
                                        &request,
                                        LegacyPythonCredentialStatusWire::Changed,
                                        None,
                                        None,
                                    )
                                } else if !projection.credentials_present {
                                    LegacyPythonCredentialResponseWire::new(
                                        &request,
                                        LegacyPythonCredentialStatusWire::VerifiedAbsent,
                                        None,
                                        Some(projection.noncredential_digest),
                                    )
                                } else {
                                    write_and_verify_entry(
                                        &entry,
                                        &projection.canonical_noncredential,
                                    )?;
                                    let verified = match entry.get_secret() {
                                        Ok(value) => {
                                            let value = Zeroizing::new(value);
                                            classify_legacy_python_credential_bytes(&value)?
                                        }
                                        Err(_) => {
                                            return Err(PortError::uncertain(
                                                "legacy_python_credential_verify",
                                                "historical credential scrub could not be verified",
                                            ));
                                        }
                                    };
                                    match verified {
                                        Ok(verified)
                                            if !verified.credentials_present
                                                && verified.noncredential_digest
                                                    == expected_noncredential =>
                                        {
                                            LegacyPythonCredentialResponseWire::new(
                                                &request,
                                                LegacyPythonCredentialStatusWire::VerifiedScrubbed,
                                                None,
                                                Some(verified.noncredential_digest),
                                            )
                                        }
                                        _ => {
                                            return Err(PortError::uncertain(
                                                "legacy_python_credential_verify",
                                                "historical credential scrub could not be verified",
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
            };
            let encoded = serde_json::to_vec(&response).map_err(|_| household_document_error())?;
            if encoded.len() > MAX_BROKER_DOCUMENT_BYTES {
                return Err(PortError::new(
                    "household_broker_size",
                    "historical Python credential broker response exceeds its limit",
                ));
            }
            return Ok(encoded);
        }
        let legacy_operation =
            if action == HouseholdBrokerOperationV1::LegacyPythonHouseholdProbe.action() {
                Some(HouseholdBrokerOperationV1::LegacyPythonHouseholdProbe)
            } else if action == HouseholdBrokerOperationV1::LegacyPythonHouseholdLoad.action() {
                Some(HouseholdBrokerOperationV1::LegacyPythonHouseholdLoad)
            } else {
                None
            };
        if let Some(operation) = legacy_operation {
            let request: LegacyPythonBrokerRequestWire =
                serde_json::from_slice(input).map_err(|_| household_document_error())?;
            let (_slot, _config_kind) = request.validate(operation, root)?;
            let locator = LegacyPythonKeyringLocatorV1::from_resolved_config_path_bytes(
                request.target.resolved_config_path.as_bytes(),
            )?;
            let entry = keyring::Entry::new(locator.service, &locator.username)
                .map_err(|_| household_secure_store_error())?;
            let (status, payload) = match entry.get_secret() {
                Ok(bytes) => {
                    let bytes = Zeroizing::new(bytes);
                    classify_legacy_python_keyring_bytes(&bytes)?
                }
                Err(keyring::Error::NoEntry) => {
                    (LegacyPythonBrokerStatusWire::AuthoritativeMissing, None)
                }
                Err(_) => (LegacyPythonBrokerStatusWire::Unavailable, None),
            };
            let payload = if operation == HouseholdBrokerOperationV1::LegacyPythonHouseholdLoad
                && matches!(
                    status,
                    LegacyPythonBrokerStatusWire::PresentHousehold
                        | LegacyPythonBrokerStatusWire::PresentNoHousehold
                ) {
                payload
            } else {
                None
            };
            return encode_legacy_python_response(
                &LegacyPythonBrokerResponseWire::status(&request, status, payload),
                operation,
            );
        }
        if action.starts_with("legacy-python-") {
            return Err(PortError::new(
                "household_broker_request",
                "legacy Python household broker operation is not available",
            ));
        }

        let request: HouseholdBrokerRequestWire =
            serde_json::from_slice(input).map_err(|_| household_document_error())?;
        let slot = request.slot.decode()?;
        if slot.native_root_instance_digest() != household_native_root_instance_digest_v1(root)? {
            return Err(PortError::new(
                "household_broker_root_mismatch",
                "household account slot does not match the broker root",
            ));
        }
        let locators = HouseholdKeyringLocatorsV1::from_account_slot(&slot)?;
        let key_entry = keyring::Entry::new(locators.service, &locators.key_bundle_username)
            .map_err(|_| household_secure_store_error())?;
        let guard_entry = keyring::Entry::new(locators.service, &locators.migration_guard_username)
            .map_err(|_| household_secure_store_error())?;

        match action {
            action if action == HouseholdBrokerOperationV1::KeyLoad.action() => {
                require_load_request(&request)?;
                let response = match load_key_bundle(&key_entry, &slot)? {
                    Some(bundle) => HouseholdBrokerResponseWire {
                        bundle: Some(HouseholdKeyBundleWire::from_bundle(&bundle)),
                        guard: None,
                        status: "ok".to_owned(),
                    },
                    None => HouseholdBrokerResponseWire {
                        bundle: None,
                        guard: None,
                        status: "not_found".to_owned(),
                    },
                };
                encode_household_response(&response)
            }
            action if action == HouseholdBrokerOperationV1::KeyInitialize.action() => {
                if !matches!(request.key_expectation, Some(KeyExpectationWire::Absent))
                    || request.expected_revision.is_some()
                    || request.expected_key_id.is_some()
                    || request.abort_initialization_id.is_some()
                    || request.guard_expectation.is_some()
                    || request.delete_guard
                {
                    return Err(household_document_error());
                }
                let expected_guard = request
                    .guard
                    .as_ref()
                    .ok_or_else(household_document_error)?
                    .decode(&slot)?;
                let bundle = request
                    .bundle
                    .as_ref()
                    .ok_or_else(household_document_error)?
                    .decode(&slot)?;
                let stored_guard = load_guard(&guard_entry, &slot)?;
                if stored_guard.as_ref() != Some(&expected_guard) {
                    return Err(PortError::new(
                        "household_key_guard_mismatch",
                        "household key initialization requires the exact ready migration guard",
                    ));
                }
                bundle.validate_initial_for(&slot, &expected_guard)?;
                if load_key_bundle(&key_entry, &slot)?.is_some() {
                    return Err(PortError::new(
                        "household_key_exists",
                        "household key bundle already exists",
                    ));
                }
                write_key_bundle(&key_entry, &slot, &bundle)
            }
            action if action == HouseholdBrokerOperationV1::KeyReplace.action() => {
                if !matches!(request.key_expectation, Some(KeyExpectationWire::Revision))
                    || request.expected_key_id.is_some()
                    || request.abort_initialization_id.is_some()
                    || request.guard_expectation.is_some()
                    || request.guard.is_some()
                    || request.delete_guard
                {
                    return Err(household_document_error());
                }
                let expected = KeyBundleRevision::new(
                    request
                        .expected_revision
                        .ok_or_else(household_document_error)?,
                )?;
                let replacement = request
                    .bundle
                    .as_ref()
                    .ok_or_else(household_document_error)?
                    .decode(&slot)?;
                if replacement.revision != expected.checked_next()? {
                    return Err(household_document_error());
                }
                let current = load_key_bundle(&key_entry, &slot)?.ok_or_else(|| {
                    PortError::new("household_key_not_found", "household key bundle is absent")
                })?;
                if current.revision != expected {
                    return Err(PortError::new(
                        "household_key_cas",
                        "household key bundle changed concurrently",
                    ));
                }
                write_key_bundle(&key_entry, &slot, &replacement)
            }
            action if action == HouseholdBrokerOperationV1::KeyDelete.action() => {
                if request.bundle.is_some()
                    || request.key_expectation.is_some()
                    || request.abort_initialization_id.is_some()
                    || request.guard_expectation.is_some()
                    || request.guard.is_some()
                    || request.delete_guard
                {
                    return Err(household_document_error());
                }
                let expected_revision = KeyBundleRevision::new(
                    request
                        .expected_revision
                        .ok_or_else(household_document_error)?,
                )?;
                let expected_key_id = KeyId::from_uuid(
                    request
                        .expected_key_id
                        .ok_or_else(household_document_error)?,
                );
                let current = load_key_bundle(&key_entry, &slot)?.ok_or_else(|| {
                    PortError::new("household_key_not_found", "household key bundle is absent")
                })?;
                if current.revision != expected_revision || current.active_key_id != expected_key_id
                {
                    return Err(PortError::new(
                        "household_key_cas",
                        "household key bundle changed concurrently",
                    ));
                }
                delete_and_verify_entry(&key_entry)
            }
            action if action == HouseholdBrokerOperationV1::KeyAbortInitialization.action() => {
                if request.bundle.is_some()
                    || request.key_expectation.is_some()
                    || request.expected_key_id.is_some()
                    || request.guard_expectation.is_some()
                    || request.delete_guard
                {
                    return Err(household_document_error());
                }
                let expected_guard = request
                    .guard
                    .as_ref()
                    .ok_or_else(household_document_error)?
                    .decode(&slot)?;
                let expected_revision = KeyBundleRevision::new(
                    request
                        .expected_revision
                        .ok_or_else(household_document_error)?,
                )?;
                let expected_initialization_id = request
                    .abort_initialization_id
                    .ok_or_else(household_document_error)?;
                if expected_guard.state() != HouseholdMigrationGuardStateV1::Aborting
                    || expected_guard.initialization_id() != expected_initialization_id
                {
                    return Err(PortError::new(
                        "household_key_abort_guard",
                        "household key abort requires a cleanup-pending migration guard",
                    ));
                }
                let stored_guard = load_guard(&guard_entry, &slot)?;
                if stored_guard.as_ref() != Some(&expected_guard) {
                    return Err(PortError::new(
                        "household_key_abort_guard",
                        "household key abort requires the authoritative cleanup-pending guard",
                    ));
                }
                let current = load_key_bundle(&key_entry, &slot)?.ok_or_else(|| {
                    PortError::new("household_key_not_found", "household key bundle is absent")
                })?;
                if current.revision != expected_revision
                    || current.phase != HouseholdKeyBundlePhase::Initializing
                    || current.initialization_id != Some(expected_initialization_id)
                {
                    return Err(PortError::new(
                        "household_key_abort_cas",
                        "household initializing key bundle changed concurrently",
                    ));
                }
                delete_and_verify_entry(&key_entry)
            }
            action if action == HouseholdBrokerOperationV1::KeyVerifyAbsent.action() => {
                require_load_request(&request)?;
                if load_key_bundle(&key_entry, &slot)?.is_some() {
                    Err(PortError::new(
                        "household_key_present",
                        "household key bundle is still present",
                    ))
                } else {
                    Ok(Vec::new())
                }
            }
            action if action == HouseholdBrokerOperationV1::MigrationGuardLoad.action() => {
                require_load_request(&request)?;
                let response = match load_guard(&guard_entry, &slot)? {
                    Some(guard) => HouseholdBrokerResponseWire {
                        bundle: None,
                        guard: Some(HouseholdMigrationGuardWire::from_guard(&guard)?),
                        status: "ok".to_owned(),
                    },
                    None => HouseholdBrokerResponseWire {
                        bundle: None,
                        guard: None,
                        status: "not_found".to_owned(),
                    },
                };
                encode_household_response(&response)
            }
            action
                if action == HouseholdBrokerOperationV1::MigrationGuardCompareExchange.action() =>
            {
                if request.bundle.is_some()
                    || request.key_expectation.is_some()
                    || request.expected_key_id.is_some()
                    || request.abort_initialization_id.is_some()
                {
                    return Err(household_document_error());
                }
                let current = load_guard(&guard_entry, &slot)?;
                let matches = match request.guard_expectation {
                    Some(GuardExpectationWire::Absent) => {
                        request.expected_revision.is_none() && current.is_none()
                    }
                    Some(GuardExpectationWire::Revision) => {
                        request.expected_revision.is_some_and(|revision| {
                            current
                                .as_ref()
                                .is_some_and(|guard| guard.guard_revision() == revision)
                        })
                    }
                    None => false,
                };
                if !matches {
                    return Err(PortError::new(
                        "household_migration_guard_cas",
                        "household migration guard changed concurrently",
                    ));
                }
                if request.delete_guard {
                    return Err(PortError::new(
                        "household_migration_guard_delete_forbidden",
                        "household migration guards are retained for the account lifetime",
                    ));
                }
                let replacement = request
                    .guard
                    .as_ref()
                    .ok_or_else(household_document_error)?
                    .decode(&slot)?;
                if matches!(
                    request.guard_expectation,
                    Some(GuardExpectationWire::Absent)
                ) {
                    if replacement.guard_revision() != 1
                        || replacement.state() != HouseholdMigrationGuardStateV1::Initializing
                        || replacement.initialization_phase()
                            != Some(HouseholdMigrationInitializationPhaseV1::ReservedSource)
                    {
                        return Err(PortError::new(
                            "household_migration_guard_revision",
                            "initial household migration guard must be reserved at revision one",
                        ));
                    }
                } else if let Some(current) = &current {
                    replacement.validate_transition_from(current)?;
                }
                write_guard(&guard_entry, &slot, &replacement)
            }
            _ => Err(PortError::new(
                "household_broker_request",
                "invalid household broker request",
            )),
        }
    }

    fn require_load_request(request: &HouseholdBrokerRequestWire) -> Result<(), PortError> {
        if request.abort_initialization_id.is_some()
            || request.bundle.is_some()
            || request.delete_guard
            || request.expected_key_id.is_some()
            || request.expected_revision.is_some()
            || request.guard.is_some()
            || request.guard_expectation.is_some()
            || request.key_expectation.is_some()
        {
            Err(household_document_error())
        } else {
            Ok(())
        }
    }

    fn encode_household_response(
        response: &HouseholdBrokerResponseWire,
    ) -> Result<Vec<u8>, PortError> {
        let encoded = serde_json::to_vec(response).map_err(|_| household_document_error())?;
        if encoded.len() > MAX_BROKER_DOCUMENT_BYTES {
            return Err(PortError::new(
                "household_broker_size",
                "household broker response exceeds its limit",
            ));
        }
        Ok(encoded)
    }

    fn encode_legacy_python_response(
        response: &LegacyPythonBrokerResponseWire,
        operation: HouseholdBrokerOperationV1,
    ) -> Result<Vec<u8>, PortError> {
        if !matches!(
            operation,
            HouseholdBrokerOperationV1::LegacyPythonHouseholdProbe
                | HouseholdBrokerOperationV1::LegacyPythonHouseholdLoad
        ) {
            return Err(household_document_error());
        }
        let encoded = serde_json::to_vec(response).map_err(|_| household_document_error())?;
        if encoded.len() > operation.response_limit() {
            return Err(PortError::new(
                "household_broker_size",
                "historical Python keyring broker response exceeds its operation limit",
            ));
        }
        Ok(encoded)
    }

    fn load_key_bundle(
        entry: &keyring::Entry,
        slot: &HouseholdAccountSlotV1,
    ) -> Result<Option<HouseholdKeyBundle>, PortError> {
        match entry.get_secret() {
            Ok(bytes) => {
                let bytes = Zeroizing::new(bytes);
                if bytes.len() > MAX_BROKER_DOCUMENT_BYTES {
                    return Err(PortError::new(
                        "household_broker_size",
                        "household key bundle exceeds its limit",
                    ));
                }
                let wire: HouseholdKeyBundleWire =
                    serde_json::from_slice(&bytes).map_err(|_| household_document_error())?;
                wire.decode(slot).map(Some)
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(household_secure_store_error()),
        }
    }

    fn write_key_bundle(
        entry: &keyring::Entry,
        slot: &HouseholdAccountSlotV1,
        bundle: &HouseholdKeyBundle,
    ) -> Result<Vec<u8>, PortError> {
        bundle.validate_for(slot)?;
        let document = Zeroizing::new(
            serde_json::to_vec(&HouseholdKeyBundleWire::from_bundle(bundle))
                .map_err(|_| household_document_error())?,
        );
        write_and_verify_entry(entry, &document)
    }

    fn load_guard(
        entry: &keyring::Entry,
        slot: &HouseholdAccountSlotV1,
    ) -> Result<Option<HouseholdMigrationGuardDocument>, PortError> {
        match entry.get_secret() {
            Ok(bytes) => {
                let bytes = Zeroizing::new(bytes);
                if bytes.len() > MAX_BROKER_DOCUMENT_BYTES {
                    return Err(PortError::new(
                        "household_broker_size",
                        "household migration guard exceeds its limit",
                    ));
                }
                HouseholdMigrationGuardDocument::from_canonical_bytes(slot, &bytes).map(Some)
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(household_secure_store_error()),
        }
    }

    fn write_guard(
        entry: &keyring::Entry,
        slot: &HouseholdAccountSlotV1,
        guard: &HouseholdMigrationGuardDocument,
    ) -> Result<Vec<u8>, PortError> {
        guard.validate_for(slot)?;
        let document = Zeroizing::new(guard.canonical_bytes()?);
        write_and_verify_entry(entry, &document)
    }

    fn write_and_verify_entry(
        entry: &keyring::Entry,
        document: &[u8],
    ) -> Result<Vec<u8>, PortError> {
        if document.len() > MAX_BROKER_DOCUMENT_BYTES {
            return Err(PortError::new(
                "household_broker_size",
                "household secure-store document exceeds its limit",
            ));
        }
        entry.set_secret(document).map_err(|_| {
            PortError::uncertain(
                "household_secure_store_write",
                "household secure-store write outcome is uncertain",
            )
        })?;
        match entry.get_secret() {
            Ok(verified) => {
                let verified = Zeroizing::new(verified);
                if verified.as_slice() == document {
                    Ok(Vec::new())
                } else {
                    Err(PortError::uncertain(
                        "household_secure_store_verify",
                        "household secure-store write could not be verified",
                    ))
                }
            }
            Err(_) => Err(PortError::uncertain(
                "household_secure_store_verify",
                "household secure-store write could not be verified",
            )),
        }
    }

    fn delete_and_verify_entry(entry: &keyring::Entry) -> Result<Vec<u8>, PortError> {
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(_) => {
                return Err(PortError::uncertain(
                    "household_secure_store_delete",
                    "household secure-store delete outcome is uncertain",
                ));
            }
        }
        match entry.get_secret() {
            Err(keyring::Error::NoEntry) => Ok(Vec::new()),
            Ok(value) => {
                let _value = Zeroizing::new(value);
                Err(PortError::uncertain(
                    "household_secure_store_delete_verify",
                    "household secure-store deletion could not be verified",
                ))
            }
            Err(_) => Err(PortError::uncertain(
                "household_secure_store_delete_verify",
                "household secure-store deletion could not be verified",
            )),
        }
    }

    fn household_secure_store_error() -> PortError {
        PortError::new(
            "household_secure_store_unavailable",
            "native household secure storage is unavailable",
        )
    }

    fn required_field<'a>(input: &'a [u8], name: &str) -> Result<&'a str, PortError> {
        let input = std::str::from_utf8(input)
            .map_err(|_| PortError::new("credential_broker_request", "request is not UTF-8"))?;
        input
            .lines()
            .find_map(|line| line.split_once('=').filter(|(key, _)| *key == name))
            .map(|(_, value)| value)
            .ok_or_else(|| {
                PortError::new(
                    "credential_broker_request",
                    format!("request is missing {name}"),
                )
            })
    }

    fn trimmed_input(input: &[u8]) -> Result<&str, PortError> {
        std::str::from_utf8(input)
            .map(str::trim)
            .map_err(|_| PortError::new("credential_broker_request", "request is not UTF-8"))
    }

    fn parse_commit_id(value: &str) -> Result<CommitId, PortError> {
        serde_json::from_value(serde_json::Value::String(value.to_owned()))
            .map_err(|_| PortError::new("credential_broker_request", "invalid commit ID"))
    }

    #[cfg(test)]
    mod tests {
        use std::io::{Read, Write};
        use std::path::{Path, PathBuf};
        use std::process::{Command as BlockingCommand, Stdio};
        use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

        #[cfg(any(target_os = "macos", target_os = "linux"))]
        use heyfood_core::AccountId;
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        use heyfood_core::CommitId;
        use sysinfo::{Pid, ProcessesToUpdate, System};
        use tokio::process::Command;
        use tokio_util::sync::CancellationToken;
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        use uuid::Uuid;
        use zeroize::Zeroizing;

        #[cfg(any(target_os = "macos", target_os = "linux"))]
        use crate::household_vault::HouseholdVault;
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        use crate::python_import::LegacyPythonConfigKindV1;

        #[cfg(any(target_os = "macos", target_os = "linux"))]
        use super::{
            HouseholdBrokerOperationV1, HouseholdKeyBundleWire, LegacyPythonBrokerRequestWire,
            LegacyPythonBrokerResponseWire, LegacyPythonHouseholdPayloadWire,
            LegacyPythonTargetWire, decode_lower_hex_32, require_legacy_payload_absent,
            validate_legacy_python_response,
        };
        use super::{
            LegacyPythonBrokerStatusWire, LegacyPythonCredentialStatusWire,
            MAX_BROKER_DOCUMENT_BYTES, classify_legacy_python_credential_bytes,
            classify_legacy_python_keyring_bytes, run_bounded_child, run_bounded_child_blocking,
            run_household_bounded_child,
        };
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        use crate::{HouseholdKeyBundle, HouseholdKeyMaterial, KeyBundleRevision, KeyId};

        #[cfg(any(target_os = "macos", target_os = "linux"))]
        fn legacy_wire_fixture() -> (
            PathBuf,
            HouseholdVault,
            LegacyPythonTargetWire,
            LegacyPythonBrokerRequestWire,
        ) {
            let root = std::env::temp_dir().join(format!(
                "heyfood-legacy-broker-wire-{}-{}",
                std::process::id(),
                Uuid::new_v4()
            ));
            std::fs::create_dir_all(&root).expect("root");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;

                std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
                    .expect("permissions");
            }
            let vault = HouseholdVault::open(
                &root,
                AccountId::parse("acct_legacy_broker").expect("account"),
            )
            .expect("vault");
            let config_path = root.join(".config").join("heyfood").join("config.json");
            let target = LegacyPythonTargetWire::new(
                LegacyPythonConfigKindV1::Current,
                &config_path,
                vault.account_slot(),
            )
            .expect("target");
            let request = LegacyPythonBrokerRequestWire::new(
                HouseholdBrokerOperationV1::LegacyPythonHouseholdProbe,
                vault.account_slot(),
                target.clone(),
            );
            (root, vault, target, request)
        }

        #[cfg(any(target_os = "macos", target_os = "linux"))]
        #[test]
        fn key_bundle_v2_round_trips_evidence_and_v1_upgrades_to_a_stable_root() {
            let (root, vault, _, _) = legacy_wire_fixture();
            let bundle = HouseholdKeyBundle::stable(
                vault.account_slot(),
                KeyBundleRevision::new(1).expect("revision"),
                KeyId::from_uuid(Uuid::parse_str("41414141-4141-4141-8141-414141414141").unwrap()),
                HouseholdKeyMaterial::from_bytes([0x42; 32]),
            )
            .reserve_commit_evidence(
                Uuid::parse_str("42424242-4242-4242-8242-424242424242").unwrap(),
                CommitId::from_uuid(
                    Uuid::parse_str("43434343-4343-4343-8343-434343434343").unwrap(),
                ),
                1_785_700_800,
                &[],
            )
            .expect("reservation");
            let encoded = serde_json::to_vec(&HouseholdKeyBundleWire::from_bundle(&bundle))
                .expect("encode v2 bundle");
            let decoded: HouseholdKeyBundleWire =
                serde_json::from_slice(&encoded).expect("decode v2 wire");
            assert_eq!(decoded.decode(vault.account_slot()).unwrap(), bundle);

            let legacy = HouseholdKeyBundle::stable(
                vault.account_slot(),
                KeyBundleRevision::new(1).expect("revision"),
                KeyId::from_uuid(Uuid::parse_str("44444444-4444-4444-8444-444444444444").unwrap()),
                HouseholdKeyMaterial::from_bytes([0x45; 32]),
            );
            let mut legacy_value =
                serde_json::to_value(HouseholdKeyBundleWire::from_bundle(&legacy))
                    .expect("legacy value");
            let object = legacy_value.as_object_mut().expect("legacy object");
            object.insert("schema_version".to_owned(), serde_json::json!(1));
            object.remove("commit_evidence_key");
            object.remove("commit_evidence_records");
            let legacy_wire: HouseholdKeyBundleWire =
                serde_json::from_value(legacy_value).expect("legacy wire");
            assert_eq!(legacy_wire.decode(vault.account_slot()).unwrap(), legacy);
            let _ = std::fs::remove_dir_all(root);
        }

        #[cfg(any(target_os = "macos", target_os = "linux"))]
        #[test]
        fn evidence_records_are_hashed_bounded_releasable_and_expiry_pruned() {
            let (root, vault, _, _) = legacy_wire_fixture();
            let now = 1_785_700_800;
            let mut bundle = HouseholdKeyBundle::stable(
                vault.account_slot(),
                KeyBundleRevision::new(1).expect("revision"),
                KeyId::new(),
                HouseholdKeyMaterial::from_bytes([0x46; 32]),
            );
            let mut identities = Vec::new();
            for _ in 0..crate::credential_broker::MAX_HOUSEHOLD_COMMIT_EVIDENCE_RECORDS {
                let proposal_ref = Uuid::new_v4();
                let commit_id = CommitId::new();
                bundle = bundle
                    .reserve_commit_evidence(proposal_ref, commit_id, now, &[])
                    .expect("bounded reservation");
                identities.push((proposal_ref, commit_id));
            }
            let encoded = serde_json::to_vec(&HouseholdKeyBundleWire::from_bundle(&bundle))
                .expect("bounded key bundle wire");
            assert!(encoded.len() <= MAX_BROKER_DOCUMENT_BYTES);
            for (proposal_ref, _) in &identities {
                assert!(!String::from_utf8_lossy(&encoded).contains(&proposal_ref.to_string()));
            }
            let (denied_proposal, denied_commit) = identities[1];
            bundle = bundle
                .deny_reserved_commit(denied_proposal, denied_commit, now)
                .expect("terminal absence keeps a bounded delayed-dispatch fence");
            assert!(bundle.denies_commit(denied_commit, now));
            let capacity = bundle
                .reserve_commit_evidence(Uuid::new_v4(), CommitId::new(), now, &[])
                .expect_err("capacity is explicit");
            assert_eq!(capacity.code, "household_commit_evidence_capacity");

            let applied_commit = identities[2].1;
            bundle = bundle
                .reserve_commit_evidence(Uuid::new_v4(), CommitId::new(), now, &[applied_commit])
                .expect("authenticated applied ledger retires auxiliary reservation");
            assert_eq!(
                bundle.commit_evidence_records.len(),
                crate::credential_broker::MAX_HOUSEHOLD_COMMIT_EVIDENCE_RECORDS
            );

            let (released_proposal, released_commit) = identities[0];
            bundle = bundle
                .release_reserved_commit(released_proposal, released_commit, now)
                .expect("release undispatched reservation");
            bundle = bundle
                .reserve_commit_evidence(Uuid::new_v4(), CommitId::new(), now, &[])
                .expect("released capacity is reusable");
            assert_eq!(
                bundle.commit_evidence_records.len(),
                crate::credential_broker::MAX_HOUSEHOLD_COMMIT_EVIDENCE_RECORDS
            );

            let after_retention = now + crate::credential_broker::COMMIT_EVIDENCE_RETENTION_SECONDS;
            assert!(!bundle.denies_commit(denied_commit, after_retention));
            bundle = bundle
                .reserve_commit_evidence(Uuid::new_v4(), CommitId::new(), after_retention, &[])
                .expect("expired orphan and terminal records are pruned");
            assert_eq!(bundle.commit_evidence_records.len(), 1);
            let encoded = serde_json::to_vec(&HouseholdKeyBundleWire::from_bundle(&bundle))
                .expect("pruned key bundle wire");
            assert!(encoded.len() <= MAX_BROKER_DOCUMENT_BYTES);
            let _ = std::fs::remove_dir_all(root);
        }

        #[test]
        fn credential_projection_removes_only_exact_secrets_and_preserves_retained_digest() {
            let source = br#"{
                "api_key":"api-canary",
                "oauth.access_token":"oauth-access-canary",
                "oauth.refresh_token":"oauth-refresh-canary",
                "session.access_token":"session-access-canary",
                "session.refresh_token":"session-refresh-canary",
                "household.state":{"owner_id":"_self"},
                "oauth":{"client_id":"public-client","access_token":"nested-access-canary"},
                "session":{"user_id":"account-a","refresh_token":"nested-refresh-canary"},
                "unknown":{"secret":"retained-noncredential-canary"}
            }"#;
            let projection = classify_legacy_python_credential_bytes(source)
                .expect("classification")
                .expect("valid");
            assert!(projection.credentials_present);
            let retained = String::from_utf8(projection.canonical_noncredential.to_vec())
                .expect("canonical UTF-8");
            for secret in [
                "api-canary",
                "oauth-access-canary",
                "oauth-refresh-canary",
                "session-access-canary",
                "session-refresh-canary",
                "nested-access-canary",
                "nested-refresh-canary",
            ] {
                assert!(!retained.contains(secret));
            }
            assert!(retained.contains("public-client"));
            assert!(retained.contains("account-a"));
            assert!(retained.contains("retained-noncredential-canary"));

            let verified =
                classify_legacy_python_credential_bytes(&projection.canonical_noncredential)
                    .expect("verification classification")
                    .expect("verification valid");
            assert!(!verified.credentials_present);
            assert_eq!(
                verified.noncredential_digest,
                projection.noncredential_digest
            );
        }

        #[test]
        fn malformed_or_oversized_credential_documents_never_authorize_scrub() {
            assert!(matches!(
                classify_legacy_python_credential_bytes(br#"{"api_key":42}"#)
                    .expect("classification"),
                Err(LegacyPythonCredentialStatusWire::Malformed)
            ));
            assert!(matches!(
                classify_legacy_python_credential_bytes(&vec![b'x'; (4 * 1024 * 1024) + 1])
                    .expect("classification"),
                Err(LegacyPythonCredentialStatusWire::Oversized)
            ));
        }

        #[test]
        fn legacy_household_sanitizer_never_returns_credentials_or_unknown_fields() {
            let raw = br#"{
                "api_key":"TOP-LEVEL-CANARY",
                "oauth.access_token":"OAUTH-CANARY",
                "household.state":{"members":[]},
                "household.local_profiles":{"owner":{"restrictions":["sesame"]}},
                "household.profile_outbox":{},
                "conversation.pending_preview":{"secret":"PREVIEW-CANARY"},
                "future":{"secret":"UNKNOWN-CANARY"}
            }"#;
            let (status, payload) = classify_legacy_python_keyring_bytes(raw).expect("classify");
            assert_eq!(status, LegacyPythonBrokerStatusWire::PresentHousehold);
            let payload = payload.expect("payload");
            let sanitized = payload
                .canonical_household_document()
                .expect("canonical payload");
            let text = String::from_utf8(sanitized).expect("UTF-8");
            assert!(text.contains("\"household.state\""));
            assert!(text.contains("\"household.local_profiles\""));
            assert!(text.contains("\"household.profile_outbox\""));
            for canary in [
                "TOP-LEVEL-CANARY",
                "OAUTH-CANARY",
                "PREVIEW-CANARY",
                "UNKNOWN-CANARY",
                "api_key",
                "oauth.access_token",
                "conversation.pending_preview",
                "future",
            ] {
                assert!(!text.contains(canary), "{canary}");
            }
        }

        #[test]
        fn legacy_household_sanitizer_reports_closed_failure_classes() {
            let (status, payload) =
                classify_legacy_python_keyring_bytes(b"not-json").expect("malformed");
            assert_eq!(status, LegacyPythonBrokerStatusWire::MalformedJson);
            assert!(payload.is_none());

            let (status, payload) =
                classify_legacy_python_keyring_bytes(br#"{"household.state":[]}"#)
                    .expect("invalid subdocument");
            assert_eq!(
                status,
                LegacyPythonBrokerStatusWire::InvalidHouseholdSubdocument
            );
            assert!(payload.is_none());

            let oversized = vec![b' '; (4 * 1024 * 1024) + 1];
            let (status, payload) =
                classify_legacy_python_keyring_bytes(&oversized).expect("oversized");
            assert_eq!(status, LegacyPythonBrokerStatusWire::OversizedEntry);
            assert!(payload.is_none());

            let (status, payload) =
                classify_legacy_python_keyring_bytes(br#"{"api_key":"ignored"}"#)
                    .expect("no household");
            assert_eq!(status, LegacyPythonBrokerStatusWire::PresentNoHousehold);
            assert_eq!(
                payload
                    .expect("empty payload")
                    .canonical_household_document()
                    .expect("canonical"),
                b"{}"
            );
        }

        #[cfg(any(target_os = "macos", target_os = "linux"))]
        #[test]
        fn legacy_broker_wires_reject_action_locator_account_and_payload_swaps() {
            let (root, vault, target, request) = legacy_wire_fixture();
            request
                .validate(
                    HouseholdBrokerOperationV1::LegacyPythonHouseholdProbe,
                    &root,
                )
                .expect("valid request");

            let mut wrong_action = request.clone();
            wrong_action.operation = HouseholdBrokerOperationV1::LegacyPythonHouseholdLoad
                .action()
                .to_owned();
            assert_eq!(
                wrong_action
                    .validate(
                        HouseholdBrokerOperationV1::LegacyPythonHouseholdProbe,
                        &root,
                    )
                    .expect_err("action swap")
                    .code,
                "legacy_python_broker_binding"
            );

            let mut wrong_suffix = request.clone();
            wrong_suffix.target.resolved_config_path = root
                .join(".config/hellofood/config.json")
                .to_string_lossy()
                .into_owned();
            assert_eq!(
                wrong_suffix
                    .validate(
                        HouseholdBrokerOperationV1::LegacyPythonHouseholdProbe,
                        &root,
                    )
                    .expect_err("suffix swap")
                    .code,
                "legacy_python_broker_binding"
            );

            let mut wrong_locator = request.clone();
            wrong_locator.target.locator_digest = "00".repeat(32);
            assert_eq!(
                wrong_locator
                    .validate(
                        HouseholdBrokerOperationV1::LegacyPythonHouseholdProbe,
                        &root,
                    )
                    .expect_err("locator swap")
                    .code,
                "legacy_python_broker_binding"
            );

            let response = LegacyPythonBrokerResponseWire::status(
                &request,
                LegacyPythonBrokerStatusWire::AuthoritativeMissing,
                None,
            );
            validate_legacy_python_response(
                &response,
                HouseholdBrokerOperationV1::LegacyPythonHouseholdProbe,
                vault.account_slot(),
                &target,
            )
            .expect("bound response");
            let mut wrong_response = response;
            wrong_response.operation = HouseholdBrokerOperationV1::LegacyPythonHouseholdLoad
                .action()
                .to_owned();
            assert_eq!(
                validate_legacy_python_response(
                    &wrong_response,
                    HouseholdBrokerOperationV1::LegacyPythonHouseholdProbe,
                    vault.account_slot(),
                    &target,
                )
                .expect_err("response action swap")
                .code,
                "legacy_python_broker_binding"
            );

            let payload = LegacyPythonHouseholdPayloadWire::from_objects(
                Some(serde_json::from_value(serde_json::json!({"members": []})).expect("map")),
                None,
                None,
            )
            .expect("payload");
            let mut tampered = payload.clone();
            tampered.payload_digest = "00".repeat(32);
            assert_eq!(
                tampered
                    .canonical_household_document()
                    .expect_err("payload digest swap")
                    .code,
                "household_broker_document"
            );
            let response_with_probe_payload = LegacyPythonBrokerResponseWire::status(
                &request,
                LegacyPythonBrokerStatusWire::PresentHousehold,
                Some(payload),
            );
            assert_eq!(
                require_legacy_payload_absent(&response_with_probe_payload)
                    .expect_err("probe value leak")
                    .code,
                "household_broker_document"
            );

            assert_eq!(
                decode_lower_hex_32(&target.locator_digest).expect("digest"),
                *super::LegacyPythonKeyringLocatorV1::from_resolved_config_path_bytes(
                    target.resolved_config_path.as_bytes()
                )
                .expect("locator")
                .locator_digest()
                .expect("locator digest")
                .as_bytes()
            );
            drop(vault);
            let _ = std::fs::remove_dir_all(root);
        }

        #[test]
        #[ignore = "spawned only by the bounded broker lifecycle test"]
        fn broker_prompt_fixture() {
            let path = std::env::var_os("HEYFOOD_BROKER_TEST_PID_FILE")
                .map(PathBuf::from)
                .expect("fixture PID path");
            std::fs::write(path, format!("{}\n", std::process::id())).expect("publish fixture PID");
            std::thread::sleep(Duration::from_secs(30));
        }

        #[test]
        #[ignore = "spawned only by the blocking broker lifecycle tests"]
        fn broker_blocking_io_fixture() {
            let mode = std::env::var("HEYFOOD_BROKER_BLOCKING_FIXTURE").expect("fixture mode");
            if let Some(path) = std::env::var_os("HEYFOOD_BROKER_TEST_PID_FILE") {
                std::fs::write(path, format!("{}\n", std::process::id()))
                    .expect("publish fixture PID");
            }
            if mode == "stall" {
                std::thread::sleep(Duration::from_secs(30));
                return;
            }
            let mut input = Vec::new();
            std::io::stdin().read_to_end(&mut input).unwrap();
            assert_eq!(input.len(), MAX_BROKER_DOCUMENT_BYTES);
            std::io::stdout()
                .write_all(&vec![0; MAX_BROKER_DOCUMENT_BYTES / 2])
                .unwrap();
        }

        fn blocking_fixture(mode: &str, pid_path: Option<&std::path::Path>) -> std::process::Child {
            let mut command =
                BlockingCommand::new(std::env::current_exe().expect("test executable"));
            command
                .args([
                    "--exact",
                    "credential_broker::native::tests::broker_blocking_io_fixture",
                    "--ignored",
                    "--nocapture",
                ])
                .env("HEYFOOD_BROKER_BLOCKING_FIXTURE", mode)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            if let Some(path) = pid_path {
                command.env("HEYFOOD_BROKER_TEST_PID_FILE", path);
            }
            command.spawn().expect("spawn blocking fixture")
        }

        fn wait_for_fixture_pid(path: &Path) -> Pid {
            let started = Instant::now();
            loop {
                if let Some(pid) = std::fs::read_to_string(path)
                    .ok()
                    .and_then(|value| value.trim().parse::<u32>().ok())
                {
                    return Pid::from_u32(pid);
                }
                assert!(
                    started.elapsed() < Duration::from_secs(10),
                    "fixture did not publish its PID"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        fn assert_process_exited(pid: Pid) {
            let mut system = System::new();
            for _ in 0..100 {
                system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
                if system.process(pid).is_none() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("timed-out broker process remained alive");
        }

        #[test]
        fn blocking_broker_concurrently_drains_full_bounded_input_and_output() {
            let child = blocking_fixture("echo", None);
            let output = run_bounded_child_blocking(
                child,
                vec![b'i'; MAX_BROKER_DOCUMENT_BYTES],
                Duration::from_secs(5),
                false,
            )
            .unwrap();
            assert_eq!(
                output.iter().filter(|byte| **byte == 0).count(),
                MAX_BROKER_DOCUMENT_BYTES / 2
            );
        }

        #[test]
        fn blocking_broker_deadline_covers_a_child_stalled_before_stdin_read() {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let pid_path = std::env::temp_dir().join(format!(
                "heyfood-blocking-broker-pid-{}-{nonce}",
                std::process::id()
            ));
            let child = blocking_fixture("stall", Some(&pid_path));
            let pid = wait_for_fixture_pid(&pid_path);
            let error = run_bounded_child_blocking(
                child,
                vec![b'i'; MAX_BROKER_DOCUMENT_BYTES],
                Duration::from_millis(250),
                true,
            )
            .unwrap_err();
            assert_eq!(error.code, "credential_broker_timeout");
            assert!(error.outcome_uncertain);
            assert_process_exited(pid);
            let _ = std::fs::remove_file(pid_path);
        }

        #[tokio::test]
        async fn timeout_terminates_a_prompting_broker_without_an_orphan() {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let pid_path = std::env::temp_dir()
                .join(format!("heyfood-broker-pid-{}-{nonce}", std::process::id()));
            let mut child = Command::new(std::env::current_exe().expect("test executable"));
            child
                .args([
                    "--exact",
                    "credential_broker::native::tests::broker_prompt_fixture",
                    "--ignored",
                    "--nocapture",
                ])
                .env("HEYFOOD_BROKER_TEST_PID_FILE", &pid_path)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            let child = child.spawn().expect("spawn prompt fixture");
            let pid = wait_for_fixture_pid(&pid_path);
            let error = run_bounded_child(child, Vec::new(), Duration::from_millis(250), true)
                .await
                .expect_err("prompting broker must time out");
            assert_eq!(error.code, "credential_broker_timeout");
            assert!(error.outcome_uncertain);

            assert_process_exited(pid);
            let _ = std::fs::remove_file(&pid_path);
        }

        #[tokio::test]
        async fn household_broker_cancellation_kills_and_reaps_the_owned_child() {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let pid_path = std::env::temp_dir().join(format!(
                "heyfood-household-broker-pid-{}-{nonce}",
                std::process::id()
            ));
            let mut child = Command::new(std::env::current_exe().expect("test executable"));
            child
                .args([
                    "--exact",
                    "credential_broker::native::tests::broker_prompt_fixture",
                    "--ignored",
                    "--nocapture",
                ])
                .env("HEYFOOD_BROKER_TEST_PID_FILE", &pid_path)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            let child = child.spawn().expect("spawn prompt fixture");
            let pid = wait_for_fixture_pid(&pid_path);
            let cancellation = CancellationToken::new();
            let trigger = cancellation.clone();
            let cancel_task = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(250)).await;
                trigger.cancel();
            });
            let error = run_household_bounded_child(
                child,
                Zeroizing::new(Vec::new()),
                Duration::from_secs(5),
                cancellation,
                MAX_BROKER_DOCUMENT_BYTES,
                true,
            )
            .await
            .expect_err("household broker must be cancelled");
            cancel_task.await.expect("cancel task");
            assert_eq!(error.code, "household_broker_cancelled");
            assert!(error.outcome_uncertain);
            assert_process_exited(pid);
            let _ = std::fs::remove_file(&pid_path);
        }
    }
} // mod native

#[cfg(feature = "native-credentials")]
pub use native::{
    CredentialBrokerStore, HouseholdKeyBroker, LegacyPythonCredentialProbeResultV1,
    LegacyPythonCredentialScrubAuthorityV1, LegacyPythonCredentialScrubResultV1,
    LegacyPythonHouseholdLoadAuthorityV1, LegacyPythonHouseholdProbeResultV1,
    run_credential_broker_if_requested,
};
