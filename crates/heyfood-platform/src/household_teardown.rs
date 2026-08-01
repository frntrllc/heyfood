//! Globally resumable native-household account teardown.
//!
//! The plaintext journal is intentionally content-free: it contains only
//! account/transaction digests, canonical identifiers, and cleanup outcomes.
//! Secret values, raw account IDs, paths, and household/profile values never
//! enter this module's durable representation.

use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, File};
#[cfg(feature = "native-credentials")]
use std::io::Read;
use std::path::{Path, PathBuf};

use heyfood_application::{BoxFuture, HouseholdEraseOutcome, PortError};
#[cfg(feature = "native-credentials")]
use heyfood_core::{
    AccountId, AppliedCommitOutcomeV1, decode_canonical_household_state_v1,
    parse_bounded_json_object_v1,
};
use heyfood_core::{
    AuthCredentialBundle, CanonicalDigestV1, CompatibilityJsonLimitsV1,
    parse_bounded_typed_json_v1, to_canonical_bytes_v1,
};
use serde::{Deserialize, Serialize};
#[cfg(feature = "native-credentials")]
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[cfg(feature = "native-credentials")]
use crate::household_vault::HouseholdTeardownVaultTargetV1;
#[cfg(feature = "native-credentials")]
use crate::python_import::LegacyPythonCredentialSourceLeaseV1;
use crate::{
    AtomicFile, HouseholdMigrationSourceIdentityV1, OwnerOnlyPath,
    household_native_root_instance_digest_v1,
};
#[cfg(feature = "native-credentials")]
use crate::{
    AuthorizationSessionStore, HouseholdAccountSlotV1, HouseholdKeyBroker, HouseholdKeyBundlePhase,
    HouseholdKeyStore, HouseholdLifecycleLease, HouseholdMigrationGuardDocument,
    HouseholdMigrationGuardStateV1, HouseholdMigrationGuardStore, HouseholdVault,
    HouseholdVaultLease, HouseholdVaultLoad, LegacyPythonConfigKindV1,
    LegacyPythonCredentialProbeResultV1, LegacyPythonCredentialScrubResultV1,
    LegacyPythonHouseholdMigrationV1, LegacyPythonKeyringLocatorV1, LegacyPythonSourceLeaseV1,
    MigrationGuardExpectation, NativeAuthStore, NativePaths,
};
#[cfg(feature = "native-credentials")]
use zeroize::Zeroizing;

pub const MAX_HOUSEHOLD_TEARDOWN_JOURNAL_BYTES: usize = 16 * 1024;
pub const MAX_HOUSEHOLD_TEARDOWN_JOURNALS: usize = 64;

