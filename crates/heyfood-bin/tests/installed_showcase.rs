//! Installed-artifact qualification harness.
//!
//! This test is ignored during ordinary Cargo runs because it requires an
//! exact packaged archive and checksum manifest. Native CLI CI invokes it
//! explicitly against an archive produced by the same job. The spawned user
//! executable is always extracted from that archive; `CARGO_BIN_EXE_heyfood`
//! is intentionally forbidden here.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use heyfood_platform::WindowsCredentialQualificationCleanup;
use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use unicode_width::UnicodeWidthChar;

const TEST_PROMPT: &str = "Plan a synthetic dinner for installed-artifact qualification.";
const TEST_RESPONSE: &str = "Installed artifact first turn complete.";
const RETURNING_PROMPT: &str = "Give me a second authenticated installed-artifact turn.";
const RETURNING_RESPONSE: &str = "Returning installed user turn complete.";
const GROCERY_CANCEL_PROMPT: &str = "Prepare onion for Maya, then let me cancel it.";
const GROCERY_EDIT_PROMPT: &str = "Prepare onion for Maya so I can edit and accept it.";
const GROCERY_STALE_LIST_PROMPT: &str =
    "Prepare a Grocery proposal with intentionally stale list authority.";
const GROCERY_STALE_CONTEXT_PROMPT: &str =
    "Prepare a Grocery proposal with intentionally stale household context.";
const GROCERY_CTRL_C_PROMPT: &str = "Prepare a Grocery proposal that I will cancel with Ctrl+C.";
const STREAM_CANCEL_PROMPT: &str = "Stream until I cancel this installed turn.";
const UNCERTAIN_PROMPT: &str = "Consume this mutation-like turn and close before responding.";
const FAILURE_PROMPT: &str = "Return a typed synthetic failure to the installed TUI.";
const WIDTH_PROMPT: &str = "Render a width-qualified installed response.";
const WIDTH_RESPONSE: &str = "Width-qualified installed response complete.";
const TEST_ACCOUNT: &str = "showcase-user";
const TEST_DEVICE_CODE: &str = "hf_dc_showcase_01234567890123456789";
const TEST_LIST_ID: &str = "00000000-0000-4000-8000-000000000123";
const CANCEL_CONFIRMATION_ID: &str = "00000000-0000-4000-8000-000000000011";
const CANCEL_IDEMPOTENCY_KEY: &str = "00000000-0000-4000-8000-000000000012";
const EDIT_CONFIRMATION_ID: &str = "00000000-0000-4000-8000-000000000021";
const EDIT_IDEMPOTENCY_KEY: &str = "00000000-0000-4000-8000-000000000022";
const STALE_LIST_CONFIRMATION_ID: &str = "00000000-0000-4000-8000-000000000031";
const STALE_LIST_IDEMPOTENCY_KEY: &str = "00000000-0000-4000-8000-000000000032";
const STALE_CONTEXT_CONFIRMATION_ID: &str = "00000000-0000-4000-8000-000000000033";
const STALE_CONTEXT_IDEMPOTENCY_KEY: &str = "00000000-0000-4000-8000-000000000034";
const CTRL_C_CONFIRMATION_ID: &str = "00000000-0000-4000-8000-000000000041";
const CTRL_C_IDEMPOTENCY_KEY: &str = "00000000-0000-4000-8000-000000000042";
const FULL_SCOPE: &str = "account:link account:delete knowledge:read menu:read menu:watch recommend:read recipes:read recipes:write claims:read_derived profile:read profile:write meals:read meals:write audio:transcribe health:read integrations:manage grocery:read grocery:write";
const CORE_MATRIX_GROUPS: [&str; 5] = [
    "clean-user",
    "returning-user",
    "household-grocery",
    "failure-safety",
    "artifact-behavior",
];
const ENTER_ALTERNATE_SCREEN: &[u8] = b"\x1b[?1049h";
const LEAVE_ALTERNATE_SCREEN: &[u8] = b"\x1b[?1049l";
const ENABLE_BRACKETED_PASTE: &[u8] = b"\x1b[?2004h";
const DISABLE_BRACKETED_PASTE: &[u8] = b"\x1b[?2004l";
const HIDE_CURSOR: &[u8] = b"\x1b[?25l";
const SHOW_CURSOR: &[u8] = b"\x1b[?25h";

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must follow the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "heyfood-installed-showcase-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create isolated showcase directory");
        Self(path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(windows)]
struct WindowsCredentialCleanup {
    cleanup: WindowsCredentialQualificationCleanup,
    verified_absent: bool,
}

#[cfg(windows)]
impl WindowsCredentialCleanup {
    fn open(root: &Path) -> Self {
        Self {
            cleanup: WindowsCredentialQualificationCleanup::open(root)
                .expect("open isolated Windows credential cleanup"),
            verified_absent: false,
        }
    }

    fn purge_and_verify_absent(&mut self) {
        self.cleanup
            .purge_and_verify_absent()
            .expect("purge and verify isolated Windows credentials");
        self.verified_absent = true;
    }
}

#[cfg(windows)]
impl Drop for WindowsCredentialCleanup {
    fn drop(&mut self) {
        if !self.verified_absent {
            let _ = self.cleanup.purge_and_verify_absent();
        }
    }
}

#[derive(Clone, Debug)]
struct RequestEvidence {
    method: String,
    path: String,
}

#[derive(Debug)]
struct FixtureSummary {
    device_authorizations: usize,
    cli_sessions: usize,
    consent_grants: usize,
    profile_uploads: usize,
    proposal_cancellations: usize,
    ctrl_c_proposal_cancellations: usize,
    proposal_accepts: usize,
    stale_list_rejections: usize,
    stale_context_rejections: usize,
    stream_cancellations: usize,
    list_version: u64,
    prompt_counts: BTreeMap<String, usize>,
}

struct FixtureState {
    expected_device_id: Option<String>,
    profile_consent: bool,
    profile_version: Option<u64>,
    list_version: u64,
    device_authorizations: usize,
    cli_sessions: usize,
    consent_grants: usize,
    profile_uploads: usize,
    proposal_cancellations: usize,
    ctrl_c_proposal_cancellations: usize,
    proposal_accepts: usize,
    stale_list_rejections: usize,
    stale_context_rejections: usize,
    stream_cancellations: usize,
    prompt_counts: BTreeMap<String, usize>,
}

impl Default for FixtureState {
    fn default() -> Self {
        Self {
            expected_device_id: None,
            profile_consent: false,
            profile_version: None,
            list_version: 4,
            device_authorizations: 0,
            cli_sessions: 0,
            consent_grants: 0,
            profile_uploads: 0,
            proposal_cancellations: 0,
            ctrl_c_proposal_cancellations: 0,
            proposal_accepts: 0,
            stale_list_rejections: 0,
            stale_context_rejections: 0,
            stream_cancellations: 0,
            prompt_counts: BTreeMap::new(),
        }
    }
}

impl FixtureState {
    fn finish(self) -> FixtureSummary {
        FixtureSummary {
            device_authorizations: self.device_authorizations,
            cli_sessions: self.cli_sessions,
            consent_grants: self.consent_grants,
            profile_uploads: self.profile_uploads,
            proposal_cancellations: self.proposal_cancellations,
            ctrl_c_proposal_cancellations: self.ctrl_c_proposal_cancellations,
            proposal_accepts: self.proposal_accepts,
            stale_list_rejections: self.stale_list_rejections,
            stale_context_rejections: self.stale_context_rejections,
            stream_cancellations: self.stream_cancellations,
            list_version: self.list_version,
            prompt_counts: self.prompt_counts,
        }
    }
}

struct FixtureService {
    base_url: String,
    requests: mpsc::Receiver<RequestEvidence>,
    shutdown: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<FixtureSummary>,
}

#[derive(Clone)]
enum PtyAction {
    Wait(String),
    Submit(String),
    CtrlC,
    CtrlD,
    Pause(Duration),
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

struct TerminalCapture {
    bytes: Mutex<Vec<u8>>,
    changed: Condvar,
    rows: usize,
    columns: usize,
}

impl TerminalCapture {
    fn new(rows: u16, columns: u16) -> Self {
        Self {
            bytes: Mutex::new(Vec::new()),
            changed: Condvar::new(),
            rows: usize::from(rows),
            columns: usize::from(columns),
        }
    }

    fn append(&self, bytes: &[u8]) {
        self.bytes
            .lock()
            .expect("lock terminal capture")
            .extend_from_slice(bytes);
        self.changed.notify_all();
    }

    fn wait_for_semantic(&self, needle: &str, timeout: Duration) -> Result<(), String> {
        let expected = compact_terminal_text(needle);
        let deadline = Instant::now() + timeout;
        let mut bytes = self.bytes.lock().map_err(|_| "terminal capture poisoned")?;
        loop {
            let observed =
                compact_terminal_text(&terminal_snapshot(&bytes, self.rows, self.columns));
            if observed.contains(&expected) {
                return Ok(());
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(format!(
                    "terminal output did not contain {:?}; observed {:?}",
                    needle, observed
                ));
            }
            let remaining = deadline.saturating_duration_since(now);
            let (next, _) = self
                .changed
                .wait_timeout(bytes, remaining.min(Duration::from_millis(100)))
                .map_err(|_| "terminal capture poisoned")?;
            bytes = next;
        }
    }

