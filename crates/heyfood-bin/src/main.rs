//! Native heyfood executable composition root.

#![forbid(unsafe_code)]

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, IsTerminal, Read, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use heyfood_agent_runtime::{
    CliAuthContext, DeviceAuthorization, HttpDeadlines, HttpService, ProvisionalReauthorization,
    ReauthorizationStageStatus, ReauthorizationStatus, RegistrationClient, RegistrationError,
    StagedReauthorization,
};
#[cfg(feature = "native-credentials")]
use heyfood_application::EnsureSessionError;
#[cfg(feature = "native-credentials")]
use heyfood_application::{
    BoxFuture, CredentialCommit, CredentialPort, Logout, LogoutLocalPort, LogoutOutcome, PortError,
};
use heyfood_application::{BrowserPort, EnsureSession, EnsureSessionOutcome};
use heyfood_cli::{Cli, Command, OutputMode, RegistrationResultDocument};
#[cfg(feature = "native-credentials")]
use heyfood_core::{AuthCredentialBundle, CommitId, SessionCredentials};
use heyfood_core::{
    BrowserUrl, NetworkPolicy, OperationId, ProfileStatus, SensitiveString, ServiceUrl,
    SessionSnapshot, terminal_safe_text,
};
#[cfg(feature = "native-credentials")]
use heyfood_mcp::{HeyfoodMcpServer, McpSessionContext, McpSessionProvider};
#[cfg(feature = "native-credentials")]
use heyfood_platform::AuthorizationSessionStore;
#[cfg(feature = "native-credentials")]
use heyfood_platform::CredentialBrokerStore;
#[cfg(all(not(windows), not(feature = "native-credentials")))]
use heyfood_platform::FileCredentialStore as NativeSessionStore;
#[cfg(all(not(windows), feature = "native-credentials"))]
use heyfood_platform::FileCredentialStore;
#[cfg(all(windows, not(feature = "native-credentials")))]
use heyfood_platform::WindowsCredentialStore as NativeSessionStore;
use heyfood_platform::{
    AuthorizationReplacementJournal, AuthorizationReplacementPhase, NativeAuthStore, NativeBrowser,
    NativeClock, NativePaths, PythonStateImporter,
};
use tokio_util::sync::CancellationToken;

#[cfg(feature = "native-credentials")]
enum NativeSessionStore {
    Platform(CredentialBrokerStore),
    #[cfg(not(windows))]
    OwnerOnlyFile(FileCredentialStore),
}

#[cfg(feature = "native-credentials")]
impl NativeSessionStore {
    #[cfg(not(windows))]
    fn open(root: impl AsRef<std::path::Path>) -> Result<Self, PortError> {
        let root = root.as_ref();
        match std::env::var("HEYFOOD_CREDENTIAL_STORE").as_deref() {
            Ok("file") => {
                eprintln!(
                    "heyfood: using explicitly requested owner-only file credential storage; unset HEYFOOD_CREDENTIAL_STORE to use the operating-system credential store"
                );
                FileCredentialStore::open(root).map(Self::OwnerOnlyFile)
            }
            Ok("native") => {
                CredentialBrokerStore::open(root, Duration::from_secs(15)).map(Self::Platform)
            }
            Err(std::env::VarError::NotPresent) => {
                let legacy = FileCredentialStore::open(root)?;
                let legacy_state_exists = match legacy.load_authorized_session() {
                    Ok(Some(_)) | Err(_) => true,
                    Ok(None) => legacy.reconciliation_required()?,
                };
                if legacy_state_exists {
                    eprintln!(
                        "heyfood: continuing with disclosed owner-only legacy credential storage; set HEYFOOD_CREDENTIAL_STORE=native after completing credential migration"
                    );
                    Ok(Self::OwnerOnlyFile(legacy))
                } else {
                    CredentialBrokerStore::open(root, Duration::from_secs(15)).map(Self::Platform)
                }
            }
            Ok(_) | Err(std::env::VarError::NotUnicode(_)) => Err(PortError::new(
                "credential_store_selection",
                "HEYFOOD_CREDENTIAL_STORE must be `native` or the explicit `file` fallback",
            )),
        }
    }

    #[cfg(windows)]
    fn open(root: impl AsRef<std::path::Path>) -> Result<Self, PortError> {
        match std::env::var("HEYFOOD_CREDENTIAL_STORE").as_deref() {
            Ok("native") | Err(std::env::VarError::NotPresent) => {
                CredentialBrokerStore::open(root.as_ref(), Duration::from_secs(15))
                    .map(Self::Platform)
            }
            Ok("file") => Err(PortError::new(
                "credential_store_selection",
                "owner-only file credential fallback is not supported on Windows",
            )),
            Ok(_) | Err(std::env::VarError::NotUnicode(_)) => Err(PortError::new(
                "credential_store_selection",
                "HEYFOOD_CREDENTIAL_STORE must be `native` on Windows",
            )),
        }
    }

    fn reconciliation_required(&self) -> Result<bool, PortError> {
        match self {
            Self::Platform(store) => store.reconciliation_required(),
            #[cfg(not(windows))]
            Self::OwnerOnlyFile(store) => store.reconciliation_required(),
        }
    }
}

#[cfg(feature = "native-credentials")]
impl CredentialPort for NativeSessionStore {
    fn load(&self) -> BoxFuture<'_, Result<Option<SessionCredentials>, PortError>> {
        match self {
            Self::Platform(store) => store.load(),
            #[cfg(not(windows))]
            Self::OwnerOnlyFile(store) => store.load(),
        }
    }

    fn commit(&self, commit: CredentialCommit) -> BoxFuture<'_, Result<(), PortError>> {
        match self {
            Self::Platform(store) => store.commit(commit),
            #[cfg(not(windows))]
            Self::OwnerOnlyFile(store) => store.commit(commit),
        }
    }

    fn mark_reconciliation_required(
        &self,
        commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        match self {
            Self::Platform(store) => store.mark_reconciliation_required(commit_id),
            #[cfg(not(windows))]
            Self::OwnerOnlyFile(store) => store.mark_reconciliation_required(commit_id),
        }
    }

    fn clear_reconciliation_required(
        &self,
        commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        match self {
            Self::Platform(store) => store.clear_reconciliation_required(commit_id),
            #[cfg(not(windows))]
            Self::OwnerOnlyFile(store) => store.clear_reconciliation_required(commit_id),
        }
    }
}

#[cfg(feature = "native-credentials")]
impl AuthorizationSessionStore for NativeSessionStore {
    fn initialize_authorized_session(
        &self,
        credentials: &SessionCredentials,
    ) -> Result<(), PortError> {
        match self {
            Self::Platform(store) => store.initialize_authorized_session(credentials),
            #[cfg(not(windows))]
            Self::OwnerOnlyFile(store) => store.initialize_authorized_session(credentials),
        }
    }

    fn load_authorized_session(&self) -> Result<Option<SessionCredentials>, PortError> {
        match self {
            Self::Platform(store) => store.load_authorized_session(),
            #[cfg(not(windows))]
            Self::OwnerOnlyFile(store) => store.load_authorized_session(),
        }
    }

    fn replace_authorized_session(
        &self,
        credentials: &SessionCredentials,
    ) -> Result<(), PortError> {
        match self {
            Self::Platform(store) => store.replace_authorized_session(credentials),
            #[cfg(not(windows))]
            Self::OwnerOnlyFile(store) => store.replace_authorized_session(credentials),
        }
    }

    fn stage_authorized_session(
        &self,
        client_transaction_id: &str,
        previous: &SessionCredentials,
        replacement: &SessionCredentials,
    ) -> Result<(), PortError> {
        match self {
            Self::Platform(store) => {
                store.stage_authorized_session(client_transaction_id, previous, replacement)
            }
            #[cfg(not(windows))]
            Self::OwnerOnlyFile(store) => {
                store.stage_authorized_session(client_transaction_id, previous, replacement)
            }
        }
    }

    fn verify_staged_authorized_session(
        &self,
        client_transaction_id: &str,
        previous: &SessionCredentials,
        replacement: &SessionCredentials,
    ) -> Result<(), PortError> {
        match self {
            Self::Platform(store) => {
                store.verify_staged_authorized_session(client_transaction_id, previous, replacement)
            }
            #[cfg(not(windows))]
            Self::OwnerOnlyFile(store) => {
                store.verify_staged_authorized_session(client_transaction_id, previous, replacement)
            }
        }
    }

    fn clear_staged_authorized_session(
        &self,
        client_transaction_id: &str,
        expected_replacement: &SessionCredentials,
    ) -> Result<(), PortError> {
        match self {
            Self::Platform(store) => {
                store.clear_staged_authorized_session(client_transaction_id, expected_replacement)
            }
            #[cfg(not(windows))]
            Self::OwnerOnlyFile(store) => {
                store.clear_staged_authorized_session(client_transaction_id, expected_replacement)
            }
        }
    }

    fn delete_authorized_session(&self) -> Result<(), PortError> {
        match self {
            Self::Platform(store) => store.delete_authorized_session(),
            #[cfg(not(windows))]
            Self::OwnerOnlyFile(store) => store.delete_authorized_session(),
        }
    }

    fn delete_authorized_session_for_logout(
        &self,
        expected: Option<&SessionCredentials>,
    ) -> Result<(), PortError> {
        match self {
            Self::Platform(store) => store.delete_authorized_session_for_logout(expected),
            #[cfg(not(windows))]
            Self::OwnerOnlyFile(store) => store.delete_authorized_session_for_logout(expected),
        }
    }

    fn delete_authorized_session_after_preflight_failure(
        &self,
        expected_account: &heyfood_core::AccountId,
    ) -> Result<(), PortError> {
        match self {
            Self::Platform(store) => {
                store.delete_authorized_session_after_preflight_failure(expected_account)
            }
            #[cfg(not(windows))]
            Self::OwnerOnlyFile(store) => {
                store.delete_authorized_session_after_preflight_failure(expected_account)
            }
        }
    }
}

#[cfg(feature = "native-credentials")]
struct NativeLogoutLocal<'a> {
    auth: &'a NativeAuthStore,
    session: &'a NativeSessionStore,
}