const JOURNAL_SCHEMA_VERSION: u16 = 1;
const TEARDOWN_DIRECTORY: &str = "household-teardown";
const TEARDOWN_PREFIX: &str = "teardown-";
const TEARDOWN_SUFFIX: &str = ".htj";
const ACCOUNT_DIGEST_DOMAIN: &[u8] = b"heyfood.household.account-digest.v1";
const ACCOUNT_LOCATOR_DOMAIN: &[u8] = b"heyfood.household.account-locator.v1";
const JOURNAL_LIMITS: CompatibilityJsonLimitsV1 = CompatibilityJsonLimitsV1 {
    maximum_bytes: MAX_HOUSEHOLD_TEARDOWN_JOURNAL_BYTES,
    maximum_depth: 5,
    maximum_object_keys: 32,
    maximum_array_entries: 4,
    maximum_nodes: 128,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HouseholdTeardownGuardStateV1 {
    Migrated,
    InitializedNoSource,
    BlockedRepair,
    BlockedAfterLogout,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HouseholdTeardownLegacyTargetKindV1 {
    CurrentConfigFile,
    LegacyConfigFile,
    CurrentConfigKeyring,
    LegacyConfigKeyring,
}

impl HouseholdTeardownLegacyTargetKindV1 {
    const ALL: [Self; 4] = [
        Self::CurrentConfigFile,
        Self::LegacyConfigFile,
        Self::CurrentConfigKeyring,
        Self::LegacyConfigKeyring,
    ];
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HouseholdTeardownLegacyTargetOutcomeV1 {
    Pending,
    CredentialsPresent,
    AuthoritativeMissing,
    ForeignAccount,
    CurrentAccountScrubbed,
    Unavailable,
    Unbound,
    Ambiguous,
    Malformed,
    Changed,
}

impl HouseholdTeardownLegacyTargetOutcomeV1 {
    #[must_use]
    pub const fn is_complete(self) -> bool {
        matches!(
            self,
            Self::AuthoritativeMissing | Self::ForeignAccount | Self::CurrentAccountScrubbed
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HouseholdTeardownLegacyTargetV1 {
    pub kind: HouseholdTeardownLegacyTargetKindV1,
    pub locator_digest: CanonicalDigestV1,
    pub expected_noncredential_digest: CanonicalDigestV1,
    pub outcome: HouseholdTeardownLegacyTargetOutcomeV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HouseholdTeardownKeyAbsenceBasisV1 {
    BlockedRepairNoCommittedVaultV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HouseholdTeardownPhaseV1 {
    Prepared,
    GuardBlocked,
    CredentialsScrubbed,
    KeyAbsent,
    ArtifactsAbsent,
    AuthAbsent,
}

/// Strict, canonical, content-free restart authority.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HouseholdTeardownJournalV1 {
    schema_version: u16,
    pub native_root_instance_digest: CanonicalDigestV1,
    pub account_digest: CanonicalDigestV1,
    pub account_locator_digest: CanonicalDigestV1,
    pub expected_guard_state: HouseholdTeardownGuardStateV1,
    pub expected_guard_revision: u64,
    pub blocked_after_logout_guard_revision: Option<u64>,
    pub source_identity: HouseholdMigrationSourceIdentityV1,
    pub migration_id: Uuid,
    pub initialization_id: Uuid,
    pub initial_commit_id: Uuid,
    pub expected_household_key_id: Option<Uuid>,
    pub expected_key_bundle_revision: Option<u64>,
    pub key_absence_basis: Option<HouseholdTeardownKeyAbsenceBasisV1>,
    pub plaintext_snapshot_digest: Option<CanonicalDigestV1>,
    pub legacy_cleanup_targets: Vec<HouseholdTeardownLegacyTargetV1>,
    pub teardown_phase: HouseholdTeardownPhaseV1,
}

impl HouseholdTeardownJournalV1 {
    pub fn new(prepared: PreparedHouseholdTeardownV1) -> Result<Self, PortError> {
        let journal = Self {
            schema_version: JOURNAL_SCHEMA_VERSION,
            native_root_instance_digest: prepared.native_root_instance_digest,
            account_digest: prepared.account_digest,
            account_locator_digest: prepared.account_locator_digest,
            expected_guard_state: prepared.expected_guard_state,
            expected_guard_revision: prepared.expected_guard_revision,
            blocked_after_logout_guard_revision: None,
            source_identity: prepared.source_identity,
            migration_id: prepared.migration_id,
            initialization_id: prepared.initialization_id,
            initial_commit_id: prepared.initial_commit_id,
            expected_household_key_id: prepared.expected_household_key_id,
            expected_key_bundle_revision: prepared.expected_key_bundle_revision,
            key_absence_basis: prepared.key_absence_basis,
            plaintext_snapshot_digest: prepared.plaintext_snapshot_digest,
            legacy_cleanup_targets: prepared.legacy_cleanup_targets,
            teardown_phase: HouseholdTeardownPhaseV1::Prepared,
        };
        journal.validate()?;
        Ok(journal)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, PortError> {
        if bytes.is_empty() || bytes.len() > MAX_HOUSEHOLD_TEARDOWN_JOURNAL_BYTES {
            return Err(journal_invalid());
        }
        let value =
            parse_bounded_typed_json_v1(bytes, JOURNAL_LIMITS).map_err(|_| journal_invalid())?;
        let journal: Self = serde_json::from_value(value).map_err(|_| journal_invalid())?;
        journal.validate()?;
        if journal.canonical_bytes()?.as_slice() != bytes {
            return Err(journal_invalid());
        }
        Ok(journal)
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, PortError> {
        self.validate()?;
        let bytes = to_canonical_bytes_v1(self).map_err(|_| journal_invalid())?;
        if bytes.is_empty() || bytes.len() > MAX_HOUSEHOLD_TEARDOWN_JOURNAL_BYTES {
            return Err(journal_invalid());
        }
        Ok(bytes)
    }

    pub fn validate(&self) -> Result<(), PortError> {
        if self.schema_version != JOURNAL_SCHEMA_VERSION
            || self.expected_guard_revision == 0
            || !canonical_v4(self.migration_id)
            || !canonical_v4(self.initialization_id)
            || !canonical_v4(self.initial_commit_id)
            || self
                .expected_household_key_id
                .is_some_and(|value| !canonical_v4(value))
            || self.expected_key_bundle_revision == Some(0)
        {
            return Err(journal_invalid());
        }
        let expected_account_locator = domain_hash(
            ACCOUNT_LOCATOR_DOMAIN,
            &[
                self.native_root_instance_digest.as_bytes(),
                self.account_digest.as_bytes(),
            ],
        )?;
        if self.account_locator_digest.as_bytes() != &expected_account_locator {
            return Err(journal_invalid());
        }
        let has_key = self.expected_household_key_id.is_some();
        if has_key != self.expected_key_bundle_revision.is_some()
            || (has_key && self.key_absence_basis.is_some())
            || (!has_key
                && self.key_absence_basis
                    != Some(HouseholdTeardownKeyAbsenceBasisV1::BlockedRepairNoCommittedVaultV1))
            || (!has_key
                && self.expected_guard_state != HouseholdTeardownGuardStateV1::BlockedRepair)
        {
            return Err(journal_invalid());
        }
        if self.legacy_cleanup_targets.len() != 4 {
            return Err(journal_invalid());
        }
        let kinds = self
            .legacy_cleanup_targets
            .iter()
            .map(|target| target.kind)
            .collect::<BTreeSet<_>>();
        if kinds.len() != 4
            || HouseholdTeardownLegacyTargetKindV1::ALL
                .iter()
                .any(|kind| !kinds.contains(kind))
        {
            return Err(journal_invalid());
        }
        if self.teardown_phase == HouseholdTeardownPhaseV1::Prepared {
            if self.blocked_after_logout_guard_revision.is_some() {
                return Err(journal_invalid());
            }
        } else if self.blocked_after_logout_guard_revision
            != self.expected_guard_revision.checked_add(1)
        {
            return Err(journal_invalid());
        }
        Ok(())
    }

    #[cfg(test)]
    fn account_hex(&self) -> String {
        self.account_digest.to_lower_hex()
    }

    fn legacy_cleanup_complete(&self) -> bool {
        self.legacy_cleanup_targets
            .iter()
            .all(|target| target.outcome.is_complete())
    }
}

impl fmt::Debug for HouseholdTeardownJournalV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HouseholdTeardownJournalV1")
            .field("schema_version", &self.schema_version)
            .field("expected_guard_state", &self.expected_guard_state)
            .field("expected_guard_revision", &self.expected_guard_revision)
            .field(
                "blocked_after_logout_guard_revision",
                &self.blocked_after_logout_guard_revision,
            )
            .field("key_absence_basis", &self.key_absence_basis)
            .field("teardown_phase", &self.teardown_phase)
            .field("legacy_target_count", &self.legacy_cleanup_targets.len())
            .finish_non_exhaustive()
    }
}

/// Exact evidence captured under lifecycle, legacy-source, and vault locks
/// before the globally discoverable journal is committed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedHouseholdTeardownV1 {
    pub native_root_instance_digest: CanonicalDigestV1,
    pub account_digest: CanonicalDigestV1,
    pub account_locator_digest: CanonicalDigestV1,
    pub expected_guard_state: HouseholdTeardownGuardStateV1,
    pub expected_guard_revision: u64,
    pub source_identity: HouseholdMigrationSourceIdentityV1,
    pub migration_id: Uuid,
    pub initialization_id: Uuid,
    pub initial_commit_id: Uuid,
    pub expected_household_key_id: Option<Uuid>,
    pub expected_key_bundle_revision: Option<u64>,
    pub key_absence_basis: Option<HouseholdTeardownKeyAbsenceBasisV1>,
    pub plaintext_snapshot_digest: Option<CanonicalDigestV1>,
    pub legacy_cleanup_targets: Vec<HouseholdTeardownLegacyTargetV1>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HouseholdTeardownAttemptV1<T> {
    Verified(T),
    Incomplete,
}

/// Platform-specific retained-lock driver.
///
/// One `Lease` retains `account-lifecycle.lock` for its entire lifetime.
/// Implementations acquire legacy config/import locks and `vault.lock` in
/// their fixed order before returning it. `release_native_state_locks`
/// releases only those narrower locks; lifecycle remains held through auth
/// cleanup and journal removal.
pub trait NativeAccountTeardownBackendV1: Send + Sync {
    type Lease: Send;

    fn acquire_authenticated<'a>(
        &'a self,
        expected: &'a AuthCredentialBundle,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<(Self::Lease, PreparedHouseholdTeardownV1), PortError>>;

    fn acquire_resume<'a>(
        &'a self,
        journal: &'a HouseholdTeardownJournalV1,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<Self::Lease, PortError>>;

    fn ensure_guard_blocked<'a>(
        &'a self,
        lease: &'a mut Self::Lease,
        journal: &'a HouseholdTeardownJournalV1,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdTeardownAttemptV1<u64>, PortError>>;

    fn scrub_legacy_credentials<'a>(
        &'a self,
        lease: &'a mut Self::Lease,
        journal: &'a HouseholdTeardownJournalV1,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<Vec<HouseholdTeardownLegacyTargetV1>, PortError>>;

    fn ensure_key_absent<'a>(
        &'a self,
        lease: &'a mut Self::Lease,
        journal: &'a HouseholdTeardownJournalV1,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdTeardownAttemptV1<()>, PortError>>;

    fn ensure_artifacts_absent<'a>(
        &'a self,
        lease: &'a mut Self::Lease,
        journal: &'a HouseholdTeardownJournalV1,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdTeardownAttemptV1<()>, PortError>>;

    fn release_native_state_locks<'a>(
        &'a self,
        lease: &'a mut Self::Lease,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdTeardownAttemptV1<()>, PortError>>;

    fn ensure_auth_absent<'a>(
        &'a self,
        lease: &'a mut Self::Lease,
        journal: &'a HouseholdTeardownJournalV1,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdTeardownAttemptV1<()>, PortError>>;
}

#[cfg(feature = "native-credentials")]
pub struct ProductionNativeAccountTeardownBackendV1<S> {
    paths: NativePaths,
    broker: HouseholdKeyBroker,
    migration: LegacyPythonHouseholdMigrationV1,
    auth: NativeAuthStore,
    session: S,
}

#[cfg(feature = "native-credentials")]
#[derive(Clone, Copy)]
enum LegacyCredentialLockAuthorityV1<'a> {
    Native(&'a HouseholdLifecycleLease),
    PreNative(&'a LegacyPythonCredentialSourceLeaseV1),
}

#[cfg(feature = "native-credentials")]
#[doc(hidden)]
pub struct ProductionNativeAccountTeardownLeaseV1 {
    target: HouseholdTeardownVaultTargetV1,
    source_lease: Option<LegacyPythonSourceLeaseV1>,
    vault_lease: Option<HouseholdVaultLease>,
    lifecycle_lease: Option<HouseholdLifecycleLease>,
}

#[cfg(feature = "native-credentials")]
impl<S> ProductionNativeAccountTeardownBackendV1<S>
where
    S: AuthorizationSessionStore + Send + Sync,
{
    pub fn open(
        paths: NativePaths,
        session: S,
        broker_deadline: std::time::Duration,
    ) -> Result<Self, PortError> {
        let broker = HouseholdKeyBroker::from_native_paths(&paths, broker_deadline)?;
        let migration = LegacyPythonHouseholdMigrationV1::discover(&paths)?;
        let auth = NativeAuthStore::open(paths.config_dir())?;
        Ok(Self {
            paths,
            broker,
            migration,
            auth,
            session,
        })
    }

    /// Credential-complete logout for the exact released flag-off account
    /// shape that predates all D2 provenance.
    ///
    /// No teardown journal can be minted without a guard/key identity. The
    /// account-bound native authorization therefore remains the restart
    /// authority until both frozen Python config files and both derived
    /// historical keyring entries are proven complete under their exact
    /// config locks. A partial result leaves that authorization untouched, so
    /// the next logout retries the same account idempotently.
    pub async fn clear_pre_native_account(
        &self,
        expected: &AuthCredentialBundle,
        cancellation: CancellationToken,
    ) -> Result<HouseholdEraseOutcome, PortError> {
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }
        let vault =
            HouseholdVault::from_native_paths(&self.paths, expected.session.account_id.clone())?;
        let source_lease = self
            .migration
            .acquire_credential_source_lease(
                vault.account_slot().clone(),
                cancellation.child_token(),
            )
            .await?;
        let observed = self.auth.load_account_bound(&self.session)?;
        if observed.as_ref() != Some(expected) {
            return Err(PortError::new(
                "household_teardown_account_changed",
                "active account authorization changed before legacy credential cleanup",
            ));
        }
        let account_digest = CanonicalDigestV1::from_bytes(vault.account_slot().account_digest());
        let mut records = Vec::with_capacity(4);
        for (kind, config_kind) in [
            (
                HouseholdTeardownLegacyTargetKindV1::CurrentConfigFile,
                LegacyPythonConfigKindV1::Current,
            ),
            (
                HouseholdTeardownLegacyTargetKindV1::LegacyConfigFile,
                LegacyPythonConfigKindV1::Legacy,
            ),
        ] {
            records.push(inspect_legacy_file_target(
                kind,
                self.migration.config_path(config_kind),
                account_digest,
            )?);
        }
        for (kind, config_kind, file_kind) in [
            (
                HouseholdTeardownLegacyTargetKindV1::CurrentConfigKeyring,
                LegacyPythonConfigKindV1::Current,
                HouseholdTeardownLegacyTargetKindV1::CurrentConfigFile,
            ),
            (
                HouseholdTeardownLegacyTargetKindV1::LegacyConfigKeyring,
                LegacyPythonConfigKindV1::Legacy,
                HouseholdTeardownLegacyTargetKindV1::LegacyConfigFile,
            ),
        ] {
            let file_outcome = records
                .iter()
                .find(|record| record.kind == file_kind)
                .ok_or_else(journal_invalid)?
                .outcome;
            records.push(
                self.inspect_legacy_keyring_target_authorized(
                    LegacyCredentialLockAuthorityV1::PreNative(&source_lease),
                    kind,
                    config_kind,
                    file_outcome,
                    cancellation.child_token(),
                )
                .await?,
            );
        }
        records.sort_by_key(|record| record.kind);

        let mut replacements = Vec::with_capacity(4);
        for (kind, config_kind) in [
            (
                HouseholdTeardownLegacyTargetKindV1::CurrentConfigFile,
                LegacyPythonConfigKindV1::Current,
            ),
            (
                HouseholdTeardownLegacyTargetKindV1::LegacyConfigFile,
                LegacyPythonConfigKindV1::Legacy,
            ),
        ] {
            let expected_target = records
                .iter()
                .find(|record| record.kind == kind)
                .ok_or_else(journal_invalid)?;
            replacements.push(
                scrub_legacy_file_target(
                    expected_target,
                    self.migration.config_path(config_kind),
                    account_digest,
                )
                .unwrap_or_else(|_| {
                    let mut replacement = expected_target.clone();
                    replacement.outcome = HouseholdTeardownLegacyTargetOutcomeV1::Changed;
                    replacement
                }),
            );
        }
        for (kind, config_kind, file_kind) in [
            (
                HouseholdTeardownLegacyTargetKindV1::CurrentConfigKeyring,
                LegacyPythonConfigKindV1::Current,
                HouseholdTeardownLegacyTargetKindV1::CurrentConfigFile,
            ),
            (
                HouseholdTeardownLegacyTargetKindV1::LegacyConfigKeyring,
                LegacyPythonConfigKindV1::Legacy,
                HouseholdTeardownLegacyTargetKindV1::LegacyConfigFile,
            ),
        ] {
            let expected_target = records
                .iter()
                .find(|record| record.kind == kind)
                .ok_or_else(journal_invalid)?;
            let file_outcome = replacements
                .iter()
                .find(|record| record.kind == file_kind)
                .ok_or_else(journal_invalid)?
                .outcome;
            replacements.push(
                match self
                    .scrub_legacy_keyring_target_authorized(
                        LegacyCredentialLockAuthorityV1::PreNative(&source_lease),
                        expected_target,
                        config_kind,
                        file_outcome,
                        cancellation.child_token(),
                    )
                    .await
                {
                    Ok(replacement) => replacement,
                    Err(_) => {
                        let mut replacement = expected_target.clone();
                        replacement.outcome = HouseholdTeardownLegacyTargetOutcomeV1::Changed;
                        replacement
                    }
                },
            );
        }
        replacements.sort_by_key(|record| record.kind);
        validate_target_replacements(&records, &replacements)?;
        self.migration
            .validate_credential_source_lease(&source_lease)?;
        let legacy_complete = replacements
            .iter()
            .all(|target| target.outcome.is_complete());
        if !legacy_complete || cancellation.is_cancelled() {
            return Ok(pre_native_outcome(legacy_complete, false));
        }

        // Config locks are intentionally narrower than native auth/session
        // locks. Releasing them here preserves the fixed lock order; exact
        // account comparison inside `clear_account_bound` prevents a newer
        // login from being erased.
        drop(source_lease);
        match self.auth.clear_account_bound(expected, &self.session) {
            Ok(()) => Ok(pre_native_outcome(true, true)),
            Err(_) => Ok(pre_native_outcome(true, false)),
        }
    }

    async fn acquire_for_slot(
        &self,
        account_slot: HouseholdAccountSlotV1,
        cancellation: CancellationToken,
    ) -> Result<ProductionNativeAccountTeardownLeaseV1, PortError> {
        let target =
            HouseholdTeardownVaultTargetV1::open(self.paths.data_dir(), account_slot.clone())?;
        let lifecycle = target
            .acquire_lifecycle_lease(cancellation.child_token())
            .await?;
        let mut source_lease = self
            .migration
            .acquire_source_lease(lifecycle, cancellation.child_token())
            .await?;
        let lifecycle = self.migration.take_lifecycle_for_vault(&mut source_lease)?;
        let vault_lease = target
            .acquire_vault_lease(lifecycle, cancellation.child_token())
            .await?;
        Ok(ProductionNativeAccountTeardownLeaseV1 {
            target,
            source_lease: Some(source_lease),
            vault_lease: Some(vault_lease),
            lifecycle_lease: None,
        })
    }

    async fn prepare_evidence(
        &self,
        expected: &AuthCredentialBundle,
        vault: &HouseholdVault,
        lease: &mut ProductionNativeAccountTeardownLeaseV1,
        cancellation: CancellationToken,
    ) -> Result<PreparedHouseholdTeardownV1, PortError> {
        let vault_lease = required_vault_lease(lease)?;
        let guard = HouseholdMigrationGuardStore::load(
            &self.broker,
            vault_lease.lifecycle_lease(),
            cancellation.child_token(),
        )
        .await?
        .ok_or_else(|| {
            PortError::new(
                "household_teardown_guard_missing",
                "native household teardown requires a migration guard",
            )
        })?;
        guard.validate_for(vault.account_slot())?;
        let key = HouseholdKeyStore::load(
            &self.broker,
            vault_lease.lifecycle_lease(),
            cancellation.child_token(),
        )
        .await?;
        let artifact_count = vault
            .startup_artifact_count(vault_lease, cancellation.child_token())
            .await?;

        let (expected_household_key_id, expected_key_bundle_revision, key_absence_basis) =
            match guard.state() {
                HouseholdMigrationGuardStateV1::Migrated
                | HouseholdMigrationGuardStateV1::InitializedNoSource => {
                    let key = key.as_ref().ok_or_else(|| {
                        PortError::new(
                            "household_teardown_key_missing",
                            "committed native household key is missing",
                        )
                    })?;
                    if !matches!(
                        key.phase,
                        HouseholdKeyBundlePhase::Stable | HouseholdKeyBundlePhase::Rewriting
                    ) {
                        return Err(PortError::new(
                            "household_teardown_key_state",
                            "committed native household key is not deletable",
                        ));
                    }
                    let loaded = vault
                        .load(vault_lease, key.clone(), cancellation.child_token())
                        .await?;
                    verify_guard_and_loaded_state(vault, &guard, &loaded)?;
                    (
                        Some(key.active_key_id.as_uuid()),
                        Some(key.revision.get()),
                        None,
                    )
                }
                HouseholdMigrationGuardStateV1::BlockedRepair => {
                    if key.is_some() || artifact_count != 0 {
                        return Err(PortError::new(
                            "household_teardown_repair_absence",
                            "repair-blocked household does not satisfy the no-key teardown invariant",
                        ));
                    }
                    (
                        None,
                        None,
                        Some(HouseholdTeardownKeyAbsenceBasisV1::BlockedRepairNoCommittedVaultV1),
                    )
                }
                HouseholdMigrationGuardStateV1::BlockedAfterLogout => {
                    return Err(PortError::new(
                        "household_teardown_already_blocked",
                        "native household is already blocked after logout",
                    ));
                }
                HouseholdMigrationGuardStateV1::Initializing
                | HouseholdMigrationGuardStateV1::Aborting => {
                    return Err(PortError::new(
                        "household_teardown_initialization_incomplete",
                        "native household initialization must be reconciled before logout",
                    ));
                }
            };
        let account_digest = CanonicalDigestV1::from_bytes(vault.account_slot().account_digest());
        let legacy_cleanup_targets = self
            .inspect_all_legacy_targets(
                vault_lease.lifecycle_lease(),
                account_digest,
                cancellation.child_token(),
            )
            .await?;
        let plaintext_snapshot_digest = guard
            .legacy_python_snapshot()
            .map(|evidence| evidence.content_digest);
        {
            let source_lease = lease.source_lease.as_ref().ok_or_else(|| {
                PortError::new(
                    "household_teardown_lock_state",
                    "legacy source locks are not retained during teardown preparation",
                )
            })?;
            let vault_lease = lease.vault_lease.as_ref().ok_or_else(|| {
                PortError::new(
                    "household_teardown_lock_state",
                    "vault lock is not retained during teardown preparation",
                )
            })?;
            self.migration
                .validate_source_lease_for_vault(source_lease, vault_lease)?;
        }
        let target = lease.target.clone();
        let snapshot_path = self.migration.snapshot_path().to_owned();
        target
            .verify_snapshot_evidence(
                required_vault_lease(lease)?,
                &snapshot_path,
                plaintext_snapshot_digest.map(|digest| *digest.as_bytes()),
                cancellation.child_token(),
            )
            .await?;
        if expected.session.account_id != *vault.account_id() {
            return Err(PortError::new(
                "household_teardown_account_mismatch",
                "authenticated account changed during teardown preparation",
            ));
        }
        Ok(PreparedHouseholdTeardownV1 {
            native_root_instance_digest: CanonicalDigestV1::from_bytes(
                vault.account_slot().native_root_instance_digest(),
            ),
            account_digest,
            account_locator_digest: CanonicalDigestV1::from_bytes(
                vault.account_slot().account_locator_digest(),
            ),
            expected_guard_state: teardown_guard_state(guard.state())?,
            expected_guard_revision: guard.guard_revision(),
            source_identity: guard.source_identity().clone(),
            migration_id: guard.migration_id(),
            initialization_id: guard.initialization_id(),
            initial_commit_id: guard.initial_commit_id(),
            expected_household_key_id,
            expected_key_bundle_revision,
            key_absence_basis,
            plaintext_snapshot_digest,
            legacy_cleanup_targets,
        })
    }

    async fn inspect_all_legacy_targets(
        &self,
        lifecycle: &HouseholdLifecycleLease,
        account_digest: CanonicalDigestV1,
        cancellation: CancellationToken,
    ) -> Result<Vec<HouseholdTeardownLegacyTargetV1>, PortError> {
        let mut records = Vec::with_capacity(4);
        for (kind, config_kind) in [
            (
                HouseholdTeardownLegacyTargetKindV1::CurrentConfigFile,
                LegacyPythonConfigKindV1::Current,
            ),
            (
                HouseholdTeardownLegacyTargetKindV1::LegacyConfigFile,
                LegacyPythonConfigKindV1::Legacy,
            ),
        ] {
            records.push(inspect_legacy_file_target(
                kind,
                self.migration.config_path(config_kind),
                account_digest,
            )?);
        }
        for (kind, config_kind, file_kind) in [
            (
                HouseholdTeardownLegacyTargetKindV1::CurrentConfigKeyring,
                LegacyPythonConfigKindV1::Current,
                HouseholdTeardownLegacyTargetKindV1::CurrentConfigFile,
            ),
            (
                HouseholdTeardownLegacyTargetKindV1::LegacyConfigKeyring,
                LegacyPythonConfigKindV1::Legacy,
                HouseholdTeardownLegacyTargetKindV1::LegacyConfigFile,
            ),
        ] {
            let file = records
                .iter()
                .find(|record| record.kind == file_kind)
                .ok_or_else(journal_invalid)?;
            records.push(
                self.inspect_legacy_keyring_target(
                    lifecycle,
                    kind,
                    config_kind,
                    file.outcome,
                    cancellation.child_token(),
                )
                .await?,
            );
        }
        records.sort_by_key(|record| record.kind);
        Ok(records)
    }

    async fn inspect_legacy_keyring_target(
        &self,
        lifecycle: &HouseholdLifecycleLease,
        kind: HouseholdTeardownLegacyTargetKindV1,
        config_kind: LegacyPythonConfigKindV1,
        config_outcome: HouseholdTeardownLegacyTargetOutcomeV1,
        cancellation: CancellationToken,
    ) -> Result<HouseholdTeardownLegacyTargetV1, PortError> {
        self.inspect_legacy_keyring_target_authorized(
            LegacyCredentialLockAuthorityV1::Native(lifecycle),
            kind,
            config_kind,
            config_outcome,
            cancellation,
        )
        .await
    }

    async fn inspect_legacy_keyring_target_authorized(
        &self,
        authority: LegacyCredentialLockAuthorityV1<'_>,
        kind: HouseholdTeardownLegacyTargetKindV1,
        config_kind: LegacyPythonConfigKindV1,
        config_outcome: HouseholdTeardownLegacyTargetOutcomeV1,
        cancellation: CancellationToken,
    ) -> Result<HouseholdTeardownLegacyTargetV1, PortError> {
        let path = self.migration.config_path(config_kind);
        let locator_digest = legacy_keyring_locator_digest(path)?;
        let probe = self
            .legacy_python_credentials_probe_authorized(authority, config_kind, path, cancellation)
            .await
            .unwrap_or(LegacyPythonCredentialProbeResultV1::Unavailable);
        let (expected_noncredential_digest, outcome) = match probe {
            LegacyPythonCredentialProbeResultV1::AuthoritativeMissing {
                noncredential_digest,
            } => (
                CanonicalDigestV1::from_bytes(noncredential_digest),
                HouseholdTeardownLegacyTargetOutcomeV1::AuthoritativeMissing,
            ),
            LegacyPythonCredentialProbeResultV1::Present(authority) => {
                let outcome = match config_outcome {
                    HouseholdTeardownLegacyTargetOutcomeV1::CredentialsPresent
                    | HouseholdTeardownLegacyTargetOutcomeV1::CurrentAccountScrubbed => {
                        if authority.credentials_present() {
                            HouseholdTeardownLegacyTargetOutcomeV1::CredentialsPresent
                        } else {
                            HouseholdTeardownLegacyTargetOutcomeV1::CurrentAccountScrubbed
                        }
                    }
                    HouseholdTeardownLegacyTargetOutcomeV1::ForeignAccount => {
                        HouseholdTeardownLegacyTargetOutcomeV1::ForeignAccount
                    }
                    HouseholdTeardownLegacyTargetOutcomeV1::AuthoritativeMissing
                    | HouseholdTeardownLegacyTargetOutcomeV1::Unbound => {
                        HouseholdTeardownLegacyTargetOutcomeV1::Unbound
                    }
                    HouseholdTeardownLegacyTargetOutcomeV1::Ambiguous => {
                        HouseholdTeardownLegacyTargetOutcomeV1::Ambiguous
                    }
                    HouseholdTeardownLegacyTargetOutcomeV1::Malformed => {
                        HouseholdTeardownLegacyTargetOutcomeV1::Malformed
                    }
                    HouseholdTeardownLegacyTargetOutcomeV1::Unavailable
                    | HouseholdTeardownLegacyTargetOutcomeV1::Changed
                    | HouseholdTeardownLegacyTargetOutcomeV1::Pending => {
                        HouseholdTeardownLegacyTargetOutcomeV1::Unavailable
                    }
                };
                (
                    CanonicalDigestV1::from_bytes(authority.noncredential_digest()),
                    outcome,
                )
            }
            LegacyPythonCredentialProbeResultV1::Unavailable => (
                status_digest("keyring_unavailable")?,
                HouseholdTeardownLegacyTargetOutcomeV1::Unavailable,
            ),
            LegacyPythonCredentialProbeResultV1::Malformed => (
                status_digest("keyring_malformed")?,
                HouseholdTeardownLegacyTargetOutcomeV1::Malformed,
            ),
        };
        Ok(HouseholdTeardownLegacyTargetV1 {
            kind,
            locator_digest,
            expected_noncredential_digest,
            outcome,
        })
    }

    async fn legacy_python_credentials_probe_authorized(
        &self,
        authority: LegacyCredentialLockAuthorityV1<'_>,
        config_kind: LegacyPythonConfigKindV1,
        path: &Path,
        cancellation: CancellationToken,
    ) -> Result<LegacyPythonCredentialProbeResultV1, PortError> {
        match authority {
            LegacyCredentialLockAuthorityV1::Native(lifecycle) => {
                self.broker
                    .legacy_python_credentials_probe(lifecycle, config_kind, path, cancellation)
                    .await
            }
            LegacyCredentialLockAuthorityV1::PreNative(source_lease) => {
                self.broker
                    .legacy_python_credentials_probe_with_source_lease(
                        source_lease,
                        config_kind,
                        path,
                        cancellation,
                    )
                    .await
            }
        }
    }

    async fn scrub_all_legacy_targets(
        &self,
        lease: &mut ProductionNativeAccountTeardownLeaseV1,
        journal: &HouseholdTeardownJournalV1,
        cancellation: CancellationToken,
    ) -> Result<Vec<HouseholdTeardownLegacyTargetV1>, PortError> {
        let lifecycle = required_vault_lease(lease)?.lifecycle_lease();
        let mut replacements = Vec::with_capacity(4);
        for (kind, config_kind) in [
            (
                HouseholdTeardownLegacyTargetKindV1::CurrentConfigFile,
                LegacyPythonConfigKindV1::Current,
            ),
            (
                HouseholdTeardownLegacyTargetKindV1::LegacyConfigFile,
                LegacyPythonConfigKindV1::Legacy,
            ),
        ] {
            let expected = journal_target(journal, kind)?;
            replacements.push(
                scrub_legacy_file_target(
                    expected,
                    self.migration.config_path(config_kind),
                    journal.account_digest,
                )
                .unwrap_or_else(|_| {
                    let mut replacement = expected.clone();
                    replacement.outcome = HouseholdTeardownLegacyTargetOutcomeV1::Changed;
                    replacement
                }),
            );
        }
        for (kind, config_kind, file_kind) in [
            (
                HouseholdTeardownLegacyTargetKindV1::CurrentConfigKeyring,
                LegacyPythonConfigKindV1::Current,
                HouseholdTeardownLegacyTargetKindV1::CurrentConfigFile,
            ),
            (
                HouseholdTeardownLegacyTargetKindV1::LegacyConfigKeyring,
                LegacyPythonConfigKindV1::Legacy,
                HouseholdTeardownLegacyTargetKindV1::LegacyConfigFile,
            ),
        ] {
            let expected = journal_target(journal, kind)?;
            let file_outcome = replacements
                .iter()
                .find(|record| record.kind == file_kind)
                .ok_or_else(journal_invalid)?
                .outcome;
            replacements.push(
                match self
                    .scrub_legacy_keyring_target(
                        lifecycle,
                        expected,
                        config_kind,
                        file_outcome,
                        cancellation.child_token(),
                    )
                    .await
                {
                    Ok(replacement) => replacement,
                    Err(_) => {
                        let mut replacement = expected.clone();
                        replacement.outcome = HouseholdTeardownLegacyTargetOutcomeV1::Changed;
                        replacement
                    }
                },
            );
        }
        replacements.sort_by_key(|record| record.kind);
        Ok(replacements)
    }

    async fn scrub_legacy_keyring_target(
        &self,
        lifecycle: &HouseholdLifecycleLease,
        expected: &HouseholdTeardownLegacyTargetV1,
        config_kind: LegacyPythonConfigKindV1,
        config_outcome: HouseholdTeardownLegacyTargetOutcomeV1,
        cancellation: CancellationToken,
    ) -> Result<HouseholdTeardownLegacyTargetV1, PortError> {
        self.scrub_legacy_keyring_target_authorized(
            LegacyCredentialLockAuthorityV1::Native(lifecycle),
            expected,
            config_kind,
            config_outcome,
            cancellation,
        )
        .await
    }

    async fn scrub_legacy_keyring_target_authorized(
        &self,
        lock_authority: LegacyCredentialLockAuthorityV1<'_>,
        expected: &HouseholdTeardownLegacyTargetV1,
        config_kind: LegacyPythonConfigKindV1,
        config_outcome: HouseholdTeardownLegacyTargetOutcomeV1,
        cancellation: CancellationToken,
    ) -> Result<HouseholdTeardownLegacyTargetV1, PortError> {
        let path = self.migration.config_path(config_kind);
        if legacy_keyring_locator_digest(path)? != expected.locator_digest {
            return Err(PortError::new(
                "household_teardown_legacy_binding",
                "legacy keyring locator changed during teardown",
            ));
        }
        let probe = self
            .legacy_python_credentials_probe_authorized(
                lock_authority,
                config_kind,
                path,
                cancellation.child_token(),
            )
            .await
            .unwrap_or(LegacyPythonCredentialProbeResultV1::Unavailable);
        let outcome = match probe {
            LegacyPythonCredentialProbeResultV1::AuthoritativeMissing {
                noncredential_digest,
            } => {
                if CanonicalDigestV1::from_bytes(noncredential_digest)
                    == expected.expected_noncredential_digest
                {
                    HouseholdTeardownLegacyTargetOutcomeV1::AuthoritativeMissing
                } else {
                    HouseholdTeardownLegacyTargetOutcomeV1::Changed
                }
            }
            LegacyPythonCredentialProbeResultV1::Present(scrub_authority) => {
                if CanonicalDigestV1::from_bytes(scrub_authority.noncredential_digest())
                    != expected.expected_noncredential_digest
                {
                    HouseholdTeardownLegacyTargetOutcomeV1::Changed
                } else if !matches!(
                    config_outcome,
                    HouseholdTeardownLegacyTargetOutcomeV1::CredentialsPresent
                        | HouseholdTeardownLegacyTargetOutcomeV1::CurrentAccountScrubbed
                ) {
                    match config_outcome {
                        HouseholdTeardownLegacyTargetOutcomeV1::ForeignAccount => {
                            HouseholdTeardownLegacyTargetOutcomeV1::ForeignAccount
                        }
                        HouseholdTeardownLegacyTargetOutcomeV1::Ambiguous => {
                            HouseholdTeardownLegacyTargetOutcomeV1::Ambiguous
                        }
                        HouseholdTeardownLegacyTargetOutcomeV1::Malformed => {
                            HouseholdTeardownLegacyTargetOutcomeV1::Malformed
                        }
                        _ => HouseholdTeardownLegacyTargetOutcomeV1::Unbound,
                    }
                } else {
                    match self
                        .legacy_python_credentials_scrub_and_verify_authorized(
                            lock_authority,
                            scrub_authority.as_ref(),
                            cancellation,
                        )
                        .await
                        .unwrap_or(LegacyPythonCredentialScrubResultV1::Unavailable)
                    {
                        LegacyPythonCredentialScrubResultV1::VerifiedAbsent {
                            noncredential_digest,
                        }
                        | LegacyPythonCredentialScrubResultV1::VerifiedScrubbed {
                            noncredential_digest,
                        } if CanonicalDigestV1::from_bytes(noncredential_digest)
                            == expected.expected_noncredential_digest =>
                        {
                            HouseholdTeardownLegacyTargetOutcomeV1::CurrentAccountScrubbed
                        }
                        LegacyPythonCredentialScrubResultV1::Changed => {
                            HouseholdTeardownLegacyTargetOutcomeV1::Changed
                        }
                        LegacyPythonCredentialScrubResultV1::Unavailable => {
                            HouseholdTeardownLegacyTargetOutcomeV1::Unavailable
                        }
                        LegacyPythonCredentialScrubResultV1::Malformed => {
                            HouseholdTeardownLegacyTargetOutcomeV1::Malformed
                        }
                        _ => HouseholdTeardownLegacyTargetOutcomeV1::Changed,
                    }
                }
            }
            LegacyPythonCredentialProbeResultV1::Unavailable => {
                HouseholdTeardownLegacyTargetOutcomeV1::Unavailable
            }
            LegacyPythonCredentialProbeResultV1::Malformed => {
                HouseholdTeardownLegacyTargetOutcomeV1::Malformed
            }
        };
        let mut replacement = expected.clone();
        replacement.outcome = outcome;
        Ok(replacement)
    }

    async fn legacy_python_credentials_scrub_and_verify_authorized(
        &self,
        authority: LegacyCredentialLockAuthorityV1<'_>,
        scrub_authority: &crate::LegacyPythonCredentialScrubAuthorityV1,
        cancellation: CancellationToken,
    ) -> Result<LegacyPythonCredentialScrubResultV1, PortError> {
        match authority {
            LegacyCredentialLockAuthorityV1::Native(lifecycle) => {
                self.broker
                    .legacy_python_credentials_scrub_and_verify(
                        lifecycle,
                        scrub_authority,
                        cancellation,
                    )
                    .await
            }
            LegacyCredentialLockAuthorityV1::PreNative(source_lease) => {
                self.broker
                    .legacy_python_credentials_scrub_and_verify_with_source_lease(
                        source_lease,
                        scrub_authority,
                        cancellation,
                    )
                    .await
            }
        }
    }
}

#[cfg(feature = "native-credentials")]
impl<S> NativeAccountTeardownBackendV1 for ProductionNativeAccountTeardownBackendV1<S>
where
    S: AuthorizationSessionStore + Send + Sync,
{
    type Lease = ProductionNativeAccountTeardownLeaseV1;

    fn acquire_authenticated<'a>(
        &'a self,
        expected: &'a AuthCredentialBundle,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<(Self::Lease, PreparedHouseholdTeardownV1), PortError>> {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(cancelled());
            }
            let vault = HouseholdVault::from_native_paths(
                &self.paths,
                expected.session.account_id.clone(),
            )?;
            let mut lease = self
                .acquire_for_slot(vault.account_slot().clone(), cancellation.child_token())
                .await?;
            let observed = self.auth.load_account_bound(&self.session)?;
            if observed.as_ref() != Some(expected) {
                return Err(PortError::new(
                    "household_teardown_account_changed",
                    "active account authorization changed before native teardown preparation",
                ));
            }
            let prepared = self
                .prepare_evidence(expected, &vault, &mut lease, cancellation.child_token())
                .await?;
            Ok((lease, prepared))
        })
    }

    fn acquire_resume<'a>(
        &'a self,
        journal: &'a HouseholdTeardownJournalV1,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<Self::Lease, PortError>> {
        Box::pin(async move {
            journal.validate()?;
            let slot = HouseholdAccountSlotV1::from_components(
                *journal.account_digest.as_bytes(),
                *journal.native_root_instance_digest.as_bytes(),
                *journal.account_locator_digest.as_bytes(),
                journal.account_digest.to_lower_hex(),
            )?;
            self.acquire_for_slot(slot, cancellation).await
        })
    }

    fn ensure_guard_blocked<'a>(
        &'a self,
        lease: &'a mut Self::Lease,
        journal: &'a HouseholdTeardownJournalV1,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdTeardownAttemptV1<u64>, PortError>> {
        Box::pin(async move {
            let target = lease.target.clone();
            let vault_lease = required_vault_lease(lease)?;
            let observed = match HouseholdMigrationGuardStore::load(
                &self.broker,
                vault_lease.lifecycle_lease(),
                cancellation.child_token(),
            )
            .await
            {
                Ok(Some(observed)) => observed,
                Ok(None) | Err(_) => return Ok(HouseholdTeardownAttemptV1::Incomplete),
            };
            if guard_is_committed_logout(&observed, journal).unwrap_or(false) {
                return Ok(HouseholdTeardownAttemptV1::Verified(
                    observed.guard_revision(),
                ));
            }
            if require_original_guard(&observed, journal).is_err() {
                return Ok(HouseholdTeardownAttemptV1::Incomplete);
            }
            if journal.key_absence_basis.is_some() {
                let key = match HouseholdKeyStore::load(
                    &self.broker,
                    vault_lease.lifecycle_lease(),
                    cancellation.child_token(),
                )
                .await
                {
                    Ok(key) => key,
                    Err(_) => return Ok(HouseholdTeardownAttemptV1::Incomplete),
                };
                let count = match target
                    .startup_artifact_count_for_teardown(vault_lease, cancellation.child_token())
                    .await
                {
                    Ok(count) => count,
                    Err(_) => return Ok(HouseholdTeardownAttemptV1::Incomplete),
                };
                if key.is_some() || count != 0 {
                    return Ok(HouseholdTeardownAttemptV1::Incomplete);
                }
            }
            let replacement = match observed.blocked_after_logout() {
                Ok(replacement) => replacement,
                Err(_) => return Ok(HouseholdTeardownAttemptV1::Incomplete),
            };
            let _exchange = HouseholdMigrationGuardStore::compare_exchange(
                &self.broker,
                vault_lease,
                MigrationGuardExpectation::Revision(observed.guard_revision()),
                Some(replacement),
                cancellation.child_token(),
            )
            .await;
            let reloaded = match HouseholdMigrationGuardStore::load(
                &self.broker,
                vault_lease.lifecycle_lease(),
                cancellation.child_token(),
            )
            .await
            {
                Ok(reloaded) => reloaded,
                Err(_) => return Ok(HouseholdTeardownAttemptV1::Incomplete),
            };
            match reloaded {
                Some(reloaded)
                    if guard_is_committed_logout(&reloaded, journal).unwrap_or(false) =>
                {
                    Ok(HouseholdTeardownAttemptV1::Verified(
                        reloaded.guard_revision(),
                    ))
                }
                _ => Ok(HouseholdTeardownAttemptV1::Incomplete),
            }
        })
    }

    fn scrub_legacy_credentials<'a>(
        &'a self,
        lease: &'a mut Self::Lease,
        journal: &'a HouseholdTeardownJournalV1,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<Vec<HouseholdTeardownLegacyTargetV1>, PortError>> {
        Box::pin(async move {
            self.scrub_all_legacy_targets(lease, journal, cancellation)
                .await
        })
    }

    fn ensure_key_absent<'a>(
        &'a self,
        lease: &'a mut Self::Lease,
        journal: &'a HouseholdTeardownJournalV1,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdTeardownAttemptV1<()>, PortError>> {
        Box::pin(async move {
            let vault_lease = required_vault_lease(lease)?;
            let guard = match HouseholdMigrationGuardStore::load(
                &self.broker,
                vault_lease.lifecycle_lease(),
                cancellation.child_token(),
            )
            .await
            {
                Ok(Some(guard)) => guard,
                Ok(None) | Err(_) => return Ok(HouseholdTeardownAttemptV1::Incomplete),
            };
            if !guard_is_committed_logout(&guard, journal).unwrap_or(false) {
                return Ok(HouseholdTeardownAttemptV1::Incomplete);
            }
            let observed = match HouseholdKeyStore::load(
                &self.broker,
                vault_lease.lifecycle_lease(),
                cancellation.child_token(),
            )
            .await
            {
                Ok(observed) => observed,
                Err(_) => return Ok(HouseholdTeardownAttemptV1::Incomplete),
            };
            if let Some(observed) = observed {
                let (Some(expected_key_id), Some(expected_revision)) = (
                    journal.expected_household_key_id,
                    journal.expected_key_bundle_revision,
                ) else {
                    return Ok(HouseholdTeardownAttemptV1::Incomplete);
                };
                if observed.active_key_id.as_uuid() != expected_key_id
                    || observed.revision.get() != expected_revision
                {
                    return Ok(HouseholdTeardownAttemptV1::Incomplete);
                }
                let _deletion = HouseholdKeyStore::delete_and_verify(
                    &self.broker,
                    vault_lease,
                    observed.revision,
                    observed.active_key_id,
                    cancellation.child_token(),
                )
                .await;
                let reloaded = match HouseholdKeyStore::load(
                    &self.broker,
                    vault_lease.lifecycle_lease(),
                    cancellation.child_token(),
                )
                .await
                {
                    Ok(reloaded) => reloaded,
                    Err(_) => return Ok(HouseholdTeardownAttemptV1::Incomplete),
                };
                if reloaded.is_none() {
                    return Ok(HouseholdTeardownAttemptV1::Verified(()));
                }
                return Ok(HouseholdTeardownAttemptV1::Incomplete);
            }
            Ok(HouseholdTeardownAttemptV1::Verified(()))
        })
    }

    fn ensure_artifacts_absent<'a>(
        &'a self,
        lease: &'a mut Self::Lease,
        journal: &'a HouseholdTeardownJournalV1,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdTeardownAttemptV1<()>, PortError>> {
        Box::pin(async move {
            let snapshot_path = self.migration.snapshot_path().to_owned();
            let target = lease.target.clone();
            let vault_lease = required_vault_lease(lease)?;
            match target
                .ensure_artifacts_absent(
                    vault_lease,
                    &snapshot_path,
                    journal
                        .plaintext_snapshot_digest
                        .map(|digest| *digest.as_bytes()),
                    cancellation,
                )
                .await
            {
                Ok(()) => Ok(HouseholdTeardownAttemptV1::Verified(())),
                Err(_) => Ok(HouseholdTeardownAttemptV1::Incomplete),
            }
        })
    }

    fn release_native_state_locks<'a>(
        &'a self,
        lease: &'a mut Self::Lease,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdTeardownAttemptV1<()>, PortError>> {
        Box::pin(async move {
            if lease.lifecycle_lease.is_some()
                && lease.vault_lease.is_none()
                && lease.source_lease.is_none()
            {
                return Ok(HouseholdTeardownAttemptV1::Verified(()));
            }
            let Some(vault_lease) = lease.vault_lease.take() else {
                return Ok(HouseholdTeardownAttemptV1::Incomplete);
            };
            let lifecycle = vault_lease.release_vault(cancellation).await?;
            let Some(source_lease) = lease.source_lease.take() else {
                return Err(PortError::new(
                    "household_teardown_lock_state",
                    "legacy source locks are not retained",
                ));
            };
            let lifecycle = self
                .migration
                .release_source_locks_retaining_lifecycle(source_lease, lifecycle)?;
            lease.lifecycle_lease = Some(lifecycle);
            Ok(HouseholdTeardownAttemptV1::Verified(()))
        })
    }

    fn ensure_auth_absent<'a>(
        &'a self,
        lease: &'a mut Self::Lease,
        journal: &'a HouseholdTeardownJournalV1,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdTeardownAttemptV1<()>, PortError>> {
        Box::pin(async move {
            let lifecycle = lease.lifecycle_lease.as_ref().ok_or_else(|| {
                PortError::new(
                    "household_teardown_lock_state",
                    "account lifecycle lock was not retained for auth cleanup",
                )
            })?;
            lifecycle.validate_for(lease.target.account_slot())?;
            match self.auth.finish_account_bound_logout_for_account_digest(
                *journal.account_digest.as_bytes(),
                &self.session,
            ) {
                Ok(true) => {
                    lifecycle.validate_for(lease.target.account_slot())?;
                    return Ok(HouseholdTeardownAttemptV1::Verified(()));
                }
                Ok(false) => {}
                Err(_) => return Ok(HouseholdTeardownAttemptV1::Incomplete),
            }
            let observed = match self.auth.load_account_bound(&self.session) {
                Ok(observed) => observed,
                Err(_) => return Ok(HouseholdTeardownAttemptV1::Incomplete),
            };
            match observed {
                None => {
                    lifecycle.validate_for(lease.target.account_slot())?;
                    Ok(HouseholdTeardownAttemptV1::Verified(()))
                }
                Some(credentials) => {
                    let digest = CanonicalDigestV1::from_bytes(domain_hash(
                        ACCOUNT_DIGEST_DOMAIN,
                        &[credentials.session.account_id.as_str().as_bytes()],
                    )?);
                    if digest != journal.account_digest {
                        return Ok(HouseholdTeardownAttemptV1::Incomplete);
                    }
                    let _clear = self.auth.clear_account_bound(&credentials, &self.session);
                    lifecycle.validate_for(lease.target.account_slot())?;
                    match self.auth.load_account_bound(&self.session) {
                        Ok(None) => Ok(HouseholdTeardownAttemptV1::Verified(())),
                        Ok(Some(_)) | Err(_) => Ok(HouseholdTeardownAttemptV1::Incomplete),
                    }
                }
            }
        })
    }
}