    fn wait_for_final_terminal_state(
        &self,
        timeout: Duration,
        settle: Duration,
    ) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let mut bytes = self.bytes.lock().map_err(|_| "terminal capture poisoned")?;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Err("terminal output did not settle in the restored final state".into());
            }
            let remaining = deadline.saturating_duration_since(now);
            let observed_length = bytes.len();
            let stable_for = if terminal_final_state(&bytes) {
                settle.min(remaining)
            } else {
                Duration::from_millis(100).min(remaining)
            };
            let (next, wait) = self
                .changed
                .wait_timeout(bytes, stable_for)
                .map_err(|_| "terminal capture poisoned")?;
            bytes = next;
            if wait.timed_out() && bytes.len() == observed_length && terminal_final_state(&bytes) {
                return Ok(());
            }
        }
    }

    fn snapshot(&self) -> Vec<u8> {
        self.bytes.lock().expect("lock terminal capture").clone()
    }
}

#[test]
#[ignore = "requires an exact packaged archive supplied by Native CLI CI"]
fn installed_archive_core_release_matrix() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("build showcase fixture runtime");
    runtime.block_on(run_installed_archive_core_release_matrix());
}

async fn run_installed_archive_core_release_matrix() {
    let archive = required_absolute_path("HEYFOOD_SHOWCASE_ARCHIVE");
    let manifest = required_absolute_path("HEYFOOD_SHOWCASE_MANIFEST");
    let evidence_directory = required_absolute_path("HEYFOOD_SHOWCASE_EVIDENCE_DIR");
    let expected_target = required_env("HEYFOOD_SHOWCASE_TARGET");
    let expected_version = required_env("HEYFOOD_SHOWCASE_VERSION");
    assert_semver(&expected_version);
    fs::create_dir_all(&evidence_directory).expect("create showcase evidence directory");

    let expected_archive_name = archive_name(&expected_version, &expected_target);
    assert_eq!(
        archive.file_name().and_then(|value| value.to_str()),
        Some(expected_archive_name.as_str()),
        "showcase archive name must identify the exact version and target"
    );
    let archive_digest = sha256_file(&archive);
    assert_manifest_digest(&manifest, &expected_archive_name, archive_digest.as_str());

    let extraction = TempRoot::new("extraction");
    let expected_binary_name = if expected_target.ends_with("-windows-msvc") {
        "heyfood.exe"
    } else {
        "heyfood"
    };
    assert_archive_policy(&archive, expected_binary_name);
    extract_archive(&archive, &extraction.0);
    let installed_binary = extraction.0.join(expected_binary_name);
    assert_single_installed_executable(&extraction.0, &installed_binary);
    let installed_binary = installed_binary
        .canonicalize()
        .expect("canonicalize installed showcase executable");
    assert!(
        installed_binary.starts_with(
            extraction
                .0
                .canonicalize()
                .expect("canonicalize extraction root")
        ),
        "installed executable must remain under the clean extraction root"
    );
    assert_not_repository_binary(&installed_binary);
    let executable_digest = sha256_file(&installed_binary);
    assert_installed_version(&installed_binary, &expected_version);

    let user = TempRoot::new("user");
    assert!(
        !user.0.join("heyfood").join("config.json").exists(),
        "clean-user registration must start without a legacy import source"
    );
    #[cfg(windows)]
    let mut credential_cleanup = WindowsCredentialCleanup::open(&user.0);
    let fixture = start_fixture_service().await;
    let FixtureService {
        base_url,
        requests: request_receiver,
        shutdown,
        task: server,
    } = fixture;

    let clean_user = run_installed_pty(
        &installed_binary,
        &user.0,
        &base_url,
        &["register", "--device", "--no-browser", "--timeout", "10"],
        80,
        false,
        vec![
            PtyAction::Wait("Kosher".into()),
            PtyAction::Submit("none".into()),
            PtyAction::Wait("colorings".into()),
            PtyAction::Submit("none".into()),
            PtyAction::Wait("Autism".into()),
            PtyAction::Submit("none".into()),
            PtyAction::Wait("Ingredients to avoid".into()),
            PtyAction::Submit("none".into()),
            PtyAction::Wait("Activity level".into()),
            PtyAction::Submit("none".into()),
            PtyAction::Wait("Georgian".into()),
            PtyAction::Submit("none".into()),
            PtyAction::Wait("Additional notes".into()),
            PtyAction::Submit("none".into()),
            PtyAction::Wait("Review dietary profile".into()),
            PtyAction::Submit("save".into()),
            PtyAction::Wait("Dietary profile saved".into()),
            PtyAction::Pause(Duration::from_millis(250)),
            PtyAction::Submit(TEST_PROMPT.into()),
            PtyAction::Wait(TEST_RESPONSE.into()),
            PtyAction::Pause(Duration::from_millis(250)),
            PtyAction::CtrlD,
        ],
    )
    .await;

    write_household_import_source(&user.0);

    let returning_user = run_installed_pty(
        &installed_binary,
        &user.0,
        &base_url,
        &[],
        80,
        false,
        vec![
            PtyAction::Wait("hey.food".into()),
            PtyAction::Submit(RETURNING_PROMPT.into()),
            PtyAction::Wait(RETURNING_RESPONSE.into()),
            PtyAction::Pause(Duration::from_millis(250)),
            PtyAction::Submit("/for Maya".into()),
            PtyAction::Wait("Future turns will consider Maya.".into()),
            PtyAction::Pause(Duration::from_millis(250)),
            PtyAction::Submit("/grocery".into()),
            PtyAction::Wait("onion for maya-uuid".into()),
            PtyAction::Wait("maya-uuid: risky · intended".into()),
            PtyAction::Wait("source: recipe:list-dahl-001".into()),
            PtyAction::Wait("Onion is high-FODMAP.".into()),
            PtyAction::Wait("green parts of scallion".into()),
            PtyAction::Pause(Duration::from_millis(250)),
            PtyAction::Submit(GROCERY_CANCEL_PROMPT.into()),
            PtyAction::Wait("Cancel proposal for Maya".into()),
            PtyAction::Submit("n".into()),
            PtyAction::Wait("cancelled without mutation".into()),
            PtyAction::Pause(Duration::from_millis(250)),
            PtyAction::Submit(GROCERY_EDIT_PROMPT.into()),
            PtyAction::Wait("Edit proposal for Maya".into()),
            PtyAction::Wait("1. onion for maya-uuid".into()),
            PtyAction::Wait("maya-uuid: risky · intended".into()),
            PtyAction::Wait("source: recipe:dahl-001".into()),
            PtyAction::Wait("scallion greens".into()),
            PtyAction::Submit("edit #1 scallion greens".into()),
            PtyAction::Wait("advanced exactly once to version 5".into()),
            PtyAction::Pause(Duration::from_millis(250)),
            PtyAction::Submit(GROCERY_STALE_LIST_PROMPT.into()),
            PtyAction::Wait("Stale list proposal for Maya".into()),
            PtyAction::Submit("y".into()),
            PtyAction::Wait("Stale Grocery list authority rejected".into()),
            PtyAction::Pause(Duration::from_millis(250)),
            PtyAction::Submit(GROCERY_STALE_CONTEXT_PROMPT.into()),
            PtyAction::Wait("Stale context proposal for Maya".into()),
            PtyAction::Submit("y".into()),
            PtyAction::Wait("Stale household context authority rejected".into()),
            PtyAction::Pause(Duration::from_millis(250)),
            PtyAction::Submit(GROCERY_CTRL_C_PROMPT.into()),
            PtyAction::Wait("Ctrl+C proposal for Maya".into()),
            PtyAction::CtrlC,
            PtyAction::Wait("Ctrl+C Grocery cancellation completed".into()),
            PtyAction::Pause(Duration::from_millis(250)),
            PtyAction::Submit(STREAM_CANCEL_PROMPT.into()),
            PtyAction::Wait("Streaming response in progress".into()),
            PtyAction::CtrlC,
            PtyAction::Wait("Turn cancelled after server acceptance".into()),
            PtyAction::Pause(Duration::from_millis(250)),
            PtyAction::Submit(UNCERTAIN_PROMPT.into()),
            PtyAction::Wait("server outcome is unknown".into()),
            PtyAction::Pause(Duration::from_millis(250)),
            PtyAction::Submit(FAILURE_PROMPT.into()),
            PtyAction::Wait("synthetic installed failure".into()),
            PtyAction::Pause(Duration::from_millis(250)),
            PtyAction::CtrlD,
        ],
    )
    .await;

    let width_40 = run_installed_pty(
        &installed_binary,
        &user.0,
        &base_url,
        &[],
        40,
        false,
        vec![
            PtyAction::Wait("hey.food".into()),
            PtyAction::Submit("/grocery".into()),
            PtyAction::Wait("Onion is high-FODMAP.".into()),
            PtyAction::Wait("green parts of scallion".into()),
            PtyAction::Pause(Duration::from_millis(250)),
            PtyAction::CtrlD,
        ],
    )
    .await;

    let width_120_no_color = run_installed_pty(
        &installed_binary,
        &user.0,
        &base_url,
        &[],
        120,
        true,
        vec![
            PtyAction::Wait("hey.food".into()),
            PtyAction::Submit(WIDTH_PROMPT.into()),
            PtyAction::Wait(WIDTH_RESPONSE.into()),
            PtyAction::Pause(Duration::from_millis(250)),
            PtyAction::CtrlD,
        ],
    )
    .await;

    let interrupt_exit = run_installed_pty(
        &installed_binary,
        &user.0,
        &base_url,
        &[],
        80,
        false,
        vec![
            PtyAction::Wait("hey.food".into()),
            PtyAction::CtrlC,
            PtyAction::Wait("Press Ctrl+C again to exit".into()),
            PtyAction::CtrlC,
        ],
    )
    .await;

    shutdown
        .send(())
        .expect("stop installed showcase fixture service");
    let summary = server.await.expect("join showcase fixture service");
    let requests = collect_request_evidence(request_receiver).await;
    assert_fixture_summary(&summary);
    assert_core_terminal_contract(
        &clean_user,
        &returning_user,
        &width_40,
        &width_120_no_color,
        &interrupt_exit,
    );

    #[cfg(windows)]
    credential_cleanup.purge_and_verify_absent();
    fs::remove_dir_all(&user.0).expect("remove isolated installed-user state");
    assert!(
        !user.0.exists(),
        "installed-user state must be absent before PASS evidence"
    );

    let captures = [
        ("clean-user.ansi", &clean_user),
        ("returning-user.ansi", &returning_user),
        ("width-40.ansi", &width_40),
        ("width-120-no-color.ansi", &width_120_no_color),
        ("interrupt-exit.ansi", &interrupt_exit),
    ];
    for (name, bytes) in &captures {
        fs::write(evidence_directory.join(name), bytes)
            .expect("write privacy-safe installed terminal evidence");
    }
    let evidence = json!({
        "schema_version": 2,
        "qualification": "installed-artifact-core-matrix",
        "release_gate_complete": false,
        "archive": {
            "file_name": expected_archive_name,
            "sha256": archive_digest,
            "target": expected_target,
            "version": expected_version
        },
        "executable": {
            "file_name": expected_binary_name,
            "sha256": executable_digest,
            "source_checkout_binary": false
        },
        "environment": {
            "clean_user_state": true,
            "legacy_import_absent_during_clean_user": true,
            "household_import_after_clean_user_exit": true,
            "credentials_absent_after_run": true,
            "pty": true,
            "columns": [40, 80, 120],
            "rows": 30,
            "no_color": true,
            "synthetic_backend": true,
            "credential_backend": if cfg!(windows) { "native" } else { "isolated_file" },
            "signed_candidate_native_backend_required": true,
            "signed_candidate_native_backend_proven": false
        },
        "core_matrix": [
            {
                "id": "clean-user",
                "status": "passed",
                "assertions": [
                    "device_registration_executed",
                    "account_bound_credentials_persisted",
                    "missing_profile_onboarding_completed",
                    "profile_sync_consent_granted",
                    "profile_uploaded_once",
                    "first_authenticated_tui_turn_completed"
                ]
            },
            {
                "id": "returning-user",
                "status": "passed",
                "assertions": [
                    "first_process_exited",
                    "second_installed_process_reloaded_credentials",
                    "registration_not_repeated",
                    "second_authenticated_turn_completed"
                ]
            },
            {
                "id": "household-grocery",
                "status": "passed",
                "assertions": [
                    "maya_household_scope_bound",
                    "active_list_member_binding_screening_substitution_and_provenance_rendered",
                    "proposal_member_binding_screening_substitution_and_provenance_rendered",
                    "proposal_cancelled_without_mutation",
                    "proposal_edited_and_accepted_once",
                    "exact_server_minted_idempotency_authority_replayed",
                    "list_advanced_once",
                    "stale_list_authority_rejected",
                    "stale_household_context_authority_rejected"
                ]
            },
            {
                "id": "failure-safety",
                "status": "passed",
                "assertions": [
                    "uncertain_dispatch_not_retried",
                    "ctrl_c_cancelled_stream",
                    "ctrl_c_cancelled_pending_confirmation_without_mutation",
                    "typed_failure_rendered_without_terminal_loss",
                    "full_presentation_restoration_after_normal_and_app_interrupt_exit"
                ],
                "companion_ci_requirements": [
                    "native_signal_and_canonical_mode_internal_pty_gate",
                    "body_error_and_panic_terminal_guard_tests"
                ]
            },
            {
                "id": "artifact-behavior",
                "status": "source-qualified",
                "assertions": [
                    "semantic_output_at_40_80_120_columns",
                    "no_color_has_no_color_sgr",
                    "archive_digest_exact",
                    "packaged_executable_only"
                ],
                "remaining": [
                    "rerun_exact_matrix_against_signed_archives",
                    "real_platform_credential_backend_on_every_signed_candidate"
                ]
            }
        ],
        "requests": requests.iter().map(|request| json!({
            "method": request.method,
            "path": request.path
        })).collect::<Vec<_>>(),
        "fixture_state": {
            "device_authorizations": summary.device_authorizations,
            "cli_sessions": summary.cli_sessions,
            "consent_grants": summary.consent_grants,
            "profile_uploads": summary.profile_uploads,
            "proposal_cancellations": summary.proposal_cancellations,
            "proposal_accepts": summary.proposal_accepts,
            "stale_list_rejections": summary.stale_list_rejections,
            "stale_context_rejections": summary.stale_context_rejections,
            "stream_cancellations": summary.stream_cancellations,
            "final_list_version": summary.list_version
        },
        "terminal": captures.iter().map(|(name, bytes)| json!({
            "file_name": name,
            "sha256": sha256_bytes(bytes),
            "contains_credentials": false
        })).collect::<Vec<_>>(),
        "deferred": {
            "native_voice": "not enabled in the default 0.5.0 artifact",
            "menu_watch_diff": "not a 0.5.0 gate",
            "health_and_menu_watch_management": "require a bounded live canary or truthful deferral"
        },
        "remaining_release_gates": [
            "production_registration_and_grocery_canaries",
            "protected_signing_environment",
            "signed_candidate_core_matrix_rerun",
            "exact_sha_release_review"
        ]
    });
    fs::write(
        evidence_directory.join("installed-core-matrix.json"),
        serde_json::to_vec_pretty(&evidence).expect("serialize installed evidence"),
    )
    .expect("write installed evidence");
}

