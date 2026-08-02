//! Live account-bound repository adapter for the encrypted household vault.
//!
//! This adapter owns no migration or teardown policy. It consumes an exact
//! migration-guard/key transaction prepared by the audited startup path,
//! retains the lifecycle and vault leases in the required order, and delegates
//! semantic replay/conflict resolution to `heyfood-application`.

use std::{fmt, sync::Arc};

use heyfood_application::{
    BoxFuture, HouseholdCommit, HouseholdCommitEvidenceRepositoryPort, HouseholdCommitOutcome,
    HouseholdErase, HouseholdEraseOutcome, HouseholdInitialize, HouseholdLoad,
    HouseholdMutationAuthorityPort, HouseholdReadLeaseV1, HouseholdRepositoryPort,
    HouseholdRepositoryResolutionV1, HouseholdSession, NativeHouseholdModeV1, PortError,
    resolve_household_commit_v1, resolve_household_initialize_v1,
};
use heyfood_core::{
    AccountId, AgentHouseholdContractErrorV1, AgentHouseholdProposalIdV1, AppliedCommitOutcomeV1,
    AppliedHouseholdCommitProofV1, CommitId, HouseholdCommitEvidenceBindingV1,
    HouseholdEffectFingerprintV1, HouseholdRevision, HouseholdStateV1, LegacySourceIdentityV1,
    UnappliedHouseholdCommitProofV1, canonical_sha256_v1, decode_canonical_household_state_v1,
    domain_hash_v1,
};
use hkdf::Hkdf;
use sha2::Sha256;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use crate::household_vault::HouseholdVaultStartupArtifactsV1;
use crate::{
    HouseholdKeyBundle, HouseholdKeyBundlePhase, HouseholdKeyMaterial, HouseholdKeyStore,
    HouseholdMigrationGuardDocument, HouseholdMigrationGuardStateV1, HouseholdMigrationGuardStore,
    HouseholdMigrationInitializationPhaseV1, HouseholdSecureStore, HouseholdVault,
    HouseholdVaultLease, HouseholdVaultLeaseModeV1, HouseholdVaultLoad, HouseholdVaultWrite,
    KeyBundleRevision, KeyId, KeyStoreExpectation, MigrationGuardExpectation, NativePaths,
    household_teardown_barrier_present_v1,
};

const ACCOUNT_DIGEST_CONTRACT: &str = "heyfood.household.account-digest.v1";
const COMMIT_EVIDENCE_HKDF_SALT: &[u8] = b"heyfood.household.commit-evidence.hkdf.salt.v1";
const COMMIT_EVIDENCE_HKDF_INFO: &[u8] = b"heyfood.household.commit-evidence.capability.v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RepositoryAccessV1 {
    ReadWrite,
    ReadOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReadyInitializationKeyV1 {
    InitializingRevisionOne,
    StableRevisionTwo,
}

/// Concrete encrypted implementation of the application household repository
/// port.
///
/// Construction accepts only the two usable committed native modes. Startup
/// and migration keep responsibility for creating the immutable guard tuple;
/// `initialize` can only finish that exact already-reserved transaction.
#[derive(Clone)]
pub struct NativeHouseholdRepository {
    account: AccountId,
    vault: HouseholdVault,
    secure_store: Arc<dyn HouseholdSecureStore>,
    access: RepositoryAccessV1,
}

impl NativeHouseholdRepository {
    pub fn from_native_paths(
        paths: &NativePaths,
        account: AccountId,
        secure_store: Arc<dyn HouseholdSecureStore>,
        mode: NativeHouseholdModeV1,
    ) -> Result<Self, PortError> {
        let vault = HouseholdVault::from_native_paths(paths, account.clone())?;
        Self::from_vault(account, vault, secure_store, mode)
    }

    pub fn from_vault(
        account: AccountId,
        vault: HouseholdVault,
        secure_store: Arc<dyn HouseholdSecureStore>,
        mode: NativeHouseholdModeV1,
    ) -> Result<Self, PortError> {
        let access = match mode {
            NativeHouseholdModeV1::NativeEnabled => RepositoryAccessV1::ReadWrite,
            NativeHouseholdModeV1::NativeRollbackReadOnly => RepositoryAccessV1::ReadOnly,
            _ => {
                return Err(PortError::new(
                    "household_repository_mode",
                    "native household repository requires a usable committed native mode",
                ));
            }
        };
        let expected_account_digest =
            domain_hash_v1(ACCOUNT_DIGEST_CONTRACT, &[account.as_str().as_bytes()])
                .map_err(state_error)?;
        if vault.account_slot().account_digest() != *expected_account_digest.as_bytes() {
            return Err(account_mismatch_error());
        }
        Ok(Self {
            account,
            vault,
            secure_store,
            access,
        })
    }

    #[must_use]
    pub fn account(&self) -> &AccountId {
        &self.account
    }

    /// Reserve the opaque verifier for one exact future proposal commit.
    /// The corresponding secret is securely rederived from the native
    /// repository key after restart. It is never exposed as data; a successful
    /// observation carries it only inside a redacted, zeroizing proof.
    pub async fn reserve_agent_commit_evidence(
        &self,
        proposal_ref: AgentHouseholdProposalIdV1,
        commit_id: CommitId,
        cancellation: CancellationToken,
    ) -> Result<HouseholdCommitEvidenceBindingV1, PortError> {
        let mut vault_lease = self
            .acquire_vault_lease(HouseholdVaultLeaseModeV1::RequireExisting, &cancellation)
            .await?;
        let (guard, key) = self
            .reread_guard_and_key(&vault_lease, &cancellation)
            .await?;
        let _ = self
            .load_committed_under_lease(&mut vault_lease, &guard, &key, cancellation.clone())
            .await?;
        check_cancelled(&cancellation)?;
        let secret =
            derive_commit_evidence_secret(&key.active_key, &self.account, proposal_ref, commit_id)?;
        Ok(HouseholdCommitEvidenceBindingV1::from_repository_secret(
            self.account.clone(),
            proposal_ref,
            commit_id,
            &secret,
        ))
    }

