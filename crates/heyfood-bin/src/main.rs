//! Native heyfood executable composition root.

#![forbid(unsafe_code)]

use std::io::{self, Write};
use std::process::ExitCode;

use heyfood_agent_runtime::{RegistrationClient, RegistrationError};
use heyfood_application::BrowserPort;
use heyfood_cli::{Cli, Command, RegistrationResultDocument};
use heyfood_core::{BrowserUrl, NetworkPolicy, OperationId, ServiceUrl};
use heyfood_platform::{NativeAuthStore, NativeBrowser, NativePaths};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> ExitCode {
    #[cfg(feature = "native-credentials")]
    if let Some(outcome) = heyfood_platform::run_credential_broker_if_requested() {
        return outcome;
    }

    let cli = Cli::parse_env();
    let machine = cli.machine_output();
    if cli.raw {
        eprintln!("--raw is deprecated; use --json.");
    }
    match cli.command {
        Some(Command::Completion { shell }) => {
            heyfood_cli::write_completions(shell, &mut io::stdout());
            ExitCode::SUCCESS
        }
        Some(Command::Register(arguments)) => register(arguments, machine).await,
        None => {
            // Bare interactive TUI remains a later phase. Native one-shot
            // commands are dispatched above instead of being held behind it.
            eprintln!("{}", heyfood_bin::QUALIFICATION_MESSAGE);
            ExitCode::from(78)
        }
    }
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
            ),
        },
        Err(error) => failure(
            error.code,
            &error.public_message,
            registration_hint(error.code),
            machine,
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
            public_message:
                "A hello.food account is already connected. Use status or log out first.".into(),
            retryable: false,
        });
    }

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
    })?;
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
    auth_store
        .initialize(&outcome.credentials)
        .map_err(platform_error)?;
    Ok(RegistrationResultDocument::completed(
        outcome.profile_status,
    ))
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
    }
}

fn registration_hint(code: &str) -> Option<&'static str> {
    match code {
        "registration_unavailable" => Some("Registration is not enabled on this service yet."),
        "account_already_connected" => {
            Some("Native logout/status arrives in the next command slice.")
        }
        "cancelled" | "authorization_expired" => {
            Some("Run `heyfood register` to start a fresh request.")
        }
        "auth_contract_error" => {
            Some("Update heyfood and retry. If it continues, check hello.food service status.")
        }
        _ => None,
    }
}

fn failure(kind: &str, message: &str, hint: Option<&str>, machine: bool) -> ExitCode {
    let output = heyfood_cli::render_error(kind, message, hint, machine)
        .unwrap_or_else(|_| "heyfood error: Could not render the requested operation.\n".into());
    if machine {
        print!("{output}");
    } else {
        eprint!("{output}");
    }
    ExitCode::FAILURE
}