fn write_household_import_source(user_root: &Path) {
    let source = user_root.join("heyfood").join("config.json");
    fs::create_dir_all(source.parent().expect("household import parent"))
        .expect("create household import parent");
    let document = json!({
        "account_user_id": TEST_ACCOUNT,
        "first_name": "Showcase",
        "household": {
            "version": 1,
            "active_scope": "_self",
            "members": [
                {
                    "id": "_self",
                    "name": "Showcase",
                    "relationship": "self",
                    "archived": false
                },
                {
                    "id": "maya-uuid",
                    "name": "Maya",
                    "relationship": "child",
                    "archived": false
                }
            ]
        },
        "household_local_profiles": {
            "maya-uuid": {
                "restrictions": ["low_fodmap"],
                "avoid_ingredients": ["onion", "garlic"]
            }
        }
    });
    fs::write(
        source,
        serde_json::to_vec_pretty(&document).expect("encode household import source"),
    )
    .expect("write household import source");
}

fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must be configured"))
}

fn required_absolute_path(name: &str) -> PathBuf {
    let path = PathBuf::from(required_env(name));
    assert!(path.is_absolute(), "{name} must be absolute");
    path
}

fn assert_semver(version: &str) {
    let parts = version.split('.').collect::<Vec<_>>();
    assert_eq!(parts.len(), 3, "version must use MAJOR.MINOR.PATCH");
    assert!(
        parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())),
        "version must use MAJOR.MINOR.PATCH"
    );
}

fn archive_name(version: &str, target: &str) -> String {
    let suffix = match target {
        "aarch64-apple-darwin"
        | "x86_64-apple-darwin"
        | "aarch64-unknown-linux-gnu"
        | "x86_64-unknown-linux-gnu" => "tar.gz",
        "x86_64-pc-windows-msvc" => "zip",
        _ => panic!("unsupported showcase target: {target}"),
    };
    format!("heyfood-v{version}-{target}.{suffix}")
}

