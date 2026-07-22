//! Classic, ANSI, and JSON command-line presentation.

#![forbid(unsafe_code)]

use std::fmt::Write as _;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use heyfood_core::{
    GroceryDecisionWire, GroceryItemStateWire, GroceryListWire, GroceryMutationProposalWire,
    HealthContextWire, HealthFreshnessStatus, HealthProvider, ProfileStatus, terminal_safe_text,
};
use serde::Serialize;
use serde_json::{Value, json};

/// The package version shared by the native workspace.
pub const VERSION: &str = heyfood_core::VERSION;

#[derive(Clone, Debug, Parser)]
#[command(
    name = "heyfood",
    version = VERSION,
    about = "hello.food for your terminal.",
    disable_help_subcommand = true
)]
pub struct CommandLine {
    /// Emit exactly one ANSI-free JSON value on stdout.
    #[arg(long, global = true, conflicts_with = "raw")]
    pub json: bool,

    /// Deprecated alias for --json.
    #[arg(long, global = true, hide = true, conflicts_with = "json")]
    pub raw: bool,

    /// Disable ANSI styling.
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Disable decorative branding.
    #[arg(long, global = true)]
    pub no_banner: bool,

    /// Print privacy-safe request diagnostics to stderr.
    #[arg(long, global = true)]
    pub verbose: bool,

    /// Never prompt for missing local input.
    #[arg(long, global = true)]
    pub no_input: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

impl CommandLine {
    #[must_use]
    pub const fn output_mode(&self, stdout_is_terminal: bool) -> OutputMode {
        if self.json || self.raw {
            OutputMode::Json
        } else if self.no_color || !stdout_is_terminal {
            OutputMode::HumanPlain
        } else {
            OutputMode::HumanAnsi
        }
    }

    pub fn command_tree() -> clap::Command {
        Self::command()
    }

    #[must_use]
    pub const fn machine_output(&self) -> bool {
        self.json || self.raw
    }

    #[must_use]
    pub fn parse_env() -> Self {
        <Self as Parser>::parse()
    }
}

/// Compatibility name used by the integrated native composition root.
pub type Cli = CommandLine;

#[derive(Clone, Debug, Subcommand)]
pub enum Command {
    /// Ask the hosted agent a one-shot question.
    Ask(AskArgs),
    /// Reply to an explicit conversation ID.
    Reply(AskArgs),
    /// Run classic line-oriented chat.
    #[command(hide = true)]
    Chat(LegacyArgs),
    /// Log a meal through the hosted agent.
    Log(LogArgs),
    /// Assess a menu or food item.
    Item(ItemArgs),
    /// Display the daily meal summary.
    #[command(hide = true)]
    Daily(LegacyArgs),
    /// Display a dietary profile.
    #[command(hide = true)]
    Profile(LegacyArgs),
    /// Complete dietary onboarding; retained for parity but implemented in Phase 4.
    #[command(hide = true)]
    Onboard(LegacyArgs),
    /// Authenticate or expand authorization for an existing account.
    Login(LoginArgs),
    /// Create and connect a hello.food account.
    Register(RegisterArgs),
    /// Revoke the local/server session.
    #[command(hide = true)]
    Logout(LegacyArgs),
    /// Show session status.
    #[command(hide = true)]
    Status(LegacyArgs),
    /// Run safe diagnostics.
    #[command(hide = true)]
    Doctor(LegacyArgs),
    /// Search restaurants.
    #[command(hide = true)]
    Search(LegacyArgs),
    /// Fetch a restaurant menu.
    #[command(hide = true)]
    Menu(LegacyArgs),
    /// Compatibility alias for menu lookup.
    #[command(hide = true)]
    GetMenu(LegacyArgs),
    /// Request recommendations.
    #[command(hide = true)]
    Recommend(LegacyArgs),
    /// Manage the active Grocery list.
    Grocery {
        #[command(subcommand)]
        command: GroceryCommand,
    },
    /// Read health context and manage provider integrations.
    Health {
        #[command(subcommand)]
        command: HealthCommand,
    },
    #[command(hide = true)]
    Recipes {
        #[command(subcommand)]
        command: RecipesCommand,
    },
    #[command(hide = true)]
    Location {
        #[command(subcommand)]
        command: LocationCommand,
    },
    #[command(hide = true)]
    Context {
        #[command(subcommand)]
        command: ContextCommand,
    },
    #[command(hide = true)]
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    #[command(hide = true)]
    Members {
        #[command(subcommand)]
        command: MembersCommand,
    },
    #[command(hide = true)]
    Household {
        #[command(subcommand)]
        command: HouseholdCommand,
    },
    #[command(hide = true)]
    Conversation {
        #[command(subcommand)]
        command: ConversationCommand,
    },
    #[command(hide = true)]
    Voice {
        #[command(subcommand)]
        command: VoiceCommand,
    },
    #[command(hide = true)]
    Account {
        #[command(subcommand)]
        command: AccountCommand,
    },
    #[command(hide = true)]
    Channels {
        #[command(subcommand)]
        command: ChannelsCommand,
    },
    /// Print shell completion syntax.
    Completion {
        #[arg(value_enum)]
        shell: CompletionShell,
    },
}

#[derive(Clone, Debug, Args)]
pub struct AskArgs {
    /// Text submitted to the hosted agent.
    #[arg(value_name = "TEXT", num_args = 0..)]
    pub text: Vec<String>,

