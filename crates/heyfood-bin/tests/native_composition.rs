#![cfg(feature = "native-credentials")]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{
    Arc, Mutex as StdMutex,
    atomic::{AtomicUsize, Ordering},
    mpsc as std_mpsc,
};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use heyfood_agent_runtime::HttpService;
use heyfood_application::{
    AudioCapture, AudioCapturePort, BoxFuture, ClockPort, CredentialCommit, CredentialPort,
    EnsureSession, HouseholdCommit, HouseholdCommitOutcome, HouseholdErase, HouseholdEraseOutcome,
    HouseholdInitialize, HouseholdLoad, HouseholdRepositoryPort, HouseholdRepositoryResolutionV1,
    HouseholdSession, PortError, ServicePort, resolve_household_commit_v1,
    resolve_household_initialize_v1,
};
use heyfood_bin::{InteractiveTurnDriver, QualifiedTurnDriver};
use heyfood_core::{
    AccountId, CanonicalDateV1, CanonicalDigestV1, CanonicalTimestampV1, CommitId,
    CredentialVersion, DisplayName, HOUSEHOLD_STATE_SCHEMA_VERSION, HouseholdDeclaredProfileV1,
    HouseholdLifecycleV1, HouseholdMemberV1, HouseholdOwnerV1, HouseholdProfileDocumentV1,
    HouseholdProfileOutboxEntryV1, HouseholdProfileRecordV1, HouseholdProfileStateV1,
    HouseholdRevision, HouseholdScope, HouseholdStateV1, HouseholdSubjectId,
    ImportedCompatibilityStateV1, LegacySourceIdentityV1, MemberId, MigrationDispositionManifestV1,
    MigrationProvenanceV1, MinorStatusV1, NetworkPolicy, OnboardingProfileInput,
    OwnerSyncIntentPhaseV1, ProfileRevision, RelationshipSourceV1, RelationshipV1, SensitiveString,
    ServiceUrl, SessionCredentials, SessionSnapshot,
};
#[cfg(windows)]
use heyfood_platform::NativeAuthStore;
use heyfood_platform::NativeHouseholdMutationAuthorityV1;
use heyfood_tui::{
    BoundedHouseholdMemberDraftV1, HouseholdAccountBindingDigestV1, HouseholdAgeEvidenceInputV1,
    HouseholdContextApplyFailureV1, HouseholdManagementFailureV1, HouseholdManagementLoadPurposeV1,
    HouseholdModeGenerationV1, HouseholdMutationFailureV1, HouseholdMutationKindV1,
    HouseholdOperationBindingV1, HouseholdOperationIdV1, HouseholdPresentationModeV1,
    HouseholdReducerCorrelationV1, NativeOwnerProfileSaveStatusV1, OwnerProfileActionLoadPurposeV1,
    OwnerProfileRetryActionV1, OwnerProfileRetryEligibilityV1,
    OwnerProfileRetryUnavailableReasonV1, OwnerSyncIntentHandleV1, ProfileActionsLoadedV1,
    ProfilePresentationModeV1, ProfileRetrySyncFinishedV1, RuntimeEvent,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "heyfood-native-composition-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn read_request(socket: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 1024];
        let count = socket.read(&mut chunk).await.unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
        })
        .unwrap_or(0);
    while bytes.len() - header_end < length {
        let mut chunk = vec![0; length - (bytes.len() - header_end)];
        let count = socket.read(&mut chunk).await.unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&chunk[..count]);
    }
    String::from_utf8(bytes).unwrap()
}

async fn respond_json(socket: &mut TcpStream, body: Value) {
    let body = serde_json::to_vec(&body).unwrap();
    socket
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    socket.write_all(&body).await.unwrap();
}

async fn run(root: &Path, base_url: &str, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_heyfood"))
        .args(args)
        .env("HEYFOOD_STATE_DIR", root)
        .env("HEYFOOD_CREDENTIAL_STORE", "native")
        .env("HEYFOOD_API_URL", base_url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .unwrap()
}

async fn cleanup(root: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_heyfood"))
        .arg("--version")
        .env("HEYFOOD_STATE_DIR", root)
        .env("HEYFOOD_TEST_DELETE_NATIVE_CREDENTIALS", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .unwrap()
}

struct MemoryHouseholdRepository {
    state: StdMutex<Option<HouseholdStateV1>>,
    load_calls: AtomicUsize,
    commit_calls: AtomicUsize,
    fail_commit_at: AtomicUsize,
}

impl MemoryHouseholdRepository {
    fn with_state(state: HouseholdStateV1) -> Self {
        Self {
            state: StdMutex::new(Some(state)),
            load_calls: AtomicUsize::new(0),
            commit_calls: AtomicUsize::new(0),
            fail_commit_at: AtomicUsize::new(0),
        }
    }

    fn fail_commit_at(&self, call: usize) {
        self.fail_commit_at.store(call, Ordering::SeqCst);
    }

    fn snapshot(&self) -> HouseholdStateV1 {
        self.state.lock().unwrap().clone().unwrap()
    }

    fn mutate_state(&self, update: impl FnOnce(&mut HouseholdStateV1)) {
        update(self.state.lock().unwrap().as_mut().unwrap());
    }
}

impl HouseholdRepositoryPort for MemoryHouseholdRepository {
    fn load<'a>(
        &'a self,
        account: &'a AccountId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<Option<HouseholdLoad>, PortError>> {
        self.load_calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(PortError::new(
                    "household_load_cancelled",
                    "household load cancelled",
                ));
            }
            let state = self.state.lock().unwrap().clone();
            if state
                .as_ref()
                .is_some_and(|state| &state.account_binding != account)
            {
                return Err(PortError::new(
                    "household_account_mismatch",
                    "household account mismatch",
                ));
            }
            state.map(HouseholdLoad::from_state).transpose()
        })
    }

    fn initialize<'a>(
        &'a self,
        command: HouseholdInitialize,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdCommitOutcome, PortError>> {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(PortError::new(
                    "household_initialize_cancelled",
                    "household initialization cancelled",
                ));
            }
            let mut state = self.state.lock().unwrap();
            match resolve_household_initialize_v1(state.as_ref(), &command)? {
                HouseholdRepositoryResolutionV1::Replay(outcome) => Ok(outcome),
                HouseholdRepositoryResolutionV1::Write {
                    state: replacement,
                    outcome,
                } => {
                    *state = Some(*replacement);
                    Ok(outcome)
                }
            }
        })
    }

    fn commit<'a>(
        &'a self,
        command: HouseholdCommit,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdCommitOutcome, PortError>> {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(PortError::new(
                    "household_commit_cancelled",
                    "household commit cancelled",
                ));
            }
            let call = self.commit_calls.fetch_add(1, Ordering::SeqCst) + 1;
            if self.fail_commit_at.load(Ordering::SeqCst) == call {
                return Err(PortError::new(
                    "fixture_commit_failure",
                    "fixture household commit failed",
                ));
            }
            let mut state = self.state.lock().unwrap();
            match resolve_household_commit_v1(state.as_ref(), &command)? {
                HouseholdRepositoryResolutionV1::Replay(outcome) => Ok(outcome),
                HouseholdRepositoryResolutionV1::Write {
                    state: replacement,
                    outcome,
                } => {
                    *state = Some(*replacement);
                    Ok(outcome)
                }
            }
        })
    }

    fn erase_account<'a>(
        &'a self,
        command: HouseholdErase,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<HouseholdEraseOutcome, PortError>> {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(PortError::new(
                    "household_erase_cancelled",
                    "household erase cancelled",
                ));
            }
            let mut state = self.state.lock().unwrap();
            if state
                .as_ref()
                .is_some_and(|current| current.account_binding != command.account)
            {
                return Err(PortError::new(
                    "household_account_mismatch",
                    "household account mismatch",
                ));
            }
            *state = None;
            Ok(HouseholdEraseOutcome {
                household_key_deleted: true,
                household_ciphertext_deleted: true,
                import_snapshot_deleted: true,
                legacy_source_retained: true,
                legacy_credentials_cleared: true,
                legacy_credentials_retained: false,
                local_credentials_cleared: true,
                outcome_uncertain: false,
            })
        })
    }
}

struct MemoryCredentialPort;