    /// Reopen the authoritative repository and prove that the exact commit is
    /// present in its authenticated applied-commit ledger. No caller-provided
    /// household state participates in this decision.
    pub async fn prove_applied_agent_commit(
        &self,
        binding: &HouseholdCommitEvidenceBindingV1,
        proposal_ref: AgentHouseholdProposalIdV1,
        commit_id: CommitId,
        cancellation: CancellationToken,
    ) -> Result<AppliedHouseholdCommitProofV1, PortError> {
        let (state, secret) = self
            .load_commit_evidence_state(proposal_ref, commit_id, cancellation)
            .await?;
        let expected_binding = HouseholdCommitEvidenceBindingV1::from_repository_secret(
            self.account.clone(),
            proposal_ref,
            commit_id,
            &secret,
        );
        if &expected_binding != binding {
            return Err(commit_evidence_mismatch_error());
        }
        let record = state
            .bounded_applied_commits
            .iter()
            .find(|record| {
                record.commit_id == commit_id && record.outcome == AppliedCommitOutcomeV1::Committed
            })
            .ok_or_else(commit_evidence_mismatch_error)?;
        binding
            .seal_applied_repository_observation(
                &secret,
                HouseholdEffectFingerprintV1::from_digest(record.fingerprint),
                record.resulting_revision,
            )
            .map_err(commit_evidence_contract_error)
    }

    /// Reopen the authoritative repository and prove exact absence only while
    /// its revision remains the frozen pre-dispatch revision.
    pub async fn prove_unapplied_agent_commit(
        &self,
        binding: &HouseholdCommitEvidenceBindingV1,
        proposal_ref: AgentHouseholdProposalIdV1,
        commit_id: CommitId,
        expected_revision: HouseholdRevision,
        cancellation: CancellationToken,
    ) -> Result<UnappliedHouseholdCommitProofV1, PortError> {
        let (state, secret) = self
            .load_commit_evidence_state(proposal_ref, commit_id, cancellation)
            .await?;
        let expected_binding = HouseholdCommitEvidenceBindingV1::from_repository_secret(
            self.account.clone(),
            proposal_ref,
            commit_id,
            &secret,
        );
        if &expected_binding != binding
            || state.revision != expected_revision
            || state
                .bounded_applied_commits
                .iter()
                .any(|record| record.commit_id == commit_id)
        {
            return Err(commit_evidence_mismatch_error());
        }
        binding
            .seal_unapplied_repository_observation(&secret, state.revision)
            .map_err(commit_evidence_contract_error)
    }

    async fn load_commit_evidence_state(
        &self,
        proposal_ref: AgentHouseholdProposalIdV1,
        commit_id: CommitId,
        cancellation: CancellationToken,
    ) -> Result<(HouseholdStateV1, Zeroizing<[u8; 32]>), PortError> {
        check_cancelled(&cancellation)?;
        let mut vault_lease = self
            .acquire_vault_lease(HouseholdVaultLeaseModeV1::RequireExisting, &cancellation)
            .await?;
        let (guard, key) = self
            .reread_guard_and_key(&vault_lease, &cancellation)
            .await?;
        let loaded = self
            .load_committed_under_lease(&mut vault_lease, &guard, &key, cancellation.clone())
            .await?;
        check_cancelled(&cancellation)?;
        let secret =
            derive_commit_evidence_secret(&key.active_key, &self.account, proposal_ref, commit_id)?;
        Ok((loaded.state, secret))
    }

    /// Wrap this concrete adapter in the live application session without
    /// caching any household state.
    #[must_use]
    pub fn into_session(
        self,
        mutation_authority: Arc<dyn HouseholdMutationAuthorityPort>,
    ) -> HouseholdSession {
        let account = self.account.clone();
        let repository: Arc<dyn HouseholdRepositoryPort> = Arc::new(self);
        HouseholdSession::new(account, repository, mutation_authority)
    }

    /// Build a live session while retaining a concrete repository handle.
    #[must_use]
    pub fn session(
        self: &Arc<Self>,
        mutation_authority: Arc<dyn HouseholdMutationAuthorityPort>,
    ) -> HouseholdSession {
        let repository: Arc<dyn HouseholdRepositoryPort> = self.clone();
        HouseholdSession::new(self.account.clone(), repository, mutation_authority)
    }

    async fn acquire_vault_lease(
        &self,
        mode: HouseholdVaultLeaseModeV1,
        cancellation: &CancellationToken,
    ) -> Result<HouseholdVaultLease, PortError> {
        check_cancelled(cancellation)?;
        let lifecycle = self
            .vault
            .acquire_lifecycle_lease(cancellation.clone())
            .await?;
        check_cancelled(cancellation)?;
        if household_teardown_barrier_present_v1(&self.vault, &lifecycle)? {
            return Err(teardown_in_progress_error());
        }
        check_cancelled(cancellation)?;
        self.vault
            .acquire_vault_lease(lifecycle, mode, cancellation.clone())
            .await
    }

    async fn reread_guard_and_optional_key(
        &self,
        vault_lease: &HouseholdVaultLease,
        cancellation: &CancellationToken,
    ) -> Result<(HouseholdMigrationGuardDocument, Option<HouseholdKeyBundle>), PortError> {
        check_cancelled(cancellation)?;
        let guard = HouseholdMigrationGuardStore::load(
            self.secure_store.as_ref(),
            vault_lease.lifecycle_lease(),
            cancellation.clone(),
        )
        .await?
        .ok_or_else(|| {
            PortError::new(
                "household_initialization_protocol_required",
                "native household state requires its account-bound migration guard",
            )
        })?;
        check_cancelled(cancellation)?;
        let key = HouseholdKeyStore::load(
            self.secure_store.as_ref(),
            vault_lease.lifecycle_lease(),
            cancellation.clone(),
        )
        .await?;
        guard.validate_for(self.vault.account_slot())?;
        if let Some(key) = &key {
            key.validate_for(self.vault.account_slot())?;
        }
        check_cancelled(cancellation)?;
        Ok((guard, key))
    }