#[cfg(feature = "native-credentials")]
impl LogoutLocalPort for NativeLogoutLocal<'_> {
    fn clear<'a>(
        &'a self,
        expected: &'a heyfood_core::AuthCredentialBundle,
    ) -> BoxFuture<'a, Result<(), PortError>> {
        Box::pin(async move { self.auth.clear_account_bound(expected, self.session) })
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    #[cfg(feature = "native-credentials")]
    if let Some(outcome) = heyfood_platform::run_credential_broker_if_requested() {
        return outcome;
    }
    #[cfg(feature = "native-credentials")]
    if raw_arguments_request_mcp()
        && let Some(name) = inherited_heyfood_environment()
    {
        // This must precede every debug/test credential hook as well as normal
        // CLI composition so *all* MCP builds reject inherited overrides
        // before credential state can be touched.
        eprintln!(
            "heyfood: MCP rejected inherited {name}; launch it with an empty HEYFOOD_* environment"
        );
        return ExitCode::FAILURE;
    }
    #[cfg(all(debug_assertions, feature = "native-credentials"))]
    if std::env::var_os("HEYFOOD_TEST_DELETE_NATIVE_CREDENTIALS").as_deref()
        == Some(std::ffi::OsStr::new("1"))
    {
        let result = NativePaths::discover().and_then(|paths| {
            CredentialBrokerStore::open(paths.config_dir(), Duration::from_secs(15))
        });
        return match result {
            Ok(store) if store.delete().await.is_ok() => ExitCode::SUCCESS,
            Ok(_) | Err(_) => ExitCode::FAILURE,
        };
    }

    let cli = Cli::parse_env();
    let machine = cli.machine_output();
    let no_input = cli.no_input;
    let output_mode = cli.output_mode(io::stdout().is_terminal());
    #[cfg(feature = "native-credentials")]
    let mcp_modifier_used =
        cli.json || cli.raw || cli.no_color || cli.no_banner || cli.verbose || cli.no_input;
    if cli.raw {
        eprintln!("--raw is deprecated; use --json.");
    }
    match cli.command {
        Some(Command::Agent { command }) => heyfood_bin::agent_discovery::run(command, machine),
        #[cfg(feature = "native-credentials")]
        Some(Command::Mcp {
            command: heyfood_cli::McpCommand::Serve,
        }) => mcp_serve(mcp_modifier_used).await,
        #[cfg(not(feature = "native-credentials"))]
        Some(Command::Mcp { .. }) => {
            eprintln!("heyfood: MCP requires the native credential feature");
            ExitCode::FAILURE
        }
        Some(Command::Completion { shell }) => {
            if machine {
                return failure(
                    "completion_json_unsupported",
                    "Shell completion source cannot be emitted as JSON.",
                    Some("Run `heyfood completion <shell>` without --json."),
                    true,
                    false,
                );
            }
            heyfood_cli::write_completions(shell, &mut io::stdout());
            ExitCode::SUCCESS
        }
        Some(Command::Register(arguments)) => register(arguments, machine, no_input).await,
        Some(Command::Login(arguments)) => login(arguments, machine).await,
        #[cfg(feature = "native-credentials")]
        Some(Command::Logout(_)) => logout(machine).await,
        #[cfg(not(feature = "native-credentials"))]
        Some(Command::Logout(_)) => failure(
            "logout_unavailable",
            "Logout requires native credential support in this build.",
            None,
            machine,
            false,
        ),
        Some(Command::Chat(_)) => chat(machine).await,
        Some(Command::Onboard(_)) => onboard(machine).await,
        Some(Command::Health { .. }) => deferred_health_command(machine),
        Some(command) if is_native_one_shot(&command) => {
            one_shot(command, output_mode, machine, no_input).await
        }
        Some(_) => pending_command(machine),
        None => bare(machine).await,
    }
}

#[cfg(feature = "native-credentials")]
fn raw_arguments_request_mcp() -> bool {
    // Reject inherited HEYFOOD_* state before debug credential hooks for every
    // Clap-valid placement of global boolean flags. Once the first positional
    // command is `mcp`, it is an MCP launch attempt; subcommand validation can
    // happen later after the environment has been proven clean.
    std::env::args_os()
        .skip(1)
        .find(|argument| !argument.to_string_lossy().starts_with('-'))
        .is_some_and(|argument| argument == std::ffi::OsStr::new("mcp"))
}

#[cfg(feature = "native-credentials")]
struct NativeMcpSessions {
    state: tokio::sync::Mutex<Option<NativeMcpSessionState>>,
}

#[cfg(feature = "native-credentials")]
struct NativeMcpSessionState {
    ensure_session: Arc<EnsureSession>,
    snapshot: SessionSnapshot,
    authorization_scope: String,
}

#[cfg(feature = "native-credentials")]
impl McpSessionProvider for NativeMcpSessions {
    fn current(
        &self,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<McpSessionContext, PortError>> {
        Box::pin(async move {
            let mut guard = self.state.lock().await;
            if guard.is_none() {
                let prepared = prepare_mcp_session(cancellation.child_token())
                    .await
                    .map_err(mcp_prepare_error)?;
                *guard = Some(NativeMcpSessionState {
                    ensure_session: prepared.ensure_session,
                    snapshot: prepared.snapshot,
                    authorization_scope: prepared.authorization_scope,
                });
            }
            let state = guard.as_mut().expect("MCP session was initialized");
            let outcome = state
                .ensure_session
                .execute(state.snapshot.clone(), cancellation)
                .await
                .map_err(mcp_session_error)?;
            let credentials = match outcome {
                EnsureSessionOutcome::Current(credentials) => credentials,
                EnsureSessionOutcome::Refreshed(credentials) => {
                    state.snapshot.credentials = credentials.clone();
                    state.snapshot.reconciliation_required = false;
                    credentials
                }
                EnsureSessionOutcome::CancelledBeforeDispatch => {
                    return Err(PortError::new(
                        "mcp_session_cancelled_before_dispatch",
                        "Session validation was cancelled before dispatch",
                    ));
                }
            };
            Ok(McpSessionContext::new(
                credentials,
                state.authorization_scope.clone(),
            ))
        })
    }
}

#[cfg(feature = "native-credentials")]
fn mcp_prepare_error(error: heyfood_bin::OneShotError) -> PortError {
    if error.outcome_uncertain {
        PortError::uncertain(error.code, error.message)
    } else {
        PortError::new(error.code, error.message)
    }
}

#[cfg(feature = "native-credentials")]
fn mcp_session_error(error: EnsureSessionError) -> PortError {
    match error {
        EnsureSessionError::Service(error) => error,
        EnsureSessionError::ReconciliationRequired
        | EnsureSessionError::ServiceReconciliationRequired(_)
        | EnsureSessionError::CredentialReconciliationRequired(_)
        | EnsureSessionError::ReconciliationMarkerWrite { .. } => PortError::uncertain(
            "mcp_session_reconciliation_required",
            "Session refresh has an unresolved outcome; stop and run `heyfood login` before retrying",
        ),
    }
}

#[cfg(feature = "native-credentials")]
async fn mcp_serve(modifier_used: bool) -> ExitCode {
    if modifier_used {
        eprintln!(
            "heyfood: `mcp serve` rejects human/one-shot output and input modifiers because stdout is reserved for MCP"
        );
        return ExitCode::FAILURE;
    }
    if let Some(name) = inherited_heyfood_environment() {
        eprintln!(
            "heyfood: MCP rejected inherited {name}; launch it with an empty HEYFOOD_* environment"
        );
        return ExitCode::FAILURE;
    }

    let (service_url, policy) = match production_service_url() {
        Ok(value) => value,
        Err(error) => {
            eprintln!(
                "heyfood: MCP startup failed [{}]: {}",
                error.code,
                terminal_safe_text(&error.message)
            );
            return ExitCode::FAILURE;
        }
    };
    let service = match HttpService::new(service_url, policy, HttpDeadlines::default()) {
        Ok(service) => Arc::new(service),
        Err(error) => {
            eprintln!(
                "heyfood: MCP startup failed [{}]: {}",
                error.code,
                terminal_safe_text(&error.message)
            );
            return ExitCode::FAILURE;
        }
    };
    let sessions = Arc::new(NativeMcpSessions {
        state: tokio::sync::Mutex::new(None),
    });
    let server = HeyfoodMcpServer::new(service, sessions);
    match heyfood_mcp::serve_stdio(server).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("heyfood: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(feature = "native-credentials")]
fn inherited_heyfood_environment() -> Option<String> {
    std::env::vars_os().find_map(|(name, _)| {
        let name = name.to_string_lossy();
        name.starts_with("HEYFOOD_").then(|| name.into_owned())
    })
}

async fn chat(machine: bool) -> ExitCode {
    if machine {
        return failure(
            "interactive_json_unsupported",
            "The interactive terminal cannot emit the one-value JSON contract.",
            Some("Use `heyfood ask --json \"your question\"` for automation."),
            true,
            false,
        );
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return failure(
            "interactive_terminal_required",
            "The interactive terminal requires terminal input and output.",
            Some("Use `heyfood ask \"your question\"` in a redirected environment."),
            false,
            false,
        );
    }
    bare(false).await
}

async fn onboard(machine: bool) -> ExitCode {
    if machine {
        return failure(
            "onboarding_json_unsupported",
            "Guided dietary onboarding requires the interactive terminal.",
            Some("Run `heyfood` in a terminal and use `/onboard`."),
            true,
            false,
        );
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return failure(
            "interactive_terminal_required",
            "Guided dietary onboarding requires terminal input and output.",
            Some("Run `heyfood onboard` from an interactive terminal."),
            false,
            false,
        );
    }
    interactive(false, true).await
}

const fn is_native_one_shot(command: &Command) -> bool {
    matches!(
        command,
        Command::Ask(_)
            | Command::Reply(_)
            | Command::Log(_)
            | Command::Item(_)
            | Command::Grocery { .. }
            | Command::Watch { .. }
    )
}

async fn bare(machine: bool) -> ExitCode {
    if machine {
        println!(
            "{{\"ok\":true,\"message\":\"Run an explicit native command.\",\"next_command\":\"heyfood register\"}}"
        );
        return ExitCode::SUCCESS;
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        println!(
            "hello.food for your terminal.\n\nStart: heyfood register\nAsk:   heyfood ask \"What can I eat?\"\nHelp:  heyfood --help"
        );
        return ExitCode::SUCCESS;
    }

    interactive(false, false).await
}

async fn interactive(machine: bool, force_onboarding: bool) -> ExitCode {
    debug_assert!(!machine, "interactive entry rejects machine mode");
    let (prepared, mut startup_notice, mut startup_onboarding) = match prepare_bare_session().await
    {
        Ok(prepared) => prepared,
        Err(error) => {
            return failure(
                error.code,
                &terminal_safe_text(&error.message),
                one_shot_hint(error.code),
                false,
                error.outcome_uncertain,
            );
        }
    };
    if force_onboarding {
        startup_onboarding = true;
        startup_notice =
            Some("Review and replace your synced dietary profile through the guided setup.".into());
    }
    let local_state = match load_interactive_state(
        &prepared.paths,
        prepared.snapshot.credentials.account_id.as_str(),
    ) {
        Ok(state) => state,
        Err(error) => {
            return failure(
                error.code,
                &terminal_safe_text(&error.message),
                None,
                false,
                error.outcome_uncertain,
            );
        }
    };
    let result = tokio::task::block_in_place(move || {
        let driver = heyfood_bin::InteractiveTurnDriver::new_http(
            prepared.service,
            prepared.ensure_session,
            prepared.snapshot,
            prepared.authorization_scope,
        )?
        .with_local_state(local_state)
        .with_startup_notice(startup_notice)
        .with_startup_onboarding(startup_onboarding);
        #[cfg(all(
            feature = "native-audio",
            any(target_os = "linux", target_os = "macos", target_os = "windows")
        ))]
        let driver = driver.with_audio_capture(Arc::new(heyfood_voice::NativeAudioCapture));
        let mut driver = driver;
        heyfood_bin::run_qualified_session(&mut driver)
            .map_err(|error| io::Error::other(error.to_string()))
    });
    match result {
        Ok(reason) => ExitCode::from(u8::try_from(reason.exit_code()).unwrap_or(1)),
        Err(error) => failure(
            "interactive_session",
            &terminal_safe_text(&error.to_string()),
            Some("Run `heyfood ask \"your question\"` if this terminal cannot host the TUI."),
            false,
            false,
        ),
    }
}