impl CredentialPort for MemoryCredentialPort {
    fn load(&self) -> BoxFuture<'_, Result<Option<SessionCredentials>, PortError>> {
        Box::pin(async { Ok(None) })
    }

    fn commit(&self, _commit: CredentialCommit) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async { Ok(()) })
    }

    fn mark_reconciliation_required(
        &self,
        _commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async { Ok(()) })
    }

    fn clear_reconciliation_required(
        &self,
        _commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async { Ok(()) })
    }
}

struct FixedClock;

impl ClockPort for FixedClock {
    fn unix_timestamp(&self) -> i64 {
        0
    }
}

#[derive(Default)]
struct ProbeAudioCapture {
    calls: AtomicUsize,
}

impl AudioCapturePort for ProbeAudioCapture {
    fn available(&self) -> bool {
        true
    }

    fn capture(
        &self,
        _stop: CancellationToken,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<AudioCapture, PortError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Err(PortError::new(
                "unexpected_audio_capture",
                "household preflight must run before audio capture",
            ))
        })
    }
}

fn native_timestamp() -> CanonicalTimestampV1 {
    CanonicalTimestampV1::parse("2026-07-30T12:00:00.000Z").unwrap()
}

fn native_account() -> AccountId {
    AccountId::parse("native-owner-account").unwrap()
}

fn incomplete_native_household() -> HouseholdStateV1 {
    let timestamp = native_timestamp();
    HouseholdStateV1 {
        schema_version: HOUSEHOLD_STATE_SCHEMA_VERSION,
        account_binding: native_account(),
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
            migration_id: CommitId::new().as_uuid(),
            initialization_id: CommitId::new().as_uuid(),
            initial_commit_id: CommitId::new(),
            migration_frozen_at: timestamp.clone(),
        },
        updated_at: timestamp,
    }
}

fn everyone_native_household() -> HouseholdStateV1 {
    let mut state = incomplete_native_household();
    let timestamp = native_timestamp();
    state.members.push(HouseholdMemberV1 {
        member_id: MemberId::new(),
        display_name: DisplayName::parse("member-context-canary").unwrap(),
        relationship: RelationshipV1::Partner,
        relationship_source: RelationshipSourceV1::NativeDeclared,
        minor_status: MinorStatusV1::Unknown,
        age_evidence: None,
        minor_status_evaluated_on: CanonicalDateV1::parse("2026-07-30").unwrap(),
        lifecycle: HouseholdLifecycleV1::Active,
        profile_state: HouseholdProfileStateV1::Incomplete,
        created_at: timestamp.clone(),
        updated_at: timestamp.clone(),
    });
    state.active_scope = HouseholdScope::Everyone;
    state.updated_at = timestamp;
    state
}

fn member_native_household() -> HouseholdStateV1 {
    let mut state = everyone_native_household();
    state.active_scope = HouseholdScope::Subject(HouseholdSubjectId::member(
        state.members[0].member_id.clone(),
    ));
    state
}

fn selectable_everyone_native_household() -> HouseholdStateV1 {
    let mut state = everyone_native_household();
    state.active_scope = HouseholdScope::Subject(HouseholdSubjectId::self_());
    state.owner.profile_state = HouseholdProfileStateV1::LocalOnly;
    state.members[0].profile_state = HouseholdProfileStateV1::LocalOnly;
    let member_subject = HouseholdSubjectId::member(state.members[0].member_id.clone());
    let document = HouseholdProfileDocumentV1::native(HouseholdDeclaredProfileV1 {
        diet_style_ids: vec!["vegan".into()],
        custom_diet_styles: Vec::new(),
        allergy_ids: Vec::new(),
        custom_restrictions: Vec::new(),
        health_condition_ids: Vec::new(),
        custom_health_conditions: Vec::new(),
        avoid_ingredients: Vec::new(),
        activity_level: None,
        cuisine_preferences: Vec::new(),
        custom_cuisines: Vec::new(),
        severity_level: None,
        notes: None,
    })
    .unwrap();
    state.profiles = vec![
        HouseholdProfileRecordV1 {
            subject: HouseholdSubjectId::self_(),
            profile_revision: ProfileRevision::new(1).unwrap(),
            document: document.clone(),
        },
        HouseholdProfileRecordV1 {
            subject: member_subject,
            profile_revision: ProfileRevision::new(1).unwrap(),
            document,
        },
    ];
    state.validate().unwrap();
    state
}

fn fixture_credentials() -> SessionCredentials {
    credentials_for(native_account())
}

fn expired_fixture_credentials() -> SessionCredentials {
    SessionCredentials::from_unix_expiry(
        native_account(),
        SensitiveString::new("expired-access-canary"),
        SensitiveString::new("expired-refresh-canary"),
        CredentialVersion::new(1),
        0,
    )
    .unwrap()
}

fn credentials_for(account: AccountId) -> SessionCredentials {
    SessionCredentials::from_unix_expiry(
        account,
        SensitiveString::new("access"),
        SensitiveString::new("refresh"),
        CredentialVersion::new(1),
        4_102_444_800,
    )
    .unwrap()
}

fn native_driver(
    repository: Arc<MemoryHouseholdRepository>,
    address: std::net::SocketAddr,
) -> InteractiveTurnDriver {
    native_driver_with_credentials(repository, address, fixture_credentials())
}

fn native_driver_with_credentials(
    repository: Arc<MemoryHouseholdRepository>,
    address: std::net::SocketAddr,
    credentials: SessionCredentials,
) -> InteractiveTurnDriver {
    let service_url =
        ServiceUrl::parse(&format!("http://{address}"), NetworkPolicy::DEVELOPMENT).unwrap();
    let service = Arc::new(
        HttpService::new(service_url, NetworkPolicy::DEVELOPMENT, Default::default()).unwrap(),
    );
    let service_port: Arc<dyn ServicePort> = service.clone();
    let ensure_session = Arc::new(EnsureSession::new(
        service_port,
        Arc::new(MemoryCredentialPort),
        Arc::new(FixedClock),
    ));
    let household_repository: Arc<dyn HouseholdRepositoryPort> = repository;
    InteractiveTurnDriver::new_http(
        service,
        ensure_session,
        SessionSnapshot {
            credentials,
            reconciliation_required: false,
        },
        "profile:read profile:write audio:transcribe",
    )
    .unwrap()
    .with_household_session(Some(HouseholdSession::new(
        native_account(),
        household_repository,
        Arc::new(NativeHouseholdMutationAuthorityV1::new()),
    )))
    .with_profile_presentation_mode(ProfilePresentationModeV1::NativeEnabled)
}

fn start_native_generation(
    driver: &mut InteractiveTurnDriver,
) -> (
    mpsc::Sender<RuntimeEvent>,
    mpsc::Receiver<RuntimeEvent>,
    HouseholdModeGenerationV1,
    HouseholdAccountBindingDigestV1,
) {
    let (events, mut receiver) = mpsc::channel(16);
    driver.start_session(events.clone()).unwrap();
    let mut generation = None;
    let mut voice_observed = false;
    while generation.is_none() || !voice_observed {
        match receiver.blocking_recv().unwrap() {
            RuntimeEvent::ProfilePresentationMode(ProfilePresentationModeV1::NativeEnabled) => {}
            RuntimeEvent::HouseholdGenerationReadyV1 {
                session_mode_generation,
                mode: HouseholdPresentationModeV1::NativeEnabled,
                account_binding_digest,
            } => generation = Some((session_mode_generation, account_binding_digest)),
            RuntimeEvent::VoiceAvailability(_) => voice_observed = true,
            other => panic!("unexpected native generation event: {other:?}"),
        }
    }
    let (generation, digest) = generation.unwrap();
    (events, receiver, generation, digest)
}

fn read_sync_request(stream: &mut std::net::TcpStream) -> String {
    use std::io::Read as _;

    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 1024];
        let count = stream.read(&mut chunk).unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
        })
        .unwrap_or(0);
    while bytes.len() - header_end < length {
        let mut chunk = vec![0; length - (bytes.len() - header_end)];
        let count = stream.read(&mut chunk).unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&chunk[..count]);
    }
    String::from_utf8(bytes).unwrap()
}