    async fn reread_guard_and_key(
        &self,
        vault_lease: &HouseholdVaultLease,
        cancellation: &CancellationToken,
    ) -> Result<(HouseholdMigrationGuardDocument, HouseholdKeyBundle), PortError> {
        let (guard, key) = self
            .reread_guard_and_optional_key(vault_lease, cancellation)
            .await?;
        let key = key.ok_or_else(missing_key_error)?;
        Ok((guard, key))
    }

    fn validate_ready_initialization_key(
        &self,
        guard: &HouseholdMigrationGuardDocument,
        key: &HouseholdKeyBundle,
    ) -> Result<ReadyInitializationKeyV1, PortError> {
        match key.phase {
            HouseholdKeyBundlePhase::Initializing => {
                key.validate_initial_for(self.vault.account_slot(), guard)?;
                Ok(ReadyInitializationKeyV1::InitializingRevisionOne)
            }
            HouseholdKeyBundlePhase::Stable if key.revision.get() == 2 => {
                key.validate_for(self.vault.account_slot())?;
                Ok(ReadyInitializationKeyV1::StableRevisionTwo)
            }
            HouseholdKeyBundlePhase::Stable | HouseholdKeyBundlePhase::Rewriting => {
                Err(initialization_protocol_error())
            }
        }
    }

    async fn load_committed_under_lease(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        guard: &HouseholdMigrationGuardDocument,
        key: &HouseholdKeyBundle,
        cancellation: CancellationToken,
    ) -> Result<HouseholdLoad, PortError> {
        require_committed_guard(guard)?;
        if key.phase != HouseholdKeyBundlePhase::Stable {
            return Err(PortError::new(
                "household_key_phase",
                "committed household state requires a stable key bundle",
            ));
        }
        check_cancelled(&cancellation)?;
        let loaded = self
            .vault
            .load(vault_lease, key.clone(), cancellation)
            .await?;
        self.decode_and_verify_load(loaded, guard)
    }

    fn decode_and_verify_load(
        &self,
        loaded: HouseholdVaultLoad,
        guard: &HouseholdMigrationGuardDocument,
    ) -> Result<HouseholdLoad, PortError> {
        let state =
            decode_canonical_household_state_v1(&loaded.canonical_state).map_err(state_error)?;
        if state.account_binding != self.account
            || state.revision.get() != loaded.state_revision
            || !state.bounded_applied_commits.iter().any(|record| {
                record.commit_id.as_uuid() == loaded.commit_id
                    && record.resulting_revision == state.revision
            })
        {
            return Err(PortError::new(
                "household_vault_state_mismatch",
                "household vault state does not match its authenticated envelope",
            ));
        }
        validate_guard_provenance(guard, &state)?;
        let load = HouseholdLoad::from_state(state)?;
        if load.state_digest.as_bytes() != &loaded.plaintext_sha256() {
            return Err(PortError::new(
                "household_vault_digest_mismatch",
                "household vault state digest does not match its authenticated plaintext",
            ));
        }
        Ok(load)
    }

    fn verify_written_state(
        &self,
        loaded: HouseholdVaultLoad,
        guard: &HouseholdMigrationGuardDocument,
        expected_state: &HouseholdStateV1,
        expected_commit: CommitId,
        expected_outcome: HouseholdCommitOutcome,
    ) -> Result<(), PortError> {
        if loaded.state_revision != expected_outcome.resulting_revision.get()
            || loaded.commit_id != expected_commit.as_uuid()
        {
            return Err(PortError::uncertain(
                "household_vault_commit_verify",
                "household vault commit identity requires reconciliation",
            ));
        }
        let verified = self.decode_and_verify_load(loaded, guard)?;
        if verified.state != *expected_state
            || verified.state.revision != expected_outcome.resulting_revision
        {
            return Err(PortError::uncertain(
                "household_vault_commit_verify",
                "household vault commit result requires reconciliation",
            ));
        }
        let record = verified
            .state
            .bounded_applied_commits
            .iter()
            .find(|record| record.commit_id == expected_commit);
        if !record.is_some_and(|record| {
            record.resulting_revision == expected_outcome.resulting_revision
                && record.outcome == expected_outcome.outcome
        }) {
            return Err(PortError::uncertain(
                "household_vault_commit_verify",
                "household vault commit ledger requires reconciliation",
            ));
        }
        Ok(())
    }

    async fn finalize_initialization(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        guard: HouseholdMigrationGuardDocument,
        key: HouseholdKeyBundle,
        cancellation: CancellationToken,
    ) -> Result<HouseholdMigrationGuardDocument, PortError> {
        let stable_key = match key.phase {
            HouseholdKeyBundlePhase::Initializing => {
                check_cancelled(&cancellation)?;
                let replacement = HouseholdKeyBundle::stable(
                    self.vault.account_slot(),
                    key.revision.checked_next()?,
                    key.active_key_id,
                    key.active_key.clone(),
                );
                let exchange = HouseholdKeyStore::compare_exchange(
                    self.secure_store.as_ref(),
                    vault_lease,
                    key.revision,
                    replacement.clone(),
                    cancellation.clone(),
                )
                .await;
                let observed = HouseholdKeyStore::load(
                    self.secure_store.as_ref(),
                    vault_lease.lifecycle_lease(),
                    CancellationToken::new(),
                )
                .await?;
                match (exchange, observed.as_ref()) {
                    (_, Some(observed)) if observed == &replacement => {}
                    (Err(error), Some(observed)) if observed == &key => return Err(error),
                    _ => {
                        return Err(PortError::uncertain(
                            "household_key_finalize",
                            "household initialization key finalization requires reconciliation",
                        ));
                    }
                }
                replacement
            }
            HouseholdKeyBundlePhase::Stable => key,
            HouseholdKeyBundlePhase::Rewriting => {
                return Err(PortError::new(
                    "household_key_phase",
                    "household initialization cannot finalize a rewriting key bundle",
                ));
            }
        };
        stable_key.validate_for(self.vault.account_slot())?;

        let completed_guard = match guard.state() {
            HouseholdMigrationGuardStateV1::Initializing => {
                check_cancelled(&cancellation)?;
                let replacement = guard.complete_initialization()?;
                let exchange = HouseholdMigrationGuardStore::compare_exchange(
                    self.secure_store.as_ref(),
                    vault_lease,
                    MigrationGuardExpectation::Revision(guard.guard_revision()),
                    Some(replacement.clone()),
                    cancellation,
                )
                .await;
                let observed = HouseholdMigrationGuardStore::load(
                    self.secure_store.as_ref(),
                    vault_lease.lifecycle_lease(),
                    CancellationToken::new(),
                )
                .await?;
                match (exchange, observed.as_ref()) {
                    (_, Some(observed)) if observed == &replacement => {}
                    (Err(error), Some(observed)) if observed == &guard => return Err(error),
                    _ => {
                        return Err(PortError::uncertain(
                            "household_guard_finalize",
                            "household initialization guard finalization requires reconciliation",
                        ));
                    }
                }
                replacement
            }
            HouseholdMigrationGuardStateV1::Migrated
            | HouseholdMigrationGuardStateV1::InitializedNoSource => guard,
            _ => {
                return Err(PortError::new(
                    "household_guard_state",
                    "household initialization guard is not eligible for finalization",
                ));
            }
        };
        Ok(completed_guard)
    }