async fn prepare_bare_session()
-> Result<(PreparedNativeSession, Option<String>, bool), heyfood_bin::OneShotError> {
    match prepare_native_session(None, CancellationToken::new()).await {
        Ok(mut prepared) => {
            let startup_onboarding = profile_needs_onboarding(&mut prepared)
                .await
                .unwrap_or(false);
            let startup_notice = startup_onboarding.then(|| {
                "Your connected account has no synced dietary profile. Complete the guided setup before your first personalized request.".into()
            });
            Ok((prepared, startup_notice, startup_onboarding))
        }
        Err(error) if error.code == "login_required" => {
            eprintln!(
                "Welcome to heyfood. Sign in or create a hello.food account in your browser to continue."
            );
            let registration = login_inner(
                heyfood_cli::LoginArgs {
                    device: true,
                    no_browser: false,
                    timeout: 600,
                },
                false,
                true,
            )
            .await
            .map_err(registration_to_one_shot)?;
            eprintln!("Your hello.food account is connected.");
            let prepared = prepare_native_session(None, CancellationToken::new()).await?;
            Ok((
                prepared,
                Some(registration_startup_notice(registration.profile_status)),
                registration.profile_status == ProfileStatus::Missing,
            ))
        }
        Err(error) => Err(error),
    }
}

async fn profile_needs_onboarding(prepared: &mut PreparedNativeSession) -> Option<bool> {
    if !["profile:read", "profile:write"].iter().all(|required| {
        prepared
            .authorization_scope
            .split_whitespace()
            .any(|scope| scope == *required)
    }) {
        return None;
    }
    let credentials = match prepared
        .ensure_session
        .execute(prepared.snapshot.clone(), CancellationToken::new())
        .await
        .ok()?
    {
        EnsureSessionOutcome::Current(credentials) => credentials,
        EnsureSessionOutcome::Refreshed(credentials) => {
            prepared.snapshot.credentials = credentials.clone();
            prepared.snapshot.reconciliation_required = false;
            credentials
        }
        EnsureSessionOutcome::CancelledBeforeDispatch => return None,
    };
    let consent = prepared
        .service
        .profile_consent_status(&credentials, OperationId::new(), CancellationToken::new())
        .await
        .ok()?;
    match consent
        .get("has_consent")
        .and_then(serde_json::Value::as_bool)
    {
        Some(false) => Some(true),
        Some(true) => match prepared
            .service
            .download_profile(
                &credentials,
                "_self",
                OperationId::new(),
                CancellationToken::new(),
            )
            .await
        {
            Ok(_) => Some(false),
            Err(error) if error.code == "resource_not_found" => Some(true),
            Err(_) => None,
        },
        None => None,
    }
}

fn registration_startup_notice(status: ProfileStatus) -> String {
    match status {
        ProfileStatus::Ready => {
            "Account connected. Your dietary profile is ready; ask your first question.".into()
        }
        ProfileStatus::Missing => "Account connected. Complete the guided dietary profile before your first personalized request.".into(),
        ProfileStatus::Unknown => "Account connected. Dietary profile readiness could not be confirmed; personalized guidance may be limited.".into(),
    }
}

fn load_interactive_state(
    paths: &NativePaths,
    account_id: &str,
) -> Result<Option<heyfood_core::ImportedPythonState>, heyfood_bin::OneShotError> {
    let importer = PythonStateImporter::discover(paths).map_err(heyfood_bin::OneShotError::from)?;
    importer.import().map_err(heyfood_bin::OneShotError::from)?;
    let state = importer
        .load_state()
        .map_err(heyfood_bin::OneShotError::from)?;
    if state
        .as_ref()
        .is_some_and(|state| state.account_user_id.as_deref() != Some(account_id))
    {
        return Err(heyfood_bin::OneShotError::new(
            "local_state_account_mismatch",
            "Saved local context belongs to a different account.",
        ));
    }
    Ok(state)
}

fn pending_command(machine: bool) -> ExitCode {
    failure(
        "command_not_available",
        "This command has not been implemented in the native Rust client yet.",
        Some("Run `heyfood --help` to see the active native commands."),
        machine,
        false,
    )
}

fn deferred_health_command(machine: bool) -> ExitCode {
    failure(
        "capability_deferred",
        "Health integrations are deferred from the supported heyfood v0.6.3 contract.",
        Some("Use `heyfood --help` to see the supported v0.6.3 commands."),
        machine,
        false,
    )
}

async fn one_shot(
    command: Command,
    output_mode: OutputMode,
    machine: bool,
    no_input: bool,
) -> ExitCode {
    // Install the signal handler before any consuming auth request. Once a
    // rotating refresh is dispatched, Ctrl-C is recorded but the bounded
    // request and durable reconciliation are allowed to finish.
    let cancellation = CancellationToken::new();
    let signal_cancellation = cancellation.clone();
    let signal = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal_cancellation.cancel();
        }
    });
    let result = one_shot_inner(command, output_mode, cancellation, no_input).await;
    signal.abort();
    match result {
        Ok(output) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => failure(
            error.code,
            &terminal_safe_text(&error.message),
            one_shot_hint(error.code),
            machine,
            error.outcome_uncertain,
        ),
    }
}

struct PreparedNativeSession {
    paths: NativePaths,
    service: Arc<HttpService>,
    ensure_session: Arc<EnsureSession>,
    snapshot: SessionSnapshot,
    authorization_scope: String,
}

#[cfg(feature = "native-credentials")]
async fn prepare_mcp_session(
    cancellation: CancellationToken,
) -> Result<PreparedNativeSession, heyfood_bin::OneShotError> {
    let paths =
        NativePaths::discover_platform_default().map_err(heyfood_bin::OneShotError::from)?;
    let auth_store =
        NativeAuthStore::open(paths.config_dir()).map_err(heyfood_bin::OneShotError::from)?;
    let credential_store = Arc::new(
        CredentialBrokerStore::open(paths.config_dir(), Duration::from_secs(15))
            .map_err(heyfood_bin::OneShotError::from)?,
    );
    auth_store
        .finish_authorization_terminal_cleanup()
        .map_err(heyfood_bin::OneShotError::from)?;
    if auth_store
        .pending_authorization_replacement()
        .map_err(heyfood_bin::OneShotError::from)?
        .is_some()
    {
        return Err(heyfood_bin::OneShotError::new(
            "reauthorization_reconciliation_required",
            "A staged login must be reconciled before MCP can start. Run `heyfood login`.",
        ));
    }
    let mut auth = auth_store
        .load_account_bound(credential_store.as_ref())
        .map_err(heyfood_bin::OneShotError::from)?
        .ok_or_else(|| {
            heyfood_bin::OneShotError::new(
                "login_required",
                "No native hello.food account is connected. Run `heyfood login` first.",
            )
        })?;

    let (service_url, policy) = production_service_url()?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_secs());
    if auth.channel.expires_at_unix() <= i64::try_from(now).unwrap_or(i64::MAX) {
        let refresh = auth_store
            .begin_refresh()
            .map_err(heyfood_bin::OneShotError::from)?;
        auth = refresh
            .load()
            .map_err(heyfood_bin::OneShotError::from)?
            .ok_or_else(|| {
                heyfood_bin::OneShotError::new(
                    "login_required",
                    "No native hello.food account is connected. Run `heyfood login` first.",
                )
            })?;
        if auth.channel.expires_at_unix() > i64::try_from(now).unwrap_or(i64::MAX) {
            drop(refresh);
        } else {
            if cancellation.is_cancelled() {
                return Err(heyfood_bin::OneShotError::new(
                    "channel_refresh_cancelled_before_dispatch",
                    "Channel refresh was cancelled before dispatch.",
                ));
            }
            let client = RegistrationClient::new(service_url.clone(), policy)
                .map_err(registration_to_one_shot)?;
            refresh.mark_reconciliation_required().map_err(|_| {
                uncertain_one_shot(
                    "channel_refresh_reconciliation_write",
                    "Channel refresh was not dispatched because its reconciliation marker could not be saved.",
                )
            })?;
            auth.channel = match client.refresh_channel(&auth.channel).await {
                Ok(channel) => channel,
                Err(error) if error.outcome_uncertain => {
                    return Err(registration_to_one_shot(error));
                }
                Err(error) => {
                    refresh.clear_reconciliation_required().map_err(|_| {
                        uncertain_one_shot(
                            "channel_refresh_reconciliation_clear",
                            "The channel refresh was rejected, but its reconciliation marker could not be cleared.",
                        )
                    })?;
                    return Err(registration_to_one_shot(error));
                }
            };
            if refresh.replace(&auth).is_err() {
                return Err(uncertain_one_shot(
                    "channel_refresh_persistence_outcome_uncertain",
                    "The channel credential rotated, but it could not be saved. Stop and contact hello.food support; do not retry.",
                ));
            }
        }
    }

    auth = auth_store
        .load_account_bound(credential_store.as_ref())
        .map_err(heyfood_bin::OneShotError::from)?
        .ok_or_else(|| {
            heyfood_bin::OneShotError::new(
                "login_required",
                "No native hello.food account is connected. Run `heyfood login` first.",
            )
        })?;
    let credentials = auth.session.clone();
    let authorization_scope = auth.channel.scope.clone();
    let reconciliation_required = credential_store
        .reconciliation_required()
        .map_err(heyfood_bin::OneShotError::from)?;
    let cli_auth = CliAuthContext::new(
        auth.channel.device_id.clone(),
        auth.channel.access_token.clone(),
        None,
    )
    .map_err(heyfood_bin::OneShotError::from)?;
    let service = Arc::new(
        HttpService::new(service_url, policy, HttpDeadlines::default())
            .map_err(heyfood_bin::OneShotError::from)?
            .with_cli_auth(cli_auth),
    );
    let ensure_session = Arc::new(EnsureSession::new(
        service.clone(),
        credential_store,
        Arc::new(NativeClock),
    ));
    Ok(PreparedNativeSession {
        paths,
        service,
        ensure_session,
        snapshot: SessionSnapshot {
            credentials,
            reconciliation_required,
        },
        authorization_scope,
    })
}

#[cfg(feature = "native-credentials")]
fn production_service_url() -> Result<(ServiceUrl, NetworkPolicy), heyfood_bin::OneShotError> {
    let policy = NetworkPolicy::HTTPS_ONLY;
    let url = ServiceUrl::parse("https://api.hello.food", policy).map_err(|_| {
        heyfood_bin::OneShotError::new(
            "mcp_service_origin",
            "The compiled production service origin is invalid",
        )
    })?;
    Ok((url, policy))
}