fn respond_sync(stream: &mut std::net::TcpStream, status: &str, body: Value) {
    use std::io::Write as _;

    let body = serde_json::to_vec(&body).unwrap();
    stream
        .write_all(
            format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .unwrap();
    stream.write_all(&body).unwrap();
}

fn owner_intent(state: &HouseholdStateV1) -> &heyfood_core::OwnerSyncIntentV1 {
    let HouseholdProfileOutboxEntryV1::OwnerSync { intent, .. } = &state.outbox[0].entry else {
        panic!("expected owner sync intent");
    };
    intent
}

fn owner_intent_handle(state: &HouseholdStateV1) -> OwnerSyncIntentHandleV1 {
    let profile = state
        .profiles
        .iter()
        .find(|profile| profile.subject == HouseholdSubjectId::self_())
        .unwrap();
    let record = &state.outbox[0];
    OwnerSyncIntentHandleV1 {
        outbox_id: record.outbox_id.clone(),
        expected_household_revision: state.revision,
        expected_profile_revision: profile.profile_revision,
        expected_outbox_revision: record.outbox_revision,
    }
}

#[test]
fn native_everyone_turn_stops_before_refresh_serialization_and_network_dispatch() {
    let repository = Arc::new(MemoryHouseholdRepository::with_state(
        everyone_native_household(),
    ));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut driver = native_driver_with_credentials(
        repository,
        listener.local_addr().unwrap(),
        expired_fixture_credentials(),
    );
    let prompt_canary = "prompt-content-canary";
    let (events, mut receiver) = mpsc::channel(2);
    driver
        .start_turn(77, prompt_canary.to_owned(), events)
        .unwrap();
    let event = receiver.blocking_recv().unwrap();
    let message = match &event {
        RuntimeEvent::TurnFailed {
            operation_id: 77,
            message,
        } => message,
        other => panic!("expected local household preflight failure, got {other:?}"),
    };
    assert!(message.starts_with("household_hosted_context_not_authorized:"));
    assert!(!message.contains(prompt_canary));
    assert!(!message.contains("member-context-canary"));
    assert!(!message.contains("expired-access-canary"));
    assert!(!message.contains("expired-refresh-canary"));
    driver
        .shutdown_and_join(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(matches!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    ));
}

#[test]
fn native_everyone_voice_stops_before_microphone_and_network_dispatch() {
    let repository = Arc::new(MemoryHouseholdRepository::with_state(
        everyone_native_household(),
    ));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let capture = Arc::new(ProbeAudioCapture::default());
    let mut driver = native_driver_with_credentials(
        repository,
        listener.local_addr().unwrap(),
        expired_fixture_credentials(),
    )
    .with_audio_capture(capture.clone());
    let (events, mut receiver) = mpsc::channel(2);
    driver.start_voice(78, events).unwrap();
    let event = receiver.blocking_recv().unwrap();
    assert!(matches!(
        event,
        RuntimeEvent::VoiceFailed {
            operation_id: 78,
            message
        } if message.starts_with("household_hosted_context_not_authorized:")
    ));
    assert_eq!(capture.calls.load(Ordering::SeqCst), 0);
    driver
        .shutdown_and_join(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(matches!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    ));
}

#[test]
fn native_member_turn_stops_before_refresh_serialization_and_network_dispatch() {
    let repository = Arc::new(MemoryHouseholdRepository::with_state(
        member_native_household(),
    ));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut driver = native_driver_with_credentials(
        repository,
        listener.local_addr().unwrap(),
        expired_fixture_credentials(),
    );
    let prompt_canary = "member-prompt-content-canary";
    let (events, mut receiver) = mpsc::channel(2);
    driver
        .start_turn(79, prompt_canary.to_owned(), events)
        .unwrap();
    let event = receiver.blocking_recv().unwrap();
    let message = match &event {
        RuntimeEvent::TurnFailed {
            operation_id: 79,
            message,
        } => message,
        other => panic!("expected local household preflight failure, got {other:?}"),
    };
    assert!(message.starts_with("household_hosted_context_not_authorized:"));
    assert!(!message.contains(prompt_canary));
    assert!(!message.contains("member-context-canary"));
    assert!(!message.contains("expired-access-canary"));
    assert!(!message.contains("expired-refresh-canary"));
    driver
        .shutdown_and_join(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(matches!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    ));
}

#[test]
fn native_member_voice_stops_before_microphone_and_network_dispatch() {
    let repository = Arc::new(MemoryHouseholdRepository::with_state(
        member_native_household(),
    ));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let capture = Arc::new(ProbeAudioCapture::default());
    let mut driver = native_driver_with_credentials(
        repository,
        listener.local_addr().unwrap(),
        expired_fixture_credentials(),
    )
    .with_audio_capture(capture.clone());
    let (events, mut receiver) = mpsc::channel(2);
    driver.start_voice(80, events).unwrap();
    let event = receiver.blocking_recv().unwrap();
    assert!(matches!(
        event,
        RuntimeEvent::VoiceFailed {
            operation_id: 80,
            message
        } if message.starts_with("household_hosted_context_not_authorized:")
    ));
    assert_eq!(capture.calls.load(Ordering::SeqCst), 0);
    driver
        .shutdown_and_join(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(matches!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    ));
}

#[test]
fn native_member_create_applies_context_and_restarts_from_committed_scope_without_http() {
    let repository = Arc::new(MemoryHouseholdRepository::with_state(
        incomplete_native_household(),
    ));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();

    let mut driver = native_driver(repository.clone(), listener.local_addr().unwrap());
    let (events, mut receiver, generation, digest) = start_native_generation(&mut driver);
    let binding = HouseholdOperationBindingV1::new(
        HouseholdOperationIdV1::new(1).unwrap(),
        generation,
        digest,
        HouseholdRevision::new(1).unwrap(),
        HouseholdReducerCorrelationV1::new(1).unwrap(),
    );
    driver
        .start_household_member_create(
            binding.clone(),
            BoundedHouseholdMemberDraftV1::new(
                "local-member-canary",
                RelationshipV1::Partner,
                HouseholdAgeEvidenceInputV1::Age18Plus,
            )
            .unwrap(),
            OnboardingProfileInput {
                diet_style_ids: vec!["vegan".into()],
                avoid_ingredients: vec!["local-profile-canary".into()],
                ..OnboardingProfileInput::default()
            },
            events.clone(),
        )
        .unwrap();
    let (resulting_household_revision, affected_subject, active_scope, bounded_active_label) =
        match receiver.blocking_recv().unwrap() {
            RuntimeEvent::HouseholdMutationCommittedV1 {
                binding: event_binding,
                kind: HouseholdMutationKindV1::CreateMember,
                resulting_household_revision,
                affected_subject,
                active_scope,
                bounded_active_label,
            } => {
                assert_eq!(event_binding, binding);
                (
                    resulting_household_revision,
                    affected_subject,
                    active_scope,
                    bounded_active_label,
                )
            }
            other => panic!("expected committed native member creation, got {other:?}"),
        };
    assert_eq!(resulting_household_revision.get(), 2);
    assert_eq!(bounded_active_label, "local-member-canary");
    assert_eq!(
        active_scope,
        HouseholdScope::Subject(affected_subject.clone().unwrap())
    );
    let committed = repository.snapshot();
    assert_eq!(committed.revision, resulting_household_revision);
    assert_eq!(committed.active_scope, active_scope);
    assert_eq!(committed.members.len(), 1);
    assert_eq!(committed.profiles.len(), 1);
    assert!(committed.outbox.is_empty());

    driver
        .start_household_context_apply(
            binding.clone(),
            resulting_household_revision,
            affected_subject,
            active_scope.clone(),
            bounded_active_label.clone(),
            events,
        )
        .unwrap();
    assert!(matches!(
        receiver.blocking_recv(),
        Some(RuntimeEvent::HouseholdContextAppliedV1 {
            binding: event_binding,
            resulting_household_revision: event_revision,
            active_scope: event_scope,
            bounded_active_label: event_label,
        }) if event_binding == binding
            && event_revision == resulting_household_revision
            && event_scope == active_scope
            && event_label == bounded_active_label
    ));
    driver
        .shutdown_and_join(std::time::Duration::from_secs(2))
        .unwrap();

    let mut restarted = native_driver(repository.clone(), listener.local_addr().unwrap());
    let (events, mut receiver, generation, digest) = start_native_generation(&mut restarted);
    let operation_id = HouseholdOperationIdV1::new(2).unwrap();
    let reducer_correlation = HouseholdReducerCorrelationV1::new(2).unwrap();
    restarted
        .start_household_management_load(
            operation_id,
            generation,
            digest,
            reducer_correlation,
            HouseholdManagementLoadPurposeV1::Bootstrap,
            events,
        )
        .unwrap();
    assert!(matches!(
        receiver.blocking_recv(),
        Some(RuntimeEvent::HouseholdManagementLoadedV1 {
            operation_id: event_operation,
            session_mode_generation: event_generation,
            reducer_correlation: event_correlation,
            purpose: HouseholdManagementLoadPurposeV1::Bootstrap,
            account_binding_digest: event_digest,
            household_revision,
            active_scope: event_scope,
            members,
        }) if event_operation == operation_id
            && event_generation == generation
            && event_correlation == reducer_correlation
            && event_digest == digest
            && household_revision == resulting_household_revision
            && event_scope == active_scope
            && members.len() == 2
    ));
    restarted
        .shutdown_and_join(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(matches!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    ));
}

#[test]
fn native_existing_member_profile_save_is_local_atomic_and_applies_exact_context() {
    let mut initial = everyone_native_household();
    initial.active_scope = HouseholdScope::Subject(HouseholdSubjectId::self_());
    let member_id = initial.members[0].member_id.clone();
    let member_subject = HouseholdSubjectId::member(member_id);
    let repository = Arc::new(MemoryHouseholdRepository::with_state(initial));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut driver = native_driver(repository.clone(), listener.local_addr().unwrap());
    let (events, mut receiver, generation, digest) = start_native_generation(&mut driver);
    let binding = HouseholdOperationBindingV1::new(
        HouseholdOperationIdV1::new(6).unwrap(),
        generation,
        digest,
        HouseholdRevision::new(1).unwrap(),
        HouseholdReducerCorrelationV1::new(6).unwrap(),
    );
    driver
        .start_household_member_profile_save(
            binding.clone(),
            member_subject.clone(),
            None,
            OnboardingProfileInput {
                diet_style_ids: vec!["vegetarian".into()],
                avoid_ingredients: vec!["member-save-local-canary".into()],
                ..OnboardingProfileInput::default()
            },
            events.clone(),
        )
        .unwrap();
    let (revision, active_scope, active_label) = match receiver.blocking_recv().unwrap() {
        RuntimeEvent::HouseholdMutationCommittedV1 {
            binding: event_binding,
            kind: HouseholdMutationKindV1::SaveMemberProfile,
            resulting_household_revision,
            affected_subject: Some(event_subject),
            active_scope,
            bounded_active_label,
        } => {
            assert_eq!(event_binding, binding);
            assert_eq!(event_subject, member_subject);
            (
                resulting_household_revision,
                active_scope,
                bounded_active_label,
            )
        }
        other => panic!("expected committed existing-member profile save, got {other:?}"),
    };
    assert_eq!(revision.get(), 2);
    assert_eq!(
        active_scope,
        HouseholdScope::Subject(HouseholdSubjectId::self_())
    );
    assert_eq!(active_label, "Me");
    let committed = repository.snapshot();
    assert_eq!(committed.revision, revision);
    assert!(committed.outbox.is_empty());
    assert_eq!(committed.profiles.len(), 1);
    assert!(committed.profiles.iter().any(|profile| {
        profile.subject == member_subject && profile.profile_revision.get() == 1
    }));
    assert_eq!(
        committed.members[0].profile_state,
        HouseholdProfileStateV1::LocalOnly
    );

    driver
        .start_household_context_apply(
            binding.clone(),
            revision,
            Some(member_subject),
            active_scope.clone(),
            active_label.clone(),
            events,
        )
        .unwrap();
    assert!(matches!(
        receiver.blocking_recv(),
        Some(RuntimeEvent::HouseholdContextAppliedV1 {
            binding: event_binding,
            resulting_household_revision,
            active_scope: event_scope,
            bounded_active_label,
        }) if event_binding == binding
            && resulting_household_revision == revision
            && event_scope == active_scope
            && bounded_active_label == active_label
    ));
    driver
        .shutdown_and_join(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(matches!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    ));
}

#[test]
fn native_everyone_selection_is_local_persistent_and_closed_without_http() {
    let repository = Arc::new(MemoryHouseholdRepository::with_state(
        selectable_everyone_native_household(),
    ));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut driver = native_driver(repository.clone(), listener.local_addr().unwrap());
    let (events, mut receiver, generation, digest) = start_native_generation(&mut driver);
    let load_operation = HouseholdOperationIdV1::new(8).unwrap();
    let load_correlation = HouseholdReducerCorrelationV1::new(8).unwrap();
    driver
        .start_household_management_load(
            load_operation,
            generation,
            digest,
            load_correlation,
            HouseholdManagementLoadPurposeV1::SelectScope,
            events.clone(),
        )
        .unwrap();
    assert!(matches!(
        receiver.blocking_recv(),
        Some(RuntimeEvent::HouseholdManagementLoadedV1 {
            operation_id,
            session_mode_generation,
            reducer_correlation,
            purpose: HouseholdManagementLoadPurposeV1::SelectScope,
            account_binding_digest,
            household_revision,
            ..
        }) if operation_id == load_operation
            && session_mode_generation == generation
            && reducer_correlation == load_correlation
            && account_binding_digest == digest
            && household_revision.get() == 1
    ));
    let binding = HouseholdOperationBindingV1::new(
        HouseholdOperationIdV1::new(9).unwrap(),
        generation,
        digest,
        HouseholdRevision::new(1).unwrap(),
        HouseholdReducerCorrelationV1::new(9).unwrap(),
    );
    driver
        .start_native_household_scope_selection(
            binding.clone(),
            HouseholdScope::Everyone,
            events.clone(),
        )
        .unwrap();
    let revision = match receiver.blocking_recv().unwrap() {
        RuntimeEvent::HouseholdMutationCommittedV1 {
            binding: event_binding,
            kind: HouseholdMutationKindV1::SelectScope,
            resulting_household_revision,
            affected_subject: None,
            active_scope: HouseholdScope::Everyone,
            bounded_active_label,
        } => {
            assert_eq!(event_binding, binding);
            assert_eq!(bounded_active_label, "Everyone");
            resulting_household_revision
        }
        other => panic!("expected committed Everyone selection, got {other:?}"),
    };
    driver
        .start_household_context_apply(
            binding.clone(),
            revision,
            None,
            HouseholdScope::Everyone,
            "Everyone".into(),
            events,
        )
        .unwrap();
    assert!(matches!(
        receiver.blocking_recv(),
        Some(RuntimeEvent::HouseholdContextAppliedV1 {
            binding: event_binding,
            resulting_household_revision,
            active_scope: HouseholdScope::Everyone,
            bounded_active_label,
        }) if event_binding == binding
            && resulting_household_revision == revision
            && bounded_active_label == "Everyone"
    ));
    driver
        .shutdown_and_join(std::time::Duration::from_secs(2))
        .unwrap();
    let state = repository.snapshot();
    assert_eq!(state.active_scope, HouseholdScope::Everyone);
    assert_eq!(state.revision, revision);
    assert!(state.outbox.is_empty());
    assert!(matches!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    ));
}

