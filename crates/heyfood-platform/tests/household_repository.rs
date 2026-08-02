#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

use heyfood_application::{
    BoxFuture, CreateMemberWithDeclaredProfileV1, HouseholdCommit, HouseholdErase,
    HouseholdInitialize, HouseholdRepositoryPort, HouseholdRepositoryResolutionV1,
    NativeHouseholdModeV1, NativeMemberAgeEvidenceV1, PortError, resolve_household_initialize_v1,
};
use heyfood_core::{
    AccountId, AgentDisclosurePurposeV1, AgentHouseholdOperationV1, AgentHouseholdProjectionV1,
    AgentHouseholdProposalIdV1, AppliedCommitOutcomeV1, CanonicalDigestV1, CanonicalTimestampV1,
    CommitId, DisplayName, GenerationId, HOUSEHOLD_STATE_SCHEMA_VERSION,
    HouseholdCommitEvidenceBindingV1, HouseholdEffectV1, HouseholdOwnerV1, HouseholdProfileStateV1,
    HouseholdRevision, HouseholdScope, HouseholdStateV1, HouseholdSubjectId,
    ImportedCompatibilityStateV1, LegacySourceIdentityV1, LocalHouseholdAuthoritySnapshotV1,
    LocalHouseholdFrozenCandidateV1, LocalHouseholdProposalAuthorityV1,
    LocalHouseholdProposalBindingV1, LocalHouseholdProposalJournalV1,
    MigrationDispositionManifestV1, MigrationProvenanceV1, OnboardingProfileInput, RelationshipV1,
    canonical_sha256_v1,
};
use heyfood_platform::{
    HouseholdKeyBundle, HouseholdKeyBundlePhase, HouseholdKeyMaterial, HouseholdKeyStore,
    HouseholdMigrationGuardDocument, HouseholdMigrationGuardStateV1, HouseholdMigrationGuardStore,
    HouseholdMigrationSourceIdentityV1, HouseholdVault, HouseholdVaultLease,
    HouseholdVaultLeaseModeV1, HouseholdVaultWrite, InMemoryHouseholdSecureStore,
    KeyBundleRevision, KeyId, KeyStoreExpectation, MigrationGuardExpectation,
    NativeHouseholdMutationAuthorityV1, NativeHouseholdRepository,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "heyfood-household-repository-{name}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("temp root");
        Self(path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct PreparedRepository {
    _root: TempRoot,
    account: AccountId,
    vault: HouseholdVault,
    store: Arc<InMemoryHouseholdSecureStore>,
    command: HouseholdInitialize,
    final_state: HouseholdStateV1,
}

#[derive(Clone)]
struct UncertainKeyInitializeStore {
    inner: Arc<InMemoryHouseholdSecureStore>,
    uncertain_once: Arc<AtomicBool>,
    initialize_calls: Arc<AtomicUsize>,
    loaded_key_override: Option<HouseholdKeyBundle>,
}

impl UncertainKeyInitializeStore {
    fn new(inner: Arc<InMemoryHouseholdSecureStore>) -> Self {
        Self {
            inner,
            uncertain_once: Arc::new(AtomicBool::new(true)),
            initialize_calls: Arc::new(AtomicUsize::new(0)),
            loaded_key_override: None,
        }
    }

    fn with_loaded_key(inner: Arc<InMemoryHouseholdSecureStore>, key: HouseholdKeyBundle) -> Self {
        Self {
            inner,
            uncertain_once: Arc::new(AtomicBool::new(false)),
            initialize_calls: Arc::new(AtomicUsize::new(0)),
            loaded_key_override: Some(key),
        }
    }
}

impl HouseholdKeyStore for UncertainKeyInitializeStore {
    fn load<'a>(
        &'a self,
        lifecycle_lease: &'a heyfood_platform::HouseholdLifecycleLease,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<Option<HouseholdKeyBundle>, PortError>> {
        if let Some(key) = &self.loaded_key_override {
            let key = key.clone();
            return Box::pin(async move {
                if cancellation.is_cancelled() {
                    return Err(PortError::new(
                        "household_operation_cancelled",
                        "injected key load was cancelled",
                    ));
                }
                let _ = lifecycle_lease.account_slot();
                Ok(Some(key))
            });
        }
        HouseholdKeyStore::load(self.inner.as_ref(), lifecycle_lease, cancellation)
    }

    fn initialize<'a>(
        &'a self,
        vault_lease: &'a mut HouseholdVaultLease,
        expected: KeyStoreExpectation,
        expected_guard: HouseholdMigrationGuardDocument,
        bundle: HouseholdKeyBundle,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<(), PortError>> {
        Box::pin(async move {
            self.initialize_calls.fetch_add(1, Ordering::SeqCst);
            HouseholdKeyStore::initialize(
                self.inner.as_ref(),
                vault_lease,
                expected,
                expected_guard,
                bundle,
                cancellation,
            )
            .await?;
            if self.uncertain_once.swap(false, Ordering::SeqCst) {
                return Err(PortError::uncertain(
                    "injected_key_initialize_uncertain",
                    "injected key initialization uncertainty",
                ));
            }
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
        HouseholdKeyStore::compare_exchange(
            self.inner.as_ref(),
            vault_lease,
            expected,
            replacement,
            cancellation,
        )
    }

    fn delete_and_verify<'a>(
        &'a self,
        vault_lease: &'a mut HouseholdVaultLease,
        expected_revision: KeyBundleRevision,
        expected_key_id: KeyId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<(), PortError>> {
        HouseholdKeyStore::delete_and_verify(
            self.inner.as_ref(),
            vault_lease,
            expected_revision,
            expected_key_id,
            cancellation,
        )
    }

    fn abort_initialization_and_verify<'a>(
        &'a self,
        vault_lease: &'a mut HouseholdVaultLease,
        expected_revision: KeyBundleRevision,
        expected_initialization_id: Uuid,
        expected_aborting_guard: HouseholdMigrationGuardDocument,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<(), PortError>> {
        HouseholdKeyStore::abort_initialization_and_verify(
            self.inner.as_ref(),
            vault_lease,
            expected_revision,
            expected_initialization_id,
            expected_aborting_guard,
            cancellation,
        )
    }
}

impl HouseholdMigrationGuardStore for UncertainKeyInitializeStore {
    fn load<'a>(
        &'a self,
        lifecycle_lease: &'a heyfood_platform::HouseholdLifecycleLease,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<Option<HouseholdMigrationGuardDocument>, PortError>> {
        HouseholdMigrationGuardStore::load(self.inner.as_ref(), lifecycle_lease, cancellation)
    }

    fn compare_exchange<'a>(
        &'a self,
        vault_lease: &'a mut HouseholdVaultLease,
        expected: MigrationGuardExpectation,
        replacement: Option<HouseholdMigrationGuardDocument>,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<(), PortError>> {
        HouseholdMigrationGuardStore::compare_exchange(
            self.inner.as_ref(),
            vault_lease,
            expected,
            replacement,
            cancellation,
        )
    }
}

fn fixed_uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).expect("fixed UUID")
}

fn timestamp(second: u8) -> CanonicalTimestampV1 {
    CanonicalTimestampV1::parse(format!("2026-07-30T12:00:{second:02}.000Z")).expect("timestamp")
}

fn initial_state(
    account: AccountId,
    commit_id: CommitId,
    migration_id: Uuid,
    initialization_id: Uuid,
) -> HouseholdStateV1 {
    let at = timestamp(0);
    HouseholdStateV1 {
        schema_version: HOUSEHOLD_STATE_SCHEMA_VERSION,
        account_binding: account,
        revision: HouseholdRevision::new(1).expect("revision"),
        owner: HouseholdOwnerV1 {
            display_name: DisplayName::parse("Owner").expect("display name"),
            relationship: RelationshipV1::Self_,
            profile_state: HouseholdProfileStateV1::Incomplete,
            created_at: at.clone(),
            updated_at: at.clone(),
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
            migration_id,
            initialization_id,
            initial_commit_id: commit_id,
            migration_frozen_at: at.clone(),
        },
        updated_at: at,
    }
}

async fn prepare_repository(name: &str) -> PreparedRepository {
    prepare_repository_with(name, |_| {}).await
}