async fn prepare_native_session(
    command: Option<&Command>,
    cancellation: CancellationToken,
) -> Result<PreparedNativeSession, heyfood_bin::OneShotError> {
    let paths = NativePaths::discover().map_err(heyfood_bin::OneShotError::from)?;
    let auth_store =
        NativeAuthStore::open(paths.config_dir()).map_err(heyfood_bin::OneShotError::from)?;
    let credential_store = Arc::new(
        NativeSessionStore::open(paths.config_dir()).map_err(heyfood_bin::OneShotError::from)?,
    );
    auth_store
        .finish_authorization_terminal_cleanup()
        .map_err(heyfood_bin::OneShotError::from)?;
    if auth_store
        .pending_authorization_replacement()
        .map_err(heyfood_bin::OneShotError::from)?
        .is_some()
    {
        return Err(heyfood_bin::OneShotError::new(
            "reauthorization_reconciliation_required",
            "A staged login must be reconciled before continuing. Run `heyfood login`.",
        ));
    }
    let mut auth = auth_store
        .load_account_bound(credential_store.as_ref())
        .map_err(heyfood_bin::OneShotError::from)?
        .ok_or_else(|| {
            heyfood_bin::OneShotError::new(
                "login_required",
                "No hello.food account is connected. Run `heyfood login` first.",
            )
        })?;
    if let Some(command) = command {
        ensure_command_scopes(command, &auth.channel.scope)?;
    }

    let (service_url, policy) = service_url().map_err(registration_to_one_shot)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_secs());
    if auth.channel.expires_at_unix() <= i64::try_from(now).unwrap_or(i64::MAX) {
        let refresh = auth_store
            .begin_refresh()
            .map_err(heyfood_bin::OneShotError::from)?;
        auth = refresh
            .load()
            .map_err(heyfood_bin::OneShotError::from)?
            .ok_or_else(|| {
                heyfood_bin::OneShotError::new(
                    "login_required",
                    "No hello.food account is connected. Run `heyfood login` first.",
                )
            })?;
        if auth.channel.expires_at_unix() > i64::try_from(now).unwrap_or(i64::MAX) {
            drop(refresh);
        } else {
            let client = RegistrationClient::new(service_url.clone(), policy)
                .map_err(registration_to_one_shot)?;
            if cancellation.is_cancelled() {
                return Err(heyfood_bin::OneShotError::new(
                    "channel_refresh_cancelled_before_dispatch",
                    "Channel refresh was cancelled before dispatch.",
                ));
            }
            refresh.mark_reconciliation_required().map_err(|_| {
                uncertain_one_shot(
                    "channel_refresh_reconciliation_write",
                    "Channel refresh was not dispatched because its reconciliation marker could not be saved.",
                )
            })?;
            auth.channel = match client.refresh_channel(&auth.channel).await {
                Ok(channel) => channel,
                Err(error) if error.outcome_uncertain => {
                    return Err(registration_to_one_shot(error));
                }
                Err(error) => {
                    refresh.clear_reconciliation_required().map_err(|_| {
                        uncertain_one_shot(
                            "channel_refresh_reconciliation_clear",
                            "The channel refresh was rejected, but its reconciliation marker could not be cleared.",
                        )
                    })?;
                    return Err(registration_to_one_shot(error));
                }
            };
            if refresh.replace(&auth).is_err() {
                return Err(uncertain_one_shot(
                    "channel_refresh_persistence_outcome_uncertain",
                    "The channel credential rotated, but it could not be saved. Stop and contact hello.food support for manual credential recovery; do not retry.",
                ));
            }
        }
    }

    auth = auth_store
        .load_account_bound(credential_store.as_ref())
        .map_err(heyfood_bin::OneShotError::from)?
        .ok_or_else(|| {
            heyfood_bin::OneShotError::new(
                "login_required",
                "No hello.food account is connected. Run `heyfood login` first.",
            )
        })?;
    let credentials = auth.session.clone();
    let authorization_scope = auth.channel.scope.clone();
    let reconciliation_required = credential_store
        .reconciliation_required()
        .map_err(heyfood_bin::OneShotError::from)?;
    let api_key = std::env::var("HEYFOOD_API_KEY")
        .ok()
        .filter(|value| !value.is_empty())
        .map(SensitiveString::new);
    let cli_auth = CliAuthContext::new(
        auth.channel.device_id.clone(),
        auth.channel.access_token.clone(),
        api_key,
    )
    .map_err(heyfood_bin::OneShotError::from)?;
    let service = Arc::new(
        HttpService::new(service_url, policy, HttpDeadlines::default())
            .map_err(heyfood_bin::OneShotError::from)?
            .with_cli_auth(cli_auth),
    );
    let ensure_session = Arc::new(EnsureSession::new(
        service.clone(),
        credential_store,
        Arc::new(NativeClock),
    ));
    Ok(PreparedNativeSession {
        paths,
        service,
        ensure_session,
        snapshot: SessionSnapshot {
            credentials,
            reconciliation_required,
        },
        authorization_scope,
    })
}

async fn one_shot_inner(
    command: Command,
    output_mode: OutputMode,
    cancellation: CancellationToken,
    no_input: bool,
) -> Result<String, heyfood_bin::OneShotError> {
    let stdin = read_command_stdin(&command)?;
    authorize_human_only_command(&command, &stdin, no_input)?;
    let prepared = prepare_native_session(Some(&command), cancellation.child_token()).await?;
    let imported_state = load_selector_state(
        &prepared.paths,
        &command,
        prepared.snapshot.credentials.account_id.as_str(),
    )?;
    heyfood_bin::execute_qualified_one_shot_with_state(
        prepared.service.as_ref(),
        prepared.ensure_session.as_ref(),
        prepared.snapshot,
        output_mode,
        command,
        &stdin,
        cancellation,
        imported_state.as_ref(),
    )
    .await
}

fn authorize_human_only_command(
    command: &Command,
    stdin: &[u8],
    no_input: bool,
) -> Result<(), heyfood_bin::OneShotError> {
    if !requires_human_terminal_authority(command) {
        return Ok(());
    }
    if no_input {
        return Err(heyfood_bin::OneShotError::new(
            "human_input_disabled",
            "This human-only command cannot run with --no-input.",
        ));
    }

    #[cfg(debug_assertions)]
    if std::env::var_os("HEYFOOD_TEST_FORCE_NO_CONTROLLING_TERMINAL").as_deref()
        == Some(std::ffi::OsStr::new("1"))
    {
        return Err(heyfood_bin::OneShotError::new(
            "human_terminal_required",
            "This human-only command requires a separate attached controlling terminal.",
        ));
    }

    let (input, mut output) = open_controlling_terminal().map_err(|_| {
        heyfood_bin::OneShotError::new(
            "human_terminal_required",
            "This human-only command requires a separate attached controlling terminal.",
        )
    })?;
    let mut input = BufReader::new(input);
    review_human_only_command(command, stdin, &mut input, &mut output)
}

const fn requires_human_terminal_authority(command: &Command) -> bool {
    matches!(
        command,
        Command::Log(_)
            | Command::Grocery {
                command: Some(
                    heyfood_cli::GroceryCommand::Add(_)
                        | heyfood_cli::GroceryCommand::Remove(_)
                        | heyfood_cli::GroceryCommand::State(_)
                        | heyfood_cli::GroceryCommand::Never(_)
                        | heyfood_cli::GroceryCommand::Confirm(_)
                )
            }
            | Command::Watch {
                command: Some(
                    heyfood_cli::MenuWatchCommand::Add(_)
                        | heyfood_cli::MenuWatchCommand::Remove(_)
                )
            }
    )
}

fn review_human_only_command(
    command: &Command,
    stdin: &[u8],
    input: &mut dyn BufRead,
    output: &mut dyn Write,
) -> Result<(), heyfood_bin::OneShotError> {
    let (review, expected) = human_review_document(command, stdin)?;
    writeln!(
        output,
        "\nheyfood human-only authorization\n\n{review}\n\nType {expected} to continue:"
    )
    .and_then(|_| output.flush())
    .map_err(|_| {
        heyfood_bin::OneShotError::new(
            "human_terminal_write",
            "Could not write the human-only review to the controlling terminal.",
        )
    })?;

    let mut decision = String::new();
    input.read_line(&mut decision).map_err(|_| {
        heyfood_bin::OneShotError::new(
            "human_terminal_read",
            "Could not read the human decision from the controlling terminal.",
        )
    })?;
    if decision.trim() != expected {
        return Err(heyfood_bin::OneShotError::new(
            "human_confirmation_declined",
            "The human-only command was not authorized; no network request was dispatched.",
        ));
    }
    Ok(())
}

fn human_review_document(
    command: &Command,
    stdin: &[u8],
) -> Result<(String, &'static str), heyfood_bin::OneShotError> {
    match command {
        Command::Log(arguments) => {
            let meal = bounded_human_data(
                if arguments.meal.is_empty() {
                    stdin_text(stdin, "meal")?
                } else {
                    arguments.meal_text()
                },
                500,
                "meal",
            )?;
            let meal_type = arguments
                .meal_type
                .map(|value| value.as_str())
                .unwrap_or("unspecified");
            Ok((
                format!(
                    "Mutation: log meal memory\nMeal: {}\nMeal type: {}\nHousehold target selector: {}",
                    inline_human_text(&meal),
                    inline_human_text(meal_type),
                    arguments
                        .checking_for
                        .as_deref()
                        .map(inline_human_text)
                        .unwrap_or_else(|| "self".into())
                ),
                "LOG",
            ))
        }
        Command::Grocery {
            command: Some(heyfood_cli::GroceryCommand::Add(arguments)),
        } => Ok((
            format!(
                "Preparation: create an add-items proposal containing commit authority\nList: {}\nExpected version: {}\nItems:\n{}\nIntended for: {}",
                inline_human_text(&arguments.list.list_id),
                arguments.list.version,
                review_lines(&arguments.items),
                arguments
                    .intended_for
                    .as_deref()
                    .map(inline_human_text)
                    .unwrap_or_else(|| "unspecified".into())
            ),
            "PREPARE",
        )),
        Command::Grocery {
            command: Some(heyfood_cli::GroceryCommand::Remove(arguments)),
        } => Ok((
            format!(
                "Preparation: create a remove-items proposal containing commit authority\nList: {}\nExpected version: {}\nItems:\n{}",
                inline_human_text(&arguments.list.list_id),
                arguments.list.version,
                review_lines(&arguments.items)
            ),
            "PREPARE",
        )),
        Command::Grocery {
            command: Some(heyfood_cli::GroceryCommand::State(arguments)),
        } => Ok((
            format!(
                "Preparation: create an item-state proposal containing commit authority\nList: {}\nExpected version: {}\nItem: {}\nNew state: {:?}",
                inline_human_text(&arguments.list.list_id),
                arguments.list.version,
                inline_human_text(&arguments.item),
                arguments.state
            ),
            "PREPARE",
        )),
        Command::Grocery {
            command: Some(heyfood_cli::GroceryCommand::Never(arguments)),
        } => Ok((
            format!(
                "Preparation: create a never-buy proposal containing commit authority\nList: {}\nExpected version: {}\nItem: {}\nAction: {} exclusion",
                inline_human_text(&arguments.list.list_id),
                arguments.list.version,
                inline_human_text(&arguments.item),
                if arguments.remove { "remove" } else { "add" }
            ),
            "PREPARE",
        )),
        Command::Grocery {
            command: Some(heyfood_cli::GroceryCommand::Confirm(arguments)),
        } => {
            if !arguments.proposal_stdin {
                return Err(heyfood_bin::OneShotError::new(
                    "confirmation_input",
                    "confirmation proposals must be read from stdin",
                ));
            }
            if stdin.is_empty() || stdin.len() > heyfood_bin::MAX_CONFIRMATION_STDIN_BYTES {
                return Err(heyfood_bin::OneShotError::new(
                    "confirmation_input",
                    "confirmation proposal stdin must contain at most 1 MiB",
                ));
            }
            let proposal: heyfood_core::GroceryMutationProposalWire = serde_json::from_slice(stdin)
                .map_err(|_| {
                    heyfood_bin::OneShotError::new(
                        "confirmation_input",
                        "confirmation proposal stdin is invalid JSON",
                    )
                })?;
            let expected = match arguments.decision {
                heyfood_cli::GroceryDecisionArgument::Accept => "ACCEPT",
                heyfood_cli::GroceryDecisionArgument::Cancel => "CANCEL",
            };
            let preview =
                serde_json::to_string_pretty(&proposal.structured_preview).map_err(|_| {
                    heyfood_bin::OneShotError::new(
                        "confirmation_input",
                        "confirmation proposal preview could not be rendered",
                    )
                })?;
            let preconditions =
                serde_json::to_string_pretty(&proposal.preconditions).map_err(|_| {
                    heyfood_bin::OneShotError::new(
                        "confirmation_input",
                        "confirmation proposal preconditions could not be rendered",
                    )
                })?;
            let operation = serde_json::to_value(proposal.operation)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "grocery_mutation".into());
            Ok((
                format!(
                    "Exact human-visible proposal intent:\nConfirmation ID: {}\nOperation: {}\nExpires: {}\nStructured preview:\n{}\nFrozen preconditions:\n{}\nRequested decision: {expected}",
                    proposal.confirmation_id.as_uuid().hyphenated(),
                    inline_human_text(&operation),
                    inline_human_text(&proposal.expires_at),
                    terminal_safe_text(&preview),
                    terminal_safe_text(&preconditions)
                ),
                expected,
            ))
        }
        Command::Watch {
            command: Some(heyfood_cli::MenuWatchCommand::Add(arguments)),
        } => Ok((
            format!(
                "Mutation: create a recurring Menu Watch subscription\nRestaurant: {}\nSchedule: {:?} {:02}:00\nTimezone: {}\nNotifications: {}\nMenu source: {}\nConfirm supplied menu identity: {}",
                inline_human_text(&arguments.restaurant_id),
                arguments.weekday,
                arguments.hour,
                arguments
                    .tz
                    .as_deref()
                    .map(inline_human_text)
                    .unwrap_or_else(|| "restaurant default".into()),
                if arguments.notify {
                    "enabled"
                } else {
                    "disabled"
                },
                arguments
                    .menu_url
                    .as_deref()
                    .map(inline_human_text)
                    .unwrap_or_else(|| "automatic discovery".into()),
                if arguments.confirm_menu_url {
                    "yes"
                } else {
                    "no"
                }
            ),
            "CREATE",
        )),
        Command::Watch {
            command: Some(heyfood_cli::MenuWatchCommand::Remove(arguments)),
        } => Ok((
            format!(
                "Mutation: remove a Menu Watch subscription\nWatch: {}",
                inline_human_text(&arguments.watch_id)
            ),
            "REMOVE",
        )),
        _ => Err(heyfood_bin::OneShotError::new(
            "human_authority_contract",
            "The human-only command is missing its authorization contract.",
        )),
    }
}