#[cfg(feature = "native-credentials")]
fn required_vault_lease(
    lease: &mut ProductionNativeAccountTeardownLeaseV1,
) -> Result<&mut HouseholdVaultLease, PortError> {
    lease.vault_lease.as_mut().ok_or_else(|| {
        PortError::new(
            "household_teardown_lock_state",
            "household teardown no longer retains the vault lock",
        )
    })
}

#[cfg(feature = "native-credentials")]
fn teardown_guard_state(
    state: HouseholdMigrationGuardStateV1,
) -> Result<HouseholdTeardownGuardStateV1, PortError> {
    match state {
        HouseholdMigrationGuardStateV1::Migrated => Ok(HouseholdTeardownGuardStateV1::Migrated),
        HouseholdMigrationGuardStateV1::InitializedNoSource => {
            Ok(HouseholdTeardownGuardStateV1::InitializedNoSource)
        }
        HouseholdMigrationGuardStateV1::BlockedRepair => {
            Ok(HouseholdTeardownGuardStateV1::BlockedRepair)
        }
        HouseholdMigrationGuardStateV1::BlockedAfterLogout => {
            Ok(HouseholdTeardownGuardStateV1::BlockedAfterLogout)
        }
        HouseholdMigrationGuardStateV1::Initializing | HouseholdMigrationGuardStateV1::Aborting => {
            Err(PortError::new(
                "household_teardown_guard_state",
                "native household initialization must be reconciled before teardown",
            ))
        }
    }
}

