//! Native Clap surface and leaf renderers. Network and persistence decisions
//! remain in the composition/application layers.

#![forbid(unsafe_code)]

use std::io;
use std::time::Duration;

use clap::{CommandFactory, Parser, Subcommand};
use heyfood_core::ProfileStatus;
use serde::Serialize;

/// The package version shared by the native workspace.
pub const VERSION: &str = heyfood_core::VERSION;

#[derive(Clone, Debug, Parser, PartialEq)]
#[command(
    name = "heyfood",
    version,
    about = "hello.food for your terminal.",
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Emit exactly one machine-readable JSON value on stdout.
    #[arg(long, global = true)]
    pub json: bool,

    /// Deprecated alias for --json.
    #[arg(long, global = true)]
    pub raw: bool,

    /// Disable decorative branding.
    #[arg(long, global = true)]
    pub no_banner: bool,

    /// Print safe request lifecycle diagnostics to stderr.
    #[arg(long, global = true)]
    pub verbose: bool,

    /// Never prompt for missing local input.
    #[arg(long, global = true)]
    pub no_input: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Clone, Debug, PartialEq, Subcommand)]
pub enum Command {
    /// Create and connect a hello.food account.
    Register(RegisterArgs),
    /// Print shell completion source.
    Completion {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

#[derive(Clone, Debug, PartialEq, clap::Args)]
pub struct RegisterArgs {
    /// Use device-code authorization. This is the native launch transport.
    #[arg(long)]
    pub device: bool,

    /// Print the approval URL without opening a browser.
    #[arg(long)]
    pub no_browser: bool,

    /// Maximum seconds to wait for approval.
    #[arg(long, default_value_t = 600, value_parser = clap::value_parser!(u64).range(1..=1800))]
    pub timeout: u64,

    /// Connect the account without starting dietary onboarding.
    #[arg(long)]
    pub no_onboard: bool,
}

impl RegisterArgs {
    #[must_use]
    pub const fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout)
    }
}

impl Cli {
    #[must_use]
    pub const fn machine_output(&self) -> bool {
        self.json || self.raw
    }

    #[must_use]
    pub fn parse_env() -> Self {
        <Self as Parser>::parse()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RegistrationResultDocument {
    pub schema_version: u16,
    pub authenticated: bool,
    pub account_outcome: Option<String>,
    pub profile_status: ProfileStatus,
    pub next_command: String,
}

impl RegistrationResultDocument {
    #[must_use]
    pub fn completed(profile_status: ProfileStatus) -> Self {
        Self {
            schema_version: 1,
            authenticated: true,
            account_outcome: None,
            profile_status,
            next_command: if profile_status == ProfileStatus::Ready {
                "heyfood chat".into()
            } else {
                "heyfood onboard".into()
            },
        }
    }
}

#[derive(Serialize)]
struct ErrorEnvelope<'a> {
    ok: bool,
    error: ErrorBody<'a>,
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<&'a str>,
}

pub fn render_registration_success(
    document: &RegistrationResultDocument,
    machine: bool,
) -> Result<String, serde_json::Error> {
    if machine {
        serde_json::to_string(document).map(|value| format!("{value}\n"))
    } else {
        let next = &document.next_command;
        Ok(format!(
            "Your hello.food account is connected.\nNext: {next}\n"
        ))
    }
}

pub fn render_error(
    kind: &str,
    message: &str,
    hint: Option<&str>,
    machine: bool,
) -> Result<String, serde_json::Error> {
    if machine {
        let envelope = ErrorEnvelope {
            ok: false,
            error: ErrorBody {
                kind,
                message,
                hint,
            },
        };
        serde_json::to_string(&envelope).map(|value| format!("{value}\n"))
    } else {
        let hint = hint.map_or_else(String::new, |value| format!("\n{value}"));
        Ok(format!("heyfood error: {message}{hint}\n"))
    }
}

pub fn write_completions(shell: clap_complete::Shell, writer: &mut impl io::Write) {
    let mut command = Cli::command();
    clap_complete::generate(shell, &mut command, "heyfood", writer);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_accepts_machine_flags_after_the_command() {
        let cli =
            Cli::try_parse_from(["heyfood", "register", "--device", "--no-browser", "--json"])
                .unwrap();
        assert!(cli.machine_output());
        assert!(matches!(
            cli.command,
            Some(Command::Register(RegisterArgs {
                device: true,
                no_browser: true,
                ..
            }))
        ));
    }

    #[test]
    fn registration_json_is_one_ansi_free_value() {
        let rendered = render_registration_success(
            &RegistrationResultDocument::completed(ProfileStatus::Missing),
            true,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["authenticated"], true);
        assert_eq!(value["account_outcome"], serde_json::Value::Null);
        assert_eq!(value["profile_status"], "missing");
        assert_eq!(value["next_command"], "heyfood onboard");
        assert!(!rendered.contains("\u{1b}"));
        assert!(rendered.ends_with('\n'));
    }

    #[test]
    fn error_json_matches_the_public_envelope() {
        let rendered = render_error(
            "registration_unavailable",
            "Registration is disabled.",
            Some("Try again later."),
            true,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["type"], "registration_unavailable");
        assert_eq!(value["error"]["hint"], "Try again later.");
    }
}