#[test]
fn native_generation_rejects_credential_account_mismatch_before_repository_or_http() {
    let repository = Arc::new(MemoryHouseholdRepository::with_state(
        incomplete_native_household(),
    ));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut driver = native_driver_with_credentials(
        repository.clone(),
        listener.local_addr().unwrap(),
        credentials_for(AccountId::parse("different-account").unwrap()),
    );
    let (events, _receiver) = mpsc::channel(4);
    let error = driver.start_session(events).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(repository.commit_calls.load(Ordering::SeqCst), 0);
    driver
        .shutdown_and_join(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(matches!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    ));
}

#[test]
fn native_context_apply_requires_exact_driver_commit_evidence_before_repository() {
    let repository = Arc::new(MemoryHouseholdRepository::with_state(
        incomplete_native_household(),
    ));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut driver = native_driver(repository.clone(), listener.local_addr().unwrap());
    let (events, mut receiver, generation, digest) = start_native_generation(&mut driver);
    let binding = HouseholdOperationBindingV1::new(
        HouseholdOperationIdV1::new(19).unwrap(),
        generation,
        digest,
        HouseholdRevision::new(1).unwrap(),
        HouseholdReducerCorrelationV1::new(19).unwrap(),
    );

    driver
        .start_household_context_apply(
            binding.clone(),
            HouseholdRevision::new(1).unwrap(),
            Some(HouseholdSubjectId::self_()),
            HouseholdScope::Subject(HouseholdSubjectId::self_()),
            "Me".into(),
            events,
        )
        .unwrap();

    assert!(matches!(
        receiver.blocking_recv(),
        Some(RuntimeEvent::HouseholdContextApplyFailedV1 {
            binding: event_binding,
            resulting_household_revision,
            reason: HouseholdContextApplyFailureV1::StateChanged,
        }) if event_binding == binding && resulting_household_revision.get() == 1
    ));
    assert_eq!(repository.load_calls.load(Ordering::SeqCst), 0);
    assert_eq!(repository.commit_calls.load(Ordering::SeqCst), 0);
    driver
        .shutdown_and_join(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(matches!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    ));
}