#[cfg(feature = "native-credentials")]
fn guard_matches_recorded_tuple(
    guard: &HouseholdMigrationGuardDocument,
    journal: &HouseholdTeardownJournalV1,
) -> Result<bool, PortError> {
    let snapshot_digest = guard
        .legacy_python_snapshot()
        .map(|snapshot| snapshot.content_digest);
    Ok(guard.source_identity() == &journal.source_identity
        && guard.migration_id() == journal.migration_id
        && guard.initialization_id() == journal.initialization_id
        && guard.initial_commit_id() == journal.initial_commit_id
        && snapshot_digest == journal.plaintext_snapshot_digest)
}

#[cfg(feature = "native-credentials")]
fn require_original_guard(
    guard: &HouseholdMigrationGuardDocument,
    journal: &HouseholdTeardownJournalV1,
) -> Result<(), PortError> {
    if guard.guard_revision() != journal.expected_guard_revision
        || teardown_guard_state(guard.state())? != journal.expected_guard_state
        || !guard_matches_recorded_tuple(guard, journal)?
    {
        return Err(PortError::uncertain(
            "household_teardown_guard_changed",
            "native household teardown guard changed after preparation",
        ));
    }
    Ok(())
}

#[cfg(feature = "native-credentials")]
fn guard_is_committed_logout(
    guard: &HouseholdMigrationGuardDocument,
    journal: &HouseholdTeardownJournalV1,
) -> Result<bool, PortError> {
    let expected_revision = journal
        .expected_guard_revision
        .checked_add(1)
        .ok_or_else(journal_invalid)?;
    Ok(
        guard.state() == HouseholdMigrationGuardStateV1::BlockedAfterLogout
            && guard.guard_revision() == expected_revision
            && guard_matches_recorded_tuple(guard, journal)?,
    )
}