fn sha256_file(path: &Path) -> String {
    let bytes = fs::read(path).expect("read file for SHA-256");
    sha256_bytes(&bytes)
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn assert_manifest_digest(manifest: &Path, file_name: &str, digest: &str) {
    let contents = fs::read_to_string(manifest).expect("read showcase checksum manifest");
    let matches = contents
        .lines()
        .filter_map(|line| {
            let (observed_digest, observed_name) = line.split_once(char::is_whitespace)?;
            let observed_name = observed_name.trim_start_matches([' ', '*']);
            (observed_name == file_name).then_some(observed_digest)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        matches,
        [digest],
        "checksum manifest must bind the exact showcase archive once"
    );
}

fn assert_archive_policy(archive: &Path, expected_binary_name: &str) {
    let output = Command::new("tar")
        .args(["-tf"])
        .arg(archive)
        .stdin(Stdio::null())
        .output()
        .expect("list installed showcase archive");
    assert!(
        output.status.success(),
        "archive listing failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let listing = String::from_utf8(output.stdout).expect("archive listing must be UTF-8");
    let entries = listing
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(
        entries,
        [expected_binary_name],
        "archive must contain exactly the native executable at its root"
    );
}

fn extract_archive(archive: &Path, destination: &Path) {
    let output = Command::new("tar")
        .args(["-xf"])
        .arg(archive)
        .arg("-C")
        .arg(destination)
        .stdin(Stdio::null())
        .output()
        .expect("extract installed showcase archive");
    assert!(
        output.status.success(),
        "archive extraction failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_single_installed_executable(root: &Path, expected: &Path) {
    let entries = fs::read_dir(root)
        .expect("read extraction root")
        .map(|entry| entry.expect("read extraction entry").path())
        .collect::<Vec<_>>();
    assert_eq!(entries, [expected], "extraction must produce one entry");
    let metadata = fs::symlink_metadata(expected).expect("inspect installed executable");
    assert!(
        metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
        "installed executable must be a regular file, not a link"
    );
}

fn assert_not_repository_binary(installed_binary: &Path) {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .expect("canonicalize package source");
    let workspace = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("resolve workspace source");
    assert!(
        !installed_binary.starts_with(workspace.join("target")),
        "source-checkout Cargo binaries are forbidden"
    );
    assert!(
        !installed_binary.starts_with(workspace),
        "installed executable must not reside in the source checkout"
    );
}

fn assert_installed_version(binary: &Path, expected_version: &str) {
    let output = Command::new(binary)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .expect("execute installed binary version");
    assert!(output.status.success(), "installed binary version failed");
    assert_eq!(
        String::from_utf8(output.stdout)
            .expect("installed version output is UTF-8")
            .trim(),
        format!("heyfood {expected_version}")
    );
    assert!(output.stderr.is_empty());
}

async fn start_fixture_service() -> FixtureService {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind installed showcase fixture service");
    let base_url = format!(
        "http://{}",
        listener
            .local_addr()
            .expect("resolve showcase fixture address")
    );
    let verification_uri = format!("{base_url}/authorize?flow=device");
    let (request_sender, request_receiver) = mpsc::channel(256);
    let (shutdown_sender, mut shutdown_receiver) = oneshot::channel();
    let server = tokio::spawn(async move {
        let mut state = FixtureState::default();
        loop {
            let accepted = tokio::select! {
                biased;
                _ = &mut shutdown_receiver => break,
                accepted = listener.accept() => accepted,
            };
            let (mut socket, _) = accepted.expect("accept showcase request");
            let request = read_http_request(&mut socket).await;
            request_sender
                .send(RequestEvidence {
                    method: request.method.clone(),
                    path: request.path.clone(),
                })
                .await
                .expect("record showcase request");
            respond_to_showcase_request(&mut socket, request, &verification_uri, &mut state).await;
        }
        state.finish()
    });
    FixtureService {
        base_url,
        requests: request_receiver,
        shutdown: shutdown_sender,
        task: server,
    }
}

async fn read_http_request(socket: &mut TcpStream) -> HttpRequest {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 2048];
        let count = socket.read(&mut chunk).await.expect("read request bytes");
        assert_ne!(count, 0, "request ended before headers completed");
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break index + 4;
        }
        assert!(bytes.len() <= 64 * 1024, "request headers are too large");
    };
    let headers =
        String::from_utf8(bytes[..header_end].to_vec()).expect("request headers are UTF-8");
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().expect("valid content length"))
            })
        })
        .unwrap_or_default();
    assert!(content_length <= 64 * 1024, "request body is too large");
    while bytes.len() < header_end + content_length {
        let mut chunk = vec![0_u8; header_end + content_length - bytes.len()];
        let count = socket.read(&mut chunk).await.expect("read request body");
        assert_ne!(count, 0, "request ended before body completed");
        bytes.extend_from_slice(&chunk[..count]);
    }
    let request_line = headers.lines().next().expect("request line");
    let mut request_headers = BTreeMap::new();
    for line in headers.lines().skip(1).filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').expect("valid request header");
        let previous = request_headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
        assert!(previous.is_none(), "duplicate request header: {name}");
    }
    let mut fields = request_line.split_whitespace();
    let method = fields.next().expect("request method").to_owned();
    let path = fields
        .next()
        .expect("request path")
        .split('?')
        .next()
        .expect("request route")
        .to_owned();
    HttpRequest {
        method,
        path,
        headers: request_headers,
        body: bytes[header_end..header_end + content_length].to_vec(),
    }
}

async fn respond_to_showcase_request(
    socket: &mut TcpStream,
    request: HttpRequest,
    verification_uri: &str,
    state: &mut FixtureState,
) {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/v1/auth/capabilities") => {
            assert_header(&request, "x-app-client-id", "heyfood-cli");
            assert_header_absent(&request, "authorization");
            if let Some(device_id) = state.expected_device_id.as_deref() {
                assert_header(&request, "x-device-id", device_id);
                assert_header(&request, "x-api-key", "showcase-api-key");
            } else {
                assert_header_absent(&request, "x-device-id");
                assert_header_absent(&request, "x-api-key");
            }
            respond_json(
                socket,
                json!({
                    "schema_version": 1,
                    "self_registration": {
                        "status": "available",
                        "regions": ["US"],
                        "identity_methods": ["sms", "email"]
                    },
                    "authorization": {
                        "loopback_pkce": true,
                        "device_code": true,
                        "identity_methods": ["sms", "email"]
                    },
                    "profile_readiness": true,
                    "application_capabilities": {
                        "grocery": "v1",
                        "health": "v1",
                        "menu_watch": "v1"
                    }
                }),
            )
            .await;
        }
        ("POST", "/v1/channel/oauth/device/authorize") => {
            assert_no_protected_headers(&request);
            assert_header_absent(&request, "x-app-client-id");
            state.device_authorizations += 1;
            let body: Value =
                serde_json::from_slice(&request.body).expect("decode device authorization");
            assert_eq!(body["client_id"], "hf_cid_heyfood_cli");
            assert_eq!(body["intent"], "create_account");
            assert_eq!(body["scope"], FULL_SCOPE);
            respond_json(
                socket,
                json!({
                    "device_code": TEST_DEVICE_CODE,
                    "user_code": "SHOW-CASE",
                    "verification_uri": verification_uri,
                    "verification_uri_complete": null,
                    "expires_in": 600,
                    "interval": 1
                }),
            )
            .await;
        }
        ("POST", "/v1/channel/oauth/device/token") => {
            assert_no_protected_headers(&request);
            assert_header_absent(&request, "x-app-client-id");
            let body: Value =
                serde_json::from_slice(&request.body).expect("decode device token request");
            assert_eq!(body["client_id"], "hf_cid_heyfood_cli");
            assert_eq!(body["device_code"], TEST_DEVICE_CODE);
            respond_json(
                socket,
                json!({
                    "access_token": "hf_ct_showcase",
                    "token_type": "bearer",
                    "refresh_token": "hf_cr_showcase",
                    "expires_in": 3600,
                    "scope": FULL_SCOPE
                }),
            )
            .await;
        }
        ("POST", "/v1/channel/oauth/cli/session") => {
            state.cli_sessions += 1;
            let body: Value =
                serde_json::from_slice(&request.body).expect("decode CLI session request");
            let device_id = body["device_id"]
                .as_str()
                .expect("CLI session request device ID");
            assert_header(&request, "authorization", "Bearer hf_ct_showcase");
            assert_header(&request, "x-app-client-id", "heyfood-cli");
            assert_header(&request, "x-device-id", device_id);
            assert_header_absent(&request, "x-api-key");
            state.expected_device_id = Some(device_id.to_owned());
            respond_json(
                socket,
                json!({
                    "user_id": TEST_ACCOUNT,
                    "device_id": device_id,
                    "session_id": "showcase-session",
                    "access_token": "hf_at_showcase",
                    "refresh_token": "hf_rt_showcase",
                    "access_expires_at": "2999-01-01T00:00:00Z",
                    "refresh_expires_at": "2999-02-01T00:00:00Z",
                    "scopes": FULL_SCOPE.split_whitespace().collect::<Vec<_>>(),
                    "is_anonymous": false
                }),
            )
            .await;
        }
        ("GET", "/v1/channel/tools/profile/readiness") => {
            assert_header(&request, "authorization", "Bearer hf_ct_showcase");
            assert_header(&request, "x-app-client-id", "heyfood-cli");
            assert_header_absent(&request, "x-device-id");
            assert_header_absent(&request, "x-api-key");
            respond_json(
                socket,
                json!({
                    "schema_version": 1,
                    "status": "missing",
                    "member_id": "_self",
                    "has_profile_sync_consent": false,
                    "profile_version": null
                }),
            )
            .await;
        }
        ("GET", "/v1/profile/consent") => {
            assert_account_request_headers(&request, &state.expected_device_id);
            respond_json(
                socket,
                json!({
                    "schema_version": 1,
                    "has_consent": state.profile_consent,
                    "consent_version": state.profile_consent.then_some(1)
                }),
            )
            .await;
        }
        ("POST", "/v1/profile/consent") => {
            assert_account_request_headers(&request, &state.expected_device_id);
            let body: Value =
                serde_json::from_slice(&request.body).expect("decode profile consent mutation");
            assert_eq!(body, json!({"consent_version": 1}));
            state.profile_consent = true;
            state.consent_grants += 1;
            respond_json(
                socket,
                json!({
                    "schema_version": 1,
                    "has_consent": true,
                    "consent_version": 1
                }),
            )
            .await;
        }
        ("GET", "/v1/profile/sync") => {
            assert_account_request_headers(&request, &state.expected_device_id);
            if let Some(version) = state.profile_version {
                respond_json(
                    socket,
                    json!({
                        "schema_version": 1,
                        "member_id": "_self",
                        "version": version,
                        "profile_data": {
                            "preferences": [],
                            "restrictions": [],
                            "avoid_ingredients": [],
                            "activity_level": null,
                            "cuisine_preferences": []
                        }
                    }),
                )
                .await;
            } else {
                respond_status(socket, "404 Not Found", "application/json", b"{}").await;
            }
        }
        ("PUT", "/v1/profile/sync") => {
            assert_account_request_headers(&request, &state.expected_device_id);
            let body: Value = serde_json::from_slice(&request.body).expect("decode profile upload");
            assert_eq!(body["member_id"], "_self");
            assert!(body["profile_data"].is_object());
            assert!(body.get("expected_version").is_none());
            state.profile_version = Some(1);
            state.profile_uploads += 1;
            respond_json(
                socket,
                json!({
                    "schema_version": 1,
                    "member_id": "_self",
                    "version": 1
                }),
            )
            .await;
        }
        ("GET", "/v1/grocery/list") => {
            assert_account_request_headers(&request, &state.expected_device_id);
            respond_json(socket, grocery_list_document(state.list_version)).await;
        }
        ("GET", "/v1/grocery/exclusions") => {
            assert_account_request_headers(&request, &state.expected_device_id);
            respond_json(socket, json!({"exclusions": ["shellfish"]})).await;
        }
        ("POST", "/v1/agent/converse") => {
            assert_account_request_headers(&request, &state.expected_device_id);
            let body: Value = serde_json::from_slice(&request.body)
                .expect("decode installed conversational turn");
            respond_to_conversation(socket, &body, state).await;
        }
        _ => panic!(
            "unexpected installed showcase request: {} {}",
            request.method, request.path
        ),
    }
}