    async fn ensure_initializing_key(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        guard: &HouseholdMigrationGuardDocument,
        observed: Option<HouseholdKeyBundle>,
        state_digest: [u8; 32],
        cancellation: &CancellationToken,
    ) -> Result<HouseholdKeyBundle, PortError> {
        if let Some(key) = observed {
            return Ok(key);
        }
        let artifacts = self
            .vault
            .classify_startup_artifacts(
                vault_lease,
                None,
                Some(guard.initial_commit_id()),
                Some(state_digest),
                cancellation.clone(),
            )
            .await?;
        if artifacts != HouseholdVaultStartupArtifactsV1::Absent {
            return Err(initialization_protocol_error());
        }

        let candidate = HouseholdKeyBundle::initializing(
            self.vault.account_slot(),
            KeyBundleRevision::new(1)?,
            KeyId::new(),
            HouseholdKeyMaterial::generate()?,
            guard.initialization_id(),
            guard.initial_commit_id(),
            guard
                .initial_effect_fingerprint()
                .ok_or_else(initialization_protocol_error)?,
            state_digest,
        );
        let initialize = HouseholdKeyStore::initialize(
            self.secure_store.as_ref(),
            vault_lease,
            KeyStoreExpectation::Absent,
            guard.clone(),
            candidate.clone(),
            cancellation.clone(),
        )
        .await;
        let reloaded = HouseholdKeyStore::load(
            self.secure_store.as_ref(),
            vault_lease.lifecycle_lease(),
            CancellationToken::new(),
        )
        .await?;
        match (initialize, reloaded) {
            (_, Some(observed)) if observed == candidate => Ok(candidate),
            (Err(error), None) => Err(error),
            (Ok(()), None) => Err(PortError::uncertain(
                "household_key_initialize",
                "household key initialization requires reconciliation",
            )),
            (_, Some(_)) => Err(initialization_protocol_error()),
        }
    }

    async fn load_exact_committed_initialization(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        guard: &HouseholdMigrationGuardDocument,
        key: &HouseholdKeyBundle,
        state_digest: [u8; 32],
        cancellation: CancellationToken,
    ) -> Result<HouseholdVaultLoad, PortError> {
        let topology = self
            .vault
            .classify_startup_artifacts(
                vault_lease,
                Some(key.clone()),
                Some(guard.initial_commit_id()),
                Some(state_digest),
                cancellation.clone(),
            )
            .await?;
        if topology != HouseholdVaultStartupArtifactsV1::MatchingCommitted {
            return Err(initialization_protocol_error());
        }
        self.vault
            .load(vault_lease, key.clone(), cancellation)
            .await
    }

    async fn initialize_vault_or_reconcile(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        guard: &HouseholdMigrationGuardDocument,
        key: &HouseholdKeyBundle,
        write: HouseholdVaultWrite,
        state_digest: [u8; 32],
        cancellation: CancellationToken,
    ) -> Result<HouseholdVaultLoad, PortError> {
        match self
            .vault
            .initialize(vault_lease, key.clone(), write, cancellation.clone())
            .await
        {
            Ok(loaded) => Ok(loaded),
            Err(error) if error.outcome_uncertain => {
                match self
                    .load_exact_committed_initialization(
                        vault_lease,
                        guard,
                        key,
                        state_digest,
                        CancellationToken::new(),
                    )
                    .await
                {
                    Ok(loaded) => Ok(loaded),
                    Err(_) => Err(error),
                }
            }
            Err(error) => Err(error),
        }
    }

    async fn initialize_inner(
        &self,
        command: HouseholdInitialize,
        cancellation: CancellationToken,
    ) -> Result<HouseholdCommitOutcome, PortError> {
        self.require_account(&command.account)?;
        let mut vault_lease = self
            .acquire_vault_lease(HouseholdVaultLeaseModeV1::CreateIfMissing, &cancellation)
            .await?;
        self.initialize_with_retained_leases(command, &mut vault_lease, cancellation)
            .await
    }