#[cfg(feature = "native-credentials")]
fn verify_guard_and_loaded_state(
    vault: &HouseholdVault,
    guard: &HouseholdMigrationGuardDocument,
    loaded: &HouseholdVaultLoad,
) -> Result<(), PortError> {
    let state = decode_canonical_household_state_v1(&loaded.canonical_state).map_err(|_| {
        PortError::new(
            "household_teardown_vault_state",
            "authenticated household state is invalid",
        )
    })?;
    let current_commit_matches = state.bounded_applied_commits.iter().any(|record| {
        record.commit_id.as_uuid() == loaded.commit_id
            && record.resulting_revision == state.revision
    });
    let mut initial_records = state
        .bounded_applied_commits
        .iter()
        .filter(|record| record.commit_id.as_uuid() == guard.initial_commit_id());
    let initial = initial_records.next();
    let source_matches = serde_json::to_value(guard.source_identity())
        .ok()
        .zip(serde_json::to_value(&state.migration_provenance.source_identity).ok())
        .is_some_and(|(left, right)| left == right);
    let snapshot_matches = serde_json::to_value(guard.legacy_python_snapshot())
        .ok()
        .zip(serde_json::to_value(&state.migration_provenance.legacy_python_snapshot).ok())
        .is_some_and(|(left, right)| left == right);
    let initial_matches = initial.is_some_and(|record| {
        record.outcome == AppliedCommitOutcomeV1::Initialized
            && record.resulting_revision.get() == 1
            && guard.initial_effect_fingerprint() == Some(*record.fingerprint.as_bytes())
    }) && initial_records.next().is_none()
        && state
            .bounded_applied_commits
            .iter()
            .filter(|record| record.outcome == AppliedCommitOutcomeV1::Initialized)
            .count()
            == 1;
    let revision_one_digest_matches = state.revision.get() != 1
        || guard.initial_state_digest() == Some(loaded.plaintext_sha256());
    if state.account_binding != *vault.account_id()
        || state.revision.get() != loaded.state_revision
        || !current_commit_matches
        || state.migration_provenance.migration_id != guard.migration_id()
        || state.migration_provenance.initialization_id != guard.initialization_id()
        || state.migration_provenance.initial_commit_id.as_uuid() != guard.initial_commit_id()
        || state.migration_provenance.migration_frozen_at != *guard.migration_frozen_at()
        || !source_matches
        || !snapshot_matches
        || !initial_matches
        || !revision_one_digest_matches
    {
        return Err(PortError::new(
            "household_teardown_vault_binding",
            "authenticated household state does not match its migration guard",
        ));
    }
    Ok(())
}

#[cfg(feature = "native-credentials")]
fn journal_target(
    journal: &HouseholdTeardownJournalV1,
    kind: HouseholdTeardownLegacyTargetKindV1,
) -> Result<&HouseholdTeardownLegacyTargetV1, PortError> {
    journal
        .legacy_cleanup_targets
        .iter()
        .find(|target| target.kind == kind)
        .ok_or_else(journal_invalid)
}

#[cfg(feature = "native-credentials")]
fn legacy_file_locator_digest(path: &Path) -> Result<CanonicalDigestV1, PortError> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        if !path.is_absolute() {
            return Err(PortError::new(
                "household_teardown_legacy_locator",
                "legacy credential locator is not absolute",
            ));
        }
        Ok(CanonicalDigestV1::from_bytes(
            Sha256::digest(path.as_os_str().as_bytes()).into(),
        ))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(PortError::new(
            "household_teardown_legacy_locator",
            "legacy credential locator is unavailable on this platform",
        ))
    }
}

#[cfg(feature = "native-credentials")]
fn legacy_keyring_locator_digest(path: &Path) -> Result<CanonicalDigestV1, PortError> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        if !path.is_absolute() {
            return Err(PortError::new(
                "household_teardown_legacy_locator",
                "legacy keyring locator is not absolute",
            ));
        }
        LegacyPythonKeyringLocatorV1::from_resolved_config_path_bytes(path.as_os_str().as_bytes())?
            .locator_digest()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(PortError::new(
            "household_teardown_legacy_locator",
            "legacy keyring locator is unavailable on this platform",
        ))
    }
}

#[cfg(feature = "native-credentials")]
fn status_digest(state: &str) -> Result<CanonicalDigestV1, PortError> {
    #[derive(Serialize)]
    #[serde(deny_unknown_fields)]
    struct Status<'a> {
        state: &'a str,
    }
    let canonical = to_canonical_bytes_v1(&Status { state }).map_err(|_| journal_invalid())?;
    Ok(CanonicalDigestV1::from_bytes(
        Sha256::digest(canonical).into(),
    ))
}

#[cfg(feature = "native-credentials")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LegacyFileBindingV1 {
    CurrentAccount,
    ForeignAccount,
    Unbound,
    Ambiguous,
}

#[cfg(feature = "native-credentials")]
struct LegacyFileProjectionV1 {
    canonical_noncredential: Zeroizing<Vec<u8>>,
    noncredential_digest: CanonicalDigestV1,
    credentials_present: bool,
    binding: LegacyFileBindingV1,
}

#[cfg(feature = "native-credentials")]
enum LegacyFileObservationV1 {
    AuthoritativeMissing,
    Present(LegacyFileProjectionV1),
    Unavailable,
    Malformed,
}

#[cfg(feature = "native-credentials")]
fn observe_legacy_file(
    path: &Path,
    account_digest: CanonicalDigestV1,
) -> Result<LegacyFileObservationV1, PortError> {
    const MAXIMUM_BYTES: usize = 4 * 1024 * 1024;
    let metadata = match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LegacyFileObservationV1::AuthoritativeMissing);
        }
        Err(_) => return Ok(LegacyFileObservationV1::Unavailable),
        Ok(metadata) => metadata,
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Ok(LegacyFileObservationV1::Unavailable);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Ok(LegacyFileObservationV1::Unavailable);
        }
    }
    let length = match usize::try_from(metadata.len()) {
        Ok(length) if length <= MAXIMUM_BYTES => length,
        _ => return Ok(LegacyFileObservationV1::Malformed),
    };
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return Ok(LegacyFileObservationV1::Unavailable),
    };
    let opened = match file.metadata() {
        Ok(opened) if same_legacy_file(&metadata, &opened) => opened,
        _ => return Ok(LegacyFileObservationV1::Unavailable),
    };
    let mut bytes = Zeroizing::new(Vec::with_capacity(length));
    if file
        .take((MAXIMUM_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() != length
        || bytes.len() > MAXIMUM_BYTES
    {
        return Ok(LegacyFileObservationV1::Unavailable);
    }
    let after = match fs::symlink_metadata(path) {
        Ok(after) if same_legacy_file(&opened, &after) => after,
        _ => return Ok(LegacyFileObservationV1::Unavailable),
    };
    if after.len() != metadata.len() {
        return Ok(LegacyFileObservationV1::Unavailable);
    }
    classify_legacy_file_bytes(&bytes, account_digest)
}

#[cfg(feature = "native-credentials")]
fn same_legacy_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        left.dev() == right.dev()
            && left.ino() == right.ino()
            && left.uid() == right.uid()
            && left.len() == right.len()
            && left.mtime() == right.mtime()
            && left.mtime_nsec() == right.mtime_nsec()
    }
    #[cfg(not(unix))]
    {
        left.len() == right.len() && left.modified().ok() == right.modified().ok()
    }
}

#[cfg(feature = "native-credentials")]
fn classify_legacy_file_bytes(
    bytes: &[u8],
    account_digest: CanonicalDigestV1,
) -> Result<LegacyFileObservationV1, PortError> {
    const FLAT_CREDENTIAL_FIELDS: [&str; 5] = [
        "api_key",
        "oauth.access_token",
        "oauth.refresh_token",
        "session.access_token",
        "session.refresh_token",
    ];
    let mut object =
        match parse_bounded_json_object_v1(bytes, CompatibilityJsonLimitsV1::MIGRATION_CANDIDATE) {
            Ok(object) => object,
            Err(_) => return Ok(LegacyFileObservationV1::Malformed),
        };
    let binding = match legacy_file_binding(&object, account_digest)? {
        Some(binding) => binding,
        None => return Ok(LegacyFileObservationV1::Malformed),
    };
    let mut credentials_present = false;
    for field in FLAT_CREDENTIAL_FIELDS {
        if let Some(value) = object.remove(field) {
            if !value.is_string() {
                return Ok(LegacyFileObservationV1::Malformed);
            }
            credentials_present = true;
        }
    }
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
                    return Ok(LegacyFileObservationV1::Malformed);
                }
                credentials_present = true;
            }
        }
    }
    let canonical_noncredential = Zeroizing::new(
        to_canonical_bytes_v1(&Value::Object(object)).map_err(|_| journal_invalid())?,
    );
    let noncredential_digest =
        CanonicalDigestV1::from_bytes(Sha256::digest(&canonical_noncredential).into());
    Ok(LegacyFileObservationV1::Present(LegacyFileProjectionV1 {
        canonical_noncredential,
        noncredential_digest,
        credentials_present,
        binding,
    }))
}

#[cfg(feature = "native-credentials")]
fn legacy_file_binding(
    object: &Map<String, Value>,
    expected_account_digest: CanonicalDigestV1,
) -> Result<Option<LegacyFileBindingV1>, PortError> {
    let mut bindings = Vec::with_capacity(3);
    for candidate in [
        object.get("account_user_id"),
        object.get("session.user_id"),
        object
            .get("session")
            .and_then(Value::as_object)
            .and_then(|session| session.get("user_id")),
    ]
    .into_iter()
    .flatten()
    {
        let Some(raw) = candidate.as_str() else {
            return Ok(None);
        };
        let account = match AccountId::parse(raw) {
            Ok(account) if account.as_str() == raw => account,
            _ => return Ok(None),
        };
        bindings.push(account);
    }
    if bindings.is_empty() {
        return Ok(Some(LegacyFileBindingV1::Unbound));
    }
    if bindings
        .iter()
        .skip(1)
        .any(|candidate| candidate != &bindings[0])
    {
        return Ok(Some(LegacyFileBindingV1::Ambiguous));
    }
    let observed = CanonicalDigestV1::from_bytes(domain_hash(
        ACCOUNT_DIGEST_DOMAIN,
        &[bindings[0].as_str().as_bytes()],
    )?);
    Ok(Some(if observed == expected_account_digest {
        LegacyFileBindingV1::CurrentAccount
    } else {
        LegacyFileBindingV1::ForeignAccount
    }))
}

#[cfg(feature = "native-credentials")]
fn inspect_legacy_file_target(
    kind: HouseholdTeardownLegacyTargetKindV1,
    path: &Path,
    account_digest: CanonicalDigestV1,
) -> Result<HouseholdTeardownLegacyTargetV1, PortError> {
    let locator_digest = legacy_file_locator_digest(path)?;
    let (expected_noncredential_digest, outcome) =
        record_legacy_file_observation(observe_legacy_file(path, account_digest)?)?;
    Ok(HouseholdTeardownLegacyTargetV1 {
        kind,
        locator_digest,
        expected_noncredential_digest,
        outcome,
    })
}

#[cfg(feature = "native-credentials")]
fn record_legacy_file_observation(
    observation: LegacyFileObservationV1,
) -> Result<(CanonicalDigestV1, HouseholdTeardownLegacyTargetOutcomeV1), PortError> {
    match observation {
        LegacyFileObservationV1::AuthoritativeMissing => Ok((
            status_digest("file_authoritative_missing")?,
            HouseholdTeardownLegacyTargetOutcomeV1::AuthoritativeMissing,
        )),
        LegacyFileObservationV1::Unavailable => Ok((
            status_digest("file_unavailable")?,
            HouseholdTeardownLegacyTargetOutcomeV1::Unavailable,
        )),
        LegacyFileObservationV1::Malformed => Ok((
            status_digest("file_malformed")?,
            HouseholdTeardownLegacyTargetOutcomeV1::Malformed,
        )),
        LegacyFileObservationV1::Present(projection) => {
            let outcome = match projection.binding {
                LegacyFileBindingV1::CurrentAccount if projection.credentials_present => {
                    HouseholdTeardownLegacyTargetOutcomeV1::CredentialsPresent
                }
                LegacyFileBindingV1::CurrentAccount => {
                    HouseholdTeardownLegacyTargetOutcomeV1::CurrentAccountScrubbed
                }
                LegacyFileBindingV1::ForeignAccount => {
                    HouseholdTeardownLegacyTargetOutcomeV1::ForeignAccount
                }
                LegacyFileBindingV1::Unbound => HouseholdTeardownLegacyTargetOutcomeV1::Unbound,
                LegacyFileBindingV1::Ambiguous => HouseholdTeardownLegacyTargetOutcomeV1::Ambiguous,
            };
            Ok((projection.noncredential_digest, outcome))
        }
    }
}

#[cfg(feature = "native-credentials")]
fn scrub_legacy_file_target(
    expected: &HouseholdTeardownLegacyTargetV1,
    path: &Path,
    account_digest: CanonicalDigestV1,
) -> Result<HouseholdTeardownLegacyTargetV1, PortError> {
    if legacy_file_locator_digest(path)? != expected.locator_digest {
        return Err(PortError::new(
            "household_teardown_legacy_binding",
            "legacy file locator changed during teardown",
        ));
    }
    let observation = observe_legacy_file(path, account_digest)?;
    let mut current = expected.clone();
    match observation {
        LegacyFileObservationV1::Present(projection)
            if projection.noncredential_digest == expected.expected_noncredential_digest
                && projection.binding == LegacyFileBindingV1::CurrentAccount =>
        {
            if projection.credentials_present {
                let replace = AtomicFile::replace(path, &projection.canonical_noncredential);
                let verified = observe_legacy_file(path, account_digest)?;
                match verified {
                    LegacyFileObservationV1::Present(verified)
                        if !verified.credentials_present
                            && verified.binding == LegacyFileBindingV1::CurrentAccount
                            && verified.noncredential_digest
                                == expected.expected_noncredential_digest
                            && replace.is_ok() =>
                    {
                        current.outcome =
                            HouseholdTeardownLegacyTargetOutcomeV1::CurrentAccountScrubbed;
                    }
                    LegacyFileObservationV1::Present(verified)
                        if !verified.credentials_present
                            && verified.binding == LegacyFileBindingV1::CurrentAccount
                            && verified.noncredential_digest
                                == expected.expected_noncredential_digest =>
                    {
                        current.outcome = HouseholdTeardownLegacyTargetOutcomeV1::Unavailable;
                    }
                    LegacyFileObservationV1::Present(verified)
                        if verified.credentials_present
                            && verified.binding == LegacyFileBindingV1::CurrentAccount
                            && verified.noncredential_digest
                                == expected.expected_noncredential_digest =>
                    {
                        current.outcome = HouseholdTeardownLegacyTargetOutcomeV1::Unavailable;
                    }
                    _ => {
                        current.outcome = HouseholdTeardownLegacyTargetOutcomeV1::Changed;
                    }
                }
            } else {
                current.outcome = HouseholdTeardownLegacyTargetOutcomeV1::CurrentAccountScrubbed;
            }
        }
        other => {
            let (digest, outcome) = record_legacy_file_observation(other)?;
            current.outcome = if digest == expected.expected_noncredential_digest {
                outcome
            } else {
                HouseholdTeardownLegacyTargetOutcomeV1::Changed
            };
        }
    }
    Ok(current)
}