async fn prepare_repository_with(
    name: &str,
    mutate_final_state: impl FnOnce(&mut HouseholdStateV1),
) -> PreparedRepository {
    let root = TempRoot::new(name);
    let account = AccountId::parse(format!("account-{name}")).expect("account");
    let vault =
        HouseholdVault::open(&root.0.join("data"), account.clone()).expect("household vault");
    let store = Arc::new(InMemoryHouseholdSecureStore::default());
    let migration_id = fixed_uuid("11111111-1111-4111-8111-111111111111");
    let initialization_id = fixed_uuid("22222222-2222-4222-8222-222222222222");
    let commit_id = CommitId::from_uuid(fixed_uuid("33333333-3333-4333-8333-333333333333"));
    let candidate = initial_state(account.clone(), commit_id, migration_id, initialization_id);
    let command = HouseholdInitialize::new(
        account.clone(),
        commit_id,
        candidate,
        HouseholdEffectV1::Initialize,
        timestamp(0),
    )
    .expect("initialize command");
    let HouseholdRepositoryResolutionV1::Write { state, .. } =
        resolve_household_initialize_v1(None, &command).expect("resolve initialization")
    else {
        panic!("new initialization must produce a write");
    };
    let mut final_state = *state;
    mutate_final_state(&mut final_state);
    final_state.validate().expect("mutated final state");
    let state_digest = *canonical_sha256_v1(&final_state)
        .expect("state digest")
        .as_bytes();
    let reserved = HouseholdMigrationGuardDocument::initializing_reserved(
        vault.account_slot(),
        HouseholdMigrationSourceIdentityV1::no_source([7; 32]),
        migration_id,
        initialization_id,
        commit_id.as_uuid(),
        timestamp(0),
    )
    .expect("reserved guard");
    let ready = reserved
        .ready_to_initialize(
            *command.claimed_effect_fingerprint.as_digest().as_bytes(),
            state_digest,
        )
        .expect("ready guard");
    let lifecycle = vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .expect("lifecycle lease");
    let mut lease = vault
        .acquire_vault_lease(
            lifecycle,
            HouseholdVaultLeaseModeV1::CreateIfMissing,
            CancellationToken::new(),
        )
        .await
        .expect("vault lease");
    HouseholdMigrationGuardStore::compare_exchange(
        store.as_ref(),
        &mut lease,
        MigrationGuardExpectation::Absent,
        Some(reserved.clone()),
        CancellationToken::new(),
    )
    .await
    .expect("reserve guard");
    HouseholdMigrationGuardStore::compare_exchange(
        store.as_ref(),
        &mut lease,
        MigrationGuardExpectation::Revision(reserved.guard_revision()),
        Some(ready.clone()),
        CancellationToken::new(),
    )
    .await
    .expect("ready guard");
    let key = HouseholdKeyBundle::initializing(
        vault.account_slot(),
        KeyBundleRevision::new(1).expect("key revision"),
        KeyId::from_uuid(fixed_uuid("44444444-4444-4444-8444-444444444444")),
        HouseholdKeyMaterial::from_bytes([0x5a; 32]),
        initialization_id,
        commit_id.as_uuid(),
        *command.claimed_effect_fingerprint.as_digest().as_bytes(),
        state_digest,
    );
    HouseholdKeyStore::initialize(
        store.as_ref(),
        &mut lease,
        KeyStoreExpectation::Absent,
        ready,
        key,
        CancellationToken::new(),
    )
    .await
    .expect("initialize key");
    drop(lease);
    PreparedRepository {
        _root: root,
        account,
        vault,
        store,
        command,
        final_state,
    }
}

fn repository(
    prepared: &PreparedRepository,
    mode: NativeHouseholdModeV1,
) -> NativeHouseholdRepository {
    NativeHouseholdRepository::from_vault(
        prepared.account.clone(),
        prepared.vault.clone(),
        prepared.store.clone(),
        mode,
    )
    .expect("repository")
}

fn artifact_bytes(directory: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut artifacts = std::fs::read_dir(directory)
        .expect("household directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .map(|path| {
            let bytes = std::fs::read(&path).expect("artifact bytes");
            (path, bytes)
        })
        .collect::<Vec<_>>();
    artifacts.sort_by(|left, right| left.0.cmp(&right.0));
    artifacts
}

async fn secure_documents(
    prepared: &PreparedRepository,
) -> (HouseholdMigrationGuardDocument, Option<HouseholdKeyBundle>) {
    let lifecycle = prepared
        .vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .expect("lifecycle lease");
    let guard = HouseholdMigrationGuardStore::load(
        prepared.store.as_ref(),
        &lifecycle,
        CancellationToken::new(),
    )
    .await
    .expect("guard load")
    .expect("guard");
    let key = HouseholdKeyStore::load(
        prepared.store.as_ref(),
        &lifecycle,
        CancellationToken::new(),
    )
    .await
    .expect("key load");
    (guard, key)
}

async fn rotate_and_finalize_household_key(prepared: &PreparedRepository) -> (KeyId, KeyId) {
    let lifecycle = prepared
        .vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .expect("lifecycle lease");
    let mut lease = prepared
        .vault
        .acquire_vault_lease(
            lifecycle,
            HouseholdVaultLeaseModeV1::RequireExisting,
            CancellationToken::new(),
        )
        .await
        .expect("vault lease");
    let previous = HouseholdKeyStore::load(
        prepared.store.as_ref(),
        lease.lifecycle_lease(),
        CancellationToken::new(),
    )
    .await
    .expect("key load")
    .expect("stable key");
    assert_eq!(previous.phase, HouseholdKeyBundlePhase::Stable);
    let old_key_id = previous.active_key_id;
    let new_key_id = KeyId::new();
    let rewriting = HouseholdKeyBundle::rewriting(
        prepared.vault.account_slot(),
        previous
            .revision
            .checked_next()
            .expect("rewriting revision"),
        new_key_id,
        HouseholdKeyMaterial::generate().expect("new household key"),
        &previous,
        Uuid::new_v4(),
    )
    .expect("rewriting key bundle");
    HouseholdKeyStore::compare_exchange(
        prepared.store.as_ref(),
        &mut lease,
        previous.revision,
        rewriting.clone(),
        CancellationToken::new(),
    )
    .await
    .expect("publish rewriting key bundle");
    prepared
        .vault
        .rotate(&mut lease, rewriting.clone(), CancellationToken::new())
        .await
        .expect("rotate household vault");
    let finalized = rewriting
        .stabilized(
            prepared.vault.account_slot(),
            rewriting.revision.checked_next().expect("stable revision"),
        )
        .expect("stable rotated key bundle");
    HouseholdKeyStore::compare_exchange(
        prepared.store.as_ref(),
        &mut lease,
        rewriting.revision,
        finalized.clone(),
        CancellationToken::new(),
    )
    .await
    .expect("finalize rotated key bundle");
    assert_eq!(finalized.phase, HouseholdKeyBundlePhase::Stable);
    assert!(finalized.previous_key.is_none());
    (old_key_id, new_key_id)
}

async fn delete_initializing_key(prepared: &PreparedRepository) -> HouseholdKeyBundle {
    let lifecycle = prepared
        .vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .expect("lifecycle lease");
    let mut lease = prepared
        .vault
        .acquire_vault_lease(
            lifecycle,
            HouseholdVaultLeaseModeV1::CreateIfMissing,
            CancellationToken::new(),
        )
        .await
        .expect("vault lease");
    let key = HouseholdKeyStore::load(
        prepared.store.as_ref(),
        lease.lifecycle_lease(),
        CancellationToken::new(),
    )
    .await
    .expect("key load")
    .expect("initializing key");
    HouseholdKeyStore::delete_and_verify(
        prepared.store.as_ref(),
        &mut lease,
        key.revision,
        key.active_key_id,
        CancellationToken::new(),
    )
    .await
    .expect("delete initializing key");
    key
}