fn stdin_text(stdin: &[u8], label: &'static str) -> Result<String, heyfood_bin::OneShotError> {
    if stdin.is_empty() || stdin.len() > heyfood_bin::MAX_CONFIRMATION_STDIN_BYTES {
        return Err(heyfood_bin::OneShotError::new(
            "human_input",
            format!("{label} input is missing or exceeds 1 MiB"),
        ));
    }
    std::str::from_utf8(stdin)
        .map(|value| value.trim_end_matches(['\r', '\n']).to_owned())
        .map_err(|_| {
            heyfood_bin::OneShotError::new("human_input", format!("{label} input is not UTF-8"))
        })
}

fn bounded_human_data(
    value: String,
    maximum: usize,
    label: &'static str,
) -> Result<String, heyfood_bin::OneShotError> {
    let value = value.trim().to_owned();
    if value.is_empty() || value.chars().count() > maximum {
        return Err(heyfood_bin::OneShotError::new(
            "human_input",
            format!("{label} must contain between 1 and {maximum} characters"),
        ));
    }
    Ok(value)
}

fn review_lines(values: &[String]) -> String {
    values
        .iter()
        .map(|value| format!("  • {}", inline_human_text(value)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn inline_human_text(value: &str) -> String {
    terminal_safe_text(value)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(unix)]
fn open_controlling_terminal() -> io::Result<(File, File)> {
    Ok((
        OpenOptions::new().read(true).open("/dev/tty")?,
        OpenOptions::new().write(true).open("/dev/tty")?,
    ))
}

#[cfg(windows)]
fn open_controlling_terminal() -> io::Result<(File, File)> {
    Ok((
        OpenOptions::new().read(true).open("CONIN$")?,
        OpenOptions::new().write(true).open("CONOUT$")?,
    ))
}

fn load_selector_state(
    paths: &NativePaths,
    command: &Command,
    account_id: &str,
) -> Result<Option<heyfood_core::ImportedPythonState>, heyfood_bin::OneShotError> {
    let required = matches!(
        command,
        Command::Log(_) | Command::Item(heyfood_cli::ItemArgs { at: Some(_), .. })
    );
    if !required {
        return Ok(None);
    }
    let importer = PythonStateImporter::discover(paths).map_err(heyfood_bin::OneShotError::from)?;
    importer.import().map_err(heyfood_bin::OneShotError::from)?;
    let imported = importer
        .load_state()
        .map_err(heyfood_bin::OneShotError::from)?;
    if imported.is_some() || !matches!(command, Command::Log(_)) {
        return Ok(imported);
    }
    Ok(Some(heyfood_core::ImportedPythonState {
        account_user_id: Some(account_id.to_owned()),
        global: std::collections::BTreeMap::new(),
        account_scoped: std::collections::BTreeMap::new(),
    }))
}

fn registration_to_one_shot(error: RegistrationError) -> heyfood_bin::OneShotError {
    heyfood_bin::OneShotError {
        code: error.code,
        message: error.public_message,
        outcome_uncertain: error.outcome_uncertain,
    }
}

fn uncertain_one_shot(code: &'static str, message: impl Into<String>) -> heyfood_bin::OneShotError {
    heyfood_bin::OneShotError {
        code,
        message: message.into(),
        outcome_uncertain: true,
    }
}

fn read_command_stdin(command: &Command) -> Result<Vec<u8>, heyfood_bin::OneShotError> {
    let should_read = match command {
        Command::Ask(arguments) | Command::Reply(arguments) => arguments.text.is_empty(),
        Command::Log(arguments) => arguments.meal.is_empty(),
        Command::Grocery {
            command: Some(heyfood_cli::GroceryCommand::Confirm(_)),
        } => true,
        _ => false,
    };
    if !should_read || io::stdin().is_terminal() {
        return Ok(Vec::new());
    }
    let mut bytes = Vec::new();
    io::stdin()
        .take((heyfood_bin::MAX_CONFIRMATION_STDIN_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| {
            heyfood_bin::OneShotError::new("stdin_read", "could not read standard input")
        })?;
    Ok(bytes)
}

fn one_shot_hint(code: &str) -> Option<&'static str> {
    match code {
        "login_required" => Some("Run `heyfood login` to connect an account, then retry."),
        "phase2_parity_pending" | "command_not_available" => Some("Run `heyfood --help`."),
        "session_cancelled_before_dispatch" => Some("Run the command again when ready."),
        "authorization_scope_upgrade_required" => {
            Some("Run `heyfood login` to approve the current supported authorization set.")
        }
        _ => None,
    }
}

fn ensure_command_scopes(
    command: &Command,
    granted_scope: &str,
) -> Result<(), heyfood_bin::OneShotError> {
    let required: &[&str] = match command {
        Command::Grocery {
            command:
                None
                | Some(
                    heyfood_cli::GroceryCommand::List
                    | heyfood_cli::GroceryCommand::Exclusions
                    | heyfood_cli::GroceryCommand::Export(_),
                ),
        } => &["grocery:read"],
        Command::Grocery { .. } => &["grocery:read", "grocery:write"],
        Command::Watch { .. } => &["menu:watch"],
        _ => &[],
    };
    let granted = granted_scope.split_whitespace().collect::<Vec<_>>();
    let missing = required
        .iter()
        .copied()
        .filter(|scope| !granted.contains(scope))
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(());
    }
    Err(heyfood_bin::OneShotError::new(
        "authorization_scope_upgrade_required",
        format!(
            "This command requires additional authorization ({}). Run `heyfood login` to approve it; token refresh cannot add scopes.",
            missing.join(", ")
        ),
    ))
}

#[cfg(feature = "native-credentials")]
async fn logout(machine: bool) -> ExitCode {
    match logout_inner().await {
        Ok(document) => match heyfood_cli::render_logout_success(&document, machine) {
            Ok(output) => {
                print!("{output}");
                ExitCode::SUCCESS
            }
            Err(_) => failure(
                "internal_error",
                "Could not render the logout result.",
                None,
                machine,
                false,
            ),
        },
        Err(error) => failure(
            error.code,
            "Could not securely clear native hello.food account credentials.",
            Some("Run `heyfood logout` again. If this persists, contact hello.food support."),
            machine,
            error.outcome_uncertain,
        ),
    }
}

#[cfg(feature = "native-credentials")]
async fn logout_inner() -> Result<LogoutOutcome, PortError> {
    let paths = NativePaths::discover()?;
    let auth_store = NativeAuthStore::open(paths.config_dir())?;
    let session_store = Arc::new(NativeSessionStore::open(paths.config_dir())?);
    auth_store.finish_authorization_terminal_cleanup()?;
    if auth_store.finish_account_bound_logout(session_store.as_ref())? {
        return Ok(LogoutOutcome::recovered_local_logout());
    }
    let Some(initial_credentials) = auth_store.load_account_bound(session_store.as_ref())? else {
        return Ok(LogoutOutcome::already_logged_out());
    };
    let (service_url, policy) = service_url().map_err(|error| {
        PortError::new(
            error.code,
            "could not construct the configured hello.food service origin",
        )
    })?;
    let cancellation = CancellationToken::new();
    let signal_cancellation = cancellation.clone();
    let signal = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal_cancellation.cancel();
        }
    });
    let prepared = prepare_logout_authority(
        &auth_store,
        Arc::clone(&session_store),
        initial_credentials.clone(),
        service_url,
        policy,
        cancellation.clone(),
    )
    .await;
    let (credentials, service) = match prepared {
        Ok(value) => value,
        Err(error) => {
            signal.abort();
            auth_store.clear_account_bound_after_preflight_failure(
                &initial_credentials,
                session_store.as_ref(),
            )?;
            return Ok(LogoutOutcome::preflight_failed(error.outcome_uncertain));
        }
    };
    let local = NativeLogoutLocal {
        auth: &auth_store,
        session: session_store.as_ref(),
    };
    let result = Logout::new(&service, &local)
        .execute(&credentials, cancellation)
        .await;
    signal.abort();
    result
}