#[derive(Clone, Debug)]
pub struct HouseholdTeardownJournalStoreV1 {
    native_root: PathBuf,
    directory: PathBuf,
    native_root_instance_digest: CanonicalDigestV1,
}

impl HouseholdTeardownJournalStoreV1 {
    pub fn open(native_root: impl Into<PathBuf>) -> Result<Self, PortError> {
        let native_root = native_root.into();
        OwnerOnlyPath::directory(&native_root)?;
        let native_root_instance_digest =
            CanonicalDigestV1::from_bytes(household_native_root_instance_digest_v1(&native_root)?);
        Ok(Self {
            directory: native_root.join(TEARDOWN_DIRECTORY),
            native_root,
            native_root_instance_digest,
        })
    }

    #[must_use]
    pub const fn native_root_instance_digest(&self) -> CanonicalDigestV1 {
        self.native_root_instance_digest
    }

    pub fn scan(&self) -> Result<Vec<HouseholdTeardownJournalV1>, PortError> {
        validate_native_root(&self.native_root)?;
        let directory_metadata = match fs::symlink_metadata(&self.directory) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(journal_unavailable()),
            Ok(metadata) => metadata,
        };
        validate_private_directory(&directory_metadata)?;
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.directory).map_err(|_| journal_unavailable())? {
            let entry = entry.map_err(|_| journal_unavailable())?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| journal_invalid())?;
            validate_journal_name(&name)?;
            names.push(name);
            if names.len() > MAX_HOUSEHOLD_TEARDOWN_JOURNALS {
                return Err(PortError::new(
                    "household_teardown_limit",
                    "too many native household teardown journals require recovery",
                ));
            }
        }
        names.sort();
        if names.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(journal_invalid());
        }
        let mut journals = Vec::with_capacity(names.len());
        for name in names {
            let journal = self.read_path(&self.directory.join(&name))?;
            if journal.native_root_instance_digest != self.native_root_instance_digest
                || name != journal_filename(&journal.account_digest)
            {
                return Err(journal_invalid());
            }
            journals.push(journal);
        }
        Ok(journals)
    }

    pub fn load(
        &self,
        account_digest: CanonicalDigestV1,
    ) -> Result<Option<HouseholdTeardownJournalV1>, PortError> {
        let path = self.directory.join(journal_filename(&account_digest));
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(journal_unavailable()),
            Ok(_) => self.read_path(&path).map(Some),
        }
    }

    pub fn replace(&self, journal: &HouseholdTeardownJournalV1) -> Result<(), PortError> {
        journal.validate()?;
        if journal.native_root_instance_digest != self.native_root_instance_digest {
            return Err(journal_invalid());
        }
        validate_native_root(&self.native_root)?;
        OwnerOnlyPath::directory(&self.directory)?;
        let path = self
            .directory
            .join(journal_filename(&journal.account_digest));
        AtomicFile::replace(&path, &journal.canonical_bytes()?)?;
        let observed = self.read_path(&path)?;
        if observed != *journal {
            return Err(PortError::uncertain(
                "household_teardown_journal_write",
                "native household teardown journal write could not be verified",
            ));
        }
        Ok(())
    }

    pub fn remove_verified(&self, expected: &HouseholdTeardownJournalV1) -> Result<(), PortError> {
        let path = self
            .directory
            .join(journal_filename(&expected.account_digest));
        let observed = self.read_path(&path)?;
        if observed != *expected {
            return Err(PortError::uncertain(
                "household_teardown_journal_changed",
                "native household teardown journal changed before finalization",
            ));
        }
        fs::remove_file(&path).map_err(|_| {
            PortError::uncertain(
                "household_teardown_journal_remove",
                "native household teardown journal removal is uncertain",
            )
        })?;
        sync_directory(&self.directory)?;
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(PortError::uncertain(
                "household_teardown_journal_remove",
                "native household teardown journal removal could not be verified",
            )),
        }
    }

    fn read_path(&self, path: &Path) -> Result<HouseholdTeardownJournalV1, PortError> {
        let metadata = fs::symlink_metadata(path).map_err(|_| journal_unavailable())?;
        validate_private_file(&metadata)?;
        let length = usize::try_from(metadata.len()).map_err(|_| journal_invalid())?;
        if length == 0 || length > MAX_HOUSEHOLD_TEARDOWN_JOURNAL_BYTES {
            return Err(journal_invalid());
        }
        let bytes = fs::read(path).map_err(|_| journal_unavailable())?;
        if bytes.len() != length {
            return Err(journal_unavailable());
        }
        HouseholdTeardownJournalV1::from_canonical_bytes(&bytes)
    }
}

/// Phase-ordered coordinator. The backend owns sensitive reads and mutations;
/// this type owns only content-free restart state and monotonic progression.
pub struct NativeAccountTeardownV1<'a, B: NativeAccountTeardownBackendV1> {
    journals: &'a HouseholdTeardownJournalStoreV1,
    backend: &'a B,
}

impl<'a, B: NativeAccountTeardownBackendV1> NativeAccountTeardownV1<'a, B> {
    #[must_use]
    pub const fn new(journals: &'a HouseholdTeardownJournalStoreV1, backend: &'a B) -> Self {
        Self { journals, backend }
    }

    pub async fn execute_authenticated(
        &self,
        expected: &AuthCredentialBundle,
        cancellation: CancellationToken,
    ) -> Result<HouseholdEraseOutcome, PortError> {
        let account_digest = CanonicalDigestV1::from_bytes(domain_hash(
            ACCOUNT_DIGEST_DOMAIN,
            &[expected.session.account_id.as_str().as_bytes()],
        )?);
        let pending = self.journals.scan()?;
        if pending
            .iter()
            .any(|journal| journal.account_digest != account_digest)
        {
            return Err(PortError::new(
                "household_teardown_resume_required",
                "another native household teardown must finish before this logout",
            ));
        }
        if let Some(journal) = pending.into_iter().next() {
            let lease = match self
                .backend
                .acquire_resume(&journal, cancellation.child_token())
                .await
            {
                Ok(lease) => lease,
                Err(_) => return Ok(partial_outcome(VerifiedTeardownProgressV1::default())),
            };
            let journal = match self.reload_exact(&journal) {
                Ok(journal) => journal,
                Err(_) => return Ok(partial_outcome(VerifiedTeardownProgressV1::default())),
            };
            return match self.drive(journal, lease, cancellation).await {
                Ok(outcome) => Ok(outcome),
                Err(_) => Ok(partial_outcome(VerifiedTeardownProgressV1::default())),
            };
        }
        let (lease, prepared) = self
            .backend
            .acquire_authenticated(expected, cancellation.child_token())
            .await?;
        if prepared.account_digest != account_digest
            || prepared.native_root_instance_digest != self.journals.native_root_instance_digest()
        {
            return Err(PortError::new(
                "household_teardown_account_mismatch",
                "native household teardown evidence does not match the authenticated account",
            ));
        }
        let journal = HouseholdTeardownJournalV1::new(prepared)?;
        self.journals.replace(&journal)?;
        match self.drive(journal, lease, cancellation).await {
            Ok(outcome) => Ok(outcome),
            Err(_) => Ok(partial_outcome(VerifiedTeardownProgressV1::default())),
        }
    }

    /// Resume every globally discoverable journal in lexical account-digest
    /// order. Startup/login must fail closed when any one remains incomplete.
    pub async fn resume_all(&self, cancellation: CancellationToken) -> Result<usize, PortError> {
        let outcomes = self.resume_all_outcomes(cancellation).await?;
        if outcomes.iter().any(|outcome| outcome.outcome_uncertain) {
            return Err(PortError::uncertain(
                "household_teardown_resume_incomplete",
                "native household teardown remains incomplete",
            ));
        }
        Ok(outcomes.len())
    }

    /// Resume every journal and return truthful local cleanup facts. This is
    /// used by `logout`, which must render a partial local result rather than
    /// erase it behind a generic startup error. Session-open callers use
    /// [`Self::resume_all`] and therefore still fail closed on any partial.
    pub async fn resume_all_outcomes(
        &self,
        cancellation: CancellationToken,
    ) -> Result<Vec<HouseholdEraseOutcome>, PortError> {
        let journals = self.journals.scan()?;
        let mut outcomes = Vec::with_capacity(journals.len());
        for journal in journals {
            if cancellation.is_cancelled() {
                // No journal-bound work has started in this iteration, so
                // preserve cancellation identity. Once `drive` starts, it
                // returns a truthful partial outcome for any interruption
                // after a destructive phase may have committed.
                return Err(cancelled());
            }
            let lease = match self
                .backend
                .acquire_resume(&journal, cancellation.child_token())
                .await
            {
                Ok(lease) => lease,
                Err(_) => {
                    outcomes.push(partial_outcome(VerifiedTeardownProgressV1::default()));
                    break;
                }
            };
            let journal = match self.reload_exact(&journal) {
                Ok(journal) => journal,
                Err(_) => {
                    outcomes.push(partial_outcome(VerifiedTeardownProgressV1::default()));
                    break;
                }
            };
            let outcome = match self.drive(journal, lease, cancellation.child_token()).await {
                Ok(outcome) => outcome,
                Err(_) => partial_outcome(VerifiedTeardownProgressV1::default()),
            };
            let partial = outcome.outcome_uncertain;
            outcomes.push(outcome);
            if partial {
                break;
            }
        }
        Ok(outcomes)
    }

    fn reload_exact(
        &self,
        expected: &HouseholdTeardownJournalV1,
    ) -> Result<HouseholdTeardownJournalV1, PortError> {
        let observed = self
            .journals
            .load(expected.account_digest)?
            .ok_or_else(|| {
                PortError::uncertain(
                    "household_teardown_journal_changed",
                    "native household teardown journal disappeared after lock acquisition",
                )
            })?;
        if observed != *expected {
            return Err(PortError::uncertain(
                "household_teardown_journal_changed",
                "native household teardown journal changed after lock acquisition",
            ));
        }
        Ok(observed)
    }

    async fn drive(
        &self,
        mut journal: HouseholdTeardownJournalV1,
        mut lease: B::Lease,
        cancellation: CancellationToken,
    ) -> Result<HouseholdEraseOutcome, PortError> {
        let mut verified = VerifiedTeardownProgressV1::default();
        if cancellation.is_cancelled() {
            return Ok(partial_outcome(verified));
        }
        let repair_without_key = journal.key_absence_basis.is_some();
        if repair_without_key
            && (journal.teardown_phase == HouseholdTeardownPhaseV1::Prepared
                || !journal.legacy_cleanup_complete())
        {
            if self
                .scrub_and_persist(&mut journal, &mut lease, &cancellation)
                .await
                .is_err()
            {
                verified.legacy_credentials_cleared = journal.legacy_cleanup_complete();
                return Ok(partial_outcome(verified));
            }
            verified.legacy_credentials_cleared = journal.legacy_cleanup_complete();
        }

        let guard_attempt = match self
            .backend
            .ensure_guard_blocked(&mut lease, &journal, cancellation.child_token())
            .await
        {
            Ok(attempt) => attempt,
            Err(_) => return Ok(partial_outcome(verified)),
        };
        match guard_attempt {
            HouseholdTeardownAttemptV1::Incomplete => return Ok(partial_outcome(verified)),
            HouseholdTeardownAttemptV1::Verified(revision) => {
                let expected = journal
                    .expected_guard_revision
                    .checked_add(1)
                    .ok_or_else(journal_invalid)?;
                if revision != expected {
                    return Err(journal_invalid());
                }
                if journal.blocked_after_logout_guard_revision != Some(revision)
                    || journal.teardown_phase == HouseholdTeardownPhaseV1::Prepared
                {
                    journal.blocked_after_logout_guard_revision = Some(revision);
                    journal.teardown_phase = HouseholdTeardownPhaseV1::GuardBlocked;
                    if self.journals.replace(&journal).is_err() {
                        return Ok(partial_outcome(verified));
                    }
                }
            }
        }

        // A plaintext journal records prior cleanup facts but is not current
        // storage authority. Re-probe all four frozen targets on every resume
        // before reporting or finalizing their absence.
        if self
            .scrub_and_persist(&mut journal, &mut lease, &cancellation)
            .await
            .is_err()
        {
            verified.legacy_credentials_cleared = journal.legacy_cleanup_complete();
            return Ok(partial_outcome(verified));
        }
        verified.legacy_credentials_cleared = journal.legacy_cleanup_complete();
        if journal.teardown_phase < HouseholdTeardownPhaseV1::CredentialsScrubbed {
            journal.teardown_phase = HouseholdTeardownPhaseV1::CredentialsScrubbed;
            if self.journals.replace(&journal).is_err() {
                return Ok(partial_outcome(verified));
            }
        }
        if cancellation.is_cancelled() {
            return Ok(partial_outcome(verified));
        }

        let key_attempt = match self
            .backend
            .ensure_key_absent(&mut lease, &journal, cancellation.child_token())
            .await
        {
            Ok(attempt) => attempt,
            Err(_) => return Ok(partial_outcome(verified)),
        };
        match key_attempt {
            HouseholdTeardownAttemptV1::Incomplete => return Ok(partial_outcome(verified)),
            HouseholdTeardownAttemptV1::Verified(()) => {
                verified.household_key_deleted = true;
                if journal.teardown_phase < HouseholdTeardownPhaseV1::KeyAbsent {
                    journal.teardown_phase = HouseholdTeardownPhaseV1::KeyAbsent;
                    if self.journals.replace(&journal).is_err() {
                        return Ok(partial_outcome(verified));
                    }
                }
            }
        }

        let artifact_attempt = match self
            .backend
            .ensure_artifacts_absent(&mut lease, &journal, cancellation.child_token())
            .await
        {
            Ok(attempt) => attempt,
            Err(_) => return Ok(partial_outcome(verified)),
        };
        match artifact_attempt {
            HouseholdTeardownAttemptV1::Incomplete => return Ok(partial_outcome(verified)),
            HouseholdTeardownAttemptV1::Verified(()) => {
                verified.artifacts_deleted = true;
                if journal.teardown_phase < HouseholdTeardownPhaseV1::ArtifactsAbsent {
                    journal.teardown_phase = HouseholdTeardownPhaseV1::ArtifactsAbsent;
                    if self.journals.replace(&journal).is_err() {
                        return Ok(partial_outcome(verified));
                    }
                }
            }
        }

        let release_attempt = match self
            .backend
            .release_native_state_locks(&mut lease, cancellation.child_token())
            .await
        {
            Ok(attempt) => attempt,
            Err(_) => return Ok(partial_outcome(verified)),
        };
        match release_attempt {
            HouseholdTeardownAttemptV1::Incomplete => return Ok(partial_outcome(verified)),
            HouseholdTeardownAttemptV1::Verified(()) => {}
        }
        let auth_attempt = match self
            .backend
            .ensure_auth_absent(&mut lease, &journal, cancellation.child_token())
            .await
        {
            Ok(attempt) => attempt,
            Err(_) => return Ok(partial_outcome(verified)),
        };
        match auth_attempt {
            HouseholdTeardownAttemptV1::Incomplete => return Ok(partial_outcome(verified)),
            HouseholdTeardownAttemptV1::Verified(()) => {
                verified.native_auth_deleted = true;
                if journal.teardown_phase < HouseholdTeardownPhaseV1::AuthAbsent {
                    journal.teardown_phase = HouseholdTeardownPhaseV1::AuthAbsent;
                    if self.journals.replace(&journal).is_err() {
                        return Ok(partial_outcome(verified));
                    }
                }
            }
        }

        if !journal.legacy_cleanup_complete() {
            return Ok(partial_outcome(verified));
        }
        if self.journals.remove_verified(&journal).is_err() {
            return Ok(partial_outcome(verified));
        }
        Ok(complete_outcome())
    }

