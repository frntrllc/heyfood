//! Native heyfood executable composition root.

#![forbid(unsafe_code)]

use std::io::{self, IsTerminal, Read, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use heyfood_agent_runtime::{
    CliAuthContext, HttpDeadlines, HttpService, RegistrationClient, RegistrationError,
};
use heyfood_application::{BrowserPort, CredentialPort, EnsureSession};
use heyfood_cli::{Cli, Command, OutputMode, RegistrationResultDocument};
use heyfood_core::{
    BrowserUrl, NetworkPolicy, OperationId, SensitiveString, ServiceUrl, SessionSnapshot,
    terminal_safe_text,
};
#[cfg(not(windows))]
use heyfood_platform::FileCredentialStore as NativeSessionStore;
#[cfg(windows)]
use heyfood_platform::WindowsCredentialStore as NativeSessionStore;
use heyfood_platform::{NativeAuthStore, NativeBrowser, NativeClock, NativePaths};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> ExitCode {
    #[cfg(feature = "native-credentials")]
    if let Some(outcome) = heyfood_platform::run_credential_broker_if_requested() {
        return outcome;
    }

    let cli = Cli::parse_env();
    let machine = cli.machine_output();
    let output_mode = cli.output_mode(io::stdout().is_terminal());
    if cli.raw {
        eprintln!("--raw is deprecated; use --json.");
    }
    match cli.command {
        Some(Command::Completion { shell }) => {
            heyfood_cli::write_completions(shell, &mut io::stdout());
            ExitCode::SUCCESS
        }
        Some(Command::Register(arguments)) => register(arguments, machine).await,
        Some(Command::Login(arguments)) => login(arguments, machine).await,
        Some(command) if is_native_one_shot(&command) => {
            one_shot(command, output_mode, machine).await
        }
        Some(_) => pending_command(machine),
        None => bare(machine),
    }
}

const fn is_native_one_shot(command: &Command) -> bool {
    matches!(
        command,
        Command::Ask(_)
            | Command::Reply(_)
            | Command::Log(_)
            | Command::Item(_)
            | Command::Grocery { .. }
            | Command::Health { .. }
    )
}

fn bare(machine: bool) -> ExitCode {
    if machine {
        println!(
            "{{\"ok\":true,\"message\":\"Run an explicit native command.\",\"next_command\":\"heyfood register\"}}"
        );
    } else {
        println!(
            "hello.food for your terminal.\n\nStart: heyfood register\nAsk:   heyfood ask \"What can I eat?\"\nHelp:  heyfood --help"
        );
    }
    ExitCode::SUCCESS
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

async fn one_shot(command: Command, output_mode: OutputMode, machine: bool) -> ExitCode {
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
    let result = one_shot_inner(command, output_mode, cancellation).await;
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

async fn one_shot_inner(
    command: Command,
    output_mode: OutputMode,
    cancellation: CancellationToken,
) -> Result<String, heyfood_bin::OneShotError> {
    let paths = NativePaths::discover().map_err(heyfood_bin::OneShotError::from)?;
    let auth_store =
        NativeAuthStore::open(paths.config_dir()).map_err(heyfood_bin::OneShotError::from)?;
    let credential_store = Arc::new(
        NativeSessionStore::open(paths.config_dir()).map_err(heyfood_bin::OneShotError::from)?,
    );
    let mut auth = auth_store
        .load_reconciling_authorization(credential_store.as_ref())
        .map_err(heyfood_bin::OneShotError::from)?
        .ok_or_else(|| {
            heyfood_bin::OneShotError::new(
                "login_required",
                "No hello.food account is connected. Run `heyfood register` first.",
            )
        })?;
    ensure_command_scopes(&command, &auth.channel.scope)?;

    let (service_url, policy) = service_url().map_err(registration_to_one_shot)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_secs());
    if auth.channel.expires_at_unix() <= i64::try_from(now).unwrap_or(i64::MAX) {
        // Refresh tokens rotate. Serialize the reload, remote consumption, and
        // durable replacement across CLI processes so a stale process cannot
        // consume and then overwrite another process's grant.
        let refresh = auth_store
            .begin_refresh()
            .map_err(heyfood_bin::OneShotError::from)?;
        auth = refresh
            .load()
            .map_err(heyfood_bin::OneShotError::from)?
            .ok_or_else(|| {
                heyfood_bin::OneShotError::new(
                    "login_required",
                    "No hello.food account is connected. Run `heyfood register` first.",
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
            // Write ahead before the consuming POST. A hard process exit or
            // Ctrl-C after dispatch therefore leaves durable evidence that the
            // old grant must not be retried.
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
            // A rotated refresh token is server-accepted state. Persist the whole
            // bundle before allowing session re-exchange to consume it.
            if refresh.replace(&auth).is_err() {
                return Err(uncertain_one_shot(
                    "channel_refresh_persistence_outcome_uncertain",
                    "The channel credential rotated, but it could not be saved. Stop and contact hello.food support for manual credential recovery; do not retry.",
                ));
            }
        }
    }

    let credentials = match credential_store
        .load()
        .await
        .map_err(heyfood_bin::OneShotError::from)?
    {
        Some(credentials) => credentials,
        None => {
            // Registration predates the rotating-session store. Seed it once
            // from the complete authorization bundle, then rotate only here.
            credential_store
                .initialize(&auth.session)
                .map_err(heyfood_bin::OneShotError::from)?;
            auth.session.clone()
        }
    };
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
    let ensure_session =
        EnsureSession::new(service.clone(), credential_store, Arc::new(NativeClock));
    let stdin = read_command_stdin(&command)?;
    heyfood_bin::execute_qualified_one_shot(
        service.as_ref(),
        &ensure_session,
        SessionSnapshot {
            credentials,
            reconciliation_required,
        },
        output_mode,
        command,
        &stdin,
        cancellation,
    )
    .await
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
        Command::Ask(arguments)
        | Command::Reply(arguments)
        | Command::Log(arguments)
        | Command::Item(arguments) => arguments.text.is_empty(),
        Command::Grocery {
            command: heyfood_cli::GroceryCommand::Confirm(_),
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
        "login_required" => Some("Run `heyfood register` and retry."),
        "phase2_parity_pending" | "command_not_available" => Some("Run `heyfood --help`."),
        "session_cancelled_before_dispatch" => Some("Run the command again when ready."),
        "authorization_scope_upgrade_required" => {
            Some("Run `heyfood login` to approve the expanded grant, then retry.")
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
            command: heyfood_cli::GroceryCommand::List | heyfood_cli::GroceryCommand::Export(_),
        } => &["grocery:read"],
        Command::Grocery { .. } => &["grocery:read", "grocery:write"],
        Command::Health {
            command: heyfood_cli::HealthCommand::Status | heyfood_cli::HealthCommand::Show,
        } => &["health:read"],
        Command::Health { .. } => &["integrations:manage"],
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

async fn login(arguments: heyfood_cli::LoginArgs, machine: bool) -> ExitCode {
    let result = login_inner(arguments, machine).await;
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
) -> Result<RegistrationResultDocument, RegistrationError> {
    let paths = NativePaths::discover().map_err(platform_error)?;
    let auth_store = NativeAuthStore::open(paths.config_dir()).map_err(platform_error)?;
    let session_store = NativeSessionStore::open(paths.config_dir()).map_err(platform_error)?;
    let current = auth_store
        .load_reconciling_authorization(&session_store)
        .map_err(platform_error)?
        .ok_or_else(|| RegistrationError {
            code: "login_required",
            public_message:
                "No prior native account is available to reauthorize. Run `heyfood register` first."
                    .into(),
            retryable: false,
            outcome_uncertain: false,
        })?;
    let (service_url, policy) = service_url()?;
    let client = RegistrationClient::new(service_url, policy)?;
    let authorization = client
        .start_device_reauthorization(&current.channel.scope)
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
    let outcome = client
        .complete_device_authorization(
            authorization,
            current.channel.device_id.clone(),
            arguments.timeout(),
            cancellation,
        )
        .await;
    signal.abort();
    let outcome = outcome?;
    if outcome.credentials.session.account_id != current.session.account_id {
        return Err(RegistrationError {
            code: "reauthorization_account_conflict",
            public_message: "The approved account does not match the connected account. Existing credentials were not changed.".into(),
            retryable: false,
            outcome_uncertain: false,
        });
    }
    auth_store
        .replace_authorization_bundle(&current, &outcome.credentials, &session_store)
        .map_err(|_| RegistrationError {
            code: "reauthorization_persistence_outcome_uncertain",
            public_message: "The expanded grant completed, but both native credential stores could not be committed. Stop and reconcile native account state before retrying.".into(),
            retryable: false,
            outcome_uncertain: true,
        })?;
    Ok(RegistrationResultDocument::completed(
        outcome.profile_status,
    ))
}

async fn register(arguments: heyfood_cli::RegisterArgs, machine: bool) -> ExitCode {
    let result = register_inner(arguments, machine).await;
    match result {
        Ok(document) => match heyfood_cli::render_registration_success(&document, machine) {
            Ok(output) => {
                print!("{output}");
                ExitCode::SUCCESS
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

async fn register_inner(
    arguments: heyfood_cli::RegisterArgs,
    machine: bool,
) -> Result<RegistrationResultDocument, RegistrationError> {
    let paths = NativePaths::discover().map_err(platform_error)?;
    let auth_store = NativeAuthStore::open(paths.config_dir()).map_err(platform_error)?;
    if auth_store.load().map_err(platform_error)?.is_some() {
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

    eprintln!(
        "Open this URL to continue: {}",
        authorization.verification_uri
    );
    eprintln!("Approval code: {}", authorization.user_code);
    io::stderr().flush().ok();

    // JSON mode is noninteractive by contract and therefore never launches a
    // browser. Human mode launches best-effort after publishing a copyable URL.
    if !machine
        && !arguments.no_browser
        && let Ok(destination) = BrowserUrl::parse(&authorization.verification_uri, policy)
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
        .complete_device_registration(authorization, device_id, arguments.timeout(), cancellation)
        .await;
    signal.abort();
    let outcome = outcome?;

    // Persist only after OAuth, app-session exchange, and contract validation
    // all succeed. The owner-only atomic store retains both grants together.
    auth_store.initialize(&outcome.credentials).map_err(|_| RegistrationError {
        code: "registration_persistence_outcome_uncertain",
        public_message: "The account was connected, but native credentials could not be saved. Do not retry registration until account state is reconciled.".into(),
        retryable: false,
        outcome_uncertain: true,
    })?;
    NativeSessionStore::open(paths.config_dir())
        .and_then(|store| store.initialize(&outcome.credentials.session))
        .map_err(|_| RegistrationError {
            code: "registration_persistence_outcome_uncertain",
            public_message: "The account was connected, but its rotating session could not be initialized. Do not retry registration until account state is reconciled.".into(),
            retryable: false,
            outcome_uncertain: true,
        })?;
    if arguments.no_onboard && outcome.profile_status != heyfood_core::ProfileStatus::Ready {
        eprintln!("Dietary onboarding was deferred. Your account remains connected.");
    }
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
            Some("Run `heyfood register` to start a fresh request.")
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
            Some("Do not start another registration attempt until account state is reconciled.")
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