    /// Finish an exact prepared initialization while the startup coordinator
    /// retains the account lifecycle, legacy-source, and vault lock order.
    ///
    /// This is deliberately crate-internal. Ordinary repository callers use
    /// `initialize`, which acquires the same leases before delegating here.
    /// The retained lease is revalidated against this repository before any
    /// guard/key read so a foreign account or native root cannot borrow the
    /// initialization algorithm.
    pub(crate) async fn initialize_with_retained_leases(
        &self,
        command: HouseholdInitialize,
        vault_lease: &mut HouseholdVaultLease,
        cancellation: CancellationToken,
    ) -> Result<HouseholdCommitOutcome, PortError> {
        self.require_account(&command.account)?;
        vault_lease.validate_for(self.vault.account_slot())?;
        if household_teardown_barrier_present_v1(&self.vault, vault_lease.lifecycle_lease())? {
            return Err(teardown_in_progress_error());
        }
        check_cancelled(&cancellation)?;
        let (guard, observed_key) = self
            .reread_guard_and_optional_key(vault_lease, &cancellation)
            .await?;

        match guard.state() {
            HouseholdMigrationGuardStateV1::Initializing => {
                if guard.initial_effect_fingerprint()
                    != Some(*command.claimed_effect_fingerprint.as_digest().as_bytes())
                {
                    return Err(PortError::new(
                        "household_initialization_guard_mismatch",
                        "household initialization command does not match its ready guard",
                    ));
                }
                let resolution = resolve_household_initialize_v1(None, &command)?;
                let HouseholdRepositoryResolutionV1::Write { state, outcome } = resolution else {
                    return Err(PortError::new(
                        "household_initialization_resolution",
                        "new household initialization did not produce a vault state",
                    ));
                };
                validate_guard_provenance(&guard, &state)?;
                let canonical = state.canonical_bytes().map_err(state_error)?;
                let state_digest = *canonical_sha256_v1(state.as_ref())
                    .map_err(state_error)?
                    .as_bytes();
                if guard.initial_state_digest() != Some(state_digest) {
                    return Err(PortError::new(
                        "household_initialization_protocol_required",
                        "household initialization requires the exact ready guard and key transaction",
                    ));
                }
                let key = self
                    .ensure_initializing_key(
                        vault_lease,
                        &guard,
                        observed_key,
                        state_digest,
                        &cancellation,
                    )
                    .await?;
                let key_state = self.validate_ready_initialization_key(&guard, &key)?;
                let topology = self
                    .vault
                    .classify_startup_artifacts(
                        vault_lease,
                        Some(key.clone()),
                        Some(guard.initial_commit_id()),
                        Some(state_digest),
                        cancellation.clone(),
                    )
                    .await?;
                if topology == HouseholdVaultStartupArtifactsV1::MatchingCommitted {
                    let loaded = self
                        .vault
                        .load(vault_lease, key.clone(), cancellation.clone())
                        .await?;
                    let current = self.decode_and_verify_load(loaded, &guard)?;
                    if current.state_digest.as_bytes() != &state_digest {
                        return Err(PortError::new(
                            "household_initialization_guard_mismatch",
                            "committed household initialization does not match its ready guard",
                        ));
                    }
                    let HouseholdRepositoryResolutionV1::Replay(replayed) =
                        resolve_household_initialize_v1(Some(&current.state), &command)?
                    else {
                        return Err(PortError::new(
                            "household_initialization_conflict",
                            "committed household initialization does not match its command",
                        ));
                    };
                    let completed = self
                        .finalize_initialization(vault_lease, guard, key, cancellation.clone())
                        .await?;
                    let stable = HouseholdKeyStore::load(
                        self.secure_store.as_ref(),
                        vault_lease.lifecycle_lease(),
                        CancellationToken::new(),
                    )
                    .await?
                    .ok_or_else(|| {
                        PortError::uncertain(
                            "household_key_finalize",
                            "household initialization key finalization requires reconciliation",
                        )
                    })?;
                    let final_load = self
                        .load_committed_under_lease(
                            vault_lease,
                            &completed,
                            &stable,
                            CancellationToken::new(),
                        )
                        .await?;
                    if final_load.state != current.state {
                        return Err(PortError::uncertain(
                            "household_initialization_verify",
                            "household initialization finalization requires reconciliation",
                        ));
                    }
                    return Ok(replayed);
                }
                if !matches!(
                    (key_state, topology),
                    (
                        ReadyInitializationKeyV1::InitializingRevisionOne,
                        HouseholdVaultStartupArtifactsV1::Absent
                            | HouseholdVaultStartupArtifactsV1::MatchingUncommitted
                    )
                ) {
                    return Err(initialization_protocol_error());
                }
                check_cancelled(&cancellation)?;
                let write = HouseholdVaultWrite::new(
                    state.revision.get(),
                    command.commit_id.as_uuid(),
                    canonical,
                )?;
                let loaded = self
                    .initialize_vault_or_reconcile(
                        vault_lease,
                        &guard,
                        &key,
                        write,
                        state_digest,
                        cancellation.clone(),
                    )
                    .await?;
                self.verify_written_state(loaded, &guard, &state, command.commit_id, outcome)?;
                let completed = self
                    .finalize_initialization(vault_lease, guard, key, cancellation.clone())
                    .await?;
                let stable = HouseholdKeyStore::load(
                    self.secure_store.as_ref(),
                    vault_lease.lifecycle_lease(),
                    CancellationToken::new(),
                )
                .await?
                .ok_or_else(|| {
                    PortError::uncertain(
                        "household_key_finalize",
                        "household initialization key finalization requires reconciliation",
                    )
                })?;
                let final_load = self
                    .load_committed_under_lease(
                        vault_lease,
                        &completed,
                        &stable,
                        CancellationToken::new(),
                    )
                    .await?;
                if final_load.state != *state {
                    return Err(PortError::uncertain(
                        "household_initialization_verify",
                        "household initialization finalization requires reconciliation",
                    ));
                }
                Ok(outcome)
            }
            HouseholdMigrationGuardStateV1::Migrated
            | HouseholdMigrationGuardStateV1::InitializedNoSource => {
                let key = observed_key.ok_or_else(missing_key_error)?;
                let current = self
                    .load_committed_under_lease(vault_lease, &guard, &key, cancellation)
                    .await?;
                match resolve_household_initialize_v1(Some(&current.state), &command)? {
                    HouseholdRepositoryResolutionV1::Replay(outcome) => Ok(outcome),
                    HouseholdRepositoryResolutionV1::Write { .. } => Err(PortError::new(
                        "household_initialization_conflict",
                        "household initialization cannot replace committed state",
                    )),
                }
            }
            _ => Err(PortError::new(
                "household_initialization_protocol_required",
                "household initialization requires an exact ready or committed guard transaction",
            )),
        }
    }

    /// Authenticated committed readback for the startup coordinator while it
    /// retains the same lifecycle/source/vault lock transaction.
    pub(crate) async fn load_committed_with_retained_leases(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        cancellation: CancellationToken,
    ) -> Result<HouseholdLoad, PortError> {
        vault_lease.validate_for(self.vault.account_slot())?;
        if household_teardown_barrier_present_v1(&self.vault, vault_lease.lifecycle_lease())? {
            return Err(teardown_in_progress_error());
        }
        check_cancelled(&cancellation)?;
        let (guard, key) = self
            .reread_guard_and_key(vault_lease, &cancellation)
            .await?;
        self.load_committed_under_lease(vault_lease, &guard, &key, cancellation)
            .await
    }