    async fn scrub_and_persist(
        &self,
        journal: &mut HouseholdTeardownJournalV1,
        lease: &mut B::Lease,
        cancellation: &CancellationToken,
    ) -> Result<(), PortError> {
        // Plaintext journal outcomes are restart hints, never current proof.
        // Clear them in memory before every attempt so cancellation or a
        // broker failure cannot be reported as a verified re-probe merely
        // because a previous attempt recorded completion.
        for target in &mut journal.legacy_cleanup_targets {
            target.outcome = HouseholdTeardownLegacyTargetOutcomeV1::Pending;
        }
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }
        let replacements = self
            .backend
            .scrub_legacy_credentials(lease, journal, cancellation.child_token())
            .await?;
        validate_target_replacements(&journal.legacy_cleanup_targets, &replacements)?;
        journal.legacy_cleanup_targets = replacements;
        self.journals.replace(journal)
    }
}

fn validate_target_replacements(
    expected: &[HouseholdTeardownLegacyTargetV1],
    replacements: &[HouseholdTeardownLegacyTargetV1],
) -> Result<(), PortError> {
    if expected.len() != 4 || replacements.len() != 4 {
        return Err(journal_invalid());
    }
    for expected_target in expected {
        let replacement = replacements
            .iter()
            .find(|candidate| candidate.kind == expected_target.kind)
            .ok_or_else(journal_invalid)?;
        if replacement.locator_digest != expected_target.locator_digest
            || replacement.expected_noncredential_digest
                != expected_target.expected_noncredential_digest
        {
            return Err(PortError::new(
                "household_teardown_legacy_binding",
                "legacy credential cleanup target binding changed",
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default)]
struct VerifiedTeardownProgressV1 {
    household_key_deleted: bool,
    artifacts_deleted: bool,
    legacy_credentials_cleared: bool,
    native_auth_deleted: bool,
}

fn partial_outcome(verified: VerifiedTeardownProgressV1) -> HouseholdEraseOutcome {
    HouseholdEraseOutcome {
        household_key_deleted: verified.household_key_deleted,
        household_ciphertext_deleted: verified.artifacts_deleted,
        import_snapshot_deleted: verified.artifacts_deleted,
        legacy_source_retained: true,
        legacy_credentials_cleared: verified.legacy_credentials_cleared,
        legacy_credentials_retained: !verified.legacy_credentials_cleared,
        local_credentials_cleared: verified.native_auth_deleted
            && verified.legacy_credentials_cleared,
        outcome_uncertain: true,
    }
}

#[cfg(feature = "native-credentials")]
const fn pre_native_outcome(
    legacy_credentials_cleared: bool,
    native_auth_deleted: bool,
) -> HouseholdEraseOutcome {
    HouseholdEraseOutcome {
        household_key_deleted: false,
        household_ciphertext_deleted: false,
        import_snapshot_deleted: false,
        legacy_source_retained: true,
        legacy_credentials_cleared,
        legacy_credentials_retained: !legacy_credentials_cleared,
        local_credentials_cleared: native_auth_deleted && legacy_credentials_cleared,
        outcome_uncertain: !native_auth_deleted || !legacy_credentials_cleared,
    }
}

const fn complete_outcome() -> HouseholdEraseOutcome {
    HouseholdEraseOutcome {
        household_key_deleted: true,
        household_ciphertext_deleted: true,
        import_snapshot_deleted: true,
        legacy_source_retained: true,
        legacy_credentials_cleared: true,
        legacy_credentials_retained: false,
        local_credentials_cleared: true,
        outcome_uncertain: false,
    }
}

fn journal_filename(account_digest: &CanonicalDigestV1) -> String {
    format!(
        "{TEARDOWN_PREFIX}{}{TEARDOWN_SUFFIX}",
        account_digest.to_lower_hex()
    )
}

fn validate_journal_name(name: &str) -> Result<(), PortError> {
    let digest = name
        .strip_prefix(TEARDOWN_PREFIX)
        .and_then(|value| value.strip_suffix(TEARDOWN_SUFFIX))
        .ok_or_else(journal_invalid)?;
    if digest.len() != 64
        || digest
            .bytes()
            .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
    {
        return Err(journal_invalid());
    }
    Ok(())
}

fn validate_native_root(path: &Path) -> Result<(), PortError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| journal_unavailable())?;
    validate_private_directory(&metadata)
}

fn validate_private_directory(metadata: &fs::Metadata) -> Result<(), PortError> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(journal_unavailable());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(journal_unavailable());
        }
    }
    Ok(())
}

fn validate_private_file(metadata: &fs::Metadata) -> Result<(), PortError> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(journal_unavailable());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(journal_unavailable());
        }
    }
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), PortError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| {
            PortError::uncertain(
                "household_teardown_directory_sync",
                "native household teardown directory sync is uncertain",
            )
        })
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), PortError> {
    Ok(())
}

fn canonical_v4(value: Uuid) -> bool {
    !value.is_nil() && value.get_version_num() == 4
}

fn domain_hash(label: &[u8], parts: &[&[u8]]) -> Result<[u8; 32], PortError> {
    if label.is_empty() || !label.is_ascii() || label.contains(&0) {
        return Err(journal_invalid());
    }
    let mut digest = Sha256::new();
    digest.update(label);
    digest.update([0]);
    for part in parts {
        let length = u32::try_from(part.len()).map_err(|_| journal_invalid())?;
        digest.update(length.to_be_bytes());
        digest.update(part);
    }
    Ok(digest.finalize().into())
}

fn journal_invalid() -> PortError {
    PortError::new(
        "household_teardown_journal_invalid",
        "native household teardown journal is invalid",
    )
}

fn journal_unavailable() -> PortError {
    PortError::new(
        "household_teardown_journal_unavailable",
        "native household teardown journal is unavailable",
    )
}