#[test]
fn native_context_apply_state_change_can_immediately_reconcile_without_task_race() {
    let repository = Arc::new(MemoryHouseholdRepository::with_state(
        selectable_everyone_native_household(),
    ));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut driver = native_driver(repository.clone(), listener.local_addr().unwrap());
    let (events, mut receiver, generation, digest) = start_native_generation(&mut driver);
    let binding = HouseholdOperationBindingV1::new(
        HouseholdOperationIdV1::new(30).unwrap(),
        generation,
        digest,
        HouseholdRevision::new(1).unwrap(),
        HouseholdReducerCorrelationV1::new(30).unwrap(),
    );
    driver
        .start_native_household_scope_selection(
            binding.clone(),
            HouseholdScope::Everyone,
            events.clone(),
        )
        .unwrap();
    let committed_revision = match receiver.blocking_recv().unwrap() {
        RuntimeEvent::HouseholdMutationCommittedV1 {
            resulting_household_revision,
            active_scope: HouseholdScope::Everyone,
            ..
        } => resulting_household_revision,
        other => panic!("expected committed Everyone selection, got {other:?}"),
    };

    let external_repository: Arc<dyn HouseholdRepositoryPort> = repository.clone();
    let external_session = HouseholdSession::new(
        native_account(),
        external_repository,
        Arc::new(NativeHouseholdMutationAuthorityV1::new()),
    );
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(external_session.select_scope(
            committed_revision,
            HouseholdScope::Subject(HouseholdSubjectId::self_()),
            CancellationToken::new(),
        ))
        .unwrap();

    driver
        .start_household_context_apply(
            binding.clone(),
            committed_revision,
            None,
            HouseholdScope::Everyone,
            "Everyone".into(),
            events.clone(),
        )
        .unwrap();
    assert!(matches!(
        receiver.blocking_recv(),
        Some(RuntimeEvent::HouseholdContextApplyFailedV1 {
            binding: event_binding,
            resulting_household_revision,
            reason: HouseholdContextApplyFailureV1::StateChanged,
        }) if event_binding == binding && resulting_household_revision == committed_revision
    ));

    let reconcile_operation = HouseholdOperationIdV1::new(31).unwrap();
    let reconcile_correlation = HouseholdReducerCorrelationV1::new(31).unwrap();
    driver
        .start_household_management_load(
            reconcile_operation,
            generation,
            digest,
            reconcile_correlation,
            HouseholdManagementLoadPurposeV1::Bootstrap,
            events,
        )
        .unwrap();
    assert!(matches!(
        receiver.blocking_recv(),
        Some(RuntimeEvent::HouseholdManagementLoadedV1 {
            operation_id,
            reducer_correlation,
            purpose: HouseholdManagementLoadPurposeV1::Bootstrap,
            household_revision,
            active_scope: HouseholdScope::Subject(HouseholdSubjectId::Self_),
            ..
        }) if operation_id == reconcile_operation
            && reducer_correlation == reconcile_correlation
            && household_revision.get() == 3
    ));
    driver
        .shutdown_and_join(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(matches!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    ));
}

#[test]
fn rollback_allows_only_bootstrap_or_panel_load_and_rejects_mutation_before_repository() {
    let repository = Arc::new(MemoryHouseholdRepository::with_state(
        incomplete_native_household(),
    ));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut driver = native_driver(repository.clone(), listener.local_addr().unwrap())
        .with_profile_presentation_mode(ProfilePresentationModeV1::NativeRollbackReadOnly);
    let (events, mut receiver) = mpsc::channel(16);
    driver.start_session(events.clone()).unwrap();
    let mut generation = None;
    let mut voice_observed = false;
    while generation.is_none() || !voice_observed {
        match receiver.blocking_recv().unwrap() {
            RuntimeEvent::ProfilePresentationMode(
                ProfilePresentationModeV1::NativeRollbackReadOnly,
            ) => {}
            RuntimeEvent::HouseholdGenerationReadyV1 {
                session_mode_generation,
                mode: HouseholdPresentationModeV1::NativeRollbackReadOnly,
                account_binding_digest,
            } => generation = Some((session_mode_generation, account_binding_digest)),
            RuntimeEvent::VoiceAvailability(_) => voice_observed = true,
            other => panic!("unexpected rollback generation event: {other:?}"),
        }
    }
    let (generation, digest) = generation.unwrap();
    let operation_id = HouseholdOperationIdV1::new(21).unwrap();
    let correlation = HouseholdReducerCorrelationV1::new(21).unwrap();
    driver
        .start_household_management_load(
            operation_id,
            generation,
            digest,
            correlation,
            HouseholdManagementLoadPurposeV1::AddMember,
            events.clone(),
        )
        .unwrap();
    assert!(matches!(
        receiver.blocking_recv(),
        Some(RuntimeEvent::HouseholdManagementLoadFailedV1 {
            operation_id: event_operation,
            session_mode_generation: event_generation,
            reducer_correlation: event_correlation,
            purpose: HouseholdManagementLoadPurposeV1::AddMember,
            account_binding_digest: event_digest,
            observed_household_revision: None,
            reason: HouseholdManagementFailureV1::ModeChanged,
        }) if event_operation == operation_id
            && event_generation == generation
            && event_correlation == correlation
            && event_digest == digest
    ));
    let binding = HouseholdOperationBindingV1::new(
        HouseholdOperationIdV1::new(22).unwrap(),
        generation,
        digest,
        HouseholdRevision::new(1).unwrap(),
        HouseholdReducerCorrelationV1::new(22).unwrap(),
    );
    driver
        .start_household_member_create(
            binding.clone(),
            BoundedHouseholdMemberDraftV1::new(
                "rollback-member-canary",
                RelationshipV1::Partner,
                HouseholdAgeEvidenceInputV1::Unknown,
            )
            .unwrap(),
            OnboardingProfileInput {
                diet_style_ids: vec!["vegan".into()],
                ..OnboardingProfileInput::default()
            },
            events,
        )
        .unwrap();
    assert!(matches!(
        receiver.blocking_recv(),
        Some(RuntimeEvent::HouseholdMutationFailedV1 {
            binding: event_binding,
            kind: HouseholdMutationKindV1::CreateMember,
            affected_subject: None,
            observed_household_revision: None,
            reason: HouseholdMutationFailureV1::Unavailable,
        }) if event_binding == binding
    ));
    assert_eq!(repository.load_calls.load(Ordering::SeqCst), 0);
    assert_eq!(repository.commit_calls.load(Ordering::SeqCst), 0);
    driver
        .shutdown_and_join(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(matches!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    ));
}