    /// Finalize an initialization whose exact canonical generation already
    /// committed before startup crashed.
    ///
    /// This path intentionally accepts no source candidate or initialization
    /// command. It authenticates the ready guard, key, journal/generations,
    /// plaintext digest, migration provenance, and initial applied-commit
    /// record under the retained lifecycle/vault transaction, then performs
    /// only the idempotent key/guard finalization. Callers must not re-read
    /// legacy config or keyring sources once this row is classified.
    pub(crate) async fn finalize_committed_initialization_with_retained_leases(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        cancellation: CancellationToken,
    ) -> Result<HouseholdLoad, PortError> {
        vault_lease.validate_for(self.vault.account_slot())?;
        if household_teardown_barrier_present_v1(&self.vault, vault_lease.lifecycle_lease())? {
            return Err(teardown_in_progress_error());
        }
        check_cancelled(&cancellation)?;
        let (guard, key) = self
            .reread_guard_and_key(vault_lease, &cancellation)
            .await?;
        if guard.state() != HouseholdMigrationGuardStateV1::Initializing
            || guard.initialization_phase()
                != Some(HouseholdMigrationInitializationPhaseV1::ReadyToInitialize)
        {
            return Err(initialization_protocol_error());
        }
        let expected_state_digest = guard
            .initial_state_digest()
            .ok_or_else(initialization_protocol_error)?;
        self.validate_ready_initialization_key(&guard, &key)?;
        let committed = self
            .load_exact_committed_initialization(
                vault_lease,
                &guard,
                &key,
                expected_state_digest,
                cancellation.clone(),
            )
            .await?;
        let authenticated = self.decode_and_verify_load(committed, &guard)?;
        if authenticated.state_digest.as_bytes() != &expected_state_digest {
            return Err(initialization_protocol_error());
        }

        let completed = self
            .finalize_initialization(vault_lease, guard, key, cancellation)
            .await?;
        let stable = HouseholdKeyStore::load(
            self.secure_store.as_ref(),
            vault_lease.lifecycle_lease(),
            CancellationToken::new(),
        )
        .await?
        .ok_or_else(|| {
            PortError::uncertain(
                "household_key_finalize",
                "household initialization key finalization requires reconciliation",
            )
        })?;
        let readback = self
            .load_committed_under_lease(vault_lease, &completed, &stable, CancellationToken::new())
            .await?;
        if readback != authenticated {
            return Err(PortError::uncertain(
                "household_initialization_verify",
                "household initialization finalization requires reconciliation",
            ));
        }
        Ok(readback)
    }

    /// Resume generation/journal creation only from an authenticated
    /// generation-0 initialization artifact, without consulting legacy
    /// config, keyring, or another source candidate.
    pub(crate) async fn resume_uncommitted_initialization_with_retained_leases(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        cancellation: CancellationToken,
    ) -> Result<HouseholdLoad, PortError> {
        vault_lease.validate_for(self.vault.account_slot())?;
        if household_teardown_barrier_present_v1(&self.vault, vault_lease.lifecycle_lease())? {
            return Err(teardown_in_progress_error());
        }
        check_cancelled(&cancellation)?;
        let (guard, key) = self
            .reread_guard_and_key(vault_lease, &cancellation)
            .await?;
        if guard.state() != HouseholdMigrationGuardStateV1::Initializing
            || guard.initialization_phase()
                != Some(HouseholdMigrationInitializationPhaseV1::ReadyToInitialize)
            || key.phase != HouseholdKeyBundlePhase::Initializing
        {
            return Err(initialization_protocol_error());
        }
        let expected_state_digest = guard
            .initial_state_digest()
            .ok_or_else(initialization_protocol_error)?;
        self.validate_ready_initialization_key(&guard, &key)?;
        let write = self
            .vault
            .recover_uncommitted_initialization_write(
                vault_lease,
                key.clone(),
                guard.clone(),
                cancellation.clone(),
            )
            .await?;
        let committed = self
            .vault
            .initialize(vault_lease, key.clone(), write, cancellation.clone())
            .await?;
        let authenticated = self.decode_and_verify_load(committed, &guard)?;
        if authenticated.state_digest.as_bytes() != &expected_state_digest {
            return Err(initialization_protocol_error());
        }

        let completed = self
            .finalize_initialization(vault_lease, guard, key, cancellation)
            .await?;
        let stable = HouseholdKeyStore::load(
            self.secure_store.as_ref(),
            vault_lease.lifecycle_lease(),
            CancellationToken::new(),
        )
        .await?
        .ok_or_else(|| {
            PortError::uncertain(
                "household_key_finalize",
                "household initialization key finalization requires reconciliation",
            )
        })?;
        let readback = self
            .load_committed_under_lease(vault_lease, &completed, &stable, CancellationToken::new())
            .await?;
        if readback != authenticated {
            return Err(PortError::uncertain(
                "household_initialization_verify",
                "household initialization finalization requires reconciliation",
            ));
        }
        Ok(readback)
    }

    async fn commit_inner(
        &self,
        command: HouseholdCommit,
        cancellation: CancellationToken,
    ) -> Result<HouseholdCommitOutcome, PortError> {
        self.require_account(&command.account)?;
        let mut vault_lease = self
            .acquire_vault_lease(HouseholdVaultLeaseModeV1::RequireExisting, &cancellation)
            .await?;
        if self.access != RepositoryAccessV1::ReadWrite {
            return Err(read_only_error());
        }
        let (guard, key) = self
            .reread_guard_and_key(&vault_lease, &cancellation)
            .await?;
        let current = self
            .load_committed_under_lease(&mut vault_lease, &guard, &key, cancellation.clone())
            .await?;
        check_cancelled(&cancellation)?;
        match resolve_household_commit_v1(Some(&current.state), &command)? {
            HouseholdRepositoryResolutionV1::Replay(outcome) => Ok(outcome),
            HouseholdRepositoryResolutionV1::Write { state, outcome } => {
                let canonical = state.canonical_bytes().map_err(state_error)?;
                check_cancelled(&cancellation)?;
                let write = HouseholdVaultWrite::new(
                    state.revision.get(),
                    command.commit_id.as_uuid(),
                    canonical,
                )?;
                let loaded = self
                    .vault
                    .commit(
                        &mut vault_lease,
                        key,
                        current.state.revision.get(),
                        write,
                        cancellation,
                    )
                    .await?;
                self.verify_written_state(loaded, &guard, &state, command.commit_id, outcome)?;
                Ok(outcome)
            }
        }
    }