#[cfg(feature = "native-credentials")]
async fn prepare_logout_authority(
    auth_store: &NativeAuthStore,
    session_store: Arc<NativeSessionStore>,
    mut credentials: AuthCredentialBundle,
    service_url: ServiceUrl,
    policy: NetworkPolicy,
    cancellation: CancellationToken,
) -> Result<(AuthCredentialBundle, HttpService), PortError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_secs());
    let now = i64::try_from(now).unwrap_or(i64::MAX);
    if credentials.channel.expires_at_unix() <= now {
        let refresh = auth_store.begin_refresh()?;
        credentials = refresh.load()?.ok_or_else(|| {
            PortError::new(
                "login_required",
                "native hello.food account authority disappeared before logout",
            )
        })?;
        if credentials.channel.expires_at_unix() <= now {
            if cancellation.is_cancelled() {
                return Err(PortError::new(
                    "channel_refresh_cancelled_before_dispatch",
                    "channel refresh was cancelled before dispatch",
                ));
            }
            let client = RegistrationClient::new(service_url.clone(), policy)
                .map_err(registration_to_port)?;
            refresh.mark_reconciliation_required()?;
            credentials.channel = match client.refresh_channel(&credentials.channel).await {
                Ok(channel) => channel,
                Err(error) if error.outcome_uncertain => {
                    return Err(registration_to_port(error));
                }
                Err(error) => {
                    refresh.clear_reconciliation_required()?;
                    return Err(registration_to_port(error));
                }
            };
            refresh.replace(&credentials).map_err(|_| {
                PortError::uncertain(
                    "channel_refresh_persistence_outcome_uncertain",
                    "channel refresh completed but durable replacement was not observed",
                )
            })?;
        }
    }

    credentials = auth_store
        .load_account_bound(session_store.as_ref())?
        .ok_or_else(|| {
            PortError::new(
                "login_required",
                "native hello.food account authority disappeared before logout",
            )
        })?;
    let api_key = std::env::var("HEYFOOD_API_KEY")
        .ok()
        .filter(|value| !value.is_empty())
        .map(SensitiveString::new);
    let cli_auth = CliAuthContext::new(
        credentials.channel.device_id.clone(),
        credentials.channel.access_token.clone(),
        api_key,
    )?;
    let service =
        HttpService::new(service_url, policy, HttpDeadlines::default())?.with_cli_auth(cli_auth);
    let ensure = EnsureSession::new(
        Arc::new(service.clone()),
        session_store.clone(),
        Arc::new(NativeClock),
    );
    let session = match ensure
        .execute(
            SessionSnapshot {
                credentials: credentials.session.clone(),
                reconciliation_required: session_store.reconciliation_required()?,
            },
            cancellation,
        )
        .await
        .map_err(ensure_session_to_port)?
    {
        EnsureSessionOutcome::Current(session) | EnsureSessionOutcome::Refreshed(session) => {
            session
        }
        EnsureSessionOutcome::CancelledBeforeDispatch => {
            return Err(PortError::new(
                "session_refresh_cancelled_before_dispatch",
                "session refresh was cancelled before dispatch",
            ));
        }
    };
    credentials = auth_store
        .load_account_bound(session_store.as_ref())?
        .ok_or_else(|| {
            PortError::new(
                "login_required",
                "native hello.food account authority disappeared before logout",
            )
        })?;
    if credentials.session != session {
        return Err(PortError::uncertain(
            "logout_refresh_persistence_outcome_uncertain",
            "refreshed logout authority did not match durable session state",
        ));
    }
    Ok((credentials, service))
}

#[cfg(feature = "native-credentials")]
fn registration_to_port(error: RegistrationError) -> PortError {
    if error.outcome_uncertain {
        PortError::uncertain(error.code, error.public_message)
    } else {
        PortError::new(error.code, error.public_message)
    }
}

#[cfg(feature = "native-credentials")]
fn ensure_session_to_port(error: EnsureSessionError) -> PortError {
    match error {
        EnsureSessionError::ReconciliationRequired => PortError::uncertain(
            "credential_reconciliation_required",
            "session credentials have an unresolved refresh outcome",
        ),
        EnsureSessionError::Service(error) => error,
        EnsureSessionError::ServiceReconciliationRequired(error)
        | EnsureSessionError::CredentialReconciliationRequired(error) => {
            PortError::uncertain(error.code, error.message)
        }
        EnsureSessionError::ReconciliationMarkerWrite { marker, .. } => {
            PortError::uncertain(marker.code, marker.message)
        }
    }
}

async fn login(arguments: heyfood_cli::LoginArgs, machine: bool) -> ExitCode {
    let result = login_inner(arguments, machine, false).await;
    match result {
        Ok(document) => match heyfood_cli::render_registration_success(&document, machine) {
            Ok(output) => {
                print!("{output}");
                ExitCode::SUCCESS
            }
            Err(_) => failure(
                "internal_error",
                "Could not render the login result.",
                None,
                machine,
                false,
            ),
        },
        Err(error) => failure(
            error.code,
            &error.public_message,
            registration_hint(error.code),
            machine,
            error.outcome_uncertain,
        ),
    }
}

async fn login_inner(
    arguments: heyfood_cli::LoginArgs,
    machine: bool,
    offer_account_choice: bool,
) -> Result<RegistrationResultDocument, RegistrationError> {
    let paths = NativePaths::discover().map_err(platform_error)?;
    let auth_store = NativeAuthStore::open(paths.config_dir()).map_err(platform_error)?;
    let session_store = NativeSessionStore::open(paths.config_dir()).map_err(platform_error)?;
    auth_store
        .finish_authorization_terminal_cleanup()
        .map_err(platform_error)?;
    let (service_url, policy) = service_url()?;
    let client = RegistrationClient::new(service_url, policy)?;

    if let Some(journal) = auth_store
        .pending_authorization_replacement()
        .map_err(platform_error)?
    {
        return resume_authorization_replacement(&client, &auth_store, &session_store, journal)
            .await;
    }

    let existing_authorization = auth_store
        .load_account_bound(&session_store)
        .map_err(platform_error)?;
    if existing_authorization.is_none() {
        let authorization = if offer_account_choice {
            client.start_device_connection().await?
        } else {
            client.start_device_login().await?
        };
        return complete_initial_authorization(
            &client,
            &auth_store,
            &session_store,
            authorization,
            InitialAuthorizationOptions {
                policy,
                no_browser: arguments.no_browser,
                timeout: arguments.timeout(),
                machine,
            },
        )
        .await;
    }

    let client_transaction_id = OperationId::new().as_uuid().to_string();
    let journal = auth_store
        .begin_authorization_replacement(client_transaction_id.clone(), &session_store)
        .map_err(platform_error)?;
    let authorization = client
        .start_device_reauthorization(
            &journal.previous.channel.scope,
            &client_transaction_id,
            &journal.previous.channel.device_id,
        )
        .await?;
    eprintln!(
        "Open this URL to continue: {}",
        authorization.verification_uri
    );
    eprintln!("Approval code: {}", authorization.user_code);
    io::stderr().flush().ok();
    if !machine
        && !arguments.no_browser
        && let Ok(destination) = BrowserUrl::parse(&authorization.verification_uri, policy)
    {
        let _ = NativeBrowser.open(destination).await;
    }
    let cancellation = CancellationToken::new();
    let signal_cancellation = cancellation.clone();
    let signal = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal_cancellation.cancel();
        }
    });
    let provisional = client
        .complete_device_reauthorization_grant(
            authorization,
            client_transaction_id.clone(),
            journal.previous.channel.device_id.clone(),
            arguments.timeout(),
            cancellation.clone(),
        )
        .await;
    signal.abort();
    let provisional = match provisional {
        Ok(value) => value,
        Err(error) => {
            // Prepare was never dispatched, so old server authority remains
            // active. Verify active stores and discard only this local intent.
            auth_store
                .finalize_unpromoted_authorization(&client_transaction_id, &session_store)
                .map_err(platform_error)?;
            return Err(error);
        }
    };
    auth_store
        .record_provisional_authorization(
            &client_transaction_id,
            provisional.authorization_transaction_id.clone(),
            provisional.access_token.clone(),
        )
        .map_err(platform_error)?;
    if cancellation.is_cancelled() {
        auth_store
            .finalize_unpromoted_authorization(&client_transaction_id, &session_store)
            .map_err(platform_error)?;
        return Err(RegistrationError {
            code: "cancelled",
            public_message:
                "Login canceled before authority was staged. Existing credentials remain active."
                    .into(),
            retryable: false,
            outcome_uncertain: false,
        });
    }
    let prepared = client.prepare_device_reauthorization(&provisional).await?;
    if matches!(
        prepared.status,
        ReauthorizationStageStatus::Aborted | ReauthorizationStageStatus::Expired
    ) {
        auth_store
            .finalize_unpromoted_authorization(&client_transaction_id, &session_store)
            .map_err(platform_error)?;
        return Err(RegistrationError {
            code: "reauthorization_not_promoted",
            public_message:
                "Login expired or was aborted before promotion. Existing credentials remain active."
                    .into(),
            retryable: prepared.status == ReauthorizationStageStatus::Expired,
            outcome_uncertain: false,
        });
    }
    let staged = staged_from_status(prepared)?;
    persist_and_promote_reauthorization(
        &client,
        &auth_store,
        &session_store,
        journal,
        staged,
        cancellation.is_cancelled(),
    )
    .await
}

async fn persist_and_promote_reauthorization(
    client: &RegistrationClient,
    auth_store: &NativeAuthStore,
    session_store: &NativeSessionStore,
    previous_journal: AuthorizationReplacementJournal,
    staged: StagedReauthorization,
    cancel_before_promotion: bool,
) -> Result<RegistrationResultDocument, RegistrationError> {
    let journal = AuthorizationReplacementJournal {
        phase: AuthorizationReplacementPhase::Prepared,
        client_transaction_id: staged.client_transaction_id.clone(),
        stage_id: Some(staged.stage_id.clone()),
        authorization_transaction_id: Some(staged.authorization_transaction_id.clone()),
        provisional_access_token: None,
        recovery_token: Some(staged.recovery_token.clone()),
        bundle_digest: Some(staged.bundle_digest.clone()),
        previous: previous_journal.previous.clone(),
        replacement: Some(staged.credentials.clone()),
    };
    if staged.credentials.session.account_id != previous_journal.previous.session.account_id
        || staged.credentials.channel.device_id != previous_journal.previous.channel.device_id
        || staged.credentials.channel.client_id != previous_journal.previous.channel.client_id
    {
        // The staged server authority is not safe to activate. Persist abort
        // intent and the exact recovery capability before dispatch.
        auth_store
            .mark_authorization_abort_dispatched(journal)
            .map_err(platform_error)?;
        let terminal = client.abort_reauthorization(&staged).await?;
        if matches!(
            terminal.status,
            ReauthorizationStageStatus::Aborted | ReauthorizationStageStatus::Expired
        ) {
            auth_store
                .finalize_unpromoted_authorization(
                    &previous_journal.client_transaction_id,
                    session_store,
                )
                .map_err(platform_error)?;
        }
        return Err(RegistrationError {
            code: "reauthorization_account_conflict",
            public_message: "The approved account, device, or client does not match the connected account. Existing credentials remain active.".into(),
            retryable: false,
            outcome_uncertain: !matches!(
                terminal.status,
                ReauthorizationStageStatus::Aborted | ReauthorizationStageStatus::Expired
            ),
        });
    }
    if let Err(error) = auth_store.stage_authorization_replacement(journal.clone(), session_store) {
        // A local expected-version race must not leave a server stage waiting
        // for accidental promotion. Abort is idempotent; lost abort response
        // remains recoverable from the still-durable preparing journal.
        auth_store
            .mark_authorization_abort_dispatched(journal.clone())
            .map_err(platform_error)?;
        if let Ok(terminal) = client.abort_reauthorization(&staged).await
            && matches!(
                terminal.status,
                ReauthorizationStageStatus::Aborted | ReauthorizationStageStatus::Expired
            )
        {
            auth_store
                .finalize_unpromoted_authorization(
                    &previous_journal.client_transaction_id,
                    session_store,
                )
                .map_err(platform_error)?;
        }
        return Err(platform_error(error));
    }
    if cancel_before_promotion {
        auth_store
            .mark_authorization_abort_dispatched(journal)
            .map_err(platform_error)?;
        let terminal = client.abort_reauthorization(&staged).await?;
        return finish_authoritative_status(auth_store, session_store, terminal.status, &staged);
    }
    auth_store
        .mark_authorization_promotion_dispatched(&staged.client_transaction_id, session_store)
        .map_err(platform_error)?;
    let terminal = client.promote_reauthorization(&staged).await?;
    finish_authoritative_status(auth_store, session_store, terminal.status, &staged)
}