    /// Continue a specific conversation.
    #[arg(long)]
    pub conversation_id: Option<String>,

    /// Latitude for location-aware requests.
    #[arg(
        long = "lat",
        alias = "latitude",
        requires = "longitude",
        allow_hyphen_values = true,
        value_parser = parse_latitude
    )]
    pub latitude: Option<f64>,

    /// Longitude for location-aware requests.
    #[arg(
        long = "lng",
        alias = "longitude",
        requires = "latitude",
        allow_hyphen_values = true,
        value_parser = parse_longitude
    )]
    pub longitude: Option<f64>,
}

impl AskArgs {
    #[must_use]
    pub fn prompt(&self) -> String {
        self.text.join(" ")
    }
}

#[derive(Clone, Debug, Args)]
pub struct LogArgs {
    /// Meal text submitted to the hosted agent.
    #[arg(value_name = "MEAL", num_args = 0..)]
    pub meal: Vec<String>,

    /// Optional meal category.
    #[arg(long = "type", value_enum)]
    pub meal_type: Option<MealType>,

    /// Household member name/id, `me`, or `everyone`.
    #[arg(long = "for", value_name = "MEMBER")]
    pub checking_for: Option<String>,
}

impl LogArgs {
    #[must_use]
    pub fn meal_text(&self) -> String {
        self.meal.join(" ")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum MealType {
    Breakfast,
    Lunch,
    Dinner,
    Snack,
}

impl MealType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Breakfast => "breakfast",
            Self::Lunch => "lunch",
            Self::Dinner => "dinner",
            Self::Snack => "snack",
        }
    }
}

#[derive(Clone, Debug, Args)]
pub struct ItemArgs {
    /// Food or menu item to evaluate.
    #[arg(value_name = "ITEM", num_args = 1..)]
    pub name: Vec<String>,

    /// Restaurant context.
    #[arg(long, short = 'r')]
    pub restaurant: Option<String>,

    /// Restaurant index from the last search.
    #[arg(long, conflicts_with = "restaurant")]
    pub at: Option<String>,
}

impl ItemArgs {
    #[must_use]
    pub fn item_name(&self) -> String {
        self.name.join(" ")
    }
}

fn parse_latitude(value: &str) -> Result<f64, String> {
    parse_coordinate(value, -90.0, 90.0, "latitude")
}

fn parse_longitude(value: &str) -> Result<f64, String> {
    parse_coordinate(value, -180.0, 180.0, "longitude")
}