async fn commit_ready_vault(prepared: &PreparedRepository, stabilize_key: bool) {
    let lifecycle = prepared
        .vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .expect("lifecycle lease");
    let mut lease = prepared
        .vault
        .acquire_vault_lease(
            lifecycle,
            HouseholdVaultLeaseModeV1::CreateIfMissing,
            CancellationToken::new(),
        )
        .await
        .expect("vault lease");
    let key = HouseholdKeyStore::load(
        prepared.store.as_ref(),
        lease.lifecycle_lease(),
        CancellationToken::new(),
    )
    .await
    .expect("key load")
    .expect("initializing key");
    let write = HouseholdVaultWrite::new(
        prepared.final_state.revision.get(),
        prepared.command.commit_id.as_uuid(),
        prepared
            .final_state
            .canonical_bytes()
            .expect("canonical state"),
    )
    .expect("vault write");
    prepared
        .vault
        .initialize(&mut lease, key.clone(), write, CancellationToken::new())
        .await
        .expect("commit initialization");
    if stabilize_key {
        let stable = HouseholdKeyBundle::stable(
            prepared.vault.account_slot(),
            key.revision.checked_next().expect("next key revision"),
            key.active_key_id,
            key.active_key.clone(),
        );
        HouseholdKeyStore::compare_exchange(
            prepared.store.as_ref(),
            &mut lease,
            key.revision,
            stable,
            CancellationToken::new(),
        )
        .await
        .expect("stabilize key");
    }
}

async fn complete_ready_guard_and_key(prepared: &PreparedRepository) {
    commit_ready_vault(prepared, true).await;
    let lifecycle = prepared
        .vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .expect("lifecycle lease");
    let mut lease = prepared
        .vault
        .acquire_vault_lease(
            lifecycle,
            HouseholdVaultLeaseModeV1::RequireExisting,
            CancellationToken::new(),
        )
        .await
        .expect("vault lease");
    let guard = HouseholdMigrationGuardStore::load(
        prepared.store.as_ref(),
        lease.lifecycle_lease(),
        CancellationToken::new(),
    )
    .await
    .expect("guard load")
    .expect("ready guard");
    let completed = guard.complete_initialization().expect("complete guard");
    HouseholdMigrationGuardStore::compare_exchange(
        prepared.store.as_ref(),
        &mut lease,
        MigrationGuardExpectation::Revision(guard.guard_revision()),
        Some(completed),
        CancellationToken::new(),
    )
    .await
    .expect("complete guard");
}

fn corrupt_artifact(path: &Path) {
    let mut bytes = std::fs::read(path).expect("artifact bytes");
    let index = bytes.len() / 2;
    bytes[index] ^= 0x80;
    std::fs::write(path, bytes).expect("corrupt artifact");
}

fn install_teardown_barrier(prepared: &PreparedRepository) {
    let native_root = prepared._root.0.join("data");
    let directory = native_root.join("household-teardown");
    std::fs::create_dir_all(&directory).expect("teardown directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .expect("teardown directory mode");
    }
    let mut digest = String::with_capacity(64);
    for byte in prepared.vault.account_slot().account_digest() {
        use std::fmt::Write as _;
        let _ = write!(digest, "{byte:02x}");
    }
    let path = directory.join(format!("teardown-{digest}.htj"));
    std::fs::write(&path, b"pending").expect("teardown journal");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .expect("teardown journal mode");
    }
}

fn next_commit(
    current: &HouseholdStateV1,
    account: &AccountId,
    expected_revision: HouseholdRevision,
    commit_id: Uuid,
    at: CanonicalTimestampV1,
) -> HouseholdCommit {
    let mut candidate = current.clone();
    candidate.revision =
        HouseholdRevision::new(expected_revision.get() + 1).expect("resulting revision");
    candidate.updated_at = at.clone();
    HouseholdCommit::new(
        account.clone(),
        expected_revision,
        CommitId::from_uuid(commit_id),
        candidate,
        HouseholdEffectV1::SelectScope {
            scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
        },
        at,
    )
    .expect("commit command")
}

#[tokio::test]
async fn initialize_load_commit_and_exact_replay_are_live_and_copy_on_write() {
    let prepared = prepare_repository("live").await;
    let repository = repository(&prepared, NativeHouseholdModeV1::NativeEnabled);
    assert!(
        repository
            .load(&prepared.account, CancellationToken::new())
            .await
            .expect("ready load")
            .is_none(),
        "an exact ready transaction is not readable before finalization"
    );
    let initialized = repository
        .initialize(prepared.command.clone(), CancellationToken::new())
        .await
        .expect("initialize");
    assert_eq!(initialized.outcome, AppliedCommitOutcomeV1::Initialized);
    let initialized_artifacts = artifact_bytes(&prepared.vault.household_directory());
    let initialization_replay = repository
        .initialize(prepared.command.clone(), CancellationToken::new())
        .await
        .expect("initialization replay");
    assert_eq!(initialization_replay, initialized);
    assert_eq!(
        artifact_bytes(&prepared.vault.household_directory()),
        initialized_artifacts,
        "initialization replay must perform no second vault write"
    );
    let loaded = repository
        .load(&prepared.account, CancellationToken::new())
        .await
        .expect("load")
        .expect("state");
    assert_eq!(loaded.state, prepared.final_state);

    let command = next_commit(
        &loaded.state,
        &prepared.account,
        loaded.state.revision,
        fixed_uuid("55555555-5555-4555-8555-555555555555"),
        timestamp(1),
    );
    let committed = repository
        .commit(command.clone(), CancellationToken::new())
        .await
        .expect("commit");
    assert_eq!(committed.outcome, AppliedCommitOutcomeV1::Committed);
    let artifacts_after_commit = artifact_bytes(&prepared.vault.household_directory());
    let replayed = repository
        .commit(command, CancellationToken::new())
        .await
        .expect("replay");
    assert_eq!(replayed, committed);
    assert_eq!(
        artifact_bytes(&prepared.vault.household_directory()),
        artifacts_after_commit,
        "exact replay must perform no second vault write"
    );
    let reloaded = repository
        .load(&prepared.account, CancellationToken::new())
        .await
        .expect("reload")
        .expect("state");
    assert_eq!(reloaded.state.revision, committed.resulting_revision);
}