    fn require_account(&self, account: &AccountId) -> Result<(), PortError> {
        if account != &self.account {
            return Err(account_mismatch_error());
        }
        Ok(())
    }
}

impl fmt::Debug for NativeHouseholdRepository {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeHouseholdRepository")
            .field(
                "account_digest",
                &self.vault.account_slot().account_digest(),
            )
            .field("access", &self.access)
            .finish_non_exhaustive()
    }
}

impl HouseholdRepositoryPort for NativeHouseholdRepository {
    fn load<'a>(
        &'a self,
        account: &'a AccountId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<Option<HouseholdLoad>, PortError>> {
        Box::pin(async move {
            self.require_account(account)?;
            let mut vault_lease = self
                .acquire_vault_lease(HouseholdVaultLeaseModeV1::RequireExisting, &cancellation)
                .await?;
            let (guard, key) = self
                .reread_guard_and_optional_key(&vault_lease, &cancellation)
                .await?;
            if guard.state() == HouseholdMigrationGuardStateV1::Initializing {
                if guard.initialization_phase()
                    != Some(HouseholdMigrationInitializationPhaseV1::ReadyToInitialize)
                {
                    return Err(initialization_protocol_error());
                }
                match key {
                    Some(key) => {
                        let key_state = self.validate_ready_initialization_key(&guard, &key)?;
                        let topology = self
                            .vault
                            .classify_startup_artifacts(
                                &mut vault_lease,
                                Some(key.clone()),
                                Some(guard.initial_commit_id()),
                                guard.initial_state_digest(),
                                cancellation,
                            )
                            .await?;
                        if key_state == ReadyInitializationKeyV1::StableRevisionTwo
                            && topology != HouseholdVaultStartupArtifactsV1::MatchingCommitted
                        {
                            return Err(initialization_protocol_error());
                        }
                    }
                    None => {
                        let topology = self
                            .vault
                            .classify_startup_artifacts(
                                &mut vault_lease,
                                None,
                                Some(guard.initial_commit_id()),
                                guard.initial_state_digest(),
                                cancellation,
                            )
                            .await?;
                        if topology != HouseholdVaultStartupArtifactsV1::Absent {
                            return Err(initialization_protocol_error());
                        }
                    }
                }
                // Never expose unfinalized state. The caller's exact prepared
                // command re-enters `initialize`, which verifies or finishes
                // this same guard/key/vault transaction.
                return Ok(None);
            }
            let key = key.ok_or_else(missing_key_error)?;
            let loaded = self
                .load_committed_under_lease(&mut vault_lease, &guard, &key, cancellation)
                .await?;
            Ok(Some(loaded))
        })
    }

    fn acquire_read_lease<'a>(
        &'a self,
        account: &'a AccountId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdReadLeaseV1, PortError>> {
        Box::pin(async move {
            self.require_account(account)?;
            let mut vault_lease = self
                .acquire_vault_lease(HouseholdVaultLeaseModeV1::RequireExisting, &cancellation)
                .await?;
            let (guard, key) = self
                .reread_guard_and_key(&vault_lease, &cancellation)
                .await?;
            let load = self
                .load_committed_under_lease(&mut vault_lease, &guard, &key, cancellation)
                .await?;
            Ok(HouseholdReadLeaseV1::new(load, Box::new(vault_lease)))
        })
    }

    fn initialize<'a>(
        &'a self,
        command: HouseholdInitialize,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdCommitOutcome, PortError>> {
        Box::pin(self.initialize_inner(command, cancellation))
    }

    fn commit<'a>(
        &'a self,
        command: HouseholdCommit,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdCommitOutcome, PortError>> {
        Box::pin(self.commit_inner(command, cancellation))
    }

    fn erase_account<'a>(
        &'a self,
        command: HouseholdErase,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdEraseOutcome, PortError>> {
        Box::pin(async move {
            self.require_account(&command.account)?;
            check_cancelled(&cancellation)?;
            Err(PortError::new(
                "household_account_teardown_required",
                "household account erasure requires the audited resumable account teardown coordinator",
            ))
        })
    }
}