#[test]
fn native_owner_onboarding_is_local_first_and_dispatches_only_after_durable_unknown_state() {
    let repository = Arc::new(MemoryHouseholdRepository::with_state(
        incomplete_native_household(),
    ));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server_repository = repository.clone();
    let server = thread::spawn(move || {
        let mut methods = Vec::new();
        for step in 0..5 {
            let (mut socket, _) = listener.accept().unwrap();
            let request = read_sync_request(&mut socket);
            let request_line = request.lines().next().unwrap().to_owned();
            methods.push(request_line.clone());
            match step {
                0 => {
                    let state = server_repository.snapshot();
                    assert_eq!(state.revision.get(), 2);
                    assert_eq!(state.profiles.len(), 1);
                    assert_eq!(state.outbox.len(), 1);
                    assert_eq!(
                        owner_intent(&state).phase,
                        OwnerSyncIntentPhaseV1::NeedsConsentCheck
                    );
                    assert!(request_line.starts_with("GET /v1/profile/consent "));
                    respond_sync(
                        &mut socket,
                        "200 OK",
                        json!({"has_consent":true,"consent_version":3}),
                    );
                }
                1 => {
                    assert!(request_line.starts_with("GET /v1/profile/consent "));
                    respond_sync(
                        &mut socket,
                        "200 OK",
                        json!({"has_consent":true,"consent_version":3}),
                    );
                }
                2 => {
                    assert!(request_line.starts_with("GET /v1/profile/sync?member_id=_self "));
                    respond_sync(&mut socket, "404 Not Found", json!({}));
                }
                3 => {
                    assert!(request_line.starts_with("GET /v1/profile/consent "));
                    respond_sync(
                        &mut socket,
                        "200 OK",
                        json!({"has_consent":true,"consent_version":3}),
                    );
                }
                4 => {
                    assert!(request_line.starts_with("PUT /v1/profile/sync "));
                    let state = server_repository.snapshot();
                    assert_eq!(
                        owner_intent(&state).phase,
                        OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown
                    );
                    assert_eq!(owner_intent(&state).attempt_count, 1);
                    let body = request.split_once("\r\n\r\n").unwrap().1.as_bytes();
                    let frozen = owner_intent(&state)
                        .request_body
                        .as_ref()
                        .unwrap()
                        .canonical_bytes()
                        .unwrap();
                    assert_eq!(body, frozen);
                    let request_id = owner_intent(&state).remote_request_id.to_string();
                    assert!(request.lines().any(|line| {
                        line.eq_ignore_ascii_case(&format!("x-request-id: {request_id}"))
                    }));
                    respond_sync(
                        &mut socket,
                        "200 OK",
                        json!({
                            "member_id":"_self",
                            "version":1,
                            "updated_at":"2026-07-30T12:00:01.000Z"
                        }),
                    );
                }
                _ => unreachable!(),
            }
        }
        methods
    });
    let mut driver = native_driver(repository.clone(), address);
    let (events, mut receiver) = mpsc::channel(4);
    driver
        .start_onboarding(
            1,
            OnboardingProfileInput {
                diet_style_ids: vec!["vegan".into()],
                ..OnboardingProfileInput::default()
            },
            events,
        )
        .unwrap();
    assert!(matches!(
        receiver.blocking_recv(),
        Some(RuntimeEvent::NativeOwnerOnboardingSaved {
            operation_id: 1,
            status: NativeOwnerProfileSaveStatusV1::SyncPending
        })
    ));
    driver
        .shutdown_and_join(std::time::Duration::from_secs(2))
        .unwrap();
    let methods = server.join().unwrap();
    assert!(methods.iter().all(|request| !request.starts_with("POST ")));
    let state = repository.snapshot();
    assert_eq!(state.owner.profile_state, HouseholdProfileStateV1::Synced);
    assert!(state.outbox.is_empty());
    assert_eq!(state.profiles[0].profile_revision.get(), 1);
}

#[test]
fn native_owner_local_failure_and_rollback_open_no_network_connection() {
    for rollback in [false, true] {
        let initial = incomplete_native_household();
        let repository = Arc::new(MemoryHouseholdRepository::with_state(initial.clone()));
        if !rollback {
            repository.fail_commit_at(1);
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut driver = native_driver(repository.clone(), listener.local_addr().unwrap());
        if rollback {
            driver = driver
                .with_profile_presentation_mode(ProfilePresentationModeV1::NativeRollbackReadOnly);
        }
        let (events, mut receiver) = mpsc::channel(2);
        driver
            .start_onboarding(1, OnboardingProfileInput::default(), events)
            .unwrap();
        assert!(matches!(
            receiver.blocking_recv(),
            Some(RuntimeEvent::OnboardingFailed {
                operation_id: 1,
                ..
            })
        ));
        driver
            .shutdown_and_join(std::time::Duration::from_secs(2))
            .unwrap();
        assert_eq!(repository.snapshot(), initial);
        assert!(matches!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        ));
    }
}

#[test]
fn remote_success_before_local_finalize_retains_dispatch_repair_authority() {
    let repository = Arc::new(MemoryHouseholdRepository::with_state(
        incomplete_native_household(),
    ));
    repository.fail_commit_at(5);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server_repository = repository.clone();
    let server = thread::spawn(move || {
        for step in 0..5 {
            let (mut socket, _) = listener.accept().unwrap();
            let request = read_sync_request(&mut socket);
            match step {
                0 | 1 | 3 => {
                    assert!(request.starts_with("GET /v1/profile/consent "));
                    respond_sync(
                        &mut socket,
                        "200 OK",
                        json!({"has_consent":true,"consent_version":7}),
                    );
                }
                2 => {
                    assert!(request.starts_with("GET /v1/profile/sync?member_id=_self "));
                    respond_sync(&mut socket, "404 Not Found", json!({}));
                }
                4 => {
                    assert!(request.starts_with("PUT /v1/profile/sync "));
                    assert_eq!(
                        owner_intent(&server_repository.snapshot()).phase,
                        OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown
                    );
                    respond_sync(
                        &mut socket,
                        "200 OK",
                        json!({
                            "member_id":"_self",
                            "version":1,
                            "updated_at":"2026-07-30T12:00:01.000Z"
                        }),
                    );
                }
                _ => unreachable!(),
            }
        }
    });
    let mut driver = native_driver(repository.clone(), address);
    let (events, mut receiver) = mpsc::channel(2);
    driver
        .start_onboarding(1, OnboardingProfileInput::default(), events)
        .unwrap();
    assert!(matches!(
        receiver.blocking_recv(),
        Some(RuntimeEvent::NativeOwnerOnboardingSaved {
            operation_id: 1,
            status: NativeOwnerProfileSaveStatusV1::SyncPending
        })
    ));
    driver
        .shutdown_and_join(std::time::Duration::from_secs(2))
        .unwrap();
    server.join().unwrap();
    let state = repository.snapshot();
    assert_eq!(
        owner_intent(&state).phase,
        OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown
    );
    assert_eq!(owner_intent(&state).attempt_count, 1);
    assert_eq!(
        state.owner.profile_state,
        HouseholdProfileStateV1::PendingSync
    );
}