fn grocery_list_document(version: u64) -> Value {
    let accepted_item = (version >= 5).then(|| {
        json!({
            "id": "item-scallion",
            "requested_name": "scallion greens",
            "canonical_name": "scallion greens",
            "quantity": 1.0,
            "unit": null,
            "package_quantity": null,
            "note": "Edited before confirmation",
            "state": "active",
            "intended_for": "maya-uuid",
            "sources": [{
                "source_type": "manual",
                "source_ref": null,
                "source_detail": "Edited confirmation"
            }],
            "safety": null,
            "created_at": "2026-07-19T12:00:00+00:00",
            "updated_at": "2026-07-19T12:00:00+00:00"
        })
    });
    let mut items = vec![
        json!({
            "id": "item-lentils",
            "requested_name": "red lentils",
            "canonical_name": "red lentils",
            "quantity": 1.0,
            "unit": "cup",
            "package_quantity": null,
            "note": null,
            "state": "active",
            "intended_for": "maya-uuid",
            "sources": [{
                "source_type": "recipe",
                "source_ref": "list-dahl-001",
                "source_detail": "Red Lentil Dahl"
            }],
            "safety": {
                "basis": "ingredient",
                "status": "generally_safer",
                "member_flags": [{
                    "member_id": "maya-uuid",
                    "status": "generally_safer",
                    "reason": null,
                    "substitutions": []
                }],
                "model_version": "test-model",
                "rules_version": "dietary-rules-1",
                "confidence": 0.9,
                "context_hash": "abc123",
                "context_hash_version": 1,
                "label_hint": "Screened at ingredient level — verify the product label."
            },
            "created_at": "2026-07-19T12:00:00+00:00",
            "updated_at": "2026-07-19T12:00:00+00:00"
        }),
        json!({
            "id": "item-onion",
            "requested_name": "onion",
            "canonical_name": "onion",
            "quantity": 1.0,
            "unit": null,
            "package_quantity": null,
            "note": null,
            "state": "active",
            "intended_for": "maya-uuid",
            "sources": [{
                "source_type": "recipe",
                "source_ref": "list-dahl-001",
                "source_detail": "Red Lentil Dahl"
            }],
            "safety": {
                "basis": "ingredient",
                "status": "risky",
                "member_flags": [{
                    "member_id": "maya-uuid",
                    "status": "risky",
                    "reason": "Onion is high-FODMAP.",
                    "substitutions": ["green parts of scallion", "garlic-infused oil"]
                }],
                "model_version": "test-model",
                "rules_version": "dietary-rules-1",
                "confidence": 0.9,
                "context_hash": "abc123",
                "context_hash_version": 1,
                "label_hint": "Screened at ingredient level — verify the product label."
            },
            "created_at": "2026-07-19T12:00:00+00:00",
            "updated_at": "2026-07-19T12:00:00+00:00"
        }),
    ];
    if let Some(accepted_item) = accepted_item {
        items.push(accepted_item);
    }
    json!({
        "id": TEST_LIST_ID,
        "title": "Maya household groceries",
        "state": "active",
        "version": version,
        "items": items,
        "created_at": "2026-07-19T12:00:00+00:00",
        "updated_at": "2026-07-19T12:00:00+00:00"
    })
}

async fn respond_to_conversation(socket: &mut TcpStream, body: &Value, state: &mut FixtureState) {
    if let Some(prompt) = body.get("query").and_then(Value::as_str) {
        *state.prompt_counts.entry(prompt.to_owned()).or_default() += 1;
        match prompt {
            TEST_PROMPT => respond_sse_message(socket, TEST_RESPONSE).await,
            RETURNING_PROMPT => respond_sse_message(socket, RETURNING_RESPONSE).await,
            WIDTH_PROMPT => respond_sse_message(socket, WIDTH_RESPONSE).await,
            GROCERY_CANCEL_PROMPT => {
                assert_household_agent_context(body);
                respond_confirmation(
                    socket,
                    CANCEL_CONFIRMATION_ID,
                    CANCEL_IDEMPOTENCY_KEY,
                    "Cancel proposal for Maya",
                    state.list_version,
                )
                .await;
            }
            GROCERY_EDIT_PROMPT => {
                assert_household_agent_context(body);
                respond_confirmation(
                    socket,
                    EDIT_CONFIRMATION_ID,
                    EDIT_IDEMPOTENCY_KEY,
                    "Edit proposal for Maya",
                    state.list_version,
                )
                .await;
            }
            GROCERY_STALE_LIST_PROMPT => {
                assert_household_agent_context(body);
                respond_confirmation(
                    socket,
                    STALE_LIST_CONFIRMATION_ID,
                    STALE_LIST_IDEMPOTENCY_KEY,
                    "Stale list proposal for Maya",
                    state.list_version.saturating_sub(1),
                )
                .await;
            }
            GROCERY_STALE_CONTEXT_PROMPT => {
                assert_household_agent_context(body);
                respond_confirmation(
                    socket,
                    STALE_CONTEXT_CONFIRMATION_ID,
                    STALE_CONTEXT_IDEMPOTENCY_KEY,
                    "Stale context proposal for Maya",
                    state.list_version,
                )
                .await;
            }
            GROCERY_CTRL_C_PROMPT => {
                assert_household_agent_context(body);
                respond_confirmation(
                    socket,
                    CTRL_C_CONFIRMATION_ID,
                    CTRL_C_IDEMPOTENCY_KEY,
                    "Ctrl+C proposal for Maya",
                    state.list_version,
                )
                .await;
            }
            STREAM_CANCEL_PROMPT => {
                assert_household_agent_context(body);
                respond_cancellable_stream(socket).await;
                state.stream_cancellations += 1;
            }
            UNCERTAIN_PROMPT => {
                socket
                    .shutdown()
                    .await
                    .expect("close uncertain-dispatch fixture response");
            }
            FAILURE_PROMPT => {
                respond_sse_error(socket, "synthetic_failure", "synthetic installed failure").await;
            }
            _ => panic!("unexpected installed conversational prompt: {prompt}"),
        }
        return;
    }

    let confirmation = body
        .get("confirm")
        .and_then(Value::as_object)
        .expect("conversational request must carry query or confirmation");
    assert!(body.get("query").is_none());
    assert_household_agent_context(body);
    let confirmation_id = confirmation["confirmation_id"]
        .as_str()
        .expect("confirmation ID");
    let expected_idempotency_key = match confirmation_id {
        CANCEL_CONFIRMATION_ID => CANCEL_IDEMPOTENCY_KEY,
        EDIT_CONFIRMATION_ID => EDIT_IDEMPOTENCY_KEY,
        STALE_LIST_CONFIRMATION_ID => STALE_LIST_IDEMPOTENCY_KEY,
        STALE_CONTEXT_CONFIRMATION_ID => STALE_CONTEXT_IDEMPOTENCY_KEY,
        CTRL_C_CONFIRMATION_ID => CTRL_C_IDEMPOTENCY_KEY,
        _ => panic!("unexpected installed confirmation ID: {confirmation_id}"),
    };
    assert_eq!(
        confirmation["idempotency_key"].as_str(),
        Some(expected_idempotency_key),
        "confirmation must replay the exact server-minted idempotency authority"
    );
    let decision = confirmation["decision"]
        .as_str()
        .expect("confirmation decision");
    match (confirmation_id, decision) {
        (CANCEL_CONFIRMATION_ID, "cancel") => {
            assert!(confirmation.get("edits").is_none());
            state.proposal_cancellations += 1;
            respond_sse_message(socket, "Grocery proposal cancelled without mutation.").await;
        }
        (CTRL_C_CONFIRMATION_ID, "cancel") => {
            assert!(confirmation.get("edits").is_none());
            state.proposal_cancellations += 1;
            state.ctrl_c_proposal_cancellations += 1;
            respond_sse_message(
                socket,
                "Ctrl+C Grocery cancellation completed without mutation.",
            )
            .await;
        }
        (EDIT_CONFIRMATION_ID, "accept") => {
            assert_eq!(
                state.list_version, 4,
                "accept must start from list version 4"
            );
            let edited_name = confirmation
                .get("edits")
                .and_then(|value| value.get("items"))
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(|item| item.get("name"))
                .and_then(Value::as_str);
            assert_eq!(edited_name, Some("scallion greens"));
            state.list_version += 1;
            state.proposal_accepts += 1;
            respond_sse_message(socket, "Grocery list advanced exactly once to version 5.").await;
        }
        (STALE_LIST_CONFIRMATION_ID, "accept") => {
            state.stale_list_rejections += 1;
            respond_sse_error(
                socket,
                "list_version_conflict",
                "Stale Grocery list authority rejected; fetch the active list again.",
            )
            .await;
        }
        (STALE_CONTEXT_CONFIRMATION_ID, "accept") => {
            state.stale_context_rejections += 1;
            respond_sse_error(
                socket,
                "household_context_conflict",
                "Stale household context authority rejected; refresh the household snapshot.",
            )
            .await;
        }
        _ => panic!("unexpected installed confirmation: {confirmation_id} {decision}"),
    }
}