fn parse_coordinate(value: &str, minimum: f64, maximum: f64, label: &str) -> Result<f64, String> {
    let coordinate = value
        .parse::<f64>()
        .map_err(|_| format!("{label} must be a number"))?;
    if !coordinate.is_finite() || !(minimum..=maximum).contains(&coordinate) {
        return Err(format!(
            "{label} must be finite and between {minimum} and {maximum}"
        ));
    }
    Ok(coordinate)
}

/// Compatibility placeholder for Phase 2 command-topology inventory. These
/// commands remain fail-closed until their application use case is ported.
#[derive(Clone, Debug, Default, Args)]
pub struct LegacyArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub arguments: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Args)]
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

#[derive(Clone, Debug, PartialEq, Args)]
pub struct LoginArgs {
    /// Use device-code authorization. This is the native launch transport.
    #[arg(long)]
    pub device: bool,

    /// Print the approval URL without opening a browser.
    #[arg(long)]
    pub no_browser: bool,

    /// Maximum seconds to wait for approval.
    #[arg(long, default_value_t = 600, value_parser = clap::value_parser!(u64).range(1..=1800))]
    pub timeout: u64,
}

impl LoginArgs {
    #[must_use]
    pub const fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout)
    }
}

#[derive(Clone, Debug, Subcommand)]
pub enum GroceryCommand {
    /// Read the active list without creating or replacing it.
    List,
    /// Prepare an add-items mutation; never commits during preparation.
    Add(GroceryAddArgs),
    /// Prepare a remove-items mutation using stable IDs or fresh list indexes.
    Remove(GroceryReferencesArgs),
    /// Prepare an item-state mutation.
    State(GroceryStateArgs),
    /// Export a list in a server-defined format.
    Export(GroceryExportArgs),
    /// Accept or cancel one server-signed proposal read from stdin.
    Confirm(GroceryConfirmArgs),
}

#[derive(Clone, Debug, Args)]
pub struct GroceryVersionArgs {
    #[arg(long, value_name = "UUID")]
    pub list_id: String,
    #[arg(long, value_name = "VERSION", value_parser = clap::value_parser!(u64).range(1..))]
    pub version: u64,
}

#[derive(Clone, Debug, Args)]
pub struct GroceryAddArgs {
    #[command(flatten)]
    pub list: GroceryVersionArgs,
    #[arg(required = true, value_name = "ITEM")]
    pub items: Vec<String>,
    #[arg(long)]
    pub intended_for: Option<String>,
}

#[derive(Clone, Debug, Args)]
pub struct GroceryReferencesArgs {
    #[command(flatten)]
    pub list: GroceryVersionArgs,
    /// Stable item UUID or a fresh one-based index written as #N.
    #[arg(required = true, value_name = "ITEM")]
    pub items: Vec<String>,
}