async fn resume_authorization_replacement(
    client: &RegistrationClient,
    auth_store: &NativeAuthStore,
    session_store: &NativeSessionStore,
    journal: AuthorizationReplacementJournal,
) -> Result<RegistrationResultDocument, RegistrationError> {
    if journal.phase == AuthorizationReplacementPhase::Preparing {
        let Some(provisional_access_token) = journal.provisional_access_token.clone() else {
            auth_store
                .finalize_unpromoted_authorization(&journal.client_transaction_id, session_store)
                .map_err(platform_error)?;
            return Err(RegistrationError {
                code: "reauthorization_restarted",
                public_message: "The interrupted login had not staged authority. Run `heyfood login` again to restart approval.".into(),
                retryable: true,
                outcome_uncertain: false,
            });
        };
        let provisional = ProvisionalReauthorization {
            client_transaction_id: journal.client_transaction_id.clone(),
            authorization_transaction_id: journal.authorization_transaction_id.clone().ok_or_else(|| RegistrationError {
                code: "reauthorization_reconciliation_required",
                public_message: "The interrupted login journal is incomplete. Local credentials remain blocked.".into(),
                retryable: false,
                outcome_uncertain: true,
            })?,
            device_id: journal.previous.channel.device_id.clone(),
            access_token: provisional_access_token,
        };
        let prepared = client.prepare_device_reauthorization(&provisional).await?;
        if matches!(
            prepared.status,
            ReauthorizationStageStatus::Aborted | ReauthorizationStageStatus::Expired
        ) {
            auth_store
                .finalize_unpromoted_authorization(&journal.client_transaction_id, session_store)
                .map_err(platform_error)?;
            return Err(RegistrationError {
                code: "reauthorization_not_promoted",
                public_message: "The interrupted login expired or was aborted. Existing credentials remain active.".into(),
                retryable: prepared.status == ReauthorizationStageStatus::Expired,
                outcome_uncertain: false,
            });
        }
        let staged = staged_from_status(prepared)?;
        return persist_and_promote_reauthorization(
            client,
            auth_store,
            session_store,
            journal,
            staged,
            false,
        )
        .await;
    }
    let staged = staged_from_journal(&journal)?;
    let status = client
        .reauthorization_status(
            &staged.stage_id,
            &staged.recovery_token,
            &staged.client_transaction_id,
            &staged.authorization_transaction_id,
            &staged.device_id,
            &staged.bundle_digest,
        )
        .await?;
    if journal.phase == AuthorizationReplacementPhase::AbortDispatched {
        return match status.status {
            ReauthorizationStageStatus::Staged => {
                let aborted = client.abort_reauthorization(&staged).await?;
                finish_authoritative_status(auth_store, session_store, aborted.status, &staged)
            }
            ReauthorizationStageStatus::Aborted | ReauthorizationStageStatus::Expired => {
                finish_authoritative_status(auth_store, session_store, status.status, &staged)
            }
            ReauthorizationStageStatus::Promoted => Err(RegistrationError {
                code: "reauthorization_abort_conflict",
                public_message: "A canceled login was unexpectedly promoted. Local credentials remain blocked for operator reconciliation.".into(),
                retryable: false,
                outcome_uncertain: true,
            }),
        };
    }
    match status.status {
        ReauthorizationStageStatus::Staged => {
            auth_store
                .mark_authorization_promotion_dispatched(
                    &staged.client_transaction_id,
                    session_store,
                )
                .map_err(platform_error)?;
            let promoted = client.promote_reauthorization(&staged).await?;
            finish_authoritative_status(auth_store, session_store, promoted.status, &staged)
        }
        terminal => {
            if terminal == ReauthorizationStageStatus::Promoted
                && journal.phase == AuthorizationReplacementPhase::Prepared
            {
                auth_store
                    .mark_authorization_promotion_dispatched(
                        &staged.client_transaction_id,
                        session_store,
                    )
                    .map_err(platform_error)?;
            }
            finish_authoritative_status(auth_store, session_store, terminal, &staged)
        }
    }
}

fn staged_from_status(
    status: ReauthorizationStatus,
) -> Result<StagedReauthorization, RegistrationError> {
    if status.status != ReauthorizationStageStatus::Staged {
        return Err(reconciliation_error());
    }
    Ok(StagedReauthorization {
        stage_id: status.stage_id,
        client_transaction_id: status.client_transaction_id,
        authorization_transaction_id: status.authorization_transaction_id,
        device_id: status.device_id,
        status: status.status,
        scopes: status.scopes,
        bundle_digest: status.bundle_digest,
        recovery_token: status.recovery_token.ok_or_else(reconciliation_error)?,
        credentials: status.credentials.ok_or_else(reconciliation_error)?,
    })
}

fn staged_from_journal(
    journal: &AuthorizationReplacementJournal,
) -> Result<StagedReauthorization, RegistrationError> {
    Ok(StagedReauthorization {
        stage_id: journal.stage_id.clone().ok_or_else(reconciliation_error)?,
        client_transaction_id: journal.client_transaction_id.clone(),
        authorization_transaction_id: journal
            .authorization_transaction_id
            .clone()
            .ok_or_else(reconciliation_error)?,
        device_id: journal.previous.channel.device_id.clone(),
        status: ReauthorizationStageStatus::Staged,
        scopes: journal
            .replacement
            .as_ref()
            .ok_or_else(reconciliation_error)?
            .channel
            .scope
            .split_whitespace()
            .map(str::to_owned)
            .collect(),
        bundle_digest: journal
            .bundle_digest
            .clone()
            .ok_or_else(reconciliation_error)?,
        recovery_token: journal
            .recovery_token
            .clone()
            .ok_or_else(reconciliation_error)?,
        credentials: journal
            .replacement
            .clone()
            .ok_or_else(reconciliation_error)?,
    })
}

fn finish_authoritative_status(
    auth_store: &NativeAuthStore,
    session_store: &NativeSessionStore,
    status: ReauthorizationStageStatus,
    staged: &StagedReauthorization,
) -> Result<RegistrationResultDocument, RegistrationError> {
    match status {
        ReauthorizationStageStatus::Promoted => {
            auth_store
                .finalize_promoted_authorization(&staged.client_transaction_id, session_store)
                .map_err(platform_error)?;
            Ok(RegistrationResultDocument::completed(
                ProfileStatus::Unknown,
            ))
        }
        ReauthorizationStageStatus::Aborted | ReauthorizationStageStatus::Expired => {
            auth_store
                .finalize_unpromoted_authorization(&staged.client_transaction_id, session_store)
                .map_err(platform_error)?;
            Err(RegistrationError {
                code: "reauthorization_not_promoted",
                public_message: "Login was not promoted. Existing credentials remain active."
                    .into(),
                retryable: status == ReauthorizationStageStatus::Expired,
                outcome_uncertain: false,
            })
        }
        ReauthorizationStageStatus::Staged => Err(reconciliation_error()),
    }
}

fn reconciliation_error() -> RegistrationError {
    RegistrationError {
        code: "reauthorization_reconciliation_required",
        public_message:
            "The staged login is incomplete. Local credentials remain blocked until it is reconciled."
                .into(),
        retryable: false,
        outcome_uncertain: true,
    }
}

async fn register(arguments: heyfood_cli::RegisterArgs, machine: bool, no_input: bool) -> ExitCode {
    let continue_to_tui = registration_continues_to_tui(
        &arguments,
        machine,
        no_input,
        io::stdin().is_terminal(),
        io::stdout().is_terminal(),
    );
    let result = register_inner(arguments, machine).await;
    match result {
        Ok(document) => match heyfood_cli::render_registration_success(&document, machine) {
            Ok(output) => {
                print!("{output}");
                if continue_to_tui {
                    io::stdout().flush().ok();
                    interactive(false, document.profile_status == ProfileStatus::Missing).await
                } else {
                    ExitCode::SUCCESS
                }
            }
            Err(_) => failure(
                "internal_error",
                "Could not render the registration result.",
                None,
                machine,
                false,
            ),
        },
        Err(error) => failure(
            error.code,
            &error.public_message,
            registration_hint(error.code),
            machine,
            error.outcome_uncertain,
        ),
    }
}

const fn registration_continues_to_tui(
    arguments: &heyfood_cli::RegisterArgs,
    machine: bool,
    no_input: bool,
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
) -> bool {
    !arguments.no_onboard && !machine && !no_input && stdin_is_terminal && stdout_is_terminal
}

async fn register_inner(
    arguments: heyfood_cli::RegisterArgs,
    machine: bool,
) -> Result<RegistrationResultDocument, RegistrationError> {
    let paths = NativePaths::discover().map_err(platform_error)?;
    let auth_store = NativeAuthStore::open(paths.config_dir()).map_err(platform_error)?;
    let session_store = NativeSessionStore::open(paths.config_dir()).map_err(platform_error)?;
    if auth_store
        .load_account_bound(&session_store)
        .map_err(platform_error)?
        .is_some()
    {
        return Err(RegistrationError {
            code: "account_already_connected",
            public_message: "A hello.food account is already connected.".into(),
            retryable: false,
            outcome_uncertain: false,
        });
    }

    let (service_url, policy) = service_url()?;
    let client = RegistrationClient::new(service_url, policy)?;
    let authorization = client.start_device_registration().await?;

    let document = complete_initial_authorization(
        &client,
        &auth_store,
        &session_store,
        authorization,
        InitialAuthorizationOptions {
            policy,
            no_browser: arguments.no_browser,
            timeout: arguments.timeout(),
            machine,
        },
    )
    .await?;
    if arguments.no_onboard && document.profile_status != heyfood_core::ProfileStatus::Ready {
        eprintln!("Dietary onboarding was deferred. Your account remains connected.");
    }
    Ok(document)
}

#[derive(Clone, Copy)]
struct InitialAuthorizationOptions {
    policy: NetworkPolicy,
    no_browser: bool,
    timeout: Duration,
    machine: bool,
}

async fn complete_initial_authorization(
    client: &RegistrationClient,
    auth_store: &NativeAuthStore,
    session_store: &NativeSessionStore,
    authorization: DeviceAuthorization,
    options: InitialAuthorizationOptions,
) -> Result<RegistrationResultDocument, RegistrationError> {
    eprintln!(
        "Open this URL to continue: {}",
        authorization.verification_uri
    );
    eprintln!("Approval code: {}", authorization.user_code);
    io::stderr().flush().ok();

    // JSON mode is noninteractive by contract and therefore never launches a
    // browser. Human mode launches best-effort after publishing a copyable URL.
    if !options.machine
        && !options.no_browser
        && let Ok(destination) = BrowserUrl::parse(&authorization.verification_uri, options.policy)
    {
        let _ = NativeBrowser.open(destination).await;
    }

    let device_id = format!("heyfood-{}", OperationId::new().as_uuid());
    let cancellation = CancellationToken::new();
    let signal_cancellation = cancellation.clone();
    let signal = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal_cancellation.cancel();
        }
    });
    let outcome = client
        .complete_device_registration(authorization, device_id, options.timeout, cancellation)
        .await;
    signal.abort();
    let outcome = outcome?;

    // Persist only after OAuth, app-session exchange, and contract validation
    // all succeed. A durable cross-store marker blocks any split outcome.
    auth_store
        .initialize_account_bound(&outcome.credentials, session_store)
        .map_err(|_| RegistrationError {
            code: "registration_persistence_outcome_uncertain",
            public_message: "The account was connected, but its account-bound native credentials could not be initialized. Do not retry registration until account state is reconciled.".into(),
            retryable: false,
            outcome_uncertain: true,
        })?;
    Ok(RegistrationResultDocument::completed(
        outcome.profile_status,
    ))
}