#[test]
fn consent_grant_does_not_advance_local_only_intent_and_only_explicit_retry_cas_resumes_it() {
    let repository = Arc::new(MemoryHouseholdRepository::with_state(
        incomplete_native_household(),
    ));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server_repository = repository.clone();
    let server = thread::spawn(move || {
        for step in 0..9 {
            let (mut socket, _) = listener.accept().unwrap();
            let request = read_sync_request(&mut socket);
            match step {
                0 => {
                    assert!(request.starts_with("GET /v1/profile/consent "));
                    respond_sync(
                        &mut socket,
                        "200 OK",
                        json!({"has_consent":false,"consent_version":null}),
                    );
                }
                1 => {
                    assert!(request.starts_with("POST /v1/profile/consent "));
                    let state = server_repository.snapshot();
                    assert_eq!(
                        owner_intent(&state).phase,
                        OwnerSyncIntentPhaseV1::LocalOnlyNoConsent
                    );
                    respond_sync(
                        &mut socket,
                        "200 OK",
                        json!({"has_consent":true,"consent_version":4}),
                    );
                }
                2 => {
                    assert!(request.starts_with("GET /v1/profile/consent "));
                    assert_eq!(
                        owner_intent(&server_repository.snapshot()).phase,
                        OwnerSyncIntentPhaseV1::LocalOnlyNoConsent
                    );
                    respond_sync(
                        &mut socket,
                        "200 OK",
                        json!({"has_consent":true,"consent_version":4}),
                    );
                }
                3 => {
                    assert!(request.starts_with("GET /v1/profile/consent "));
                    assert_eq!(
                        owner_intent(&server_repository.snapshot()).phase,
                        OwnerSyncIntentPhaseV1::LocalOnlyNoConsent
                    );
                    respond_sync(
                        &mut socket,
                        "200 OK",
                        json!({"has_consent":true,"consent_version":4}),
                    );
                }
                4 => {
                    assert!(request.starts_with("GET /v1/profile/consent "));
                    assert_eq!(
                        owner_intent(&server_repository.snapshot()).phase,
                        OwnerSyncIntentPhaseV1::NeedsConsentCheck
                    );
                    respond_sync(
                        &mut socket,
                        "200 OK",
                        json!({"has_consent":true,"consent_version":4}),
                    );
                }
                5 => {
                    assert!(request.starts_with("GET /v1/profile/consent "));
                    respond_sync(
                        &mut socket,
                        "200 OK",
                        json!({"has_consent":true,"consent_version":4}),
                    );
                }
                6 => {
                    assert!(request.starts_with("GET /v1/profile/sync?member_id=_self "));
                    respond_sync(&mut socket, "404 Not Found", json!({}));
                }
                7 => {
                    assert!(request.starts_with("GET /v1/profile/consent "));
                    respond_sync(
                        &mut socket,
                        "200 OK",
                        json!({"has_consent":true,"consent_version":4}),
                    );
                }
                8 => {
                    assert!(request.starts_with("PUT /v1/profile/sync "));
                    assert_eq!(
                        owner_intent(&server_repository.snapshot()).phase,
                        OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown
                    );
                    respond_sync(
                        &mut socket,
                        "200 OK",
                        json!({
                            "member_id":"_self",
                            "version":1,
                            "updated_at":"2026-07-30T12:00:01.000Z"
                        }),
                    );
                }
                _ => unreachable!(),
            }
        }
    });
    let mut driver = native_driver(repository.clone(), address);
    let (events, mut receiver) = mpsc::channel(4);
    driver
        .start_onboarding(1, OnboardingProfileInput::default(), events.clone())
        .unwrap();
    assert!(matches!(
        receiver.blocking_recv(),
        Some(RuntimeEvent::NativeOwnerOnboardingSaved {
            operation_id: 1,
            status: NativeOwnerProfileSaveStatusV1::SavedWithAbsentConsent
        })
    ));
    let local_only = repository.snapshot();
    let local_only_revision = local_only.revision;
    assert_eq!(
        owner_intent(&local_only).phase,
        OwnerSyncIntentPhaseV1::LocalOnlyNoConsent
    );
    thread::sleep(std::time::Duration::from_millis(10));

    driver
        .start_owner_profile_consent(2, events.clone())
        .unwrap();
    assert!(matches!(
        receiver.blocking_recv(),
        Some(RuntimeEvent::ProfileConsentFinished {
            operation_id: 2,
            result: Ok(finished)
        }) if finished.consent_version.get() == 4 && finished.retry_offered
    ));
    let after_consent = repository.snapshot();
    assert_eq!(after_consent.revision, local_only_revision);
    assert_eq!(
        owner_intent(&after_consent).phase,
        OwnerSyncIntentPhaseV1::LocalOnlyNoConsent
    );
    thread::sleep(std::time::Duration::from_millis(10));

    driver
        .start_owner_profile_actions(
            3,
            OwnerProfileActionLoadPurposeV1::ExplicitRetry,
            events.clone(),
        )
        .unwrap();
    let actions = match receiver.blocking_recv() {
        Some(RuntimeEvent::ProfileActionsLoaded {
            operation_id: 3,
            loaded: ProfileActionsLoadedV1::NativeActions(actions),
        }) => actions,
        other => panic!("unexpected Profile action event: {other:?}"),
    };
    assert_eq!(
        actions.retry,
        OwnerProfileRetryEligibilityV1::StartLocalOnlyAfterConsent
    );
    let action = actions.retry.available_action().unwrap();
    let handle = actions.intent.unwrap();
    thread::sleep(std::time::Duration::from_millis(10));
    driver
        .start_owner_profile_retry(4, action, handle, events)
        .unwrap();
    assert!(matches!(
        receiver.blocking_recv(),
        Some(RuntimeEvent::ProfileRetrySyncFinished {
            operation_id: 4,
            ..
        })
    ));
    driver
        .shutdown_and_join(std::time::Duration::from_secs(2))
        .unwrap();
    server.join().unwrap();
    let state = repository.snapshot();
    assert_eq!(state.owner.profile_state, HouseholdProfileStateV1::Synced);
    assert!(state.outbox.is_empty());
}

#[test]
fn cancellation_after_send_is_durably_classified_as_outcome_uncertain() {
    let repository = Arc::new(MemoryHouseholdRepository::with_state(
        incomplete_native_household(),
    ));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server_repository = repository.clone();
    let (accepted_tx, accepted_rx) = std_mpsc::channel();
    let (release_tx, release_rx) = std_mpsc::channel();
    let server = thread::spawn(move || {
        for step in 0..8 {
            let (mut socket, _) = listener.accept().unwrap();
            let request = read_sync_request(&mut socket);
            match step {
                0 | 1 | 3 | 5 | 6 => respond_sync(
                    &mut socket,
                    "200 OK",
                    json!({"has_consent":true,"consent_version":9}),
                ),
                2 => respond_sync(&mut socket, "404 Not Found", json!({})),
                4 => {
                    assert!(request.starts_with("PUT /v1/profile/sync "));
                    assert_eq!(
                        owner_intent(&server_repository.snapshot()).phase,
                        OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown
                    );
                    accepted_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    // Close without a response after the caller has cancelled.
                }
                7 => {
                    assert!(request.starts_with("GET /v1/profile/sync?member_id=_self "));
                    let state = server_repository.snapshot();
                    let profile_data = owner_intent(&state)
                        .request_body
                        .as_ref()
                        .unwrap()
                        .as_map()
                        .get("profile_data")
                        .unwrap()
                        .clone();
                    respond_sync(
                        &mut socket,
                        "200 OK",
                        json!({
                            "schema_version":1,
                            "member_id":"_self",
                            "version":1,
                            "profile_data":profile_data
                        }),
                    );
                }
                _ => unreachable!(),
            }
        }
    });
    let mut driver = native_driver(repository.clone(), address);
    let (events, mut receiver) = mpsc::channel(2);
    driver
        .start_onboarding(1, OnboardingProfileInput::default(), events)
        .unwrap();
    accepted_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    driver.cancel_turn(1).unwrap();
    release_tx.send(()).unwrap();
    assert!(matches!(
        receiver.blocking_recv(),
        Some(RuntimeEvent::NativeOwnerOnboardingSaved {
            operation_id: 1,
            status: NativeOwnerProfileSaveStatusV1::SyncPending
        })
    ));
    let state = repository.snapshot();
    assert_eq!(
        owner_intent(&state).phase,
        OwnerSyncIntentPhaseV1::OutcomeUncertain
    );
    assert_eq!(owner_intent(&state).attempt_count, 1);
    assert!(owner_intent(&state).last_definite_error.is_none());
    thread::sleep(std::time::Duration::from_millis(10));

    let (events, mut receiver) = mpsc::channel(2);
    driver
        .start_owner_profile_actions(
            2,
            OwnerProfileActionLoadPurposeV1::ExplicitRetry,
            events.clone(),
        )
        .unwrap();
    let actions = match receiver.blocking_recv() {
        Some(RuntimeEvent::ProfileActionsLoaded {
            operation_id: 2,
            loaded: ProfileActionsLoadedV1::NativeActions(actions),
        }) => actions,
        other => panic!("unexpected reconciliation action event: {other:?}"),
    };
    assert_eq!(
        actions.retry,
        OwnerProfileRetryEligibilityV1::ReconcileOutcomeUncertain
    );
    thread::sleep(std::time::Duration::from_millis(10));
    driver
        .start_owner_profile_retry(
            3,
            actions.retry.available_action().unwrap(),
            actions.intent.unwrap(),
            events,
        )
        .unwrap();
    assert!(matches!(
        receiver.blocking_recv(),
        Some(RuntimeEvent::ProfileRetrySyncFinished {
            operation_id: 3,
            ..
        })
    ));
    driver
        .shutdown_and_join(std::time::Duration::from_secs(2))
        .unwrap();
    server.join().unwrap();
    let state = repository.snapshot();
    assert_eq!(state.owner.profile_state, HouseholdProfileStateV1::Synced);
    assert!(state.outbox.is_empty());
}