fn assert_household_agent_context(body: &Value) {
    assert_eq!(body["meal_context"]["active_member_id"], "maya-uuid");
    assert_eq!(body["meal_context"]["active_member_name"], "Maya");
    assert_eq!(body["meal_context"]["is_cook_mode"], false);
    assert!(
        body["dietary_context"]["restrictions"]
            .as_array()
            .is_some_and(|values| values.contains(&Value::String("low_fodmap".into()))),
        "Maya low-FODMAP context must reach the conversational request"
    );
    assert!(
        body["device_context"]["household"]["members"]
            .as_array()
            .is_some_and(|members| members.iter().any(|member| member["id"] == "maya-uuid")),
        "authoritative household members must reach the conversational request"
    );
}

async fn respond_confirmation(
    socket: &mut TcpStream,
    confirmation_id: &str,
    idempotency_key: &str,
    preview: &str,
    expected_version: u64,
) {
    respond_sse_document(
        socket,
        json!({
            "message": "I prepared a Grocery proposal.",
            "conversation_id": format!("conversation-{confirmation_id}"),
            "structured": {
                "type": "action_confirmation",
                "confirmation_id": confirmation_id,
                "idempotency_key": idempotency_key,
                "action": "grocery_list_add_items",
                "preview": preview,
                "expires_at": "2999-01-01T00:00:00Z",
                "card_form": "item_list",
                "structured_preview": {
                    "action": "add_items",
                    "list_id": TEST_LIST_ID,
                    "expected_version": expected_version,
                    "items": [{
                        "name": "onion",
                        "quantity": 1.0,
                        "unit": null,
                        "note": "For red lentil dahl",
                        "intended_for": "maya-uuid",
                        "provenance": "recipe:dahl-001",
                        "sources": [{
                            "source_type": "recipe",
                            "source_ref": "dahl-001",
                            "source_detail": "Red Lentil Dahl"
                        }],
                        "safety": {
                            "basis": "ingredient",
                            "status": "risky",
                            "member_flags": [{
                                "member_id": "maya-uuid",
                                "status": "risky",
                                "reason": "Onion is high-FODMAP.",
                                "substitutions": ["scallion greens"]
                            }],
                            "model_version": "test-model",
                            "rules_version": "dietary-rules-1",
                            "confidence": 0.9,
                            "context_hash": "f5c4ef0eec2f500b3ab4fe579bfae80d8c72d1199f0032a08f9a40273d4ca8b6",
                            "context_hash_version": 1,
                            "label_hint": "Screened at ingredient level — verify the product label."
                        }
                    }]
                }
            }
        }),
    )
    .await;
}

fn assert_header(request: &HttpRequest, name: &str, expected: &str) {
    assert_eq!(
        request.headers.get(name).map(String::as_str),
        Some(expected),
        "{} {} must carry exact {name}",
        request.method,
        request.path
    );
}

fn assert_header_absent(request: &HttpRequest, name: &str) {
    assert!(
        !request.headers.contains_key(name),
        "{} {} must not carry {name}",
        request.method,
        request.path
    );
}

fn assert_no_protected_headers(request: &HttpRequest) {
    for name in ["authorization", "x-device-id", "x-api-key"] {
        assert_header_absent(request, name);
    }
}

fn assert_account_request_headers(request: &HttpRequest, expected_device_id: &Option<String>) {
    assert_header(request, "authorization", "Bearer hf_at_showcase");
    assert_header(request, "x-app-client-id", "heyfood-cli");
    assert_header(
        request,
        "x-device-id",
        expected_device_id
            .as_deref()
            .expect("CLI exchange must bind the device before account requests"),
    );
    assert_header(request, "x-api-key", "showcase-api-key");
}

async fn respond_json(socket: &mut TcpStream, body: Value) {
    let body = serde_json::to_vec(&body).expect("encode fixture JSON");
    respond(socket, "application/json", &body).await;
}

async fn respond_sse_message(socket: &mut TcpStream, message: &str) {
    let partial = serde_json::to_string(&json!({"text": message})).expect("encode SSE partial");
    let result = serde_json::to_string(&json!({
        "message": message,
        "conversation_id": "showcase-conversation"
    }))
    .expect("encode SSE result");
    let body = format!("event: partial\ndata: {partial}\n\nevent: result\ndata: {result}\n\n");
    respond(socket, "text/event-stream", body.as_bytes()).await;
}

async fn respond_sse_document(socket: &mut TcpStream, document: Value) {
    let document = serde_json::to_string(&document).expect("encode SSE document");
    let body = format!("event: result\ndata: {document}\n\n");
    respond(socket, "text/event-stream", body.as_bytes()).await;
}

async fn respond_sse_error(socket: &mut TcpStream, code: &str, message: &str) {
    let error = serde_json::to_string(&json!({
        "code": code,
        "message": message,
        "retryable": false
    }))
    .expect("encode SSE error");
    let body = format!("event: error\ndata: {error}\n\n");
    respond(socket, "text/event-stream", body.as_bytes()).await;
}