#[tokio::test]
async fn commit_evidence_is_rederived_after_repository_reopen_and_ignores_synthesized_state() {
    let prepared = prepare_repository("commit-evidence-reopen").await;
    let native_repository = repository(&prepared, NativeHouseholdModeV1::NativeEnabled);
    native_repository
        .initialize(prepared.command.clone(), CancellationToken::new())
        .await
        .expect("initialize repository");
    let loaded = native_repository
        .load(&prepared.account, CancellationToken::new())
        .await
        .expect("load repository")
        .expect("initialized state");
    let proposal_ref = AgentHouseholdProposalIdV1::new();
    let commit_id = CommitId::from_uuid(fixed_uuid("55555555-5555-4555-8555-555555555555"));
    let command = next_commit(
        &loaded.state,
        &prepared.account,
        loaded.state.revision,
        commit_id.as_uuid(),
        timestamp(1),
    );
    let evidence = native_repository
        .reserve_agent_commit_evidence(proposal_ref, commit_id, CancellationToken::new())
        .await
        .expect("repository reserves evidence verifier");
    let proposal_digest = CanonicalDigestV1::from_bytes([0x61; 32]);
    let disclosure_digest = CanonicalDigestV1::from_bytes([0x62; 32]);
    let lifecycle_generation = GenerationId::new(7);
    let disclosure_generation = GenerationId::new(8);
    let binding = LocalHouseholdProposalBindingV1::new(
        prepared.account.clone(),
        proposal_ref,
        AgentHouseholdOperationV1::Scope,
        disclosure_generation,
        disclosure_digest,
        AgentDisclosurePurposeV1::HouseholdAgentProposalStatus,
        lifecycle_generation,
        AgentHouseholdProjectionV1::ContentFree,
        loaded.state.revision,
        None,
        commit_id,
        evidence.clone(),
        None,
        loaded.state.active_scope.clone(),
        CanonicalDigestV1::from_bytes([0x63; 32]),
        CanonicalDigestV1::from_bytes([0x64; 32]),
        timestamp(0),
        CanonicalTimestampV1::parse("2026-07-30T12:10:00.000Z").expect("expiry"),
    )
    .expect("proposal binding");
    let authority = LocalHouseholdAuthoritySnapshotV1::new(
        prepared.account.clone(),
        disclosure_generation,
        disclosure_digest,
        AgentDisclosurePurposeV1::HouseholdAgentProposalStatus,
        true,
        AgentHouseholdProjectionV1::ContentFree,
        lifecycle_generation,
        loaded.state.revision,
        None,
        timestamp(1),
    );
    let frozen = LocalHouseholdFrozenCandidateV1::new(
        proposal_digest,
        command.claimed_effect_fingerprint,
        CanonicalDigestV1::from_bytes([0x65; 32]),
        CanonicalDigestV1::from_bytes([0x66; 32]),
        loaded.state.active_scope.clone(),
        false,
        timestamp(1),
    );
    let mut journal =
        LocalHouseholdProposalJournalV1::new(LocalHouseholdProposalAuthorityV1::prepared(binding))
            .expect("proposal journal");
    let prepared_token = journal.cas_token();
    journal
        .freeze_for_review(&prepared_token, &authority, frozen)
        .expect("freeze proposal");
    let review_token = journal.cas_token();
    journal
        .begin_commit(&review_token, &authority, proposal_digest)
        .expect("begin commit");
    let committing_bytes = journal.persisted_bytes().expect("persist journal");

    drop(native_repository);
    let reopened = repository(&prepared, NativeHouseholdModeV1::NativeEnabled);
    let unapplied = reopened
        .prove_unapplied_agent_commit(
            &evidence,
            proposal_ref,
            commit_id,
            loaded.state.revision,
            CancellationToken::new(),
        )
        .await
        .expect("reopened repository proves exact absence");
    let mut absent_journal =
        LocalHouseholdProposalJournalV1::restore(&committing_bytes).expect("restore journal");
    let committing_token = absent_journal.cas_token();
    absent_journal
        .mark_reconciliation_required(&committing_token)
        .expect("mark uncertain");
    let reconciliation_token = absent_journal.cas_token();
    absent_journal
        .reconcile_unapplied_commit(&reconciliation_token, &unapplied)
        .expect("close exact absence");

    let forged = HouseholdCommitEvidenceBindingV1::from_repository_secret(
        prepared.account.clone(),
        proposal_ref,
        commit_id,
        &[0xa7; 32],
    );
    let mut synthesized_state = loaded.state.clone();
    synthesized_state.updated_at = timestamp(9);
    assert_ne!(synthesized_state.updated_at, loaded.state.updated_at);
    let forged_error = reopened
        .prove_unapplied_agent_commit(
            &forged,
            proposal_ref,
            commit_id,
            synthesized_state.revision,
            CancellationToken::new(),
        )
        .await
        .expect_err("proposal-created verifier cannot replace repository authority");
    assert_eq!(forged_error.code, "household_commit_evidence_mismatch");

    let denied = reopened
        .commit(command.clone(), CancellationToken::new())
        .await
        .expect_err("authoritative absence permanently fences the exact commit");
    assert_eq!(denied.code, "household_commit_permanently_denied");

    let applied_proposal_ref = AgentHouseholdProposalIdV1::new();
    let applied_commit_id = CommitId::from_uuid(fixed_uuid("56565656-5656-4656-8656-565656565656"));
    let applied_command = next_commit(
        &loaded.state,
        &prepared.account,
        loaded.state.revision,
        applied_commit_id.as_uuid(),
        timestamp(2),
    );
    let applied_evidence = reopened
        .reserve_agent_commit_evidence(
            applied_proposal_ref,
            applied_commit_id,
            CancellationToken::new(),
        )
        .await
        .expect("reserve applied evidence");
    reopened
        .commit(applied_command, CancellationToken::new())
        .await
        .expect("commit separate exact proposal effect");
    drop(reopened);
    let reopened_after_commit = repository(&prepared, NativeHouseholdModeV1::NativeEnabled);
    let _applied = reopened_after_commit
        .prove_applied_agent_commit(
            &applied_evidence,
            applied_proposal_ref,
            applied_commit_id,
            CancellationToken::new(),
        )
        .await
        .expect("reopened repository proves applied ledger entry");
}

#[tokio::test]
async fn commit_evidence_reservations_survive_finalized_key_rotation_for_both_outcomes() {
    let prepared = prepare_repository("commit-evidence-key-rotation").await;
    let initial_repository = repository(&prepared, NativeHouseholdModeV1::NativeEnabled);
    initial_repository
        .initialize(prepared.command.clone(), CancellationToken::new())
        .await
        .expect("initialize repository");
    let loaded = initial_repository
        .load(&prepared.account, CancellationToken::new())
        .await
        .expect("load repository")
        .expect("initialized state");

    let applied_proposal = AgentHouseholdProposalIdV1::new();
    let applied_commit = CommitId::from_uuid(fixed_uuid("57575757-5757-4757-8757-575757575757"));
    let applied_command = next_commit(
        &loaded.state,
        &prepared.account,
        loaded.state.revision,
        applied_commit.as_uuid(),
        timestamp(3),
    );
    let applied_binding = initial_repository
        .reserve_agent_commit_evidence(applied_proposal, applied_commit, CancellationToken::new())
        .await
        .expect("reserve applied evidence before rotation");

    let absent_proposal = AgentHouseholdProposalIdV1::new();
    let absent_commit = CommitId::from_uuid(fixed_uuid("58585858-5858-4858-8858-585858585858"));
    let absent_command = next_commit(
        &loaded.state,
        &prepared.account,
        loaded.state.revision,
        absent_commit.as_uuid(),
        timestamp(4),
    );
    let absent_binding = initial_repository
        .reserve_agent_commit_evidence(absent_proposal, absent_commit, CancellationToken::new())
        .await
        .expect("reserve absence evidence before rotation");
    drop(initial_repository);

    let (old_key_id, new_key_id) = rotate_and_finalize_household_key(&prepared).await;
    assert_ne!(old_key_id, new_key_id);
    let (_, finalized_key) = secure_documents(&prepared).await;
    let finalized_key = finalized_key.expect("finalized key");
    assert_eq!(finalized_key.active_key_id, new_key_id);
    assert!(finalized_key.previous_key.is_none());

    let reopened = repository(&prepared, NativeHouseholdModeV1::NativeEnabled);
    let _absence = reopened
        .prove_unapplied_agent_commit(
            &absent_binding,
            absent_proposal,
            absent_commit,
            loaded.state.revision,
            CancellationToken::new(),
        )
        .await
        .expect("prove exact absence after finalized rotation");
    let denied = reopened
        .commit(absent_command, CancellationToken::new())
        .await
        .expect_err("absence proof fences delayed exact commit");
    assert_eq!(denied.code, "household_commit_permanently_denied");

    reopened
        .commit(applied_command, CancellationToken::new())
        .await
        .expect("apply reserved commit after finalized rotation");
    drop(reopened);
    let reopened_after_apply = repository(&prepared, NativeHouseholdModeV1::NativeEnabled);
    let _applied = reopened_after_apply
        .prove_applied_agent_commit(
            &applied_binding,
            applied_proposal,
            applied_commit,
            CancellationToken::new(),
        )
        .await
        .expect("prove applied ledger entry after rotation and restart");
    let already_applied = reopened_after_apply
        .reserve_agent_commit_evidence(
            AgentHouseholdProposalIdV1::new(),
            applied_commit,
            CancellationToken::new(),
        )
        .await
        .expect_err("an applied commit cannot acquire a later reservation");
    assert_eq!(already_applied.code, "household_commit_evidence_mismatch");
}