#[test]
fn mismatched_authenticated_account_blocks_retry_without_http_or_local_transition() {
    let repository = Arc::new(MemoryHouseholdRepository::with_state(
        incomplete_native_household(),
    ));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mismatched_credentials =
        credentials_for(AccountId::parse("different-native-account").unwrap());
    let mut driver = native_driver_with_credentials(
        repository.clone(),
        listener.local_addr().unwrap(),
        mismatched_credentials,
    );
    let (events, mut receiver) = mpsc::channel(2);

    // The reviewed save remains local-first. Account mismatch is discovered
    // only after that durable commit and prevents the first HTTP read.
    driver
        .start_onboarding(1, OnboardingProfileInput::default(), events.clone())
        .unwrap();
    assert!(matches!(
        receiver.blocking_recv(),
        Some(RuntimeEvent::NativeOwnerOnboardingSaved {
            operation_id: 1,
            status: NativeOwnerProfileSaveStatusV1::SyncPending
        })
    ));
    let before_retry = repository.snapshot();
    assert_eq!(
        owner_intent(&before_retry).phase,
        OwnerSyncIntentPhaseV1::NeedsConsentCheck
    );
    let handle = owner_intent_handle(&before_retry);

    driver
        .start_owner_profile_retry(
            2,
            OwnerProfileRetryActionV1::ResumeNeedsConsentCheck,
            handle,
            events,
        )
        .unwrap();
    assert!(matches!(
        receiver.blocking_recv(),
        Some(RuntimeEvent::ProfileRetrySyncFinished {
            operation_id: 2,
            outcome: ProfileRetrySyncFinishedV1::Unavailable {
                reason: OwnerProfileRetryUnavailableReasonV1::ModeOrAccountIneligible
            }
        })
    ));
    driver
        .shutdown_and_join(std::time::Duration::from_secs(2))
        .unwrap();

    assert_eq!(repository.snapshot(), before_retry);
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn attempt_count_overflow_fails_closed_before_transport_send() {
    let repository = Arc::new(MemoryHouseholdRepository::with_state(
        incomplete_native_household(),
    ));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let mut requests = Vec::new();
        for step in 0..6 {
            let (mut socket, _) = listener.accept().unwrap();
            let request = read_sync_request(&mut socket);
            requests.push(request.lines().next().unwrap().to_owned());
            match step {
                0 | 1 | 4 | 5 => respond_sync(
                    &mut socket,
                    "200 OK",
                    json!({"has_consent":true,"consent_version":11}),
                ),
                2 => respond_sync(&mut socket, "404 Not Found", json!({})),
                3 => respond_sync(&mut socket, "500 Internal Server Error", json!({})),
                _ => unreachable!(),
            }
        }
        requests
    });
    let mut driver = native_driver(repository.clone(), address);
    let (events, mut receiver) = mpsc::channel(2);
    driver
        .start_onboarding(1, OnboardingProfileInput::default(), events.clone())
        .unwrap();
    assert!(matches!(
        receiver.blocking_recv(),
        Some(RuntimeEvent::NativeOwnerOnboardingSaved {
            operation_id: 1,
            status: NativeOwnerProfileSaveStatusV1::SyncPending
        })
    ));
    assert_eq!(
        owner_intent(&repository.snapshot()).phase,
        OwnerSyncIntentPhaseV1::ReadyToDispatch
    );
    repository.mutate_state(|state| {
        let HouseholdProfileOutboxEntryV1::OwnerSync { intent, .. } = &mut state.outbox[0].entry
        else {
            unreachable!();
        };
        intent.attempt_count = u32::MAX;
        state.validate().unwrap();
    });
    thread::sleep(std::time::Duration::from_millis(10));

    driver
        .start_owner_profile_actions(
            2,
            OwnerProfileActionLoadPurposeV1::ExplicitRetry,
            events.clone(),
        )
        .unwrap();
    let actions = match receiver.blocking_recv() {
        Some(RuntimeEvent::ProfileActionsLoaded {
            operation_id: 2,
            loaded: ProfileActionsLoadedV1::NativeActions(actions),
        }) => actions,
        other => panic!("unexpected overflow action event: {other:?}"),
    };
    assert_eq!(
        actions.retry,
        OwnerProfileRetryEligibilityV1::ResumeReadyToDispatch
    );
    thread::sleep(std::time::Duration::from_millis(10));
    driver
        .start_owner_profile_retry(
            3,
            actions.retry.available_action().unwrap(),
            actions.intent.unwrap(),
            events,
        )
        .unwrap();
    assert!(matches!(
        receiver.blocking_recv(),
        Some(RuntimeEvent::ProfileRetrySyncFinished {
            operation_id: 3,
            outcome: heyfood_tui::ProfileRetrySyncFinishedV1::Interrupted
        })
    ));
    driver
        .shutdown_and_join(std::time::Duration::from_secs(2))
        .unwrap();
    let requests = server.join().unwrap();
    assert!(requests.iter().all(|request| !request.starts_with("PUT ")));
    let state = repository.snapshot();
    assert_eq!(
        owner_intent(&state).phase,
        OwnerSyncIntentPhaseV1::ReadyToDispatch
    );
    assert_eq!(owner_intent(&state).attempt_count, u32::MAX);
}

#[tokio::test]
async fn default_executable_uses_brokered_native_account_bound_credentials() {
    let root = TempRoot::new();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/", listener.local_addr().unwrap());
    let service_url = base_url.clone();
    let server = tokio::spawn(async move {
        let full_scope = "account:link account:delete knowledge:read menu:read menu:watch recommend:read recipes:read recipes:write claims:read_derived profile:read profile:write meals:read meals:write audio:transcribe grocery:read grocery:write";
        let verification_uri = format!("{service_url}authorize");
        for _ in 0..6 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            let path = request
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap();
            match path {
                "/v1/auth/capabilities" => {
                    respond_json(&mut socket, json!({
                        "schema_version": 1,
                        "self_registration": {"status": "available", "regions": ["US"], "identity_methods": ["sms", "email"]},
                        "authorization": {"loopback_pkce": true, "device_code": true, "identity_methods": ["sms", "email"]},
                        "profile_readiness": true,
                        "application_capabilities": {}
                    })).await;
                }
                "/v1/channel/oauth/device/authorize" => {
                    respond_json(
                        &mut socket,
                        json!({
                            "device_code": "hf_dc_01234567890123456789",
                            "user_code": "ABCD-EFGH",
                            "verification_uri": verification_uri,
                            "verification_uri_complete": null,
                            "expires_in": 600,
                            "interval": 1
                        }),
                    )
                    .await;
                }
                "/v1/channel/oauth/device/token" => {
                    respond_json(
                        &mut socket,
                        json!({
                            "access_token": "channel-access",
                            "token_type": "bearer",
                            "refresh_token": "channel-refresh",
                            "expires_in": 3600,
                            "scope": full_scope
                        }),
                    )
                    .await;
                }
                "/v1/channel/oauth/cli/session" => {
                    let request_body: Value =
                        serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
                    let device_id = request_body["device_id"].as_str().unwrap();
                    respond_json(
                        &mut socket,
                        json!({
                            "user_id": "native-composition-account",
                            "device_id": device_id,
                            "session_id": "native-composition-session",
                            "access_token": "session-access",
                            "refresh_token": "session-refresh",
                            "access_expires_at": "2999-01-01T00:00:00Z",
                            "refresh_expires_at": "2999-02-01T00:00:00Z",
                            "scopes": full_scope.split_whitespace().collect::<Vec<_>>(),
                            "is_anonymous": false
                        }),
                    )
                    .await;
                }
                "/v1/channel/tools/profile/readiness" => {
                    respond_json(
                        &mut socket,
                        json!({
                            "schema_version": 1,
                            "status": "ready",
                            "member_id": "_self",
                            "has_profile_sync_consent": true,
                            "profile_version": 1
                        }),
                    )
                    .await;
                }
                "/v1/agent/converse" => {
                    assert!(
                        request
                            .to_ascii_lowercase()
                            .contains("authorization: bearer session-access")
                    );
                    let body = b"event: result\ndata: {\"message\":\"native broker ok\"}\n\nevent: done\ndata: {}\n\n";
                    socket.write_all(format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    ).as_bytes()).await.unwrap();
                    socket.write_all(body).await.unwrap();
                }
                _ => panic!("unexpected path: {path}"),
            }
        }
    });
    let registration = run(
        &root.0,
        &base_url,
        &[
            "--json",
            "register",
            "--device",
            "--no-browser",
            "--timeout",
            "5",
            "--no-onboard",
        ],
    )
    .await;
    assert!(
        registration.status.success(),
        "registration stdout: {}; stderr: {}",
        String::from_utf8_lossy(&registration.stdout),
        String::from_utf8_lossy(&registration.stderr)
    );
    let output = run(
        &root.0,
        &base_url,
        &["--json", "ask", "native", "composition"],
    )
    .await;
    if !output.status.success() {
        server.abort();
        let _ = cleanup(&root.0).await;
        panic!(
            "status: {}; stdout: {}; stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    server.await.unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["message"],
        "native broker ok"
    );
    assert!(cleanup(&root.0).await.status.success());
    #[cfg(windows)]
    NativeAuthStore::open(&root.0).unwrap().delete().unwrap();
}