async fn respond_cancellable_stream(socket: &mut TcpStream) {
    socket
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\nevent: partial\ndata: {\"text\":\"Streaming response in progress\"}\n\n",
        )
        .await
        .expect("write cancellable stream fixture");
    socket.flush().await.expect("flush cancellable stream");
    let observed_close = tokio::time::timeout(Duration::from_secs(10), async {
        let mut byte = [0_u8; 1];
        loop {
            match socket.read(&mut byte).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    })
    .await;
    assert!(
        observed_close.is_ok(),
        "installed client did not close the cancelled response stream"
    );
}

async fn respond(socket: &mut TcpStream, content_type: &str, body: &[u8]) {
    respond_status(socket, "200 OK", content_type, body).await;
}

async fn respond_status(socket: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) {
    socket
        .write_all(
            format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .expect("write fixture response headers");
    socket
        .write_all(body)
        .await
        .expect("write fixture response body");
    socket.shutdown().await.expect("close fixture response");
}

async fn run_installed_pty(
    installed_binary: &Path,
    user_root: &Path,
    base_url: &str,
    arguments: &[&str],
    columns: u16,
    no_color: bool,
    actions: Vec<PtyAction>,
) -> Vec<u8> {
    let installed_binary = installed_binary.to_owned();
    let user_root = user_root.to_owned();
    let base_url = base_url.to_owned();
    let arguments = arguments
        .iter()
        .map(|argument| (*argument).to_owned())
        .collect::<Vec<_>>();
    tokio::task::spawn_blocking(move || {
        run_installed_pty_blocking(
            &installed_binary,
            &user_root,
            &base_url,
            &arguments,
            columns,
            no_color,
            actions,
        )
    })
    .await
    .expect("join installed PTY driver")
}

fn run_installed_pty_blocking(
    installed_binary: &Path,
    user_root: &Path,
    base_url: &str,
    arguments: &[String],
    columns: u16,
    no_color: bool,
    actions: Vec<PtyAction>,
) -> Vec<u8> {
    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 30,
            cols: columns,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open installed showcase PTY");
    let mut command = CommandBuilder::new(installed_binary);
    command.args(arguments);
    for name in [
        "HEYFOOD_API_KEY",
        "HEYFOOD_API_URL",
        "HEYFOOD_CREDENTIAL_STORE",
        "HEYFOOD_STATE_DIR",
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "ALL_PROXY",
        "NO_COLOR",
    ] {
        command.env_remove(name);
    }
    command.env("HEYFOOD_API_URL", base_url);
    command.env("HEYFOOD_API_KEY", "showcase-api-key");
    command.env("HEYFOOD_STATE_DIR", user_root);
    #[cfg(not(windows))]
    command.env("HEYFOOD_CREDENTIAL_STORE", "file");
    #[cfg(windows)]
    command.env("HEYFOOD_CREDENTIAL_STORE", "native");
    command.env("HOME", user_root);
    command.env("XDG_CONFIG_HOME", user_root);
    command.env("XDG_DATA_HOME", user_root.join("data"));
    command.env("XDG_CACHE_HOME", user_root.join("cache"));
    command.env("USERPROFILE", user_root);
    command.env("APPDATA", user_root.join("appdata"));
    command.env("LOCALAPPDATA", user_root.join("local-appdata"));
    command.env("NO_PROXY", "127.0.0.1,localhost");
    command.env("TERM", "xterm-256color");
    if no_color {
        command.env("NO_COLOR", "1");
    }

    let mut child = pair
        .slave
        .spawn_command(command)
        .expect("spawn installed showcase executable");
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().expect("clone PTY reader");
    let writer = Arc::new(Mutex::new(
        pair.master.take_writer().expect("take PTY writer"),
    ));
    let capture = Arc::new(TerminalCapture::new(30, columns));
    let reader_capture = Arc::clone(&capture);
    let cursor_writer = Arc::clone(&writer);
    let reader_task = std::thread::spawn(move || {
        let mut cursor_query_replied = false;
        loop {
            let mut chunk = [0_u8; 4096];
            let count = reader.read(&mut chunk).expect("read installed PTY");
            if count == 0 {
                break;
            }
            reader_capture.append(&chunk[..count]);
            if !cursor_query_replied
                && reader_capture
                    .snapshot()
                    .windows(b"\x1b[6n".len())
                    .any(|window| window == b"\x1b[6n")
            {
                let mut writer = cursor_writer.lock().expect("lock cursor reply");
                writer
                    .write_all(b"\x1b[1;1R")
                    .expect("reply to terminal cursor query");
                writer.flush().expect("flush cursor reply");
                cursor_query_replied = true;
            }
        }
    });

    for action in actions {
        match action {
            PtyAction::Wait(needle) => capture
                .wait_for_semantic(&needle, Duration::from_secs(30))
                .unwrap_or_else(|message| terminate_and_panic(&mut *child, message)),
            PtyAction::Submit(value) => {
                let mut writer = writer.lock().expect("lock installed PTY input");
                writer
                    .write_all(format!("{value}\r").as_bytes())
                    .expect("submit installed PTY input");
                writer.flush().expect("flush installed PTY input");
            }
            PtyAction::CtrlC => {
                let mut writer = writer.lock().expect("lock installed PTY Ctrl+C");
                writer.write_all(&[3]).expect("send Ctrl+C");
                writer.flush().expect("flush Ctrl+C");
            }
            PtyAction::CtrlD => {
                let mut writer = writer.lock().expect("lock installed PTY Ctrl+D");
                writer.write_all(&[4]).expect("send Ctrl+D");
                writer.flush().expect("flush Ctrl+D");
            }
            PtyAction::Pause(duration) => std::thread::sleep(duration),
        }
    }
    let status = wait_for_child(&mut *child, Duration::from_secs(20));
    if status.success() {
        capture
            .wait_for_final_terminal_state(Duration::from_secs(2), Duration::from_millis(200))
            .unwrap_or_else(|message| panic!("{message}"));
    }
    drop(pair.master);
    let _ = reader_task.join();
    assert!(
        status.success(),
        "installed TUI exited unsuccessfully: {status:?}"
    );
    capture.snapshot()
}

fn terminate_and_panic(child: &mut dyn Child, message: String) -> ! {
    let _ = child.kill();
    let _ = child.wait();
    panic!("{message}")
}

fn wait_for_child(child: &mut dyn Child, timeout: Duration) -> portable_pty::ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("poll installed PTY child") {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let status = child.wait().expect("wait after installed PTY timeout");
            panic!("installed TUI did not exit within {timeout:?}: {status:?}");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

async fn collect_request_evidence(
    mut receiver: mpsc::Receiver<RequestEvidence>,
) -> Vec<RequestEvidence> {
    let mut requests = Vec::new();
    while let Some(request) = receiver.recv().await {
        requests.push(request);
    }
    requests
}

fn assert_fixture_summary(summary: &FixtureSummary) {
    assert_eq!(
        summary.device_authorizations, 1,
        "returning installed processes must not repeat registration"
    );
    assert_eq!(
        summary.cli_sessions, 1,
        "returning installed processes must reload the account-bound session"
    );
    assert_eq!(summary.consent_grants, 1);
    assert_eq!(summary.profile_uploads, 1);
    assert_eq!(summary.proposal_cancellations, 2);
    assert_eq!(summary.ctrl_c_proposal_cancellations, 1);
    assert_eq!(summary.proposal_accepts, 1);
    assert_eq!(summary.stale_list_rejections, 1);
    assert_eq!(summary.stale_context_rejections, 1);
    assert_eq!(summary.stream_cancellations, 1);
    assert_eq!(
        summary.list_version, 5,
        "one accepted proposal must advance the Grocery list exactly once"
    );
    for prompt in [
        TEST_PROMPT,
        RETURNING_PROMPT,
        GROCERY_CANCEL_PROMPT,
        GROCERY_EDIT_PROMPT,
        GROCERY_STALE_LIST_PROMPT,
        GROCERY_STALE_CONTEXT_PROMPT,
        GROCERY_CTRL_C_PROMPT,
        STREAM_CANCEL_PROMPT,
        UNCERTAIN_PROMPT,
        FAILURE_PROMPT,
        WIDTH_PROMPT,
    ] {
        assert_eq!(
            summary.prompt_counts.get(prompt),
            Some(&1),
            "installed prompt must be dispatched exactly once: {prompt}"
        );
    }
    assert_eq!(
        summary.prompt_counts.len(),
        11,
        "fixture must observe only the bounded core-matrix turns"
    );
}

fn assert_core_terminal_contract(
    clean_user: &[u8],
    returning_user: &[u8],
    width_40: &[u8],
    width_120_no_color: &[u8],
    interrupt_exit: &[u8],
) {
    for expected in [
        "Open this URL to continue:",
        "Approval code: SHOW-CASE",
        "Your hello.food account is connected.",
    ] {
        assert_raw_terminal_text(clean_user, expected);
    }
    assert_no_color_sgr(width_120_no_color);

    for terminal in [
        clean_user,
        returning_user,
        width_40,
        width_120_no_color,
        interrupt_exit,
    ] {
        assert_terminal_restored(terminal);
        assert_terminal_redacted(terminal);
    }
}

fn assert_raw_terminal_text(terminal: &[u8], expected: &str) {
    assert!(
        terminal
            .windows(expected.len())
            .any(|window| window == expected.as_bytes()),
        "installed terminal evidence omitted raw text {expected:?}"
    );
}

fn assert_terminal_restored(terminal: &[u8]) {
    assert!(
        terminal_final_state(terminal),
        "installed TUI must finish on the primary screen with paste disabled and cursor visible"
    );
}

fn terminal_final_state(terminal: &[u8]) -> bool {
    let after = |restored: &[u8], entered: &[u8]| {
        terminal
            .windows(restored.len())
            .rposition(|window| window == restored)
            .zip(
                terminal
                    .windows(entered.len())
                    .rposition(|window| window == entered),
            )
            .is_some_and(|(restored, entered)| restored > entered)
    };
    after(LEAVE_ALTERNATE_SCREEN, ENTER_ALTERNATE_SCREEN)
        && after(DISABLE_BRACKETED_PASTE, ENABLE_BRACKETED_PASTE)
        && after(SHOW_CURSOR, HIDE_CURSOR)
}

fn assert_terminal_redacted(terminal: &[u8]) {
    for forbidden in [
        TEST_DEVICE_CODE.as_bytes(),
        b"hf_ct_showcase".as_slice(),
        b"hf_cr_showcase".as_slice(),
        b"hf_at_showcase".as_slice(),
        b"hf_rt_showcase".as_slice(),
        b"showcase-api-key".as_slice(),
    ] {
        assert!(
            !terminal
                .windows(forbidden.len())
                .any(|window| window == forbidden),
            "terminal evidence contains a fixture credential"
        );
    }
}

fn assert_no_color_sgr(terminal: &[u8]) {
    let mut index = 0;
    while index + 2 < terminal.len() {
        if terminal[index] != 0x1b || terminal[index + 1] != b'[' {
            index += 1;
            continue;
        }
        let parameters_start = index + 2;
        let mut end = parameters_start;
        while end < terminal.len() && !(0x40..=0x7e).contains(&terminal[end]) {
            end += 1;
        }
        if end >= terminal.len() {
            break;
        }
        if terminal[end] == b'm' {
            let parameters = String::from_utf8_lossy(&terminal[parameters_start..end]);
            for value in parameters
                .split(';')
                .filter_map(|part| part.parse::<u16>().ok())
            {
                assert!(
                    !matches!(value, 30..=38 | 40..=48 | 90..=97 | 100..=107),
                    "NO_COLOR terminal emitted color SGR {value} in {parameters:?}"
                );
            }
        }
        index = end + 1;
    }
}

fn compact_terminal_text(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

struct VirtualScreen {
    cells: Vec<Vec<char>>,
    row: usize,
    column: usize,
    saved_row: usize,
    saved_column: usize,
}

impl VirtualScreen {
    fn new(rows: usize, columns: usize) -> Self {
        Self {
            cells: vec![vec![' '; columns]; rows],
            row: 0,
            column: 0,
            saved_row: 0,
            saved_column: 0,
        }
    }

    fn clear(&mut self) {
        for row in &mut self.cells {
            row.fill(' ');
        }
        self.row = 0;
        self.column = 0;
    }

    fn columns(&self) -> usize {
        self.cells.first().map_or(0, Vec::len)
    }

    fn scroll_up(&mut self, count: usize) {
        if self.cells.is_empty() {
            return;
        }
        let count = count.min(self.cells.len());
        self.cells.rotate_left(count);
        for row in self.cells.iter_mut().rev().take(count) {
            row.fill(' ');
        }
    }

    fn scroll_down(&mut self, count: usize) {
        if self.cells.is_empty() {
            return;
        }
        let count = count.min(self.cells.len());
        self.cells.rotate_right(count);
        for row in self.cells.iter_mut().take(count) {
            row.fill(' ');
        }
    }

    fn normalize_cursor(&mut self) {
        if self.cells.is_empty() {
            self.row = 0;
            self.column = 0;
            return;
        }
        if self.row >= self.cells.len() {
            let overflow = self.row - self.cells.len() + 1;
            self.scroll_up(overflow);
            self.row = self.cells.len() - 1;
        }
        self.column = self.column.min(self.columns().saturating_sub(1));
    }

    fn newline(&mut self) {
        self.row = self.row.saturating_add(1);
        self.normalize_cursor();
    }

    fn put(&mut self, character: char) {
        let columns = self.columns();
        if self.cells.is_empty() || columns == 0 {
            return;
        }
        if self.column >= columns {
            self.column = 0;
            self.newline();
        }
        let width = UnicodeWidthChar::width(character).unwrap_or(0);
        if width == 0 {
            return;
        }
        self.cells[self.row][self.column] = character;
        for offset in 1..width {
            if self.column + offset < columns {
                self.cells[self.row][self.column + offset] = ' ';
            }
        }
        self.column = self.column.saturating_add(width);
    }

    fn erase_display(&mut self, mode: usize) {
        match mode {
            0 => {
                self.erase_line(0);
                for row in self.cells.iter_mut().skip(self.row.saturating_add(1)) {
                    row.fill(' ');
                }
            }
            1 => {
                self.erase_line(1);
                for row in self.cells.iter_mut().take(self.row) {
                    row.fill(' ');
                }
            }
            2 | 3 => {
                for row in &mut self.cells {
                    row.fill(' ');
                }
            }
            _ => {}
        }
    }

    fn erase_line(&mut self, mode: usize) {
        let columns = self.columns();
        if self.cells.is_empty() || columns == 0 {
            return;
        }
        match mode {
            0 => self.cells[self.row][self.column.min(columns)..].fill(' '),
            1 => self.cells[self.row][..=self.column.min(columns - 1)].fill(' '),
            2 => self.cells[self.row].fill(' '),
            _ => {}
        }
    }

    fn apply_csi(&mut self, parameters: &[u8], final_byte: u8) {
        let parameters = parameters.strip_prefix(b"?").unwrap_or(parameters);
        let values = parameters
            .split(|byte| *byte == b';')
            .map(|value| {
                std::str::from_utf8(value)
                    .ok()
                    .and_then(|value| value.split(':').next())
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(0)
            })
            .collect::<Vec<_>>();
        let value = |index: usize, default: usize| {
            values
                .get(index)
                .copied()
                .filter(|value| *value != 0)
                .unwrap_or(default)
        };
        match final_byte {
            b'A' => self.row = self.row.saturating_sub(value(0, 1)),
            b'B' => {
                self.row = self
                    .row
                    .saturating_add(value(0, 1))
                    .min(self.cells.len().saturating_sub(1));
            }
            b'C' => {
                self.column = self
                    .column
                    .saturating_add(value(0, 1))
                    .min(self.columns().saturating_sub(1));
            }
            b'D' => self.column = self.column.saturating_sub(value(0, 1)),
            b'E' => {
                self.row = self
                    .row
                    .saturating_add(value(0, 1))
                    .min(self.cells.len().saturating_sub(1));
                self.column = 0;
            }
            b'F' => {
                self.row = self.row.saturating_sub(value(0, 1));
                self.column = 0;
            }
            b'G' | b'`' => self.column = value(0, 1).saturating_sub(1),
            b'H' | b'f' => {
                self.row = value(0, 1).saturating_sub(1);
                self.column = value(1, 1).saturating_sub(1);
                self.normalize_cursor();
            }
            b'J' => self.erase_display(values.first().copied().unwrap_or(0)),
            b'K' => self.erase_line(values.first().copied().unwrap_or(0)),
            b'P' => {
                let count = value(0, 1).min(self.columns().saturating_sub(self.column));
                if let Some(row) = self.cells.get_mut(self.row) {
                    row[self.column..].rotate_left(count);
                    row.iter_mut()
                        .rev()
                        .take(count)
                        .for_each(|cell| *cell = ' ');
                }
            }
            b'S' => self.scroll_up(value(0, 1)),
            b'T' => self.scroll_down(value(0, 1)),
            b'X' => {
                let end = self.column.saturating_add(value(0, 1)).min(self.columns());
                if let Some(row) = self.cells.get_mut(self.row) {
                    row[self.column..end].fill(' ');
                }
            }
            b'd' => {
                self.row = value(0, 1)
                    .saturating_sub(1)
                    .min(self.cells.len().saturating_sub(1));
            }
            b's' => {
                self.saved_row = self.row;
                self.saved_column = self.column;
            }
            b'u' => {
                self.row = self.saved_row.min(self.cells.len().saturating_sub(1));
                self.column = self.saved_column.min(self.columns().saturating_sub(1));
            }
            _ => {}
        }
    }

    fn text(&self) -> String {
        self.cells
            .iter()
            .map(|row| row.iter().collect::<String>().trim_end().to_owned())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn terminal_snapshot(value: &[u8], rows: usize, columns: usize) -> String {
    let mut primary = VirtualScreen::new(rows, columns);
    let mut alternate = VirtualScreen::new(rows, columns);
    let mut in_alternate_screen = false;
    let mut index = 0;
    while index < value.len() {
        let screen = if in_alternate_screen {
            &mut alternate
        } else {
            &mut primary
        };
        match value[index] {
            b'\r' => {
                screen.column = 0;
                index += 1;
            }
            b'\n' => {
                screen.newline();
                index += 1;
            }
            0x08 => {
                screen.column = screen.column.saturating_sub(1);
                index += 1;
            }
            b'\t' => {
                screen.column = ((screen.column / 8) + 1)
                    .saturating_mul(8)
                    .min(screen.columns().saturating_sub(1));
                index += 1;
            }
            0x1b => {
                index += 1;
                match value.get(index).copied() {
                    Some(b'[') => {
                        index += 1;
                        let parameters_start = index;
                        while index < value.len() && !(0x40..=0x7e).contains(&value[index]) {
                            index += 1;
                        }
                        if index >= value.len() {
                            break;
                        }
                        let final_byte = value[index];
                        let parameters = &value[parameters_start..index];
                        if matches!(final_byte, b'h' | b'l')
                            && parameters.strip_prefix(b"?") == Some(b"1049")
                        {
                            in_alternate_screen = final_byte == b'h';
                            if in_alternate_screen {
                                alternate.clear();
                            }
                        } else {
                            let screen = if in_alternate_screen {
                                &mut alternate
                            } else {
                                &mut primary
                            };
                            screen.apply_csi(parameters, final_byte);
                        }
                        index += 1;
                    }
                    Some(b']') => {
                        index += 1;
                        while index < value.len() {
                            if value[index] == 0x07 {
                                index += 1;
                                break;
                            }
                            if value[index] == 0x1b && value.get(index + 1) == Some(&b'\\') {
                                index += 2;
                                break;
                            }
                            index += 1;
                        }
                    }
                    Some(b'7') => {
                        screen.saved_row = screen.row;
                        screen.saved_column = screen.column;
                        index += 1;
                    }
                    Some(b'8') => {
                        screen.row = screen.saved_row.min(screen.cells.len().saturating_sub(1));
                        screen.column = screen.saved_column.min(screen.columns().saturating_sub(1));
                        index += 1;
                    }
                    Some(_) => index += 1,
                    None => {}
                }
            }
            byte if byte < 0x20 || byte == 0x7f => index += 1,
            _ => {
                let width = (1..=4)
                    .find(|width| {
                        index + width <= value.len()
                            && std::str::from_utf8(&value[index..index + width])
                                .ok()
                                .is_some_and(|text| text.chars().count() == 1)
                    })
                    .unwrap_or(1);
                if let Ok(text) = std::str::from_utf8(&value[index..index + width])
                    && let Some(character) = text.chars().next()
                {
                    screen.put(character);
                }
                index += width;
            }
        }
    }
    if in_alternate_screen {
        alternate.text()
    } else {
        primary.text()
    }
}

#[test]
fn terminal_snapshot_reconstructs_differential_updates() {
    let bytes = concat!(
        "\u{1b}[?1049h",
        "\u{1b}[2J",
        "\u{1b}[1;1Hhello",
        "\u{1b}[2;1Hworld",
        "\u{1b}[1;1Hj",
        "\u{1b}[2;3H\u{1b}[2X",
        "\u{1b}[2;3Hrl"
    );
    let snapshot = terminal_snapshot(bytes.as_bytes(), 3, 12);
    assert_eq!(snapshot, "jello\nworld\n");
}

#[test]
fn terminal_final_state_accepts_conpty_interleaving() {
    let restored = concat!(
        "\u{1b}[?1049h",
        "\u{1b}[?2004h",
        "\u{1b}[?25l",
        "\u{1b}[?25h",
        "\u{1b}[?2004l",
        "\u{1b}[?1049l",
        "\u{1b}[?25l",
        "conpty bookkeeping",
        "\u{1b}[?25h"
    );
    assert!(terminal_final_state(restored.as_bytes()));
    assert!(!terminal_final_state(
        format!("{restored}\u{1b}[?25l").as_bytes()
    ));
}

#[test]
fn installed_harness_inventory_matches_core_release_contract() {
    let contract: Value = serde_json::from_str(include_str!(
        "../../../tests/showcase/core-release-matrix.v1.json"
    ))
    .expect("decode installed core release contract");
    assert_eq!(contract["schema_version"], 1);
    assert_eq!(contract["release"], "0.5.0");
    let observed = contract["groups"]
        .as_array()
        .expect("core release groups")
        .iter()
        .map(|group| group["id"].as_str().expect("core group ID").to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        observed,
        CORE_MATRIX_GROUPS
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(
        contract["explicit_non_gates"],
        json!(["native_voice", "menu_watch_diff"])
    );
    assert_eq!(
        contract["canary_or_defer"],
        json!(["health", "menu_watch_management"])
    );
}