#[derive(Clone, Debug, Args)]
pub struct GroceryStateArgs {
    #[command(flatten)]
    pub list: GroceryVersionArgs,
    #[arg(value_name = "ITEM")]
    pub item: String,
    #[arg(value_enum)]
    pub state: GroceryStateArgument,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum GroceryStateArgument {
    Active,
    Purchased,
    Dismissed,
}

impl From<GroceryStateArgument> for GroceryItemStateWire {
    fn from(value: GroceryStateArgument) -> Self {
        match value {
            GroceryStateArgument::Active => Self::Active,
            GroceryStateArgument::Purchased => Self::Purchased,
            GroceryStateArgument::Dismissed => Self::Dismissed,
        }
    }
}

#[derive(Clone, Debug, Args)]
pub struct GroceryExportArgs {
    #[arg(value_name = "UUID")]
    pub list_id: String,
    #[arg(long, value_enum, default_value_t = GroceryExportFormat::Markdown)]
    pub format: GroceryExportFormat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum GroceryExportFormat {
    Markdown,
    Text,
    Json,
}

impl GroceryExportFormat {
    #[must_use]
    pub const fn as_wire_value(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Text => "text",
            Self::Json => "json",
        }
    }
}

#[derive(Clone, Debug, Args)]
pub struct GroceryConfirmArgs {
    #[arg(long, value_enum)]
    pub decision: GroceryDecisionArgument,
    /// Read exactly one proposal JSON object from stdin. Tokens are never CLI arguments.
    #[arg(long, default_value_t = true)]
    pub proposal_stdin: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum GroceryDecisionArgument {
    Accept,
    Cancel,
}

impl From<GroceryDecisionArgument> for GroceryDecisionWire {
    fn from(value: GroceryDecisionArgument) -> Self {
        match value {
            GroceryDecisionArgument::Accept => Self::Accept,
            GroceryDecisionArgument::Cancel => Self::Cancel,
        }
    }
}

#[derive(Clone, Debug, Subcommand)]
pub enum HealthCommand {
    /// Show connection states without health values.
    Status,
    /// Read server-held H1 health context.
    Show,
    /// Begin a server-owned provider authorization.
    Connect(HealthProviderArgs),
    /// Request a server-side provider sync.
    Sync(HealthProviderArgs),
    /// Disconnect a provider after explicit confirmation.
    Disconnect(HealthDisconnectArgs),
}

#[derive(Clone, Debug, Args)]
pub struct HealthProviderArgs {
    #[arg(value_enum, default_value_t = HealthProviderArgument::Oura)]
    pub provider: HealthProviderArgument,
}

#[derive(Clone, Debug, Args)]
pub struct HealthDisconnectArgs {
    #[command(flatten)]
    pub provider: HealthProviderArgs,
    #[arg(long)]
    pub yes: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum HealthProviderArgument {
    Oura,
}

impl From<HealthProviderArgument> for HealthProvider {
    fn from(_: HealthProviderArgument) -> Self {
        Self::Oura
    }
}

macro_rules! legacy_subcommands {
    ($name:ident { $($(#[$meta:meta])* $variant:ident),+ $(,)? }) => {
        #[derive(Clone, Debug, Subcommand)]
        pub enum $name {
            $($(#[$meta])* $variant(LegacyArgs)),+
        }
    };
}

legacy_subcommands!(RecipesCommand {
    Search,
    Save,
    Saved
});
legacy_subcommands!(LocationCommand { Show, Set, Clear });
legacy_subcommands!(ContextCommand {
    List,
    Show,
    Use,
    Set
});
legacy_subcommands!(ConfigCommand {
    Path,
    Show,
    Validate
});
legacy_subcommands!(MembersCommand { List });
legacy_subcommands!(HouseholdCommand {
    List,
    Current,
    Use,
    Label
});
legacy_subcommands!(ConversationCommand {
    List,
    Resume,
    Clear
});
legacy_subcommands!(VoiceCommand {
    Devices,
    Status,
    Set,
    Reset
});
legacy_subcommands!(AccountCommand { Delete });
legacy_subcommands!(ChannelsCommand { List, Disconnect });

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum CompletionShell {
    Bash,
    Elvish,
    Fish,
    PowerShell,
    Zsh,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputMode {
    HumanAnsi,
    HumanPlain,
    Json,
}

impl OutputMode {
    #[must_use]
    pub const fn ansi(self) -> bool {
        matches!(self, Self::HumanAnsi)
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
            // Classic chat and native onboarding are not implemented yet.
            // Never send a newly connected user to a placeholder command.
            next_command: "heyfood ask \"What can I eat?\"".into(),
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
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    outcome_uncertain: bool,
}

pub fn render_registration_success(
    document: &RegistrationResultDocument,
    machine: bool,
) -> Result<String, serde_json::Error> {
    if machine {
        serde_json::to_string(document).map(|value| format!("{value}\n"))
    } else {
        Ok(format!(
            "Your hello.food account is connected.\nNext: {}\n",
            document.next_command
        ))
    }
}

pub fn render_error(
    kind: &str,
    message: &str,
    hint: Option<&str>,
    machine: bool,
) -> Result<String, serde_json::Error> {
    render_error_with_outcome(kind, message, hint, machine, false)
}

pub fn render_error_with_outcome(
    kind: &str,
    message: &str,
    hint: Option<&str>,
    machine: bool,
    outcome_uncertain: bool,
) -> Result<String, serde_json::Error> {
    if machine {
        let envelope = ErrorEnvelope {
            ok: false,
            error: ErrorBody {
                kind,
                message,
                hint,
                outcome_uncertain,
            },
        };
        serde_json::to_string(&envelope).map(|value| format!("{value}\n"))
    } else {
        let hint = hint.map_or_else(String::new, |value| format!("\n{value}"));
        Ok(format!("heyfood error: {message}{hint}\n"))
    }
}

pub fn render_json<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    let mut output = serde_json::to_string(value)?;
    output.push('\n');
    debug_assert!(!output.contains('\u{1b}'));
    Ok(output)
}

#[must_use]
pub fn error_document(kind: &str, message: &str, uncertain: bool) -> Value {
    json!({
        "ok": false,
        "error": {
            "kind": terminal_safe_text(kind),
            "message": terminal_safe_text(message),
            "outcome_uncertain": uncertain
        }
    })
}

#[must_use]
pub fn render_grocery_list(list: &GroceryListWire, mode: OutputMode) -> String {
    if mode == OutputMode::Json {
        return render_json(list).expect("Grocery list DTO is serializable");
    }
    let mut output = String::new();
    let title = terminal_safe_text(&list.title);
    if mode.ansi() {
        let _ = writeln!(
            output,
            "\u{1b}[1m{title}\u{1b}[0m  version {}",
            list.version
        );
    } else {
        let _ = writeln!(output, "{title}  version {}", list.version);
    }
    if list.items.is_empty() {
        output.push_str("No grocery items.\n");
        return output;
    }
    for (index, item) in list.items.iter().enumerate() {
        let name = terminal_safe_text(&item.requested_name);
        let state = match item.state {
            GroceryItemStateWire::Active => "active",
            GroceryItemStateWire::Purchased => "purchased",
            GroceryItemStateWire::Dismissed => "dismissed",
        };
        let intended = item
            .intended_for
            .as_deref()
            .map(terminal_safe_text)
            .map(|value| format!(" for {value}"))
            .unwrap_or_default();
        let _ = writeln!(output, "{}. {name}{intended} [{state}]", index + 1);
        if let Some(safety) = &item.safety {
            let status = serde_json::to_value(safety.status)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "unable_to_evaluate".into());
            let _ = writeln!(output, "   ingredient screening: {status}");
            let _ = writeln!(output, "   {}", terminal_safe_text(&safety.label_hint));
        }
    }
    output
}

#[must_use]
pub fn render_grocery_proposal(proposal: &GroceryMutationProposalWire, mode: OutputMode) -> String {
    if mode == OutputMode::Json {
        return render_json(proposal).expect("Grocery proposal DTO is serializable");
    }
    let operation = serde_json::to_value(proposal.operation)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "grocery_mutation".into());
    format!(
        "Prepared {operation}; expires at {}. Review the structured preview and explicitly accept or cancel.\n",
        terminal_safe_text(&proposal.expires_at)
    )
}

#[must_use]
pub fn render_health_context(context: &HealthContextWire, mode: OutputMode) -> String {
    if mode == OutputMode::Json {
        return render_json(context).expect("Health context DTO is serializable");
    }
    let mut output = String::new();
    let status = match context.status {
        HealthFreshnessStatus::Connected => "connected",
        HealthFreshnessStatus::Stale => "stale",
        HealthFreshnessStatus::NotConnected => "not connected",
    };
    let _ = writeln!(output, "Health context: {status}");
    if let Some(provider) = &context.provider {
        let _ = writeln!(output, "Provider: {}", terminal_safe_text(provider));
    }
    if let Some(hours) = context.data_freshness_hours {
        let _ = writeln!(output, "Freshness: {hours} hours");
    }
    for (label, value) in [
        ("Sleep", context.sleep_avg),
        ("Readiness", context.readiness_avg),
        ("Activity", context.activity_avg),
        ("Steps", context.steps_avg),
        ("Active calories", context.active_calories_avg),
    ] {
        if let Some(value) = value {
            let _ = writeln!(output, "{label}: {value}");
        }
    }
    output
}

#[must_use]
pub fn render_agent_result(document: &Value, mode: OutputMode) -> String {
    if mode == OutputMode::Json {
        return render_json(document).expect("agent result is serializable JSON");
    }
    let mut output = String::new();
    if let Some(message) = ["message", "text", "response"]
        .into_iter()
        .find_map(|key| document.get(key).and_then(Value::as_str))
    {
        let _ = writeln!(output, "{}", terminal_safe_text(message));
    }
    if let Some(choice_document) = document.get("choices").and_then(Value::as_object)
        && let Some(choices) = choice_document.get("choices").and_then(Value::as_array)
        && !choices.is_empty()
    {
        if !output.is_empty() {
            output.push('\n');
        }
        let allow_multiple = choice_document
            .get("allow_multiple")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let _ = writeln!(
            output,
            "{}",
            if allow_multiple {
                "Choose one or more:"
            } else {
                "Choose one:"
            }
        );
        for (index, choice) in choices.iter().filter_map(Value::as_str).enumerate() {
            let _ = writeln!(output, "{}. {}", index + 1, terminal_safe_text(choice));
        }
    }
    if output.is_empty() {
        let encoded = serde_json::to_string(document).unwrap_or_else(|_| "{}".into());
        let _ = writeln!(output, "{}", terminal_safe_text(&encoded));
    }
    output
}

#[must_use]
pub fn render_item_result(document: &Value, mode: OutputMode) -> String {
    if mode == OutputMode::Json {
        return render_json(document).expect("item result is serializable JSON");
    }
    let item = document
        .get("item_name")
        .and_then(Value::as_str)
        .unwrap_or("Item");
    let status = document
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .replace('_', " ");
    let summary = document
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("No summary returned.");
    let mut output = format!(
        "{}  {}\n{}\n",
        terminal_safe_text(item),
        terminal_safe_text(&status),
        terminal_safe_text(summary)
    );
    if let Some(confidence) = document.get("confidence").and_then(Value::as_f64) {
        let _ = writeln!(output, "Confidence: {confidence:.2}");
    }
    if let Some(member) = [
        "member_name",
        "member_label",
        "member_id",
        "affected_member",
    ]
    .into_iter()
    .find_map(|key| document.get(key).and_then(Value::as_str))
    {
        let _ = writeln!(output, "Applies to: {}", terminal_safe_text(member));
    }
    append_item_conflicts(&mut output, document);
    for (heading, keys) in [
        ("Ask staff", &["questions_to_ask"][..]),
        ("Uncertainties", &["uncertainties", "uncertainty"][..]),
        (
            "Possible modifications",
            &["modifications", "suggested_modifications"][..],
        ),
        ("Alternatives", &["alternatives"][..]),
    ] {
        if let Some(values) = keys.iter().find_map(|key| {
            document
                .get(*key)
                .and_then(Value::as_array)
                .filter(|values| !values.is_empty())
        }) {
            let _ = writeln!(output, "\n{heading}");
            for value in values.iter().filter_map(Value::as_str) {
                let _ = writeln!(output, "- {}", terminal_safe_text(value));
            }
        }
    }
    if let Some(provenance) = ["provenance", "source"]
        .into_iter()
        .find_map(|key| document.get(key).and_then(Value::as_str))
    {
        let _ = writeln!(output, "Source: {}", terminal_safe_text(provenance));
    }
    if let Some(freshness) = ["menu_freshness", "freshness"]
        .into_iter()
        .find_map(|key| document.get(key).and_then(Value::as_str))
    {
        let _ = writeln!(output, "Freshness: {}", terminal_safe_text(freshness));
    }
    output
}

fn append_item_conflicts(output: &mut String, document: &Value) {
    let Some(conflicts) = document
        .get("conflicts")
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty())
    else {
        return;
    };
    let _ = writeln!(output, "\nConflicts");
    for conflict in conflicts.iter().filter_map(Value::as_object) {
        let ingredient = conflict
            .get("ingredient")
            .and_then(Value::as_str)
            .unwrap_or("Unknown ingredient");
        let reason = conflict.get("reason").and_then(Value::as_str).unwrap_or("");
        let _ = writeln!(
            output,
            "{}: {}",
            terminal_safe_text(ingredient),
            terminal_safe_text(reason)
        );
    }
}

pub fn generate_completion(shell: CompletionShell) -> Vec<u8> {
    let mut command = CommandLine::command();
    let mut output = Vec::new();
    match shell {
        CompletionShell::Bash => clap_complete::generate(
            clap_complete::shells::Bash,
            &mut command,
            "heyfood",
            &mut output,
        ),
        CompletionShell::Elvish => clap_complete::generate(
            clap_complete::shells::Elvish,
            &mut command,
            "heyfood",
            &mut output,
        ),
        CompletionShell::Fish => clap_complete::generate(
            clap_complete::shells::Fish,
            &mut command,
            "heyfood",
            &mut output,
        ),
        CompletionShell::PowerShell => clap_complete::generate(
            clap_complete::shells::PowerShell,
            &mut command,
            "heyfood",
            &mut output,
        ),
        CompletionShell::Zsh => clap_complete::generate(
            clap_complete::shells::Zsh,
            &mut command,
            "heyfood",
            &mut output,
        ),
    }
    output
}

pub fn write_completions(shell: CompletionShell, writer: &mut impl io::Write) {
    let _ = writer.write_all(&generate_completion(shell));
}

/// Validated input source reserved for confirmation proposal documents.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProposalInput {
    Stdin,
    File(PathBuf),
}

#[cfg(test)]
mod registration_tests {
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
        let value: Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["authenticated"], true);
        assert_eq!(value["account_outcome"], Value::Null);
        assert_eq!(value["profile_status"], "missing");
        assert_eq!(value["next_command"], "heyfood ask \"What can I eat?\"");
        assert!(!rendered.contains('\u{1b}'));
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
        let value: Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["type"], "registration_unavailable");
        assert_eq!(value["error"]["hint"], "Try again later.");
        assert!(value["error"].get("outcome_uncertain").is_none());
    }