#[tokio::test]
async fn commit_dispatch_and_unapplied_proof_have_one_linearizable_winner() {
    let prepared = prepare_repository("commit-evidence-race").await;
    let repository = repository(&prepared, NativeHouseholdModeV1::NativeEnabled);
    repository
        .initialize(prepared.command.clone(), CancellationToken::new())
        .await
        .expect("initialize repository");
    let loaded = repository
        .load(&prepared.account, CancellationToken::new())
        .await
        .expect("load repository")
        .expect("initialized state");
    let proposal_ref = AgentHouseholdProposalIdV1::new();
    let commit_id = CommitId::from_uuid(fixed_uuid("59595959-5959-4959-8959-595959595959"));
    let command = next_commit(
        &loaded.state,
        &prepared.account,
        loaded.state.revision,
        commit_id.as_uuid(),
        timestamp(5),
    );
    let binding = repository
        .reserve_agent_commit_evidence(proposal_ref, commit_id, CancellationToken::new())
        .await
        .expect("reserve race evidence");

    let (commit_result, absence_result) = tokio::join!(
        repository.commit(command.clone(), CancellationToken::new()),
        repository.prove_unapplied_agent_commit(
            &binding,
            proposal_ref,
            commit_id,
            loaded.state.revision,
            CancellationToken::new(),
        )
    );
    match (commit_result, absence_result) {
        (Ok(_), Err(error)) => {
            assert_eq!(error.code, "household_commit_evidence_mismatch");
            repository
                .prove_applied_agent_commit(
                    &binding,
                    proposal_ref,
                    commit_id,
                    CancellationToken::new(),
                )
                .await
                .expect("commit winner remains provably applied");
        }
        (Err(error), Ok(_)) => {
            assert_eq!(error.code, "household_commit_permanently_denied");
            let readback = repository
                .load(&prepared.account, CancellationToken::new())
                .await
                .expect("load after absence winner")
                .expect("state after absence winner");
            assert_eq!(readback.state.revision, loaded.state.revision);
            let denied_replay = repository
                .commit(command, CancellationToken::new())
                .await
                .expect_err("absence winner permanently fences replay");
            assert_eq!(denied_replay.code, "household_commit_permanently_denied");
        }
        (Ok(_), Ok(_)) => panic!("commit and authoritative absence cannot both win"),
        (Err(commit_error), Err(absence_error)) => panic!(
            "race must have one winner: commit={}, absence={}",
            commit_error.code, absence_error.code
        ),
    }
}

#[tokio::test]
async fn undispatched_cancellation_releases_reservations_without_capacity_leak() {
    let prepared = prepare_repository("commit-evidence-release").await;
    let repository = repository(&prepared, NativeHouseholdModeV1::NativeEnabled);
    repository
        .initialize(prepared.command.clone(), CancellationToken::new())
        .await
        .expect("initialize repository");
    let loaded = repository
        .load(&prepared.account, CancellationToken::new())
        .await
        .expect("load repository")
        .expect("initialized state");

    for _ in 0..80 {
        let proposal_ref = AgentHouseholdProposalIdV1::new();
        let commit_id = CommitId::new();
        let binding = repository
            .reserve_agent_commit_evidence(proposal_ref, commit_id, CancellationToken::new())
            .await
            .expect("reserve pre-dispatch evidence");
        repository
            .release_undispatched_agent_commit_evidence(
                &binding,
                proposal_ref,
                commit_id,
                CancellationToken::new(),
            )
            .await
            .expect("release cancelled pre-dispatch evidence");
        let released = repository
            .prove_unapplied_agent_commit(
                &binding,
                proposal_ref,
                commit_id,
                loaded.state.revision,
                CancellationToken::new(),
            )
            .await
            .expect_err("released reservation cannot issue a later proof");
        assert_eq!(released.code, "household_commit_evidence_mismatch");
    }
}