fn cancelled() -> PortError {
    PortError::new(
        "household_teardown_cancelled",
        "native household teardown was cancelled",
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    use heyfood_core::{
        AccountId, ChannelCredentials, CredentialVersion, SensitiveString, SessionCredentials,
    };

    use super::*;

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(label: &str) -> Self {
            let sequence = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "heyfood-household-teardown-{label}-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            }
            Self(path)
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum FailStep {
        Guard,
        Key,
        Artifacts,
        Release,
        Auth,
    }

    #[derive(Default)]
    struct FakeState {
        events: Vec<&'static str>,
        fail_once: Option<FailStep>,
        scrub_complete: bool,
    }

    #[derive(Clone)]
    struct FakeBackend {
        prepared: PreparedHouseholdTeardownV1,
        state: Arc<Mutex<FakeState>>,
    }

    struct FakeLease {
        native_locks_held: bool,
    }

    impl FakeBackend {
        fn new(prepared: PreparedHouseholdTeardownV1) -> Self {
            Self {
                prepared,
                state: Arc::new(Mutex::new(FakeState {
                    scrub_complete: true,
                    ..FakeState::default()
                })),
            }
        }

        fn fail_once(&self, step: FailStep) {
            self.state.lock().unwrap().fail_once = Some(step);
        }

        fn set_scrub_complete(&self, complete: bool) {
            self.state.lock().unwrap().scrub_complete = complete;
        }

        fn attempt<T>(&self, step: FailStep, value: T) -> HouseholdTeardownAttemptV1<T> {
            let mut state = self.state.lock().unwrap();
            if state.fail_once == Some(step) {
                state.fail_once = None;
                HouseholdTeardownAttemptV1::Incomplete
            } else {
                HouseholdTeardownAttemptV1::Verified(value)
            }
        }

        fn events(&self) -> Vec<&'static str> {
            self.state.lock().unwrap().events.clone()
        }
    }

    impl NativeAccountTeardownBackendV1 for FakeBackend {
        type Lease = FakeLease;

        fn acquire_authenticated<'a>(
            &'a self,
            _expected: &'a AuthCredentialBundle,
            cancellation: CancellationToken,
        ) -> BoxFuture<'a, Result<(Self::Lease, PreparedHouseholdTeardownV1), PortError>> {
            Box::pin(async move {
                if cancellation.is_cancelled() {
                    return Err(cancelled());
                }
                self.state.lock().unwrap().events.push("acquire");
                Ok((
                    FakeLease {
                        native_locks_held: true,
                    },
                    self.prepared.clone(),
                ))
            })
        }

        fn acquire_resume<'a>(
            &'a self,
            _journal: &'a HouseholdTeardownJournalV1,
            cancellation: CancellationToken,
        ) -> BoxFuture<'a, Result<Self::Lease, PortError>> {
            Box::pin(async move {
                if cancellation.is_cancelled() {
                    return Err(cancelled());
                }
                self.state.lock().unwrap().events.push("resume");
                Ok(FakeLease {
                    native_locks_held: true,
                })
            })
        }

        fn ensure_guard_blocked<'a>(
            &'a self,
            lease: &'a mut Self::Lease,
            journal: &'a HouseholdTeardownJournalV1,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'a, Result<HouseholdTeardownAttemptV1<u64>, PortError>> {
            Box::pin(async move {
                assert!(lease.native_locks_held);
                self.state.lock().unwrap().events.push("guard");
                Ok(self.attempt(FailStep::Guard, journal.expected_guard_revision + 1))
            })
        }

        fn scrub_legacy_credentials<'a>(
            &'a self,
            lease: &'a mut Self::Lease,
            journal: &'a HouseholdTeardownJournalV1,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'a, Result<Vec<HouseholdTeardownLegacyTargetV1>, PortError>> {
            Box::pin(async move {
                assert!(lease.native_locks_held);
                let mut state = self.state.lock().unwrap();
                state.events.push("scrub");
                let complete = state.scrub_complete;
                drop(state);
                Ok(journal
                    .legacy_cleanup_targets
                    .iter()
                    .cloned()
                    .map(|mut target| {
                        target.outcome = if complete {
                            HouseholdTeardownLegacyTargetOutcomeV1::CurrentAccountScrubbed
                        } else {
                            HouseholdTeardownLegacyTargetOutcomeV1::Unavailable
                        };
                        target
                    })
                    .collect())
            })
        }

        fn ensure_key_absent<'a>(
            &'a self,
            lease: &'a mut Self::Lease,
            _journal: &'a HouseholdTeardownJournalV1,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'a, Result<HouseholdTeardownAttemptV1<()>, PortError>> {
            Box::pin(async move {
                assert!(lease.native_locks_held);
                self.state.lock().unwrap().events.push("key");
                Ok(self.attempt(FailStep::Key, ()))
            })
        }

        fn ensure_artifacts_absent<'a>(
            &'a self,
            lease: &'a mut Self::Lease,
            _journal: &'a HouseholdTeardownJournalV1,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'a, Result<HouseholdTeardownAttemptV1<()>, PortError>> {
            Box::pin(async move {
                assert!(lease.native_locks_held);
                self.state.lock().unwrap().events.push("artifacts");
                Ok(self.attempt(FailStep::Artifacts, ()))
            })
        }

        fn release_native_state_locks<'a>(
            &'a self,
            lease: &'a mut Self::Lease,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'a, Result<HouseholdTeardownAttemptV1<()>, PortError>> {
            Box::pin(async move {
                self.state.lock().unwrap().events.push("release");
                let attempt = self.attempt(FailStep::Release, ());
                if matches!(attempt, HouseholdTeardownAttemptV1::Verified(())) {
                    lease.native_locks_held = false;
                }
                Ok(attempt)
            })
        }

        fn ensure_auth_absent<'a>(
            &'a self,
            lease: &'a mut Self::Lease,
            _journal: &'a HouseholdTeardownJournalV1,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'a, Result<HouseholdTeardownAttemptV1<()>, PortError>> {
            Box::pin(async move {
                assert!(!lease.native_locks_held);
                self.state.lock().unwrap().events.push("auth");
                Ok(self.attempt(FailStep::Auth, ()))
            })
        }
    }

    fn credentials(account: &str) -> AuthCredentialBundle {
        AuthCredentialBundle {
            channel: ChannelCredentials::from_rfc3339_expiry(
                "client-1",
                "device-1",
                SensitiveString::new("channel-access"),
                SensitiveString::new("channel-refresh"),
                "2099-01-01T00:00:00Z",
                "profile:read profile:write",
            )
            .unwrap(),
            session: SessionCredentials::from_unix_expiry(
                AccountId::parse(account).unwrap(),
                SensitiveString::new("session-access"),
                SensitiveString::new("session-refresh"),
                CredentialVersion::new(1),
                4_102_444_800,
            )
            .unwrap(),
        }
    }

    fn digest(label: &str) -> CanonicalDigestV1 {
        CanonicalDigestV1::from_bytes(Sha256::digest(label.as_bytes()).into())
    }

    fn prepared(
        store: &HouseholdTeardownJournalStoreV1,
        account: &str,
        repair_without_key: bool,
    ) -> PreparedHouseholdTeardownV1 {
        let account_digest = CanonicalDigestV1::from_bytes(
            domain_hash(ACCOUNT_DIGEST_DOMAIN, &[account.as_bytes()]).unwrap(),
        );
        let account_locator_digest = CanonicalDigestV1::from_bytes(
            domain_hash(
                ACCOUNT_LOCATOR_DOMAIN,
                &[
                    store.native_root_instance_digest().as_bytes(),
                    account_digest.as_bytes(),
                ],
            )
            .unwrap(),
        );
        PreparedHouseholdTeardownV1 {
            native_root_instance_digest: store.native_root_instance_digest(),
            account_digest,
            account_locator_digest,
            expected_guard_state: if repair_without_key {
                HouseholdTeardownGuardStateV1::BlockedRepair
            } else {
                HouseholdTeardownGuardStateV1::Migrated
            },
            expected_guard_revision: 7,
            source_identity: HouseholdMigrationSourceIdentityV1::no_source([0x31; 32]),
            migration_id: Uuid::new_v4(),
            initialization_id: Uuid::new_v4(),
            initial_commit_id: Uuid::new_v4(),
            expected_household_key_id: (!repair_without_key).then(Uuid::new_v4),
            expected_key_bundle_revision: (!repair_without_key).then_some(3),
            key_absence_basis: repair_without_key
                .then_some(HouseholdTeardownKeyAbsenceBasisV1::BlockedRepairNoCommittedVaultV1),
            plaintext_snapshot_digest: Some(digest("snapshot")),
            legacy_cleanup_targets: HouseholdTeardownLegacyTargetKindV1::ALL
                .into_iter()
                .map(|kind| HouseholdTeardownLegacyTargetV1 {
                    kind,
                    locator_digest: digest(&format!("locator-{kind:?}")),
                    expected_noncredential_digest: digest(&format!("retained-{kind:?}")),
                    outcome: HouseholdTeardownLegacyTargetOutcomeV1::Pending,
                })
                .collect(),
        }
    }

    #[test]
    fn journal_is_canonical_bounded_and_debug_is_redacted() {
        let root = TempRoot::new("canonical");
        let store = HouseholdTeardownJournalStoreV1::open(&root.0).unwrap();
        let journal =
            HouseholdTeardownJournalV1::new(prepared(&store, "account-a", false)).unwrap();
        let bytes = journal.canonical_bytes().unwrap();
        assert!(bytes.len() <= MAX_HOUSEHOLD_TEARDOWN_JOURNAL_BYTES);
        assert_eq!(
            HouseholdTeardownJournalV1::from_canonical_bytes(&bytes).unwrap(),
            journal
        );
        let debug = format!("{journal:?}");
        assert!(!debug.contains(&journal.account_hex()));
        assert!(!debug.contains(&journal.migration_id.to_string()));

        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["unknown"] = serde_json::json!(true);
        let noncanonical = serde_json::to_vec(&value).unwrap();
        assert_eq!(
            HouseholdTeardownJournalV1::from_canonical_bytes(&noncanonical)
                .unwrap_err()
                .code,
            "household_teardown_journal_invalid"
        );
    }

    #[test]
    fn store_rejects_swapped_filename_symlink_and_foreign_root() {
        let root = TempRoot::new("bindings");
        let store = HouseholdTeardownJournalStoreV1::open(&root.0).unwrap();
        let journal =
            HouseholdTeardownJournalV1::new(prepared(&store, "account-a", false)).unwrap();
        store.replace(&journal).unwrap();
        assert_eq!(store.scan().unwrap(), vec![journal.clone()]);

        let correct = store
            .directory
            .join(journal_filename(&journal.account_digest));
        let swapped = store.directory.join(format!(
            "{TEARDOWN_PREFIX}{}{TEARDOWN_SUFFIX}",
            "0".repeat(64)
        ));
        fs::rename(&correct, &swapped).unwrap();
        assert_eq!(
            store.scan().unwrap_err().code,
            "household_teardown_journal_invalid"
        );
        fs::rename(&swapped, &correct).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            fs::remove_file(&correct).unwrap();
            symlink("/dev/null", &correct).unwrap();
            assert_eq!(
                store.scan().unwrap_err().code,
                "household_teardown_journal_unavailable"
            );
        }
    }

    #[tokio::test]
    async fn every_native_crash_boundary_is_resumable_without_false_success() {
        for step in [
            FailStep::Guard,
            FailStep::Key,
            FailStep::Artifacts,
            FailStep::Release,
            FailStep::Auth,
        ] {
            let root = TempRoot::new(&format!("resume-{step:?}"));
            let store = HouseholdTeardownJournalStoreV1::open(&root.0).unwrap();
            let backend = FakeBackend::new(prepared(&store, "account-a", false));
            backend.fail_once(step);
            let coordinator = NativeAccountTeardownV1::new(&store, &backend);
            let partial = coordinator
                .execute_authenticated(&credentials("account-a"), CancellationToken::new())
                .await
                .unwrap();
            assert!(partial.outcome_uncertain);
            assert_eq!(store.scan().unwrap().len(), 1);
            assert!(!partial.local_credentials_cleared);
            let resumed = coordinator
                .resume_all(CancellationToken::new())
                .await
                .unwrap();
            assert_eq!(resumed, 1);
            assert!(store.scan().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn unavailable_legacy_target_retains_journal_after_safe_native_cleanup() {
        let root = TempRoot::new("legacy-partial");
        let store = HouseholdTeardownJournalStoreV1::open(&root.0).unwrap();
        let backend = FakeBackend::new(prepared(&store, "account-a", false));
        backend.set_scrub_complete(false);
        let coordinator = NativeAccountTeardownV1::new(&store, &backend);
        let partial = coordinator
            .execute_authenticated(&credentials("account-a"), CancellationToken::new())
            .await
            .unwrap();
        assert!(partial.household_key_deleted);
        assert!(partial.household_ciphertext_deleted);
        assert!(partial.import_snapshot_deleted);
        assert!(partial.legacy_credentials_retained);
        assert!(!partial.legacy_credentials_cleared);
        assert!(!partial.local_credentials_cleared);
        assert!(partial.outcome_uncertain);
        assert_eq!(
            store.scan().unwrap()[0].teardown_phase,
            HouseholdTeardownPhaseV1::AuthAbsent
        );

        backend.set_scrub_complete(true);
        assert_eq!(
            coordinator
                .resume_all(CancellationToken::new())
                .await
                .unwrap(),
            1
        );
        assert!(store.scan().unwrap().is_empty());
    }

    #[tokio::test]
    async fn resume_reprobes_completed_legacy_targets_before_journal_removal() {
        let root = TempRoot::new("legacy-reprobe");
        let store = HouseholdTeardownJournalStoreV1::open(&root.0).unwrap();
        let backend = FakeBackend::new(prepared(&store, "account-a", false));
        backend.fail_once(FailStep::Auth);
        let coordinator = NativeAccountTeardownV1::new(&store, &backend);
        let first = coordinator
            .execute_authenticated(&credentials("account-a"), CancellationToken::new())
            .await
            .unwrap();
        assert!(first.legacy_credentials_cleared);
        assert!(!first.local_credentials_cleared);
        assert_eq!(store.scan().unwrap().len(), 1);

        backend.set_scrub_complete(false);
        let resumed = coordinator
            .resume_all_outcomes(CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(resumed.len(), 1);
        assert!(resumed[0].household_key_deleted);
        assert!(resumed[0].household_ciphertext_deleted);
        assert!(!resumed[0].legacy_credentials_cleared);
        assert!(resumed[0].legacy_credentials_retained);
        assert!(!resumed[0].local_credentials_cleared);
        assert_eq!(store.scan().unwrap().len(), 1);

        backend.set_scrub_complete(true);
        assert_eq!(
            coordinator
                .resume_all(CancellationToken::new())
                .await
                .unwrap(),
            1
        );
        assert!(store.scan().unwrap().is_empty());
    }

    #[tokio::test]
    async fn repair_without_key_scrubs_before_double_absence_and_guard_cas() {
        let root = TempRoot::new("repair-order");
        let store = HouseholdTeardownJournalStoreV1::open(&root.0).unwrap();
        let backend = FakeBackend::new(prepared(&store, "account-a", true));
        let coordinator = NativeAccountTeardownV1::new(&store, &backend);
        let outcome = coordinator
            .execute_authenticated(&credentials("account-a"), CancellationToken::new())
            .await
            .unwrap();
        assert!(!outcome.outcome_uncertain);
        let events = backend.events();
        let scrub = events.iter().position(|event| *event == "scrub").unwrap();
        let guard = events.iter().position(|event| *event == "guard").unwrap();
        assert!(scrub < guard);
    }

    #[tokio::test]
    async fn repair_without_key_continues_safe_native_cleanup_when_legacy_is_partial() {
        let root = TempRoot::new("repair-partial");
        let store = HouseholdTeardownJournalStoreV1::open(&root.0).unwrap();
        let backend = FakeBackend::new(prepared(&store, "account-a", true));
        backend.set_scrub_complete(false);
        let coordinator = NativeAccountTeardownV1::new(&store, &backend);
        let outcome = coordinator
            .execute_authenticated(&credentials("account-a"), CancellationToken::new())
            .await
            .unwrap();
        assert!(outcome.household_key_deleted);
        assert!(outcome.household_ciphertext_deleted);
        assert!(outcome.legacy_credentials_retained);
        assert!(!outcome.local_credentials_cleared);
        assert!(outcome.outcome_uncertain);
        let events = backend.events();
        let scrub = events.iter().position(|event| *event == "scrub").unwrap();
        let guard = events.iter().position(|event| *event == "guard").unwrap();
        assert!(scrub < guard);
        assert_eq!(
            store.scan().unwrap()[0].teardown_phase,
            HouseholdTeardownPhaseV1::AuthAbsent
        );
    }

    #[cfg(feature = "native-credentials")]
    #[test]
    fn legacy_file_scrub_removes_only_exact_credentials_and_retains_projection() {
        let root = TempRoot::new("legacy-file-scrub");
        let path = root.0.join("current").join("config.json");
        let source = br#"{"account_user_id":"account-a","api_key":"api-canary","credential_store":"file","household":{"state":{"retained":true}},"oauth":{"access_token":"nested-access-canary","client_id":"public-client","refresh_token":"nested-refresh-canary"},"oauth.access_token":"flat-access-canary","oauth.refresh_token":"flat-refresh-canary","session":{"access_token":"session-access-canary","refresh_token":"session-refresh-canary","user_id":"account-a"},"session.access_token":"flat-session-access-canary","session.refresh_token":"flat-session-refresh-canary","unknown":{"secret":"retained-noncredential-canary"}}"#;
        AtomicFile::replace(&path, source).unwrap();
        let account_digest = CanonicalDigestV1::from_bytes(
            domain_hash(ACCOUNT_DIGEST_DOMAIN, &[b"account-a"]).unwrap(),
        );
        let expected = inspect_legacy_file_target(
            HouseholdTeardownLegacyTargetKindV1::CurrentConfigFile,
            &path,
            account_digest,
        )
        .unwrap();
        assert_eq!(
            expected.outcome,
            HouseholdTeardownLegacyTargetOutcomeV1::CredentialsPresent
        );
        let scrubbed = scrub_legacy_file_target(&expected, &path, account_digest).unwrap();
        assert_eq!(
            scrubbed.outcome,
            HouseholdTeardownLegacyTargetOutcomeV1::CurrentAccountScrubbed
        );
        assert_eq!(
            scrubbed.expected_noncredential_digest,
            expected.expected_noncredential_digest
        );
        let bytes = fs::read(&path).unwrap();
        let rendered = String::from_utf8(bytes.clone()).unwrap();
        for canary in [
            "api-canary",
            "nested-access-canary",
            "nested-refresh-canary",
            "flat-access-canary",
            "flat-refresh-canary",
            "session-access-canary",
            "session-refresh-canary",
            "flat-session-access-canary",
            "flat-session-refresh-canary",
        ] {
            assert!(!rendered.contains(canary));
        }
        assert!(rendered.contains("retained-noncredential-canary"));
        assert!(rendered.contains("public-client"));
        assert!(rendered.contains("\"retained\":true"));
        assert!(rendered.contains("\"user_id\":\"account-a\""));
        let verified = inspect_legacy_file_target(
            HouseholdTeardownLegacyTargetKindV1::CurrentConfigFile,
            &path,
            account_digest,
        )
        .unwrap();
        assert_eq!(verified, scrubbed);
    }

    #[cfg(feature = "native-credentials")]
    #[test]
    fn legacy_file_scrub_never_erases_foreign_or_ambiguous_credentials() {
        let root = TempRoot::new("legacy-file-binding");
        let path = root.0.join("legacy").join("config.json");
        let account_digest = CanonicalDigestV1::from_bytes(
            domain_hash(ACCOUNT_DIGEST_DOMAIN, &[b"account-a"]).unwrap(),
        );
        let foreign = br#"{"account_user_id":"account-b","api_key":"foreign-canary"}"#;
        AtomicFile::replace(&path, foreign).unwrap();
        let expected = inspect_legacy_file_target(
            HouseholdTeardownLegacyTargetKindV1::LegacyConfigFile,
            &path,
            account_digest,
        )
        .unwrap();
        assert_eq!(
            expected.outcome,
            HouseholdTeardownLegacyTargetOutcomeV1::ForeignAccount
        );
        let before = fs::read(&path).unwrap();
        assert_eq!(
            scrub_legacy_file_target(&expected, &path, account_digest)
                .unwrap()
                .outcome,
            HouseholdTeardownLegacyTargetOutcomeV1::ForeignAccount
        );
        assert_eq!(fs::read(&path).unwrap(), before);

        let ambiguous = br#"{"account_user_id":"account-a","api_key":"ambiguous-canary","session":{"user_id":"account-b"}}"#;
        AtomicFile::replace(&path, ambiguous).unwrap();
        let expected = inspect_legacy_file_target(
            HouseholdTeardownLegacyTargetKindV1::LegacyConfigFile,
            &path,
            account_digest,
        )
        .unwrap();
        assert_eq!(
            expected.outcome,
            HouseholdTeardownLegacyTargetOutcomeV1::Ambiguous
        );
        let before = fs::read(&path).unwrap();
        assert_eq!(
            scrub_legacy_file_target(&expected, &path, account_digest)
                .unwrap()
                .outcome,
            HouseholdTeardownLegacyTargetOutcomeV1::Ambiguous
        );
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[tokio::test]
    async fn account_switch_and_cancel_are_fail_closed() {
        let root = TempRoot::new("account-switch");
        let store = HouseholdTeardownJournalStoreV1::open(&root.0).unwrap();
        let backend = FakeBackend::new(prepared(&store, "account-a", false));
        backend.fail_once(FailStep::Guard);
        let coordinator = NativeAccountTeardownV1::new(&store, &backend);
        let _ = coordinator
            .execute_authenticated(&credentials("account-a"), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            coordinator
                .execute_authenticated(&credentials("account-b"), CancellationToken::new())
                .await
                .unwrap_err()
                .code,
            "household_teardown_resume_required"
        );

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert_eq!(
            coordinator.resume_all(cancellation).await.unwrap_err().code,
            "household_teardown_cancelled"
        );
    }
}