    #[test]
    fn uncertain_error_is_explicit_for_machine_consumers() {
        let rendered = render_error_with_outcome(
            "session_exchange_outcome_uncertain",
            "Reconcile before retrying.",
            None,
            true,
            true,
        )
        .unwrap();
        let value: Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["error"]["outcome_uncertain"], true);
    }

    #[test]
    fn agent_human_output_preserves_partial_text_and_choices() {
        let rendered = render_agent_result(
            &json!({
                "text": "Try soup.",
                "choices": {
                    "choices": ["First", "Second"],
                    "allow_multiple": false
                }
            }),
            OutputMode::HumanPlain,
        );
        for line in ["Try soup.", "Choose one:", "1. First", "2. Second"] {
            assert!(rendered.lines().any(|rendered| rendered == line));
        }
    }

    #[test]
    fn item_human_output_uses_the_dedicated_python_compatible_shape() {
        let rendered = render_item_result(
            &json!({
                "item_name": "veggie burger",
                "status": "compatible",
                "summary": "This item fits the profile.",
                "confidence": 0.95,
                "member_name": "Sarah"
            }),
            OutputMode::HumanPlain,
        );
        for line in [
            "veggie burger  compatible",
            "This item fits the profile.",
            "Confidence: 0.95",
            "Applies to: Sarah",
        ] {
            assert!(rendered.lines().any(|rendered| rendered == line));
        }
    }
}