#[tokio::test]
async fn retained_read_lease_blocks_cross_process_style_scope_commits_until_dispatch_releases_it() {
    let prepared = prepare_repository("retained-read-lease").await;
    let repository = repository(&prepared, NativeHouseholdModeV1::NativeEnabled);
    repository
        .initialize(prepared.command.clone(), CancellationToken::new())
        .await
        .expect("initialize");

    let read_lease = repository
        .acquire_read_lease(&prepared.account, CancellationToken::new())
        .await
        .expect("retained read lease");
    let command = next_commit(
        &read_lease.load().state,
        &prepared.account,
        read_lease.load().state.revision,
        fixed_uuid("56565656-5656-4656-8656-565656565656"),
        timestamp(1),
    );
    let competing_repository = repository.clone();
    let competing = tokio::spawn(async move {
        competing_repository
            .commit(command, CancellationToken::new())
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(75)).await;
    assert!(
        !competing.is_finished(),
        "a competing scope commit crossed the retained hosted read lease"
    );

    drop(read_lease);
    let committed = tokio::time::timeout(std::time::Duration::from_secs(2), competing)
        .await
        .expect("competing commit resumed after lease release")
        .expect("competing task")
        .expect("competing commit");
    assert_eq!(committed.outcome, AppliedCommitOutcomeV1::Committed);
}

#[tokio::test]
async fn encrypted_member_profile_and_selected_scope_survive_repository_reconstruction() {
    let prepared = prepare_repository("member-scope-restart").await;
    repository(&prepared, NativeHouseholdModeV1::NativeEnabled)
        .initialize(prepared.command.clone(), CancellationToken::new())
        .await
        .expect("initialize");

    let session = repository(&prepared, NativeHouseholdModeV1::NativeEnabled)
        .into_session(Arc::new(NativeHouseholdMutationAuthorityV1::new()));
    let created = session
        .create_member_with_declared_profile(
            CreateMemberWithDeclaredProfileV1 {
                expected_household_revision: HouseholdRevision::new(1).expect("revision"),
                display_name: DisplayName::parse("Restart canary").expect("display name"),
                relationship: RelationshipV1::Child,
                age_evidence: NativeMemberAgeEvidenceV1::Age13_17,
                declared_profile: OnboardingProfileInput {
                    diet_style_ids: vec!["vegan".to_owned()],
                    avoid_ingredients: vec!["encrypted restart canary".to_owned()],
                    ..OnboardingProfileInput::default()
                },
            },
            CancellationToken::new(),
        )
        .await
        .expect("create member");
    drop(session);

    let reconstructed = repository(&prepared, NativeHouseholdModeV1::NativeEnabled);
    let reloaded = reconstructed
        .load(&prepared.account, CancellationToken::new())
        .await
        .expect("reload after reconstruction")
        .expect("committed state");
    let subject = HouseholdSubjectId::member(created.member_id.clone());

    assert_eq!(
        reloaded.state.revision,
        created.resulting_household_revision
    );
    assert_eq!(
        reloaded.state.active_scope,
        HouseholdScope::Subject(subject.clone())
    );
    assert!(reloaded.state.members.iter().any(|member| {
        member.member_id == created.member_id
            && member.profile_state == HouseholdProfileStateV1::LocalOnly
    }));
    assert!(reloaded.state.profiles.iter().any(|profile| {
        profile.subject == subject
            && profile
                .document
                .declared_profile
                .as_ref()
                .is_some_and(|declared| {
                    declared
                        .avoid_ingredients
                        .iter()
                        .any(|ingredient| ingredient == "encrypted restart canary")
                })
    }));
}

#[tokio::test]
async fn stale_revision_and_account_mismatch_fail_closed() {
    let prepared = prepare_repository("conflict").await;
    let repository = repository(&prepared, NativeHouseholdModeV1::NativeEnabled);
    repository
        .initialize(prepared.command.clone(), CancellationToken::new())
        .await
        .expect("initialize");
    let loaded = repository
        .load(&prepared.account, CancellationToken::new())
        .await
        .expect("load")
        .expect("state");
    let stale = next_commit(
        &loaded.state,
        &prepared.account,
        HouseholdRevision::new(99).expect("stale revision"),
        fixed_uuid("66666666-6666-4666-8666-666666666666"),
        timestamp(2),
    );
    let conflict = repository
        .commit(stale, CancellationToken::new())
        .await
        .expect_err("revision conflict");
    assert_eq!(conflict.code, "household_revision_conflict");

    let other = AccountId::parse("another-account").expect("other account");
    let mismatch = repository
        .load(&other, CancellationToken::new())
        .await
        .expect_err("account mismatch");
    assert_eq!(mismatch.code, "household_account_mismatch");
}

#[tokio::test]
async fn rollback_read_only_denies_commit_without_touching_vault() {
    let prepared = prepare_repository("read-only").await;
    let writable = repository(&prepared, NativeHouseholdModeV1::NativeEnabled);
    writable
        .initialize(prepared.command.clone(), CancellationToken::new())
        .await
        .expect("initialize");
    let loaded = writable
        .load(&prepared.account, CancellationToken::new())
        .await
        .expect("load")
        .expect("state");
    let command = next_commit(
        &loaded.state,
        &prepared.account,
        loaded.state.revision,
        fixed_uuid("77777777-7777-4777-8777-777777777777"),
        timestamp(3),
    );
    let before = artifact_bytes(&prepared.vault.household_directory());
    let read_only = repository(&prepared, NativeHouseholdModeV1::NativeRollbackReadOnly);
    let error = read_only
        .commit(command, CancellationToken::new())
        .await
        .expect_err("read-only denial");
    assert_eq!(error.code, "household_repository_read_only");
    assert_eq!(
        artifact_bytes(&prepared.vault.household_directory()),
        before
    );
}

#[tokio::test]
async fn rollback_read_only_may_finish_only_the_exact_ready_initialization() {
    let prepared = prepare_repository("read-only-finalize").await;
    let read_only = repository(&prepared, NativeHouseholdModeV1::NativeRollbackReadOnly);
    let outcome = read_only
        .initialize(prepared.command.clone(), CancellationToken::new())
        .await
        .expect("finish exact initialization");
    assert_eq!(outcome.outcome, AppliedCommitOutcomeV1::Initialized);
    let loaded = read_only
        .load(&prepared.account, CancellationToken::new())
        .await
        .expect("load")
        .expect("committed state");
    assert_eq!(loaded.state, prepared.final_state);
}

#[tokio::test]
async fn cancellation_is_checked_before_lock_or_write_boundaries() {
    let prepared = prepare_repository("cancelled").await;
    let repository = repository(&prepared, NativeHouseholdModeV1::NativeEnabled);
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let error = repository
        .load(&prepared.account, cancellation.clone())
        .await
        .expect_err("cancelled load");
    assert_eq!(error.code, "household_operation_cancelled");
    let error = repository
        .initialize(prepared.command.clone(), cancellation)
        .await
        .expect_err("cancelled initialize");
    assert_eq!(error.code, "household_operation_cancelled");
    assert!(
        !prepared
            .vault
            .household_directory()
            .join("commit.hfj")
            .exists(),
        "cancelled initialization must not write a vault journal"
    );
}

#[tokio::test]
async fn restart_finalizes_the_exact_committed_initialization_without_rewrite() {
    let prepared = prepare_repository("restart-finalize").await;
    let lifecycle = prepared
        .vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .expect("lifecycle lease");
    let mut lease = prepared
        .vault
        .acquire_vault_lease(
            lifecycle,
            HouseholdVaultLeaseModeV1::CreateIfMissing,
            CancellationToken::new(),
        )
        .await
        .expect("vault lease");
    let key = HouseholdKeyStore::load(
        prepared.store.as_ref(),
        lease.lifecycle_lease(),
        CancellationToken::new(),
    )
    .await
    .expect("key load")
    .expect("initializing key");
    let write = HouseholdVaultWrite::new(
        prepared.final_state.revision.get(),
        prepared.command.commit_id.as_uuid(),
        prepared
            .final_state
            .canonical_bytes()
            .expect("canonical state"),
    )
    .expect("vault write");
    prepared
        .vault
        .initialize(&mut lease, key, write, CancellationToken::new())
        .await
        .expect("commit initialization");
    drop(lease);
    let committed_artifacts = artifact_bytes(&prepared.vault.household_directory());

    let repository = repository(&prepared, NativeHouseholdModeV1::NativeEnabled);
    let outcome = repository
        .initialize(prepared.command.clone(), CancellationToken::new())
        .await
        .expect("finalize exact initialization");
    assert_eq!(outcome.outcome, AppliedCommitOutcomeV1::Initialized);
    assert_eq!(
        artifact_bytes(&prepared.vault.household_directory()),
        committed_artifacts,
        "restart finalization must verify the committed vault without rewriting it"
    );

    let lifecycle = prepared
        .vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .expect("lifecycle lease");
    let guard = HouseholdMigrationGuardStore::load(
        prepared.store.as_ref(),
        &lifecycle,
        CancellationToken::new(),
    )
    .await
    .expect("guard load")
    .expect("completed guard");
    let key = HouseholdKeyStore::load(
        prepared.store.as_ref(),
        &lifecycle,
        CancellationToken::new(),
    )
    .await
    .expect("key load")
    .expect("stable key");
    assert_eq!(
        guard.state(),
        HouseholdMigrationGuardStateV1::InitializedNoSource
    );
    assert_eq!(key.phase, HouseholdKeyBundlePhase::Stable);
}

#[tokio::test]
async fn restart_finishes_ready_guard_after_key_was_already_stabilized() {
    let prepared = prepare_repository("restart-stable-key").await;
    let lifecycle = prepared
        .vault
        .acquire_lifecycle_lease(CancellationToken::new())
        .await
        .expect("lifecycle lease");
    let mut lease = prepared
        .vault
        .acquire_vault_lease(
            lifecycle,
            HouseholdVaultLeaseModeV1::CreateIfMissing,
            CancellationToken::new(),
        )
        .await
        .expect("vault lease");
    let key = HouseholdKeyStore::load(
        prepared.store.as_ref(),
        lease.lifecycle_lease(),
        CancellationToken::new(),
    )
    .await
    .expect("key load")
    .expect("initializing key");
    let write = HouseholdVaultWrite::new(
        prepared.final_state.revision.get(),
        prepared.command.commit_id.as_uuid(),
        prepared
            .final_state
            .canonical_bytes()
            .expect("canonical state"),
    )
    .expect("vault write");
    prepared
        .vault
        .initialize(&mut lease, key.clone(), write, CancellationToken::new())
        .await
        .expect("commit initialization");
    let stable = HouseholdKeyBundle::stable(
        prepared.vault.account_slot(),
        key.revision.checked_next().expect("next key revision"),
        key.active_key_id,
        key.active_key.clone(),
    );
    HouseholdKeyStore::compare_exchange(
        prepared.store.as_ref(),
        &mut lease,
        key.revision,
        stable,
        CancellationToken::new(),
    )
    .await
    .expect("stabilize key");
    drop(lease);
    let committed_artifacts = artifact_bytes(&prepared.vault.household_directory());

    let repository = repository(&prepared, NativeHouseholdModeV1::NativeEnabled);
    repository
        .initialize(prepared.command.clone(), CancellationToken::new())
        .await
        .expect("finalize ready guard");
    assert_eq!(
        artifact_bytes(&prepared.vault.household_directory()),
        committed_artifacts
    );
    let loaded = repository
        .load(&prepared.account, CancellationToken::new())
        .await
        .expect("load")
        .expect("committed state");
    assert_eq!(loaded.state, prepared.final_state);
}

#[tokio::test]
async fn teardown_barrier_preempts_load_initialize_and_commit_without_mutation() {
    for (name, mode) in [
        ("teardown-enabled", NativeHouseholdModeV1::NativeEnabled),
        (
            "teardown-rollback",
            NativeHouseholdModeV1::NativeRollbackReadOnly,
        ),
    ] {
        let prepared = prepare_repository(name).await;
        repository(&prepared, NativeHouseholdModeV1::NativeEnabled)
            .initialize(prepared.command.clone(), CancellationToken::new())
            .await
            .expect("initialize");
        let before_artifacts = artifact_bytes(&prepared.vault.household_directory());
        let before_documents = secure_documents(&prepared).await;
        let command = next_commit(
            &prepared.final_state,
            &prepared.account,
            prepared.final_state.revision,
            fixed_uuid("88888888-8888-4888-8888-888888888888"),
            timestamp(1),
        );
        install_teardown_barrier(&prepared);
        let repository = repository(&prepared, mode);

        let load_error = repository
            .load(&prepared.account, CancellationToken::new())
            .await
            .expect_err("teardown blocks load");
        assert_eq!(load_error.code, "household_account_teardown_in_progress");
        let initialize_error = repository
            .initialize(prepared.command.clone(), CancellationToken::new())
            .await
            .expect_err("teardown blocks initialize");
        assert_eq!(
            initialize_error.code,
            "household_account_teardown_in_progress"
        );
        let commit_error = repository
            .commit(command, CancellationToken::new())
            .await
            .expect_err("teardown blocks commit");
        assert_eq!(commit_error.code, "household_account_teardown_in_progress");
        assert_eq!(
            artifact_bytes(&prepared.vault.household_directory()),
            before_artifacts
        );
        assert_eq!(secure_documents(&prepared).await, before_documents);
    }
}

#[tokio::test]
async fn ready_guard_with_missing_key_mints_once_and_completes_in_both_modes() {
    for (name, mode) in [
        ("remint-enabled", NativeHouseholdModeV1::NativeEnabled),
        (
            "remint-rollback",
            NativeHouseholdModeV1::NativeRollbackReadOnly,
        ),
    ] {
        let prepared = prepare_repository(name).await;
        let deleted = delete_initializing_key(&prepared).await;
        let repository = repository(&prepared, mode);
        assert!(
            repository
                .load(&prepared.account, CancellationToken::new())
                .await
                .expect("ready load")
                .is_none()
        );
        assert!(secure_documents(&prepared).await.1.is_none());

        let outcome = repository
            .initialize(prepared.command.clone(), CancellationToken::new())
            .await
            .expect("initialize from missing key");
        assert_eq!(outcome.outcome, AppliedCommitOutcomeV1::Initialized);
        let first_documents = secure_documents(&prepared).await;
        let first_key = first_documents.1.as_ref().expect("stable key");
        assert_eq!(first_key.phase, HouseholdKeyBundlePhase::Stable);
        assert_ne!(first_key.active_key_id, deleted.active_key_id);
        let first_artifacts = artifact_bytes(&prepared.vault.household_directory());

        assert_eq!(
            repository
                .initialize(prepared.command.clone(), CancellationToken::new())
                .await
                .expect("exact replay"),
            outcome
        );
        assert_eq!(secure_documents(&prepared).await, first_documents);
        assert_eq!(
            artifact_bytes(&prepared.vault.household_directory()),
            first_artifacts
        );
    }
}

#[tokio::test]
async fn uncertain_key_initialize_reloads_the_same_key_without_a_second_mint() {
    for (name, mode) in [
        (
            "uncertain-key-enabled",
            NativeHouseholdModeV1::NativeEnabled,
        ),
        (
            "uncertain-key-rollback",
            NativeHouseholdModeV1::NativeRollbackReadOnly,
        ),
    ] {
        let prepared = prepare_repository(name).await;
        let _ = delete_initializing_key(&prepared).await;
        let uncertain_store = Arc::new(UncertainKeyInitializeStore::new(prepared.store.clone()));
        let repository = NativeHouseholdRepository::from_vault(
            prepared.account.clone(),
            prepared.vault.clone(),
            uncertain_store.clone(),
            mode,
        )
        .expect("repository");

        let outcome = repository
            .initialize(prepared.command.clone(), CancellationToken::new())
            .await
            .expect("reconcile uncertain key initialization");
        assert_eq!(outcome.outcome, AppliedCommitOutcomeV1::Initialized);
        assert_eq!(uncertain_store.initialize_calls.load(Ordering::SeqCst), 1);
        let first_documents = secure_documents(&prepared).await;
        let first_artifacts = artifact_bytes(&prepared.vault.household_directory());

        assert_eq!(
            repository
                .initialize(prepared.command.clone(), CancellationToken::new())
                .await
                .expect("replay"),
            outcome
        );
        assert_eq!(uncertain_store.initialize_calls.load(Ordering::SeqCst), 1);
        assert_eq!(secure_documents(&prepared).await, first_documents);
        assert_eq!(
            artifact_bytes(&prepared.vault.household_directory()),
            first_artifacts
        );
    }
}

#[tokio::test]
async fn missing_key_with_any_initialization_artifact_never_remints() {
    for (name, mode) in [
        (
            "artifact-no-key-enabled",
            NativeHouseholdModeV1::NativeEnabled,
        ),
        (
            "artifact-no-key-rollback",
            NativeHouseholdModeV1::NativeRollbackReadOnly,
        ),
    ] {
        let prepared = prepare_repository(name).await;
        commit_ready_vault(&prepared, false).await;
        let _ = delete_initializing_key(&prepared).await;
        let before_artifacts = artifact_bytes(&prepared.vault.household_directory());
        let before_guard = secure_documents(&prepared).await.0;

        let error = repository(&prepared, mode)
            .initialize(prepared.command.clone(), CancellationToken::new())
            .await
            .expect_err("artifact-bearing key absence must fail");
        assert!(
            matches!(
                error.code,
                "household_native_evidence_contradiction"
                    | "household_initialization_protocol_required"
            ),
            "{}",
            error.code
        );
        assert_eq!(
            artifact_bytes(&prepared.vault.household_directory()),
            before_artifacts
        );
        let after_documents = secure_documents(&prepared).await;
        assert_eq!(after_documents.0, before_guard);
        assert!(after_documents.1.is_none());
    }
}

#[tokio::test]
async fn committed_guard_requires_exact_initial_ledger_in_both_modes() {
    for (name, mode, missing) in [
        (
            "ledger-corrupt-enabled",
            NativeHouseholdModeV1::NativeEnabled,
            false,
        ),
        (
            "ledger-corrupt-rollback",
            NativeHouseholdModeV1::NativeRollbackReadOnly,
            false,
        ),
        (
            "ledger-missing-enabled",
            NativeHouseholdModeV1::NativeEnabled,
            true,
        ),
        (
            "ledger-missing-rollback",
            NativeHouseholdModeV1::NativeRollbackReadOnly,
            true,
        ),
    ] {
        let prepared = prepare_repository_with(name, |state| {
            if missing {
                state.bounded_applied_commits.clear();
            } else {
                state.bounded_applied_commits[0].fingerprint =
                    CanonicalDigestV1::from_bytes([0xa5; 32]);
            }
        })
        .await;
        complete_ready_guard_and_key(&prepared).await;
        let before_artifacts = artifact_bytes(&prepared.vault.household_directory());
        let before_documents = secure_documents(&prepared).await;

        let error = repository(&prepared, mode)
            .load(&prepared.account, CancellationToken::new())
            .await
            .expect_err("invalid initial ledger");
        assert!(
            matches!(
                error.code,
                "household_initial_ledger_mismatch" | "household_vault_state_mismatch"
            ),
            "{}",
            error.code
        );
        assert_eq!(
            artifact_bytes(&prepared.vault.household_directory()),
            before_artifacts
        );
        assert_eq!(secure_documents(&prepared).await, before_documents);
    }
}

#[tokio::test]
async fn ready_guard_resume_rejects_corrupt_committed_topology_without_repair() {
    for (name, mode, artifact) in [
        (
            "topology-previous-enabled",
            NativeHouseholdModeV1::NativeEnabled,
            "generation-1.hfv",
        ),
        (
            "topology-previous-rollback",
            NativeHouseholdModeV1::NativeRollbackReadOnly,
            "generation-1.hfv",
        ),
        (
            "topology-journal-enabled",
            NativeHouseholdModeV1::NativeEnabled,
            "commit.hfj",
        ),
        (
            "topology-journal-rollback",
            NativeHouseholdModeV1::NativeRollbackReadOnly,
            "commit.hfj",
        ),
    ] {
        let prepared = prepare_repository(name).await;
        commit_ready_vault(&prepared, true).await;
        corrupt_artifact(&prepared.vault.household_directory().join(artifact));
        let before_artifacts = artifact_bytes(&prepared.vault.household_directory());
        let before_documents = secure_documents(&prepared).await;

        let repository = repository(&prepared, mode);
        let load_error = repository
            .load(&prepared.account, CancellationToken::new())
            .await
            .expect_err("corrupt committed initialization topology on load");
        assert_eq!(load_error.code, "household_native_evidence_contradiction");
        let error = repository
            .initialize(prepared.command.clone(), CancellationToken::new())
            .await
            .expect_err("corrupt committed initialization topology");
        assert_eq!(error.code, "household_native_evidence_contradiction");
        assert_eq!(
            artifact_bytes(&prepared.vault.household_directory()),
            before_artifacts
        );
        assert_eq!(secure_documents(&prepared).await, before_documents);
        assert_eq!(
            before_documents.0.state(),
            HouseholdMigrationGuardStateV1::Initializing
        );
    }
}

#[tokio::test]
async fn exact_uncommitted_topologies_resume_in_both_modes() {
    for (name, mode, retain_generation_one) in [
        (
            "resume-gen0-enabled",
            NativeHouseholdModeV1::NativeEnabled,
            false,
        ),
        (
            "resume-gen0-rollback",
            NativeHouseholdModeV1::NativeRollbackReadOnly,
            false,
        ),
        (
            "resume-gen01-enabled",
            NativeHouseholdModeV1::NativeEnabled,
            true,
        ),
        (
            "resume-gen01-rollback",
            NativeHouseholdModeV1::NativeRollbackReadOnly,
            true,
        ),
    ] {
        let prepared = prepare_repository(name).await;
        commit_ready_vault(&prepared, false).await;
        std::fs::remove_file(prepared.vault.household_directory().join("commit.hfj"))
            .expect("remove journal");
        if !retain_generation_one {
            std::fs::remove_file(
                prepared
                    .vault
                    .household_directory()
                    .join("generation-1.hfv"),
            )
            .expect("remove generation one");
        }

        let repository = repository(&prepared, mode);
        assert!(
            repository
                .load(&prepared.account, CancellationToken::new())
                .await
                .expect("uncommitted load")
                .is_none()
        );
        let outcome = repository
            .initialize(prepared.command.clone(), CancellationToken::new())
            .await
            .expect("resume exact uncommitted topology");
        assert_eq!(outcome.outcome, AppliedCommitOutcomeV1::Initialized);
        assert_eq!(
            repository
                .load(&prepared.account, CancellationToken::new())
                .await
                .expect("load")
                .expect("committed state")
                .state,
            prepared.final_state
        );
    }
}

#[tokio::test]
async fn generation_one_only_is_a_byte_identical_denial_in_both_modes() {
    for (name, mode) in [
        ("gen1-only-enabled", NativeHouseholdModeV1::NativeEnabled),
        (
            "gen1-only-rollback",
            NativeHouseholdModeV1::NativeRollbackReadOnly,
        ),
    ] {
        let prepared = prepare_repository(name).await;
        commit_ready_vault(&prepared, false).await;
        std::fs::remove_file(
            prepared
                .vault
                .household_directory()
                .join("generation-0.hfv"),
        )
        .expect("remove generation zero");
        std::fs::remove_file(prepared.vault.household_directory().join("commit.hfj"))
            .expect("remove journal");
        let before_artifacts = artifact_bytes(&prepared.vault.household_directory());
        let before_documents = secure_documents(&prepared).await;
        let repository = repository(&prepared, mode);

        let load_error = repository
            .load(&prepared.account, CancellationToken::new())
            .await
            .expect_err("generation one only load");
        assert_eq!(load_error.code, "household_native_evidence_contradiction");
        let initialize_error = repository
            .initialize(prepared.command.clone(), CancellationToken::new())
            .await
            .expect_err("generation one only initialize");
        assert_eq!(
            initialize_error.code,
            "household_native_evidence_contradiction"
        );
        assert_eq!(
            artifact_bytes(&prepared.vault.household_directory()),
            before_artifacts
        );
        assert_eq!(secure_documents(&prepared).await, before_documents);
    }
}

#[tokio::test]
async fn ready_guard_accepts_only_initial_revision_one_or_stable_revision_two_committed() {
    for mode in [
        NativeHouseholdModeV1::NativeEnabled,
        NativeHouseholdModeV1::NativeRollbackReadOnly,
    ] {
        for case in [
            "initializing-revision-two",
            "stable-revision-one",
            "stable-revision-three",
            "rewriting",
            "stable-revision-two-uncommitted",
        ] {
            let mode_name = match mode {
                NativeHouseholdModeV1::NativeEnabled => "enabled",
                NativeHouseholdModeV1::NativeRollbackReadOnly => "rollback",
                _ => unreachable!("closed native test modes"),
            };
            let name = format!("ready-key-{case}-{mode_name}");
            let prepared = prepare_repository(&name).await;
            commit_ready_vault(&prepared, false).await;
            let (guard, key) = secure_documents(&prepared).await;
            let key = key.expect("initializing key");
            let replacement = match case {
                "initializing-revision-two" => HouseholdKeyBundle::initializing(
                    prepared.vault.account_slot(),
                    KeyBundleRevision::new(2).expect("revision"),
                    key.active_key_id,
                    key.active_key.clone(),
                    guard.initialization_id(),
                    guard.initial_commit_id(),
                    guard.initial_effect_fingerprint().expect("fingerprint"),
                    guard.initial_state_digest().expect("state digest"),
                ),
                "stable-revision-one" => HouseholdKeyBundle::stable(
                    prepared.vault.account_slot(),
                    KeyBundleRevision::new(1).expect("revision"),
                    key.active_key_id,
                    key.active_key.clone(),
                ),
                "stable-revision-three" => HouseholdKeyBundle::stable(
                    prepared.vault.account_slot(),
                    KeyBundleRevision::new(3).expect("revision"),
                    key.active_key_id,
                    key.active_key.clone(),
                ),
                "rewriting" => {
                    let previous = HouseholdKeyBundle::stable(
                        prepared.vault.account_slot(),
                        KeyBundleRevision::new(1).expect("revision"),
                        KeyId::new(),
                        HouseholdKeyMaterial::from_bytes([0x7b; 32]),
                    );
                    HouseholdKeyBundle::rewriting(
                        prepared.vault.account_slot(),
                        KeyBundleRevision::new(2).expect("revision"),
                        key.active_key_id,
                        key.active_key.clone(),
                        &previous,
                        fixed_uuid("99999999-9999-4999-8999-999999999999"),
                    )
                    .expect("rewriting key")
                }
                "stable-revision-two-uncommitted" => {
                    std::fs::remove_file(prepared.vault.household_directory().join("commit.hfj"))
                        .expect("remove journal");
                    HouseholdKeyBundle::stable(
                        prepared.vault.account_slot(),
                        KeyBundleRevision::new(2).expect("revision"),
                        key.active_key_id,
                        key.active_key.clone(),
                    )
                }
                _ => unreachable!("closed adversarial key cases"),
            };
            let injected_store = Arc::new(UncertainKeyInitializeStore::with_loaded_key(
                prepared.store.clone(),
                replacement,
            ));
            let repository = NativeHouseholdRepository::from_vault(
                prepared.account.clone(),
                prepared.vault.clone(),
                injected_store,
                mode,
            )
            .expect("repository");
            let before_artifacts = artifact_bytes(&prepared.vault.household_directory());
            let before_documents = secure_documents(&prepared).await;

            assert!(
                repository
                    .load(&prepared.account, CancellationToken::new())
                    .await
                    .is_err(),
                "{case} {mode_name} load"
            );
            assert!(
                repository
                    .initialize(prepared.command.clone(), CancellationToken::new())
                    .await
                    .is_err(),
                "{case} {mode_name} initialize"
            );
            assert_eq!(
                artifact_bytes(&prepared.vault.household_directory()),
                before_artifacts,
                "{case} {mode_name} artifacts"
            );
            assert_eq!(
                secure_documents(&prepared).await,
                before_documents,
                "{case} {mode_name} secure documents"
            );
        }
    }
}

#[tokio::test]
async fn initialization_without_an_audited_guard_key_protocol_is_refused() {
    let prepared = prepare_repository("missing-protocol").await;
    let empty_store = Arc::new(InMemoryHouseholdSecureStore::default());
    let repository = NativeHouseholdRepository::from_vault(
        prepared.account.clone(),
        prepared.vault.clone(),
        empty_store,
        NativeHouseholdModeV1::NativeEnabled,
    )
    .expect("repository");
    let error = repository
        .initialize(prepared.command.clone(), CancellationToken::new())
        .await
        .expect_err("missing protocol");
    assert_eq!(error.code, "household_initialization_protocol_required");
    assert!(
        !prepared
            .vault
            .household_directory()
            .join("commit.hfj")
            .exists()
    );
}

#[tokio::test]
async fn erase_requires_the_audited_teardown_coordinator() {
    let prepared = prepare_repository("erase").await;
    let repository = repository(&prepared, NativeHouseholdModeV1::NativeEnabled);
    let error = repository
        .erase_account(
            HouseholdErase {
                account: prepared.account.clone(),
                expected_revision: None,
            },
            CancellationToken::new(),
        )
        .await
        .expect_err("teardown required");
    assert_eq!(error.code, "household_account_teardown_required");
}