impl HouseholdCommitEvidenceRepositoryPort for NativeHouseholdRepository {
    fn reserve_agent_commit_evidence(
        &self,
        proposal_ref: AgentHouseholdProposalIdV1,
        commit_id: CommitId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<HouseholdCommitEvidenceBindingV1, PortError>> {
        Box::pin(NativeHouseholdRepository::reserve_agent_commit_evidence(
            self,
            proposal_ref,
            commit_id,
            cancellation,
        ))
    }

    fn prove_applied_agent_commit<'a>(
        &'a self,
        binding: &'a HouseholdCommitEvidenceBindingV1,
        proposal_ref: AgentHouseholdProposalIdV1,
        commit_id: CommitId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<AppliedHouseholdCommitProofV1, PortError>> {
        Box::pin(NativeHouseholdRepository::prove_applied_agent_commit(
            self,
            binding,
            proposal_ref,
            commit_id,
            cancellation,
        ))
    }

    fn prove_unapplied_agent_commit<'a>(
        &'a self,
        binding: &'a HouseholdCommitEvidenceBindingV1,
        proposal_ref: AgentHouseholdProposalIdV1,
        commit_id: CommitId,
        expected_revision: HouseholdRevision,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<UnappliedHouseholdCommitProofV1, PortError>> {
        Box::pin(NativeHouseholdRepository::prove_unapplied_agent_commit(
            self,
            binding,
            proposal_ref,
            commit_id,
            expected_revision,
            cancellation,
        ))
    }
}

fn require_committed_guard(guard: &HouseholdMigrationGuardDocument) -> Result<(), PortError> {
    if !matches!(
        guard.state(),
        HouseholdMigrationGuardStateV1::Migrated
            | HouseholdMigrationGuardStateV1::InitializedNoSource
    ) {
        return Err(PortError::new(
            "household_guard_state",
            "native household state is not in a committed readable guard state",
        ));
    }
    Ok(())
}

fn derive_commit_evidence_secret(
    root_key: &HouseholdKeyMaterial,
    account: &AccountId,
    proposal_ref: AgentHouseholdProposalIdV1,
    commit_id: CommitId,
) -> Result<Zeroizing<[u8; 32]>, PortError> {
    let account_bytes = account.as_str().as_bytes();
    let mut info =
        Vec::with_capacity(COMMIT_EVIDENCE_HKDF_INFO.len() + 8 + account_bytes.len() + (2 * 16));
    info.extend_from_slice(COMMIT_EVIDENCE_HKDF_INFO);
    info.extend_from_slice(
        &u64::try_from(account_bytes.len())
            .map_err(|_| commit_evidence_mismatch_error())?
            .to_be_bytes(),
    );
    info.extend_from_slice(account_bytes);
    info.extend_from_slice(proposal_ref.as_uuid().as_bytes());
    info.extend_from_slice(commit_id.as_uuid().as_bytes());
    let hkdf = Hkdf::<Sha256>::new(Some(COMMIT_EVIDENCE_HKDF_SALT), root_key.expose());
    let mut secret = Zeroizing::new([0_u8; 32]);
    hkdf.expand(&info, secret.as_mut())
        .map_err(|_| commit_evidence_mismatch_error())?;
    Ok(secret)
}

fn commit_evidence_contract_error(_: AgentHouseholdContractErrorV1) -> PortError {
    commit_evidence_mismatch_error()
}

fn commit_evidence_mismatch_error() -> PortError {
    PortError::new(
        "household_commit_evidence_mismatch",
        "household commit evidence did not match the authoritative repository",
    )
}

fn validate_guard_provenance(
    guard: &HouseholdMigrationGuardDocument,
    state: &HouseholdStateV1,
) -> Result<(), PortError> {
    guard.canonical_bytes()?;
    if state.migration_provenance.initialization_id != guard.initialization_id()
        || state.migration_provenance.initial_commit_id.as_uuid() != guard.initial_commit_id()
    {
        return Err(PortError::new(
            "household_migration_provenance_mismatch",
            "household vault provenance does not match its migration guard",
        ));
    }
    let expected_initial_fingerprint = guard
        .initial_effect_fingerprint()
        .ok_or_else(initialization_protocol_error)?;
    let initial_record = state
        .bounded_applied_commits
        .iter()
        .find(|record| record.commit_id.as_uuid() == guard.initial_commit_id())
        .ok_or_else(initial_ledger_error)?;
    if initial_record.outcome != AppliedCommitOutcomeV1::Initialized
        || initial_record.resulting_revision.get() != 1
        || !constant_time_bytes_eq(
            initial_record.fingerprint.as_bytes(),
            &expected_initial_fingerprint,
        )
    {
        return Err(initial_ledger_error());
    }
    let guard_value: serde_json::Value = serde_json::from_slice(&guard.canonical_bytes()?)
        .map_err(|_| {
            PortError::new(
                "household_migration_guard_invalid",
                "household migration guard document is invalid",
            )
        })?;
    let state_source = serde_json::to_value(&state.migration_provenance.source_identity)
        .map_err(|_| state_error("household source provenance is invalid"))?;
    let expected_migration_id = serde_json::to_value(state.migration_provenance.migration_id)
        .map_err(|_| state_error("household migration provenance is invalid"))?;
    let expected_frozen_at = serde_json::to_value(&state.migration_provenance.migration_frozen_at)
        .map_err(|_| state_error("household migration provenance is invalid"))?;
    if guard_value.get("source_identity") != Some(&state_source)
        || guard_value.get("migration_id") != Some(&expected_migration_id)
        || guard_value.get("migration_frozen_at") != Some(&expected_frozen_at)
    {
        return Err(PortError::new(
            "household_migration_provenance_mismatch",
            "household vault provenance does not match its migration guard",
        ));
    }
    match (guard.state(), &state.migration_provenance.source_identity) {
        (HouseholdMigrationGuardStateV1::Migrated, LegacySourceIdentityV1::Present { .. })
        | (
            HouseholdMigrationGuardStateV1::InitializedNoSource,
            LegacySourceIdentityV1::NoSource { .. },
        )
        | (HouseholdMigrationGuardStateV1::Initializing, _) => Ok(()),
        _ => Err(PortError::new(
            "household_migration_provenance_mismatch",
            "household vault source identity does not match its migration guard state",
        )),
    }
}

#[inline(never)]
fn constant_time_bytes_eq(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn check_cancelled(cancellation: &CancellationToken) -> Result<(), PortError> {
    if cancellation.is_cancelled() {
        Err(PortError::new(
            "household_operation_cancelled",
            "household repository operation was cancelled",
        ))
    } else {
        Ok(())
    }
}

fn account_mismatch_error() -> PortError {
    PortError::new(
        "household_account_mismatch",
        "household repository is bound to another account",
    )
}

fn read_only_error() -> PortError {
    PortError::new(
        "household_repository_read_only",
        "native rollback mode does not permit household writes",
    )
}

fn teardown_in_progress_error() -> PortError {
    PortError::new(
        "household_account_teardown_in_progress",
        "household account teardown blocks repository access",
    )
}

fn missing_key_error() -> PortError {
    PortError::new(
        "household_key_missing",
        "native household ciphertext requires its account-bound household key",
    )
}

fn initialization_protocol_error() -> PortError {
    PortError::new(
        "household_initialization_protocol_required",
        "household initialization requires the exact ready guard, key, and vault transaction",
    )
}

fn initial_ledger_error() -> PortError {
    PortError::new(
        "household_initial_ledger_mismatch",
        "household initial applied ledger does not match its migration guard",
    )
}

fn state_error(error: impl fmt::Display) -> PortError {
    let _ = error;
    PortError::new(
        "household_state_invalid",
        "canonical household state is invalid",
    )
}