fn service_url() -> Result<(ServiceUrl, NetworkPolicy), RegistrationError> {
    let api_url =
        std::env::var("HEYFOOD_API_URL").unwrap_or_else(|_| "https://api.hello.food".into());
    let policy = if api_url.starts_with("http://localhost")
        || api_url.starts_with("http://127.0.0.1")
        || api_url.starts_with("http://[::1]")
    {
        NetworkPolicy::DEVELOPMENT
    } else {
        NetworkPolicy::HTTPS_ONLY
    };
    let service_url = ServiceUrl::parse(&api_url, policy).map_err(|_| RegistrationError {
        code: "service_url",
        public_message: "HEYFOOD_API_URL is not a valid secure hello.food service URL.".into(),
        retryable: false,
        outcome_uncertain: false,
    })?;
    Ok((service_url, policy))
}

fn platform_error(error: heyfood_application::PortError) -> RegistrationError {
    let public_message = match error.code {
        "auth_exists" => {
            "A hello.food account is already connected. Log out before registering another account."
        }
        "lock_timeout" => "Native account state is busy. Wait a moment and retry.",
        _ => "Could not securely read or save native hello.food account state.",
    };
    RegistrationError {
        code: error.code,
        public_message: public_message.into(),
        retryable: error.outcome_uncertain,
        outcome_uncertain: error.outcome_uncertain,
    }
}

fn registration_hint(code: &str) -> Option<&'static str> {
    match code {
        "registration_unavailable" => Some("Registration is not enabled on this service yet."),
        "account_already_connected" => Some("Run `heyfood ask \"What can I eat?\"`."),
        "cancelled" | "authorization_expired" => {
            Some("Run `heyfood` to start a fresh account-connection request.")
        }
        "auth_contract_error" => {
            Some("Update heyfood and retry. If it continues, check hello.food service status.")
        }
        "reauthorization_scope_unsupported" => {
            Some("Update heyfood before attempting to replace this grant.")
        }
        "reauthorization_persistence_outcome_uncertain" => Some(
            "Run one authenticated command; native state will reconcile locally before network dispatch.",
        ),
        "session_exchange_outcome_uncertain"
        | "session_exchange_contract_uncertain"
        | "registration_persistence_outcome_uncertain" => {
            Some("Do not start another account-connection attempt until state is reconciled.")
        }
        _ => None,
    }
}

fn failure(
    kind: &str,
    message: &str,
    hint: Option<&str>,
    machine: bool,
    outcome_uncertain: bool,
) -> ExitCode {
    let output =
        heyfood_cli::render_error_with_outcome(kind, message, hint, machine, outcome_uncertain)
            .unwrap_or_else(|_| {
                "heyfood error: Could not render the requested operation.\n".into()
            });
    if machine {
        print!("{output}");
    } else {
        eprint!("{output}");
    }
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use clap::Parser;

    use super::*;

    fn registration_arguments(no_onboard: bool) -> heyfood_cli::RegisterArgs {
        heyfood_cli::RegisterArgs {
            device: true,
            no_browser: true,
            timeout: 600,
            no_onboard,
        }
    }

    #[test]
    fn explicit_interactive_registration_continues_into_the_tui_by_default() {
        assert!(registration_continues_to_tui(
            &registration_arguments(false),
            false,
            false,
            true,
            true,
        ));
    }

    #[test]
    fn no_onboard_is_the_explicit_registration_handoff_opt_out() {
        assert!(!registration_continues_to_tui(
            &registration_arguments(true),
            false,
            false,
            true,
            true,
        ));
    }

    #[test]
    fn registration_never_starts_a_tui_for_json_or_redirected_output() {
        let arguments = registration_arguments(false);
        assert!(!registration_continues_to_tui(
            &arguments, true, false, true, true
        ));
        assert!(!registration_continues_to_tui(
            &arguments, false, false, false, true
        ));
        assert!(!registration_continues_to_tui(
            &arguments, false, false, true, false
        ));
    }

    #[test]
    fn no_input_never_hands_registration_into_the_questionnaire_tui() {
        assert!(!registration_continues_to_tui(
            &registration_arguments(false),
            false,
            true,
            true,
            true,
        ));
    }

    fn parsed_command(arguments: &[&str]) -> Command {
        Cli::try_parse_from(arguments).unwrap().command.unwrap()
    }

    #[test]
    fn only_classified_human_mutations_require_terminal_authority() {
        assert!(requires_human_terminal_authority(&parsed_command(&[
            "heyfood", "log", "oatmeal"
        ])));
        assert!(requires_human_terminal_authority(&parsed_command(&[
            "heyfood",
            "watch",
            "remove",
            "00000000-0000-4000-8000-000000000010"
        ])));
        assert!(!requires_human_terminal_authority(&parsed_command(&[
            "heyfood", "grocery", "show"
        ])));
        assert!(!requires_human_terminal_authority(&parsed_command(&[
            "heyfood", "ask", "hello"
        ])));
    }

    #[test]
    fn no_input_rejects_human_only_authority_before_opening_a_terminal() {
        let command = parsed_command(&["heyfood", "log", "oatmeal"]);
        let error = authorize_human_only_command(&command, &[], true).unwrap_err();
        assert_eq!(error.code, "human_input_disabled");
    }

    #[test]
    fn meal_review_requires_the_exact_terminal_phrase_and_sanitizes_controls() {
        let command = parsed_command(&[
            "heyfood",
            "log",
            "oatmeal\nforged\u{1b}[2J",
            "--for",
            "child\nforged\u{1b}[3J",
        ]);
        let mut approved_input = Cursor::new(b"LOG\n");
        let mut review = Vec::new();

        review_human_only_command(&command, &[], &mut approved_input, &mut review).unwrap();

        let review = String::from_utf8(review).unwrap();
        assert!(review.contains("Mutation: log meal memory"));
        assert!(review.contains("Meal: oatmeal forged[2J"));
        assert!(review.contains("Household target selector: child forged[3J"));
        assert!(!review.contains('\u{1b}'));
        assert!(!review.contains("\nforged"));

        let mut rejected_input = Cursor::new(b"yes\n");
        let error = review_human_only_command(&command, &[], &mut rejected_input, &mut Vec::new())
            .unwrap_err();
        assert_eq!(error.code, "human_confirmation_declined");
    }

    #[test]
    fn grocery_confirm_reviews_the_exact_proposal_and_matches_the_requested_decision() {
        let command = parsed_command(&["heyfood", "grocery", "confirm", "--decision", "accept"]);
        let proposal = serde_json::to_vec(&serde_json::json!({
            "confirmation_id": "00000000-0000-4000-8000-000000000001",
            "idempotency_key": "00000000-0000-4000-8000-000000000002",
            "operation": "add_items",
            "expires_at": "2026-07-21T12:05:00Z",
            "structured_preview": {
                "items": [{
                    "requested_name": "milk",
                    "quantity": 2.0,
                    "unit": "cartons",
                    "note": "lactose-free",
                    "sources": [{"source_type": "manual"}]
                }],
                "exclusions": ["pork"]
            },
            "preconditions": [
                {"type": "list_version", "list_id": "00000000-0000-4000-8000-000000000123", "expected_version": 4},
                {"type": "context_hash", "expected": "context-v4"}
            ],
            "confirmation_token": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        }))
        .unwrap();
        let mut input = Cursor::new(b"ACCEPT\n");
        let mut review = Vec::new();

        review_human_only_command(&command, &proposal, &mut input, &mut review).unwrap();

        let review = String::from_utf8(review).unwrap();
        assert!(review.contains("Exact human-visible proposal intent"));
        assert!(review.contains("Confirmation ID: 00000000-0000-4000-8000-000000000001"));
        assert!(review.contains("Operation: add_items"));
        assert!(review.contains("\"quantity\": 2.0"));
        assert!(review.contains("\"unit\": \"cartons\""));
        assert!(review.contains("\"note\": \"lactose-free\""));
        assert!(review.contains("\"source_type\": \"manual\""));
        assert!(review.contains("\"exclusions\""));
        assert!(review.contains("\"list_id\""));
        assert!(review.contains("\"expected_version\": 4"));
        assert!(review.contains("\"expected\": \"context-v4\""));
        assert!(review.contains("Requested decision: ACCEPT"));
        assert!(!review.contains("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
        assert!(!review.contains("00000000-0000-4000-8000-000000000002"));
    }

    #[test]
    fn menu_watch_review_displays_the_identity_confirmation_flag() {
        for (argument, expected) in [
            (None, "Confirm supplied menu identity: no"),
            (
                Some("--confirm-menu-url"),
                "Confirm supplied menu identity: yes",
            ),
        ] {
            let mut arguments = vec![
                "heyfood",
                "watch",
                "add",
                "0c1cb790-0000-4000-8000-000000000000",
                "--weekday",
                "thursday",
                "--hour",
                "9",
                "--menu-url",
                "https://restaurant.example/menu",
            ];
            if let Some(argument) = argument {
                arguments.push(argument);
            }
            let command = parsed_command(&arguments);
            let (review, expected_phrase) = human_review_document(&command, &[]).unwrap();

            assert_eq!(expected_phrase, "CREATE");
            assert!(review.contains(expected));
            assert!(review.contains("Menu source: https://restaurant.example/menu"));
        }
    }

    #[test]
    fn every_guarded_command_has_an_exact_review_contract() {
        let proposal = serde_json::to_vec(&serde_json::json!({
            "confirmation_id": "00000000-0000-4000-8000-000000000001",
            "idempotency_key": "00000000-0000-4000-8000-000000000002",
            "operation": "add_items",
            "expires_at": "2026-07-21T12:05:00Z",
            "structured_preview": {"items": []},
            "preconditions": [{"type": "list_version", "expected_version": 4}],
            "confirmation_token": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        }))
        .unwrap();
        let cases = [
            (
                parsed_command(&["heyfood", "log", "oatmeal"]),
                Vec::new(),
                "LOG",
            ),
            (
                parsed_command(&[
                    "heyfood",
                    "grocery",
                    "add",
                    "--list-id",
                    "00000000-0000-4000-8000-000000000123",
                    "--version",
                    "4",
                    "onion",
                ]),
                Vec::new(),
                "PREPARE",
            ),
            (
                parsed_command(&[
                    "heyfood",
                    "grocery",
                    "remove",
                    "--list-id",
                    "00000000-0000-4000-8000-000000000123",
                    "--version",
                    "4",
                    "item-1",
                ]),
                Vec::new(),
                "PREPARE",
            ),
            (
                parsed_command(&[
                    "heyfood",
                    "grocery",
                    "state",
                    "--list-id",
                    "00000000-0000-4000-8000-000000000123",
                    "--version",
                    "4",
                    "item-1",
                    "purchased",
                ]),
                Vec::new(),
                "PREPARE",
            ),
            (
                parsed_command(&[
                    "heyfood",
                    "grocery",
                    "never",
                    "--list-id",
                    "00000000-0000-4000-8000-000000000123",
                    "--version",
                    "4",
                    "pork",
                ]),
                Vec::new(),
                "PREPARE",
            ),
            (
                parsed_command(&["heyfood", "grocery", "confirm", "--decision", "accept"]),
                proposal,
                "ACCEPT",
            ),
            (
                parsed_command(&[
                    "heyfood",
                    "watch",
                    "add",
                    "0c1cb790-0000-4000-8000-000000000000",
                    "--weekday",
                    "thursday",
                    "--hour",
                    "9",
                ]),
                Vec::new(),
                "CREATE",
            ),
            (
                parsed_command(&[
                    "heyfood",
                    "watch",
                    "remove",
                    "00000000-0000-4000-8000-000000000010",
                ]),
                Vec::new(),
                "REMOVE",
            ),
        ];

        for (command, stdin, expected) in cases {
            assert!(requires_human_terminal_authority(&command));
            assert_eq!(human_review_document(&command, &stdin).unwrap().1, expected);
        }
    }
}
