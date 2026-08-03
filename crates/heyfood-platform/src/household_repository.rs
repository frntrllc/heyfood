//! Live account-bound repository adapter for the encrypted household vault.
//!
//! This adapter owns no migration or teardown policy. It consumes an exact
//! migration-guard/key transaction prepared by the audited startup path,
//! retains the lifecycle and vault leases in the required order, and delegates
//! semantic replay/conflict resolution to `heyfood-application`.

use std::{
    fmt, fs,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use chacha20poly1305::aead::{Aead as _, Generate as _, Payload};
use chacha20poly1305::{KeyInit as _, XChaCha20Poly1305, XNonce};
use heyfood_application::{
    AuthorizedAgentHouseholdPrepareV1, BoundAgentHouseholdDisclosureV1,
    BoundAgentHouseholdOutcomeReceiptV1, BoundAgentHouseholdProposalV1, BoundAgentHouseholdReadV1,
    BoundAgentHouseholdRosterAuthorityV1, BoxFuture, HouseholdAgentDisclosureAccessV1,
    HouseholdAgentDisclosureControlPort, HouseholdAgentPhase0Port, HouseholdCommit,
    HouseholdCommitEvidenceRepositoryPort, HouseholdCommitOutcome, HouseholdErase,
    HouseholdEraseOutcome, HouseholdInitialize, HouseholdLoad, HouseholdMutationAuthorityPort,
    HouseholdReadLeaseV1, HouseholdRepositoryPort, HouseholdRepositoryResolutionV1,
    HouseholdSession, NativeHouseholdModeV1, PortError, resolve_household_commit_v1,
    resolve_household_initialize_v1,
};
use heyfood_core::agent_household::HouseholdCommitEvidenceRepositoryAuthorityV1;
use heyfood_core::{
    AGENT_HOUSEHOLD_CONTRACT_VERSION, AGENT_HOUSEHOLD_MAX_MEMBERS_PER_PAGE, AccountId,
    AgentDisclosureGrantSubjectV1, AgentDisclosureLedgerV1, AgentDisclosurePurposeV1,
    AgentHouseholdContractErrorV1, AgentHouseholdMemberProjectionV1, AgentHouseholdProposalIdV1,
    AgentHouseholdReadResultKindV1, AgentHouseholdReadSnapshotV1, AgentHouseholdSubjectV1,
    AgentMinimizedDeclaredProfileV1, AppliedCommitOutcomeV1, AppliedHouseholdCommitProofV1,
    CanonicalTimestampV1, CommitId, GenerationId, HouseholdCommitEvidenceBindingV1,
    HouseholdEffectFingerprintV1, HouseholdLifecycleV1, HouseholdRevision, HouseholdScope,
    HouseholdStateV1, HouseholdSubjectId, LegacySourceIdentityV1, MinorStatusV1,
    UnappliedHouseholdCommitProofV1, canonical_sha256_v1, decode_canonical_household_state_v1,
    domain_hash_v1,
};
use hkdf::Hkdf;
use sha2::Sha256;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use crate::credential_broker::HouseholdCommitEvidenceStateV1;
use crate::household_vault::HouseholdVaultStartupArtifactsV1;
use crate::{
    AtomicFile, HouseholdKeyBundle, HouseholdKeyBundlePhase, HouseholdKeyMaterial,
    HouseholdKeyStore, HouseholdMigrationGuardDocument, HouseholdMigrationGuardStateV1,
    HouseholdMigrationGuardStore, HouseholdMigrationInitializationPhaseV1, HouseholdSecureStore,
    HouseholdVault, HouseholdVaultLease, HouseholdVaultLeaseModeV1, HouseholdVaultLoad,
    HouseholdVaultWrite, KeyBundleRevision, KeyId, KeyStoreExpectation, MigrationGuardExpectation,
    NativePaths, household_teardown_barrier_present_v1,
};

const ACCOUNT_DIGEST_CONTRACT: &str = "heyfood.household.account-digest.v1";
const COMMIT_EVIDENCE_HKDF_SALT: &[u8] = b"heyfood.household.commit-evidence.hkdf.salt.v1";
const COMMIT_EVIDENCE_HKDF_INFO: &[u8] = b"heyfood.household.commit-evidence.capability.v1";
const AGENT_DISCLOSURE_MAGIC: &[u8; 8] = b"HFAGENT1";
const AGENT_DISCLOSURE_HKDF_SALT: &[u8] = b"heyfood.household.agent-disclosure.salt.v1";
const AGENT_DISCLOSURE_HKDF_INFO: &[u8] = b"heyfood.household.agent-disclosure.key.v1";
const MAX_AGENT_DISCLOSURE_ENVELOPE_BYTES: usize = 320 * 1024;
const AGENT_DISCLOSURE_ENVELOPE_VERSION: u16 = 1;
const AGENT_DISCLOSURE_HEADER_BYTES: usize = 8 + 2 + 24 + 4;
const AGENT_DISCLOSURE_FILE: &str = "agent-disclosure.hfa";

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

    /// Persist explicit local disclosure authority for one exact subject.
    /// Profile disclosure is accepted only for an authoritative adult subject;
    /// minors can receive roster-only authority.
    pub async fn grant_agent_disclosure(
        &self,
        subject: AgentDisclosureGrantSubjectV1,
        include_minimized_profile: bool,
        issued_at: CanonicalTimestampV1,
        cancellation: CancellationToken,
    ) -> Result<GenerationId, PortError> {
        if self.access != RepositoryAccessV1::ReadWrite {
            return Err(read_only_error());
        }
        let mut lease = self
            .acquire_vault_lease(HouseholdVaultLeaseModeV1::RequireExisting, &cancellation)
            .await?;
        let (guard, key) = self.reread_guard_and_key(&lease, &cancellation).await?;
        let loaded = self
            .load_committed_under_lease(&mut lease, &guard, &key, cancellation.clone())
            .await?;
        let minor_status = authoritative_disclosure_subject_status(&loaded.state, &subject)?;
        let mut ledger = self.load_agent_disclosure_ledger(&key)?;
        ledger
            .grant(subject, minor_status, include_minimized_profile, issued_at)
            .map_err(agent_disclosure_contract_error)?;
        check_cancelled(&cancellation)?;
        self.persist_agent_disclosure_ledger(&key, &ledger)?;
        Ok(ledger.generation())
    }

    /// Revoke all read and proposal-status disclosure authority for one exact
    /// subject. A missing grant is an idempotent no-op.
    pub async fn revoke_agent_disclosure(
        &self,
        subject: AgentDisclosureGrantSubjectV1,
        revoked_at: CanonicalTimestampV1,
        cancellation: CancellationToken,
    ) -> Result<GenerationId, PortError> {
        if self.access != RepositoryAccessV1::ReadWrite {
            return Err(read_only_error());
        }
        let mut lease = self
            .acquire_vault_lease(HouseholdVaultLeaseModeV1::RequireExisting, &cancellation)
            .await?;
        let (guard, key) = self.reread_guard_and_key(&lease, &cancellation).await?;
        let _ = self
            .load_committed_under_lease(&mut lease, &guard, &key, cancellation.clone())
            .await?;
        let mut ledger = self.load_agent_disclosure_ledger(&key)?;
        if ledger
            .revoke(&subject, revoked_at)
            .map_err(agent_disclosure_contract_error)?
        {
            check_cancelled(&cancellation)?;
            self.persist_agent_disclosure_ledger(&key, &ledger)?;
        }
        Ok(ledger.generation())
    }

    fn agent_disclosure_path(&self) -> std::path::PathBuf {
        self.vault.household_directory().join(AGENT_DISCLOSURE_FILE)
    }

    fn load_agent_disclosure_ledger(
        &self,
        key: &HouseholdKeyBundle,
    ) -> Result<AgentDisclosureLedgerV1, PortError> {
        let path = self.agent_disclosure_path();
        let bytes = match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !metadata.file_type().is_file()
                    || metadata.file_type().is_symlink()
                    || usize::try_from(metadata.len()).map_or(true, |length| {
                        !(AGENT_DISCLOSURE_HEADER_BYTES + 16..=MAX_AGENT_DISCLOSURE_ENVELOPE_BYTES)
                            .contains(&length)
                    })
                {
                    return Err(agent_disclosure_format_error());
                }
                let bytes = fs::read(&path).map_err(|_| agent_disclosure_format_error())?;
                if !(AGENT_DISCLOSURE_HEADER_BYTES + 16..=MAX_AGENT_DISCLOSURE_ENVELOPE_BYTES)
                    .contains(&bytes.len())
                {
                    return Err(agent_disclosure_format_error());
                }
                bytes
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(AgentDisclosureLedgerV1::empty(self.account.clone()));
            }
            Err(_) => return Err(agent_disclosure_format_error()),
        };
        decrypt_agent_disclosure_envelope(
            &bytes,
            key.commit_evidence_key(),
            &self.account,
            self.vault.account_slot().account_digest(),
            self.vault.account_slot().native_root_instance_digest(),
        )
    }

    fn persist_agent_disclosure_ledger(
        &self,
        key: &HouseholdKeyBundle,
        ledger: &AgentDisclosureLedgerV1,
    ) -> Result<(), PortError> {
        if !ledger.account_matches(&self.account) {
            return Err(account_mismatch_error());
        }
        let envelope = encrypt_agent_disclosure_envelope(
            ledger,
            key.commit_evidence_key(),
            &self.account,
            self.vault.account_slot().account_digest(),
            self.vault.account_slot().native_root_instance_digest(),
        )?;
        AtomicFile::replace(&self.agent_disclosure_path(), &envelope)
    }

    async fn load_agent_household_state(
        &self,
        cancellation: CancellationToken,
    ) -> Result<HouseholdLoad, PortError> {
        let mut lease = self
            .acquire_vault_lease(HouseholdVaultLeaseModeV1::RequireExisting, &cancellation)
            .await?;
        let (guard, key) = self.reread_guard_and_key(&lease, &cancellation).await?;
        self.load_committed_under_lease(&mut lease, &guard, &key, cancellation)
            .await
    }

    /// Reserve the opaque verifier for one exact future proposal commit.
    /// The corresponding secret is securely rederived from the native
    /// repository key after restart. It is never exposed as data; a successful
    /// observation carries only its tuple-bound verifier inside a redacted
    /// proof.
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
        let loaded = self
            .load_committed_under_lease(&mut vault_lease, &guard, &key, cancellation.clone())
            .await?;
        check_cancelled(&cancellation)?;
        if loaded
            .state
            .bounded_applied_commits
            .iter()
            .any(|record| record.commit_id == commit_id)
        {
            return Err(commit_evidence_mismatch_error());
        }
        let secret = derive_commit_evidence_secret(
            key.commit_evidence_key(),
            &self.account,
            proposal_ref,
            commit_id,
        )?;
        let now_unix_seconds = commit_evidence_now_unix_seconds()?;
        let applied_commit_ids = loaded
            .state
            .bounded_applied_commits
            .iter()
            .map(|record| record.commit_id)
            .collect::<Vec<_>>();
        let replacement = key.reserve_commit_evidence(
            proposal_ref.as_uuid(),
            commit_id,
            now_unix_seconds,
            &applied_commit_ids,
        )?;
        if replacement != key {
            self.replace_commit_evidence_key_bundle(
                &mut vault_lease,
                &key,
                &replacement,
                cancellation,
            )
            .await?;
        }
        Ok(
            HouseholdCommitEvidenceRepositoryAuthorityV1::from_repository_secret(
                self.account.clone(),
                proposal_ref,
                commit_id,
                &secret,
            )
            .binding(),
        )
    }

    /// Remove an exact reservation after the durable proposal journal has
    /// terminally established that dispatch never began. The repository
    /// rechecks authoritative absence under the same vault lease before
    /// releasing the content-free record.
    pub async fn release_undispatched_agent_commit_evidence(
        &self,
        binding: &HouseholdCommitEvidenceBindingV1,
        proposal_ref: AgentHouseholdProposalIdV1,
        commit_id: CommitId,
        cancellation: CancellationToken,
    ) -> Result<(), PortError> {
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
        let now_unix_seconds = commit_evidence_now_unix_seconds()?;
        if loaded
            .state
            .bounded_applied_commits
            .iter()
            .any(|record| record.commit_id == commit_id)
            || key.commit_evidence_record(proposal_ref.as_uuid(), commit_id, now_unix_seconds)
                != Some(HouseholdCommitEvidenceStateV1::Reserved)
        {
            return Err(commit_evidence_mismatch_error());
        }
        let secret = derive_commit_evidence_secret(
            key.commit_evidence_key(),
            &self.account,
            proposal_ref,
            commit_id,
        )?;
        let authority = HouseholdCommitEvidenceRepositoryAuthorityV1::from_repository_secret(
            self.account.clone(),
            proposal_ref,
            commit_id,
            &secret,
        );
        if &authority.binding() != binding {
            return Err(commit_evidence_mismatch_error());
        }
        let replacement =
            key.release_reserved_commit(proposal_ref.as_uuid(), commit_id, now_unix_seconds)?;
        self.replace_commit_evidence_key_bundle(&mut vault_lease, &key, &replacement, cancellation)
            .await
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
        check_cancelled(&cancellation)?;
        let mut vault_lease = self
            .acquire_vault_lease(HouseholdVaultLeaseModeV1::RequireExisting, &cancellation)
            .await?;
        let (guard, key) = self
            .reread_guard_and_key(&vault_lease, &cancellation)
            .await?;
        let loaded = self
            .load_committed_under_lease(&mut vault_lease, &guard, &key, cancellation)
            .await?;
        let now_unix_seconds = commit_evidence_now_unix_seconds()?;
        if key.commit_evidence_record(proposal_ref.as_uuid(), commit_id, now_unix_seconds)
            != Some(HouseholdCommitEvidenceStateV1::Reserved)
        {
            return Err(commit_evidence_mismatch_error());
        }
        let secret = derive_commit_evidence_secret(
            key.commit_evidence_key(),
            &self.account,
            proposal_ref,
            commit_id,
        )?;
        let authority = HouseholdCommitEvidenceRepositoryAuthorityV1::from_repository_secret(
            self.account.clone(),
            proposal_ref,
            commit_id,
            &secret,
        );
        if &authority.binding() != binding {
            return Err(commit_evidence_mismatch_error());
        }
        let record = loaded
            .state
            .bounded_applied_commits
            .iter()
            .find(|record| {
                record.commit_id == commit_id && record.outcome == AppliedCommitOutcomeV1::Committed
            })
            .ok_or_else(commit_evidence_mismatch_error)?;
        authority
            .seal_applied_repository_observation(
                binding,
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
        let now_unix_seconds = commit_evidence_now_unix_seconds()?;
        if key
            .commit_evidence_record(proposal_ref.as_uuid(), commit_id, now_unix_seconds)
            .is_none()
        {
            return Err(commit_evidence_mismatch_error());
        }
        let secret = derive_commit_evidence_secret(
            key.commit_evidence_key(),
            &self.account,
            proposal_ref,
            commit_id,
        )?;
        let authority = HouseholdCommitEvidenceRepositoryAuthorityV1::from_repository_secret(
            self.account.clone(),
            proposal_ref,
            commit_id,
            &secret,
        );
        if &authority.binding() != binding
            || loaded.state.revision != expected_revision
            || loaded
                .state
                .bounded_applied_commits
                .iter()
                .any(|record| record.commit_id == commit_id)
        {
            return Err(commit_evidence_mismatch_error());
        }
        let replacement =
            key.deny_reserved_commit(proposal_ref.as_uuid(), commit_id, now_unix_seconds)?;
        if replacement != key {
            self.replace_commit_evidence_key_bundle(
                &mut vault_lease,
                &key,
                &replacement,
                cancellation,
            )
            .await?;
        }
        authority
            .seal_unapplied_repository_observation(binding, loaded.state.revision)
            .map_err(commit_evidence_contract_error)
    }

    async fn replace_commit_evidence_key_bundle(
        &self,
        vault_lease: &mut HouseholdVaultLease,
        current: &HouseholdKeyBundle,
        replacement: &HouseholdKeyBundle,
        cancellation: CancellationToken,
    ) -> Result<(), PortError> {
        check_cancelled(&cancellation)?;
        let exchange = HouseholdKeyStore::compare_exchange(
            self.secure_store.as_ref(),
            vault_lease,
            current.revision,
            replacement.clone(),
            cancellation,
        )
        .await;
        let observed = HouseholdKeyStore::load(
            self.secure_store.as_ref(),
            vault_lease.lifecycle_lease(),
            CancellationToken::new(),
        )
        .await?;
        match (exchange, observed) {
            (_, Some(observed)) if observed == *replacement => Ok(()),
            (Err(error), Some(observed)) if observed == *current => Err(error),
            _ => Err(PortError::uncertain(
                "household_commit_evidence_persist",
                "household commit evidence persistence requires reconciliation",
            )),
        }
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

    /// Retain the concrete account-bound repository as the read/disclosure
    /// adapter while application composition separately builds its household
    /// mutation session. This accessor exposes no mutation authority.
    #[must_use]
    pub fn agent_phase0_port(self: &Arc<Self>) -> Arc<dyn HouseholdAgentPhase0Port> {
        self.clone()
    }

    /// Human-attached-terminal disclosure control kept separate from the
    /// agent-facing read port.
    #[must_use]
    pub fn agent_disclosure_control_port(
        self: &Arc<Self>,
    ) -> Arc<dyn HouseholdAgentDisclosureControlPort> {
        self.clone()
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
                let replacement =
                    key.stabilized(self.vault.account_slot(), key.revision.checked_next()?)?;
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
        )?;
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
        if key.denies_commit(command.commit_id, commit_evidence_now_unix_seconds()?) {
            return Err(PortError::new(
                "household_commit_permanently_denied",
                "household commit was permanently denied after authoritative reconciliation",
            ));
        }
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

impl HouseholdAgentPhase0Port for NativeHouseholdRepository {
    fn eligible_roster(
        &self,
        account: AccountId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<BoundAgentHouseholdRosterAuthorityV1, PortError>> {
        Box::pin(async move {
            self.require_account(&account)?;
            let loaded = self.load_agent_household_state(cancellation).await?;
            let eligible_subjects = eligible_agent_subjects(&loaded.state)?;
            Ok(BoundAgentHouseholdRosterAuthorityV1 {
                account,
                household_revision: loaded.state.revision,
                eligible_subjects,
            })
        })
    }

    fn disclosure(
        &self,
        account: AccountId,
        purpose: AgentDisclosurePurposeV1,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<BoundAgentHouseholdDisclosureV1, PortError>> {
        Box::pin(async move {
            self.require_account(&account)?;
            let mut lease = self
                .acquire_vault_lease(HouseholdVaultLeaseModeV1::RequireExisting, &cancellation)
                .await?;
            let (guard, key) = self.reread_guard_and_key(&lease, &cancellation).await?;
            let loaded = self
                .load_committed_under_lease(&mut lease, &guard, &key, cancellation)
                .await?;
            let ledger = self.load_agent_disclosure_ledger(&key)?;
            let grants = ledger
                .grant_set(purpose, loaded.state.updated_at)
                .map_err(agent_disclosure_contract_error)?;
            Ok(BoundAgentHouseholdDisclosureV1 { account, grants })
        })
    }

    fn read(
        &self,
        account: AccountId,
        request: heyfood_core::AgentHouseholdReadRequestV1,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<BoundAgentHouseholdReadV1, PortError>> {
        Box::pin(async move {
            self.require_account(&account)?;
            let loaded = self.load_agent_household_state(cancellation).await?;
            let snapshot = project_agent_household_read(&loaded.state, &request)?;
            Ok(BoundAgentHouseholdReadV1 { account, snapshot })
        })
    }

    fn prepare(
        &self,
        _account: AccountId,
        _request: AuthorizedAgentHouseholdPrepareV1,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<BoundAgentHouseholdProposalV1, PortError>> {
        Box::pin(async { Err(agent_household_lifecycle_unavailable_error()) })
    }

    fn status(
        &self,
        _account: AccountId,
        _proposal_ref: AgentHouseholdProposalIdV1,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<BoundAgentHouseholdProposalV1, PortError>> {
        Box::pin(async { Err(agent_household_lifecycle_unavailable_error()) })
    }

    fn cancel(
        &self,
        _account: AccountId,
        _proposal_ref: AgentHouseholdProposalIdV1,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<BoundAgentHouseholdOutcomeReceiptV1, PortError>> {
        Box::pin(async { Err(agent_household_lifecycle_unavailable_error()) })
    }
}

impl HouseholdAgentDisclosureControlPort for NativeHouseholdRepository {
    fn current_access(
        &self,
        account: AccountId,
        subject: AgentDisclosureGrantSubjectV1,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<HouseholdAgentDisclosureAccessV1, PortError>> {
        Box::pin(async move {
            self.require_account(&account)?;
            let mut lease = self
                .acquire_vault_lease(HouseholdVaultLeaseModeV1::RequireExisting, &cancellation)
                .await?;
            let (guard, key) = self.reread_guard_and_key(&lease, &cancellation).await?;
            let loaded = self
                .load_committed_under_lease(&mut lease, &guard, &key, cancellation)
                .await?;
            let _ = authoritative_disclosure_subject_status(&loaded.state, &subject)?;
            let ledger = self.load_agent_disclosure_ledger(&key)?;
            let grants = ledger
                .grant_set(
                    AgentDisclosurePurposeV1::HouseholdAgentRead,
                    agent_disclosure_now()?,
                )
                .map_err(agent_disclosure_contract_error)?;
            Ok(HouseholdAgentDisclosureAccessV1 {
                account,
                generation: grants.generation(),
                projection: grants.maximum_projection_for(&[subject]),
            })
        })
    }

    fn grant_access(
        &self,
        account: AccountId,
        subject: AgentDisclosureGrantSubjectV1,
        include_minimized_profile: bool,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<HouseholdAgentDisclosureAccessV1, PortError>> {
        Box::pin(async move {
            self.require_account(&account)?;
            let generation = self
                .grant_agent_disclosure(
                    subject.clone(),
                    include_minimized_profile,
                    agent_disclosure_now()?,
                    cancellation.clone(),
                )
                .await?;
            let access = self.current_access(account, subject, cancellation).await?;
            if access.generation != generation {
                return Err(agent_disclosure_reconciliation_error());
            }
            Ok(access)
        })
    }

    fn revoke_access(
        &self,
        account: AccountId,
        subject: AgentDisclosureGrantSubjectV1,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<HouseholdAgentDisclosureAccessV1, PortError>> {
        Box::pin(async move {
            self.require_account(&account)?;
            let generation = self
                .revoke_agent_disclosure(
                    subject.clone(),
                    agent_disclosure_now()?,
                    cancellation.clone(),
                )
                .await?;
            let access = self.current_access(account, subject, cancellation).await?;
            if access.generation != generation
                || access.projection != heyfood_core::AgentHouseholdProjectionV1::ContentFree
            {
                return Err(agent_disclosure_reconciliation_error());
            }
            Ok(access)
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

    fn release_undispatched_agent_commit_evidence<'a>(
        &'a self,
        binding: &'a HouseholdCommitEvidenceBindingV1,
        proposal_ref: AgentHouseholdProposalIdV1,
        commit_id: CommitId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<(), PortError>> {
        Box::pin(
            NativeHouseholdRepository::release_undispatched_agent_commit_evidence(
                self,
                binding,
                proposal_ref,
                commit_id,
                cancellation,
            ),
        )
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

fn eligible_agent_subjects(
    state: &HouseholdStateV1,
) -> Result<Vec<AgentDisclosureGrantSubjectV1>, PortError> {
    let mut subjects = vec![AgentDisclosureGrantSubjectV1::Self_];
    subjects.extend(
        state
            .members
            .iter()
            .filter(|member| member.lifecycle == HouseholdLifecycleV1::Active)
            .map(|member| AgentDisclosureGrantSubjectV1::Member(member.member_id.clone())),
    );
    if subjects.len() > usize::from(AGENT_HOUSEHOLD_MAX_MEMBERS_PER_PAGE) {
        return Err(PortError::new(
            "household_agent_roster_too_large",
            "the active household roster exceeds the closed agent read contract",
        ));
    }
    Ok(subjects)
}

fn authoritative_disclosure_subject_status(
    state: &HouseholdStateV1,
    subject: &AgentDisclosureGrantSubjectV1,
) -> Result<MinorStatusV1, PortError> {
    match subject {
        AgentDisclosureGrantSubjectV1::Self_ => Ok(MinorStatusV1::Adult),
        AgentDisclosureGrantSubjectV1::Member(member_ref) => state
            .members
            .iter()
            .find(|member| {
                member.member_id == *member_ref && member.lifecycle == HouseholdLifecycleV1::Active
            })
            .map(|member| member.minor_status)
            .ok_or_else(|| {
                PortError::new(
                    "household_agent_subject_unavailable",
                    "agent disclosure requires an active authoritative household subject",
                )
            }),
    }
}

fn project_agent_household_read(
    state: &HouseholdStateV1,
    request: &heyfood_core::AgentHouseholdReadRequestV1,
) -> Result<AgentHouseholdReadSnapshotV1, PortError> {
    request
        .validate_wire_shape()
        .map_err(|_| agent_household_read_contract_error())?;
    if request.cursor.is_some() {
        return Err(PortError::new(
            "household_agent_cursor_unsupported",
            "the native household roster is returned atomically and does not accept a cursor",
        ));
    }
    let eligible = eligible_agent_subjects(state)?;
    let resolved_from_active_scope = request.subject.is_none();
    let resolved_subject = request
        .subject
        .clone()
        .unwrap_or_else(|| agent_subject_from_scope(&state.active_scope));
    let members = match &resolved_subject {
        AgentHouseholdSubjectV1::Self_ => Vec::new(),
        AgentHouseholdSubjectV1::Member(member_ref) => vec![
            state
                .members
                .iter()
                .find(|member| {
                    member.member_id == *member_ref
                        && member.lifecycle == HouseholdLifecycleV1::Active
                })
                .ok_or_else(agent_household_subject_unavailable_error)
                .and_then(|member| project_agent_member(state, member))?,
        ],
        AgentHouseholdSubjectV1::Everyone => state
            .members
            .iter()
            .filter(|member| member.lifecycle == HouseholdLifecycleV1::Active)
            .map(|member| project_agent_member(state, member))
            .collect::<Result<Vec<_>, _>>()?,
    };
    if members.len() > usize::from(request.limit) {
        return Err(PortError::new(
            "household_agent_read_limit",
            "the requested limit cannot represent the complete household read safely",
        ));
    }
    let eligible_member_count =
        u16::try_from(eligible.len()).map_err(|_| agent_household_read_contract_error())?;
    let snapshot = AgentHouseholdReadSnapshotV1 {
        schema_version: AGENT_HOUSEHOLD_CONTRACT_VERSION,
        kind: AgentHouseholdReadResultKindV1::HouseholdReadResult,
        projection: heyfood_core::AgentHouseholdProjectionV1::Profile,
        resolved_subject: Some(resolved_subject),
        resolved_from_active_scope,
        active_scope: Some(state.active_scope.clone()),
        household_revision: state.revision,
        // The application controller replaces this placeholder with the
        // independently loaded encrypted ledger generation before returning.
        disclosure_generation: GenerationId::new(1),
        eligible_member_count,
        restricted_member_count: 0,
        members,
        next_cursor: None,
    };
    snapshot
        .validate_wire_shape()
        .map_err(|_| agent_household_read_contract_error())?;
    Ok(snapshot)
}

fn agent_subject_from_scope(scope: &HouseholdScope) -> AgentHouseholdSubjectV1 {
    match scope {
        HouseholdScope::Subject(HouseholdSubjectId::Self_) => AgentHouseholdSubjectV1::Self_,
        HouseholdScope::Subject(HouseholdSubjectId::Member(member)) => {
            AgentHouseholdSubjectV1::Member(member.clone())
        }
        HouseholdScope::Everyone => AgentHouseholdSubjectV1::Everyone,
    }
}

fn project_agent_member(
    state: &HouseholdStateV1,
    member: &heyfood_core::HouseholdMemberV1,
) -> Result<AgentHouseholdMemberProjectionV1, PortError> {
    let subject = HouseholdSubjectId::member(member.member_id.clone());
    let profile = state
        .profiles
        .iter()
        .find(|profile| profile.subject == subject);
    let minimized_declared_profile = profile
        .and_then(|profile| profile.document.declared_profile.as_ref())
        .map(minimize_declared_profile)
        .transpose()?;
    Ok(AgentHouseholdMemberProjectionV1 {
        member_ref: member.member_id.clone(),
        display_label: member.display_name.clone(),
        relationship: member.relationship,
        lifecycle: member.lifecycle,
        profile_state: member.profile_state,
        profile_schema_version: profile.map(|profile| profile.document.schema_version),
        profile_revision: profile.map(|profile| profile.profile_revision),
        profile_complete: minimized_declared_profile.is_some(),
        minimized_declared_profile,
    })
}

fn minimize_declared_profile(
    profile: &heyfood_core::HouseholdDeclaredProfileV1,
) -> Result<AgentMinimizedDeclaredProfileV1, PortError> {
    fn combined(left: &[String], right: &[String]) -> Vec<String> {
        let mut values = Vec::with_capacity(left.len() + right.len());
        for value in left.iter().chain(right) {
            if !values.contains(value) {
                values.push(value.clone());
            }
        }
        values
    }
    let minimized = AgentMinimizedDeclaredProfileV1 {
        diet_styles: combined(&profile.diet_style_ids, &profile.custom_diet_styles),
        allergies: profile.allergy_ids.clone(),
        restrictions: profile.custom_restrictions.clone(),
        health_conditions: combined(
            &profile.health_condition_ids,
            &profile.custom_health_conditions,
        ),
        avoid_ingredients: profile.avoid_ingredients.clone(),
    };
    minimized
        .validate_wire_shape()
        .map_err(|_| agent_household_read_contract_error())?;
    Ok(minimized)
}

fn agent_disclosure_aad(
    account: &AccountId,
    account_digest: &[u8; 32],
    native_root_digest: &[u8; 32],
    header: &[u8],
) -> Result<Vec<u8>, PortError> {
    let account_bytes = account.as_str().as_bytes();
    let account_length =
        u32::try_from(account_bytes.len()).map_err(|_| account_mismatch_error())?;
    let mut aad = Vec::with_capacity(
        AGENT_DISCLOSURE_HKDF_INFO.len()
            + 4
            + account_bytes.len()
            + account_digest.len()
            + native_root_digest.len()
            + header.len(),
    );
    aad.extend_from_slice(AGENT_DISCLOSURE_HKDF_INFO);
    aad.extend_from_slice(&account_length.to_be_bytes());
    aad.extend_from_slice(account_bytes);
    aad.extend_from_slice(account_digest);
    aad.extend_from_slice(native_root_digest);
    aad.extend_from_slice(header);
    Ok(aad)
}

fn derive_agent_disclosure_key(
    root_key: &HouseholdKeyMaterial,
    account: &AccountId,
    account_digest: &[u8; 32],
    native_root_digest: &[u8; 32],
) -> Result<Zeroizing<[u8; 32]>, PortError> {
    let mut info =
        Vec::with_capacity(AGENT_DISCLOSURE_HKDF_INFO.len() + account.as_str().len() + 64);
    info.extend_from_slice(AGENT_DISCLOSURE_HKDF_INFO);
    info.extend_from_slice(account.as_str().as_bytes());
    info.extend_from_slice(account_digest);
    info.extend_from_slice(native_root_digest);
    let hkdf = Hkdf::<Sha256>::new(Some(AGENT_DISCLOSURE_HKDF_SALT), root_key.expose());
    let mut key = Zeroizing::new([0_u8; 32]);
    hkdf.expand(&info, key.as_mut())
        .map_err(|_| agent_disclosure_crypto_error())?;
    Ok(key)
}

fn encrypt_agent_disclosure_envelope(
    ledger: &AgentDisclosureLedgerV1,
    root_key: &HouseholdKeyMaterial,
    account: &AccountId,
    account_digest: [u8; 32],
    native_root_digest: [u8; 32],
) -> Result<Zeroizing<Vec<u8>>, PortError> {
    let plaintext = ledger
        .encode_canonical()
        .map_err(agent_disclosure_contract_error)?;
    let nonce = XNonce::try_generate().map_err(|_| agent_disclosure_crypto_error())?;
    let ciphertext_length = plaintext
        .len()
        .checked_add(16)
        .and_then(|length| u32::try_from(length).ok())
        .ok_or_else(agent_disclosure_format_error)?;
    let mut header = Vec::with_capacity(AGENT_DISCLOSURE_HEADER_BYTES);
    header.extend_from_slice(AGENT_DISCLOSURE_MAGIC);
    header.extend_from_slice(&AGENT_DISCLOSURE_ENVELOPE_VERSION.to_be_bytes());
    header.extend_from_slice(&nonce);
    header.extend_from_slice(&ciphertext_length.to_be_bytes());
    let aad = agent_disclosure_aad(account, &account_digest, &native_root_digest, &header)?;
    let key = derive_agent_disclosure_key(root_key, account, &account_digest, &native_root_digest)?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_slice())
        .map_err(|_| agent_disclosure_crypto_error())?;
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: &plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| agent_disclosure_crypto_error())?;
    let mut envelope = Zeroizing::new(header);
    envelope.extend_from_slice(&ciphertext);
    if envelope.len() > MAX_AGENT_DISCLOSURE_ENVELOPE_BYTES {
        return Err(agent_disclosure_format_error());
    }
    Ok(envelope)
}

fn decrypt_agent_disclosure_envelope(
    envelope: &[u8],
    root_key: &HouseholdKeyMaterial,
    account: &AccountId,
    account_digest: [u8; 32],
    native_root_digest: [u8; 32],
) -> Result<AgentDisclosureLedgerV1, PortError> {
    if envelope.len() < AGENT_DISCLOSURE_HEADER_BYTES + 16
        || envelope.len() > MAX_AGENT_DISCLOSURE_ENVELOPE_BYTES
        || &envelope[..8] != AGENT_DISCLOSURE_MAGIC
        || u16::from_be_bytes(
            envelope[8..10]
                .try_into()
                .map_err(|_| agent_disclosure_format_error())?,
        ) != AGENT_DISCLOSURE_ENVELOPE_VERSION
    {
        return Err(agent_disclosure_format_error());
    }
    let nonce = XNonce::from(
        <[u8; 24]>::try_from(&envelope[10..34]).map_err(|_| agent_disclosure_format_error())?,
    );
    let ciphertext_length = usize::try_from(u32::from_be_bytes(
        envelope[34..38]
            .try_into()
            .map_err(|_| agent_disclosure_format_error())?,
    ))
    .map_err(|_| agent_disclosure_format_error())?;
    if ciphertext_length != envelope.len() - AGENT_DISCLOSURE_HEADER_BYTES {
        return Err(agent_disclosure_format_error());
    }
    let header = &envelope[..AGENT_DISCLOSURE_HEADER_BYTES];
    let aad = agent_disclosure_aad(account, &account_digest, &native_root_digest, header)?;
    let key = derive_agent_disclosure_key(root_key, account, &account_digest, &native_root_digest)?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_slice())
        .map_err(|_| agent_disclosure_crypto_error())?;
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &envelope[AGENT_DISCLOSURE_HEADER_BYTES..],
                    aad: &aad,
                },
            )
            .map_err(|_| agent_disclosure_crypto_error())?,
    );
    AgentDisclosureLedgerV1::decode_canonical(&plaintext, account)
        .map_err(agent_disclosure_contract_error)
}

fn agent_disclosure_contract_error(_: AgentHouseholdContractErrorV1) -> PortError {
    PortError::new(
        "household_agent_disclosure_invalid",
        "local household agent disclosure authority is invalid",
    )
}

fn agent_disclosure_format_error() -> PortError {
    PortError::new(
        "household_agent_disclosure_format",
        "local household agent disclosure storage is invalid",
    )
}

fn agent_disclosure_crypto_error() -> PortError {
    PortError::new(
        "household_agent_disclosure_crypto",
        "local household agent disclosure storage could not be authenticated",
    )
}

fn agent_disclosure_now() -> Result<CanonicalTimestampV1, PortError> {
    CanonicalTimestampV1::from_datetime(OffsetDateTime::now_utc()).map_err(|_| {
        PortError::new(
            "household_agent_disclosure_clock",
            "local household agent disclosure time is unavailable",
        )
    })
}

fn agent_disclosure_reconciliation_error() -> PortError {
    PortError::uncertain(
        "household_agent_disclosure_reconciliation",
        "local household agent disclosure requires reconciliation",
    )
}

fn agent_household_read_contract_error() -> PortError {
    PortError::new(
        "household_agent_read_contract",
        "native household data cannot be represented by the closed agent read contract",
    )
}

fn agent_household_subject_unavailable_error() -> PortError {
    PortError::new(
        "household_agent_subject_unavailable",
        "the requested active household subject is unavailable",
    )
}

fn agent_household_lifecycle_unavailable_error() -> PortError {
    PortError::new(
        "household_agent_lifecycle_unavailable",
        "agent household lifecycle operations are not active in this release phase",
    )
}

fn commit_evidence_now_unix_seconds() -> Result<u64, PortError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| {
            PortError::new(
                "household_commit_evidence_clock",
                "household commit evidence clock is unavailable",
            )
        })
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
