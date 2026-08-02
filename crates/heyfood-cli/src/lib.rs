//! Classic, ANSI, and JSON command-line presentation.

#![forbid(unsafe_code)]

use std::fmt::Write as _;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use heyfood_application::household_evaluation::contains_private_household_identifier;
use heyfood_application::{
    GroceryDisplayList, GroceryExclusions, LogoutOutcome, MenuWatchList, MenuWatchSnapshot,
    UNRENDERABLE_AGENT_RESULT_MESSAGE, household_evaluation_document, household_menu_document,
    render_household_evaluation, render_household_menu,
};
use heyfood_core::{
    GroceryDecisionWire, GroceryItemStateWire, GroceryMutationOperationWire,
    GroceryMutationProposalWire, GroceryMutationResultWire, GroceryMutationStatusWire,
    GrocerySafetyStatus, HealthContextWire, HealthFreshnessStatus, HealthProvider, ProfileStatus,
    WatchWeekday, terminal_safe_text,
};
use serde::Serialize;
use serde_json::{Value, json};

const UNPRESENTABLE_ITEM_RESULT_MESSAGE: &str =
    "hey.food returned item guidance this version can’t display safely. Ask about the item again.";
const UNPRESENTABLE_GROCERY_LIST_MESSAGE: &str = "hey.food returned a Grocery list this version can’t display safely. Refresh the list and try again.";
const UNPRESENTABLE_GROCERY_PROPOSAL_MESSAGE: &str =
    "hey.food returned a Grocery change this version can’t display safely. Nothing changed.";

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
    /// Inspect and configure the supported agent integration.
    Agent {
        #[command(subcommand)]
        command: Option<AgentCommand>,
    },
    /// Run the local, bounded Model Context Protocol server.
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    /// Ask the hosted agent a one-shot question.
    Ask(AskArgs),
    /// Reply to an explicit conversation ID.
    Reply(AskArgs),
    /// Open the native interactive terminal.
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
    /// Open guided dietary onboarding in the native TUI.
    #[command(hide = true)]
    Onboard(LegacyArgs),
    /// Connect an existing account, or replace this machine's authorization.
    Login(LoginArgs),
    /// Create and connect a hello.food account.
    Register(RegisterArgs),
    /// Revoke this device's hosted authority and clear local credentials.
    Logout(LogoutArgs),
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
        command: Option<GroceryCommand>,
    },
    /// Health integrations are deferred from the supported v0.7.0 contract.
    #[command(
        hide = true,
        about = "Health integrations are deferred from the supported v0.7.0 contract."
    )]
    Health {
        #[command(subcommand)]
        command: HealthCommand,
    },
    /// Schedule and manage recurring restaurant Menu Watch subscriptions.
    Watch {
        #[command(subcommand)]
        command: Option<MenuWatchCommand>,
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

#[derive(Clone, Debug, Subcommand)]
pub enum AgentCommand {
    /// Describe the exact installed agent contract as deterministic JSON.
    Describe(AgentDiscoveryArgs),
    /// Print the embedded integration or safety guide.
    Guide(AgentGuideArgs),
    /// Print one embedded JSON Schema.
    Schema(AgentSchemaArgs),
    /// Run credential-free, network-free local integration diagnostics.
    Doctor(AgentDiscoveryArgs),
    /// Plan or install the Agent Skill and read-only MCP registration.
    Setup(AgentSetupArgs),
    /// Remove only an exact receipt-bound skill and MCP registration.
    Uninstall(AgentUninstallArgs),
}

#[derive(Clone, Debug, Eq, PartialEq, Args)]
pub struct AgentDiscoveryArgs {
    /// Explicit discovery schema; v1 remains the compatibility default.
    #[arg(
        long,
        value_name = "1|2",
        default_value_t = 1,
        value_parser = clap::value_parser!(u16).range(1..=2)
    )]
    pub schema_version: u16,
}

impl Default for AgentDiscoveryArgs {
    fn default() -> Self {
        Self { schema_version: 1 }
    }
}

#[derive(Clone, Debug, Subcommand)]
pub enum McpCommand {
    /// Serve the six read/discovery tools over newline-delimited stdio.
    Serve,
}

#[derive(Clone, Debug, Args)]
pub struct AgentGuideArgs {
    /// Output format for the embedded guide.
    #[arg(long, value_enum, default_value_t = AgentGuideFormat::Markdown)]
    pub format: AgentGuideFormat,

    /// Print the normative safety contract instead of the integration guide.
    #[arg(long)]
    pub safety: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum AgentGuideFormat {
    /// Markdown suitable for an agent instruction context.
    #[default]
    Markdown,
}

#[derive(Clone, Debug, Args)]
pub struct AgentSchemaArgs {
    /// List every public embedded schema and its digest.
    #[arg(long, conflicts_with = "schema")]
    pub list: bool,

    /// Public schema name or exact schema identifier.
    #[arg(value_name = "NAME_OR_ID")]
    pub schema: Option<String>,
}

#[derive(Clone, Debug, Args)]
pub struct AgentSetupArgs {
    /// Agent host to configure.
    #[arg(long, value_enum)]
    pub target: AgentSetupTarget,

    /// Installation scope; project scope requires --project-root.
    #[arg(long, value_enum)]
    pub scope: AgentSetupScope,

    /// Explicit absolute Git worktree root for project scope.
    #[arg(long, value_name = "ABSOLUTE_PATH")]
    pub project_root: Option<PathBuf>,

    /// Apply the displayed plan. Without this flag setup is a dry-run.
    #[arg(long, conflicts_with = "dry_run")]
    pub apply: bool,

    /// Exact SHA-256 printed by the reviewed dry-run; required with --apply.
    #[arg(long, value_name = "SHA256", requires = "apply")]
    pub plan_sha256: Option<String>,

    /// Explicitly request the default non-mutating dry-run.
    #[arg(long, conflicts_with = "apply")]
    pub dry_run: bool,

    /// Replace only an exact prior receipt-bound heyfood installation.
    #[arg(long)]
    pub replace: bool,
}

#[derive(Clone, Debug, Args)]
pub struct AgentUninstallArgs {
    /// Agent host to remove.
    #[arg(long, value_enum)]
    pub target: AgentSetupTarget,

    /// Installation scope; project scope requires --project-root.
    #[arg(long, value_enum)]
    pub scope: AgentSetupScope,

    /// Explicit absolute Git worktree root for project scope.
    #[arg(long, value_name = "ABSOLUTE_PATH")]
    pub project_root: Option<PathBuf>,

    /// Apply the displayed plan. Without this flag uninstall is a dry-run.
    #[arg(long, conflicts_with = "dry_run")]
    pub apply: bool,

    /// Exact SHA-256 printed by the reviewed dry-run; required with --apply.
    #[arg(long, value_name = "SHA256", requires = "apply")]
    pub plan_sha256: Option<String>,

    /// Explicitly request the default non-mutating dry-run.
    #[arg(long, conflicts_with = "apply")]
    pub dry_run: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum AgentSetupTarget {
    Codex,
    Claude,
    All,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum AgentSetupScope {
    User,
    Project,
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
    #[arg(long)]
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

#[derive(Clone, Debug, Default, Args)]
pub struct LogoutArgs {}

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
    #[command(name = "show", alias = "list")]
    List,
    /// Prepare an add-items mutation; never commits during preparation.
    Add(GroceryAddArgs),
    /// Prepare a remove-items mutation using stable IDs or fresh list indexes.
    Remove(GroceryReferencesArgs),
    /// Prepare an item-state mutation.
    State(GroceryStateArgs),
    /// Read the account's canonical never-buy exclusions.
    Exclusions,
    /// Prepare a never-buy exclusion change; never commits during preparation.
    Never(GroceryExclusionArgs),
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

#[derive(Clone, Debug, Args)]
pub struct GroceryExclusionArgs {
    #[command(flatten)]
    pub list: GroceryVersionArgs,
    #[arg(value_name = "ITEM")]
    pub item: String,
    /// Remove this item from the never-buy list instead of adding it.
    #[arg(long)]
    pub remove: bool,
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
    /// Write sensitive dietary/member annotations to an owner-only file. Required for human output.
    #[arg(long, value_name = "FILE")]
    pub out: Option<PathBuf>,
    /// Atomically replace an existing regular file.
    #[arg(long, requires = "out")]
    pub overwrite: bool,
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
    /// Retained for future compatibility; unavailable in v0.7.0.
    #[command(hide = true)]
    Status,
    /// Retained for future compatibility; unavailable in v0.7.0.
    #[command(hide = true)]
    Show,
    /// Retained for future compatibility; unavailable in v0.7.0.
    #[command(hide = true)]
    Connect(HealthProviderArgs),
    /// Retained for future compatibility; unavailable in v0.7.0.
    #[command(hide = true)]
    Sync(HealthProviderArgs),
    /// Retained for future compatibility; unavailable in v0.7.0.
    #[command(hide = true)]
    Disconnect(HealthDisconnectArgs),
}

#[derive(Clone, Debug, Subcommand)]
pub enum MenuWatchCommand {
    /// List the current account's Menu Watch subscriptions.
    #[command(name = "show", alias = "list")]
    List,
    /// Create a recurring Menu Watch subscription.
    #[command(name = "add", alias = "create")]
    Add(MenuWatchAddArgs),
    /// Remove one Menu Watch subscription.
    #[command(name = "remove", alias = "rm", alias = "delete")]
    Remove(MenuWatchRemoveArgs),
}

#[derive(Clone, Debug, Args)]
pub struct MenuWatchAddArgs {
    /// Internal restaurant UUID returned by restaurant discovery.
    #[arg(value_name = "RESTAURANT_ID")]
    pub restaurant_id: String,
    /// Restaurant-local weekday for the recurring check.
    #[arg(long, value_enum)]
    pub weekday: WatchWeekdayArgument,
    /// Restaurant-local hour in 24-hour time.
    #[arg(long, value_name = "HOUR", value_parser = clap::value_parser!(u8).range(0..=23))]
    pub hour: u8,
    /// Record a quick-read event when a scheduled run finds a real change.
    #[arg(long)]
    pub notify: bool,
    /// Explicit menu URL to verify and watch.
    #[arg(long, value_name = "URL")]
    pub menu_url: Option<String>,
    /// Confirm that the selected menu URL belongs to this restaurant.
    #[arg(long)]
    pub confirm_menu_url: bool,
    /// IANA timezone override when restaurant coordinates are insufficient.
    #[arg(long, value_name = "IANA_TIMEZONE")]
    pub tz: Option<String>,
}

#[derive(Clone, Debug, Args)]
pub struct MenuWatchRemoveArgs {
    #[arg(value_name = "WATCH_ID")]
    pub watch_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum WatchWeekdayArgument {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl WatchWeekdayArgument {
    #[must_use]
    pub const fn as_contract_value(self) -> u8 {
        match self {
            Self::Monday => 0,
            Self::Tuesday => 1,
            Self::Wednesday => 2,
            Self::Thursday => 3,
            Self::Friday => 4,
            Self::Saturday => 5,
            Self::Sunday => 6,
        }
    }
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
            next_command: "heyfood".into(),
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

pub fn render_logout_success(
    document: &LogoutOutcome,
    machine: bool,
) -> Result<String, serde_json::Error> {
    if machine {
        serde_json::to_string(document).map(|value| format!("{value}\n"))
    } else if !document.local_credentials_cleared {
        Ok("Logout is incomplete. Native household cleanup will resume automatically; some local credentials may remain until repair completes.\n".into())
    } else if document.remote_complete {
        Ok("Logged out.\n".into())
    } else {
        Ok("Logged out locally. Some server cleanup could not be confirmed; remaining sessions will expire automatically.\n".into())
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
pub fn render_grocery_list(list: &GroceryDisplayList, mode: OutputMode) -> String {
    if mode == OutputMode::Json {
        return render_json(list).expect("Grocery list DTO is serializable");
    }
    let private_member_ids = list
        .items
        .iter()
        .flat_map(|item| {
            item.intended_for.iter().map(String::as_str).chain(
                item.safety
                    .iter()
                    .flat_map(|safety| safety.member_flags.iter())
                    .map(|flag| flag.member_id.as_str()),
            )
        })
        .collect::<Vec<_>>();
    if grocery_list_human_fields_are_private(list, &private_member_ids) {
        return format!("{UNPRESENTABLE_GROCERY_LIST_MESSAGE}\n");
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
            .map(grocery_member_inline_label)
            .map(|value| format!(" for {value}"))
            .unwrap_or_default();
        let quantity = match (item.quantity, item.unit.as_deref(), item.package_quantity) {
            (Some(value), Some(unit), _) => {
                format!(" · {value} {}", terminal_safe_text(unit))
            }
            (Some(value), None, _) => format!(" · {value}"),
            (None, _, Some(packages)) => format!(" · {packages} package(s)"),
            _ => String::new(),
        };
        let _ = writeln!(
            output,
            "{}. {name}{intended}{quantity} [{state}]  id:{}",
            index + 1,
            terminal_safe_text(&item.id)
        );
        if !item.sources.is_empty() {
            let provenance = item
                .sources
                .iter()
                .map(|source| {
                    let kind = terminal_safe_text(&source.source_type);
                    source
                        .source_ref
                        .as_deref()
                        .map_or(kind.clone(), |reference| {
                            format!("{kind}:{}", terminal_safe_text(reference))
                        })
                })
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(output, "   source: {provenance}");
        }
        if let Some(safety) = &item.safety {
            let _ = writeln!(
                output,
                "   ingredient screening: {}",
                grocery_safety_label(safety.status)
            );
            for flag in &safety.member_flags {
                let intended_marker = item
                    .intended_for
                    .as_deref()
                    .filter(|member| *member == flag.member_id)
                    .map_or("", |_| " · intended");
                let _ = writeln!(
                    output,
                    "   • {}: {}{intended_marker}",
                    grocery_member_heading_label(&flag.member_id),
                    grocery_safety_label(flag.status)
                );
                if let Some(reason) = flag.reason.as_deref() {
                    let _ = writeln!(output, "     {}", terminal_safe_text(reason));
                }
                if !flag.substitutions.is_empty() {
                    let substitutions = flag
                        .substitutions
                        .iter()
                        .map(|value| terminal_safe_text(value))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let _ = writeln!(output, "     try: {substitutions}");
                }
            }
            let _ = writeln!(output, "   {}", terminal_safe_text(&safety.label_hint));
        }
    }
    if contains_known_household_identifier(&output, &private_member_ids) {
        format!("{UNPRESENTABLE_GROCERY_LIST_MESSAGE}\n")
    } else {
        output
    }
}

const fn grocery_safety_label(status: GrocerySafetyStatus) -> &'static str {
    match status {
        GrocerySafetyStatus::GenerallySafer => "generally safer",
        GrocerySafetyStatus::Risky => "risky",
        GrocerySafetyStatus::Avoid => "avoid",
        GrocerySafetyStatus::UnableToEvaluate => "unable to evaluate",
    }
}

fn grocery_member_inline_label(member_id: &str) -> &'static str {
    if member_id == "_self" {
        "you"
    } else {
        "a household member"
    }
}

fn grocery_member_heading_label(member_id: &str) -> &'static str {
    if member_id == "_self" {
        "You"
    } else {
        "Household member"
    }
}

fn grocery_list_human_fields_are_private(
    list: &GroceryDisplayList,
    private_member_ids: &[&str],
) -> bool {
    grocery_human_text_is_private(&list.title, private_member_ids)
        || list.items.iter().any(|item| {
            grocery_human_text_is_private(&item.requested_name, private_member_ids)
                || item
                    .unit
                    .as_deref()
                    .is_some_and(|value| grocery_human_text_is_private(value, private_member_ids))
                || item.safety.as_ref().is_some_and(|safety| {
                    grocery_human_text_is_private(&safety.label_hint, private_member_ids)
                        || safety.member_flags.iter().any(|flag| {
                            flag.reason.as_deref().is_some_and(|value| {
                                grocery_human_text_is_private(value, private_member_ids)
                            }) || flag.substitutions.iter().any(|value| {
                                grocery_human_text_is_private(value, private_member_ids)
                            })
                        })
                })
        })
}

fn grocery_human_text_is_private(value: &str, private_member_ids: &[&str]) -> bool {
    contains_private_household_identifier(value)
        || contains_known_household_identifier(value, private_member_ids)
}

fn collect_declared_household_ids(value: &Value) -> Option<Vec<&str>> {
    collect_declared_household_ids_from_pending(vec![(None, value, 0)])
}

fn collect_declared_household_ids_from_object(
    object: &serde_json::Map<String, Value>,
) -> Option<Vec<&str>> {
    collect_declared_household_ids_from_pending(
        object
            .iter()
            .map(|(key, value)| (Some(key.as_str()), value, 0))
            .collect(),
    )
}

fn collect_declared_household_ids_from_pending<'a>(
    mut pending: Vec<(Option<&'a str>, &'a Value, usize)>,
) -> Option<Vec<&'a str>> {
    const MAX_IDENTITY_NESTING: usize = 32;
    const MAX_IDENTITY_VALUES: usize = 4_096;

    let mut identifiers = Vec::new();
    let mut visited = 0usize;
    while let Some((key, value, depth)) = pending.pop() {
        visited = visited.checked_add(1)?;
        if visited > MAX_IDENTITY_VALUES || depth > MAX_IDENTITY_NESTING {
            return None;
        }
        if key.is_some_and(|key| {
            matches!(
                key,
                "member_id" | "intended_for" | "affected_member" | "active_member_id"
            )
        }) {
            match value {
                Value::String(identifier)
                    if !identifier.is_empty() && identifier.trim() == identifier =>
                {
                    identifiers.push(identifier.as_str());
                }
                Value::Null => {}
                _ => return None,
            }
        }
        match value {
            Value::Object(object) => {
                for (key, value) in object {
                    pending.push((Some(key.as_str()), value, depth + 1));
                }
            }
            Value::Array(values) => {
                pending.extend(values.iter().map(|value| (None, value, depth + 1)));
            }
            _ => {}
        }
    }
    Some(identifiers)
}

fn contains_known_household_identifier(value: &str, private_member_ids: &[&str]) -> bool {
    const MIN_PRIVATE_ID_PREFIX_CHARACTERS: usize = 8;

    private_member_ids.iter().any(|identifier| {
        if identifier.is_empty() {
            return false;
        }
        if *identifier == "_self" {
            return value.contains("_self");
        }
        if value.match_indices(*identifier).any(|(start, matched)| {
            let before = value[..start].chars().next_back();
            let end = start + matched.len();
            let after = value[end..].chars().next();
            before.is_none_or(|character| !household_identifier_character(character))
                && after.is_none_or(|character| !household_identifier_character(character))
        }) {
            return true;
        }
        let compact_identifier = identifier
            .chars()
            .filter(|character| !character.is_whitespace() && *character != '_')
            .collect::<String>();
        if compact_identifier.is_empty() {
            return false;
        }
        let compact_value = value
            .chars()
            .filter(|character| !character.is_whitespace() && *character != '_')
            .collect::<String>();
        compact_value.contains(&compact_identifier) || {
            let compact_value = compact_value.to_ascii_lowercase();
            let compact_identifier = compact_identifier.to_ascii_lowercase();
            compact_value.contains(&compact_identifier)
                || (compact_identifier.chars().count() >= MIN_PRIVATE_ID_PREFIX_CHARACTERS
                    && compact_value.contains(
                        &compact_identifier
                            .chars()
                            .take(MIN_PRIVATE_ID_PREFIX_CHARACTERS)
                            .collect::<String>(),
                    ))
        }
    })
}

const fn household_identifier_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
}

fn grocery_proposal_human_fields_are_private(items: &[Value], private_member_ids: &[&str]) -> bool {
    items.iter().any(|item| {
        ["name", "requested_name", "canonical_name"]
            .into_iter()
            .find_map(|key| item.get(key).and_then(Value::as_str))
            .is_some_and(|value| grocery_human_text_is_private(value, private_member_ids))
            || item.get("safety").is_some_and(|safety| {
                safety
                    .get("status")
                    .and_then(Value::as_str)
                    .is_some_and(|value| grocery_human_text_is_private(value, private_member_ids))
                    || safety
                        .get("label_hint")
                        .and_then(Value::as_str)
                        .is_some_and(|value| {
                            grocery_human_text_is_private(value, private_member_ids)
                        })
                    || safety
                        .get("member_flags")
                        .and_then(Value::as_array)
                        .is_some_and(|flags| {
                            flags.iter().any(|flag| {
                                flag.get("status")
                                    .and_then(Value::as_str)
                                    .is_some_and(|value| {
                                        grocery_human_text_is_private(value, private_member_ids)
                                    })
                                    || flag.get("reason").and_then(Value::as_str).is_some_and(
                                        |value| {
                                            grocery_human_text_is_private(value, private_member_ids)
                                        },
                                    )
                                    || flag
                                        .get("substitutions")
                                        .and_then(Value::as_array)
                                        .is_some_and(|substitutions| {
                                            substitutions.iter().filter_map(Value::as_str).any(
                                                |value| {
                                                    grocery_human_text_is_private(
                                                        value,
                                                        private_member_ids,
                                                    )
                                                },
                                            )
                                        })
                            })
                        })
            })
    })
}

#[must_use]
pub fn render_grocery_exclusions(exclusions: &GroceryExclusions, mode: OutputMode) -> String {
    if mode == OutputMode::Json {
        return render_json(exclusions).expect("Grocery exclusions DTO is serializable");
    }
    if exclusions.exclusions.is_empty() {
        return "Never-buy list is empty.\n".into();
    }
    let mut output = String::from("Never buy\n");
    for exclusion in &exclusions.exclusions {
        let _ = writeln!(output, "• {}", terminal_safe_text(exclusion));
    }
    output
}

#[must_use]
pub fn render_grocery_proposal(proposal: &GroceryMutationProposalWire, mode: OutputMode) -> String {
    if mode == OutputMode::Json {
        return render_json(proposal).expect("Grocery proposal DTO is serializable");
    }
    let items = proposal
        .structured_preview
        .get("items")
        .and_then(Value::as_array);
    let Some(private_member_ids) =
        collect_declared_household_ids_from_object(&proposal.structured_preview)
    else {
        return format!("{UNPRESENTABLE_GROCERY_PROPOSAL_MESSAGE}\n");
    };
    if items
        .is_some_and(|items| grocery_proposal_human_fields_are_private(items, &private_member_ids))
    {
        return format!("{UNPRESENTABLE_GROCERY_PROPOSAL_MESSAGE}\n");
    }
    let operation = serde_json::to_value(proposal.operation)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "grocery_mutation".into());
    let mut output = format!("Review {operation}\n");
    if let Some(items) = proposal
        .structured_preview
        .get("items")
        .and_then(Value::as_array)
    {
        for (index, item) in items.iter().enumerate() {
            let name = ["name", "requested_name", "canonical_name"]
                .into_iter()
                .find_map(|key| item.get(key).and_then(Value::as_str))
                .map(terminal_safe_text)
                .unwrap_or_else(|| "item".into());
            let intended = item
                .get("intended_for")
                .and_then(Value::as_str)
                .map(grocery_member_inline_label)
                .map(|member| format!(" for {member}"))
                .unwrap_or_default();
            let _ = writeln!(output, "{}. {name}{intended}", index + 1);
            if let Some(safety) = item.get("safety") {
                if let Some(status) = safety.get("status").and_then(Value::as_str) {
                    let _ = writeln!(
                        output,
                        "   ingredient screening: {}",
                        terminal_safe_text(status).replace('_', " ")
                    );
                }
                if let Some(flags) = safety.get("member_flags").and_then(Value::as_array) {
                    for flag in flags {
                        let member = flag
                            .get("member_id")
                            .and_then(Value::as_str)
                            .map(grocery_member_heading_label)
                            .unwrap_or("Household member");
                        let status = flag
                            .get("status")
                            .and_then(Value::as_str)
                            .map(terminal_safe_text)
                            .unwrap_or_else(|| "unable to evaluate".into());
                        let _ = writeln!(output, "   • {member}: {status}");
                        if let Some(substitutions) = flag
                            .get("substitutions")
                            .and_then(Value::as_array)
                            .filter(|values| !values.is_empty())
                        {
                            let substitutions = substitutions
                                .iter()
                                .filter_map(Value::as_str)
                                .map(terminal_safe_text)
                                .collect::<Vec<_>>()
                                .join(", ");
                            if !substitutions.is_empty() {
                                let _ = writeln!(output, "     try: {substitutions}");
                            }
                        }
                    }
                }
                if let Some(hint) = safety.get("label_hint").and_then(Value::as_str) {
                    let _ = writeln!(output, "   {}", terminal_safe_text(hint));
                }
            }
        }
    }
    let _ = writeln!(
        output,
        "Expires: {}",
        terminal_safe_text(&proposal.expires_at)
    );
    output.push_str(
        "Nothing has changed. Use `--json` and pipe this proposal to `heyfood grocery confirm --decision accept|cancel`.\n",
    );
    if contains_known_household_identifier(&output, &private_member_ids) {
        format!("{UNPRESENTABLE_GROCERY_PROPOSAL_MESSAGE}\n")
    } else {
        output
    }
}

#[must_use]
pub fn render_grocery_mutation_result(
    result: &GroceryMutationResultWire,
    mode: OutputMode,
) -> String {
    if mode == OutputMode::Json {
        return render_json(result).expect("Grocery mutation result is serializable JSON");
    }
    let operation = match result.operation {
        GroceryMutationOperationWire::AddItems => "items added",
        GroceryMutationOperationWire::RemoveItems => "items removed",
        GroceryMutationOperationWire::UpdateItemState => "item status updated",
        GroceryMutationOperationWire::AddExclusion => "never-buy item added",
        GroceryMutationOperationWire::RemoveExclusion => "never-buy item removed",
    };
    match result.status {
        GroceryMutationStatusWire::Committed => {
            let mut output = format!("Grocery change confirmed: {operation}.\n");
            if let Some(list) = result.list.as_ref() {
                let _ = writeln!(
                    output,
                    "List version {} now has {} items.",
                    list.version,
                    list.items.len()
                );
            }
            if let Some(exclusions) = result.exclusions.as_ref() {
                let _ = writeln!(output, "Never-buy list now has {} items.", exclusions.len());
            }
            output
        }
        GroceryMutationStatusWire::Cancelled => {
            "Grocery change cancelled. Nothing changed.\n".into()
        }
    }
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
pub fn render_menu_watch(watch: &MenuWatchSnapshot, mode: OutputMode) -> String {
    if mode == OutputMode::Json {
        return render_json(watch).expect("Menu Watch DTO is serializable");
    }
    render_menu_watch_entry(watch, mode)
}

#[must_use]
pub fn render_menu_watch_list(watches: &MenuWatchList, mode: OutputMode) -> String {
    if mode == OutputMode::Json {
        return render_json(watches).expect("Menu Watch list DTO is serializable");
    }
    let mut output = if mode.ansi() {
        "\u{1b}[1mMenu Watch\u{1b}[0m\n".to_owned()
    } else {
        "Menu Watch\n".to_owned()
    };
    if watches.watches.is_empty() {
        output.push_str("No watched menus.\n");
        return output;
    }
    for watch in &watches.watches {
        output.push_str(&render_menu_watch_entry(watch, mode));
    }
    output
}

fn render_menu_watch_entry(watch: &MenuWatchSnapshot, mode: OutputMode) -> String {
    let weekday = weekday_label(watch.cadence.weekday);
    let status = if watch.active { "active" } else { "inactive" };
    let notification = if watch.notify {
        "change events enabled"
    } else {
        "change events disabled"
    };
    let mut output = String::new();
    let watch_id = watch.id.as_uuid().hyphenated().to_string();
    let restaurant_id = watch.restaurant_id.as_uuid().hyphenated().to_string();
    if mode.ansi() {
        let _ = writeln!(
            output,
            "\u{1b}[1m{weekday} {:02}:00\u{1b}[0m · {status}",
            watch.cadence.hour.get()
        );
    } else {
        let _ = writeln!(
            output,
            "{weekday} {:02}:00 · {status}",
            watch.cadence.hour.get()
        );
    }
    let _ = writeln!(output, "  watch: {watch_id}");
    let _ = writeln!(output, "  restaurant: {restaurant_id}");
    let _ = writeln!(output, "  timezone: {}", inline_terminal_text(&watch.tz));
    let _ = writeln!(
        output,
        "  next check: {}",
        inline_terminal_text(&watch.next_run_at)
    );
    let _ = writeln!(output, "  {notification}");
    if let Some(snapshot) = watch.last_snapshot_id.as_deref() {
        let _ = writeln!(
            output,
            "  baseline snapshot: {}",
            inline_terminal_text(snapshot)
        );
    } else {
        output.push_str("  awaiting first successful baseline\n");
    }
    if let Some(source) = watch.menu_url.as_deref() {
        let _ = writeln!(output, "  menu source: {}", inline_terminal_text(source));
    }
    if let Some(verdict) = watch.identity_verdict.as_deref() {
        let confidence = watch
            .identity_confidence
            .map(|value| format!(" · confidence {value:.3}"))
            .unwrap_or_default();
        let _ = writeln!(
            output,
            "  identity: {}{confidence}",
            inline_terminal_text(verdict)
        );
    }
    if let Some(reasoning) = watch.identity_reasoning.as_deref() {
        let _ = writeln!(
            output,
            "  identity evidence: {}",
            inline_terminal_text(reasoning)
        );
    }
    if watch.identity_confirmed == Some(true) {
        output.push_str("  identity source explicitly confirmed\n");
    }
    if let Some(change) = &watch.last_change {
        let _ = writeln!(
            output,
            "  last change: {}",
            inline_terminal_text(&change.changed_at)
        );
        let _ = writeln!(
            output,
            "    +{} added · -{} removed · {} modified",
            change.summary.added, change.summary.removed, change.summary.modified
        );
        let _ = writeln!(
            output,
            "    {} price increases · {} price decreases",
            change.summary.price_increases, change.summary.price_decreases
        );
        let _ = writeln!(
            output,
            "    snapshots: {} → {}",
            inline_terminal_text(&change.previous_snapshot_id),
            inline_terminal_text(&change.new_snapshot_id)
        );
    }
    output
}

fn inline_terminal_text(value: &str) -> String {
    terminal_safe_text(value)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

const fn weekday_label(weekday: WatchWeekday) -> &'static str {
    match weekday.get() {
        0 => "Monday",
        1 => "Tuesday",
        2 => "Wednesday",
        3 => "Thursday",
        4 => "Friday",
        5 => "Saturday",
        6 => "Sunday",
        _ => unreachable!(),
    }
}

#[must_use]
pub fn render_agent_result(document: &Value, mode: OutputMode) -> String {
    render_agent_result_with_private_household_ids(document, mode, &[])
}

/// Render an agent result while refusing any human presentation that echoes a
/// stable Household identifier known only to the local native roster. Machine
/// output remains the exact service document.
#[must_use]
pub fn render_agent_result_with_private_household_ids(
    document: &Value,
    mode: OutputMode,
    private_household_ids: &[&str],
) -> String {
    render_agent_result_with_private_authorities(document, mode, private_household_ids, &[])
}

/// Render an agent result while retaining choice-value privacy authority from
/// every streamed Choices event, including values no longer present in the
/// terminal document after a later Choices event replaces the visible card.
#[must_use]
pub fn render_agent_result_with_private_authorities(
    document: &Value,
    mode: OutputMode,
    private_household_ids: &[&str],
    retained_choice_values: &[&str],
) -> String {
    let output = render_agent_result_inner(document, mode);
    if mode == OutputMode::Json {
        return output;
    }
    let declared_household_ids = collect_declared_household_ids(document);
    let declared_choice_values = declared_choice_values(document);
    let mut all_private_household_ids = private_household_ids.to_vec();
    if let Some(declared_household_ids) = declared_household_ids.as_ref() {
        all_private_household_ids.extend(declared_household_ids.iter().copied());
    }
    let displayed_choice_count = document
        .get("choices")
        .and_then(Value::as_object)
        .and_then(|choices| choices.get("choices"))
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let all_choice_values = declared_choice_values
        .as_ref()
        .map(|values| {
            retained_choice_values
                .iter()
                .copied()
                .chain(values.iter().copied())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let visible_choice_values = all_choice_values
        .iter()
        .copied()
        .filter(|value| {
            value.parse::<usize>().map_or(true, |ordinal| {
                !(1..=displayed_choice_count).contains(&ordinal)
            })
        })
        .collect::<Vec<_>>();
    if declared_household_ids.is_none()
        || declared_choice_values.is_none()
        || agent_source_fields_echo_choice_values(document, &all_choice_values)
        || contains_known_household_identifier(&output, &visible_choice_values)
        || contains_private_household_identifier(&output)
        || contains_known_household_identifier(&output, &all_private_household_ids)
    {
        let message = if household_evaluation_document(document).is_some() {
            heyfood_application::UNPRESENTABLE_HOUSEHOLD_EVALUATION_MESSAGE
        } else if household_menu_document(document).is_some() {
            heyfood_application::household_menu::UNPRESENTABLE_HOUSEHOLD_MENU_MESSAGE
        } else {
            UNRENDERABLE_AGENT_RESULT_MESSAGE
        };
        return format!("{message}\n");
    }
    output
}

fn declared_choice_values(document: &Value) -> Option<Vec<&str>> {
    const MAX_CHOICE_DETAILS: usize = 4_096;

    let Some(choice_document) = document.get("choices").and_then(Value::as_object) else {
        return Some(Vec::new());
    };
    let Some(details) = choice_document.get("choice_details") else {
        return Some(Vec::new());
    };
    let details = details.as_array()?;
    if details.len() > MAX_CHOICE_DETAILS {
        return None;
    }
    let mut values = Vec::with_capacity(details.len());
    for detail in details {
        let detail = detail.as_object()?;
        let label = detail.get("label")?.as_str()?;
        let value = detail.get("value")?.as_str()?;
        if label.is_empty() || value.is_empty() || label.trim() != label || value.trim() != value {
            return None;
        }
        values.push(value);
    }
    Some(values)
}

fn agent_source_fields_echo_choice_values(document: &Value, values: &[&str]) -> bool {
    ["message", "text", "response"]
        .into_iter()
        .filter_map(|key| document.get(key).and_then(Value::as_str))
        .chain(
            document
                .get("choices")
                .and_then(Value::as_object)
                .and_then(|choices| choices.get("choices"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str),
        )
        .any(|value| contains_known_household_identifier(value, values))
}

fn render_agent_result_inner(document: &Value, mode: OutputMode) -> String {
    if mode == OutputMode::Json {
        return render_json(document).expect("agent result is serializable JSON");
    }
    let household_evaluation_candidate = household_evaluation_document(document).is_some();
    let household_evaluation = match render_household_evaluation(document) {
        Ok(evaluation) => evaluation,
        Err(error) => return format!("{error}\n"),
    };
    let household_menu = render_household_menu(document);
    let has_structured_household_presentation =
        household_evaluation.is_some() || household_menu.is_some();
    let mut output = String::new();
    if !has_structured_household_presentation
        && let Some(message) = ["message", "text", "response"]
            .into_iter()
            .find_map(|key| document.get(key).and_then(Value::as_str))
    {
        if household_evaluation_candidate && contains_private_household_identifier(message) {
            return format!(
                "{}\n",
                heyfood_application::UNPRESENTABLE_HOUSEHOLD_EVALUATION_MESSAGE
            );
        }
        let _ = writeln!(output, "{}", terminal_safe_text(message));
    }
    if let Some(evaluation) = household_evaluation {
        output.push_str(&evaluation);
    } else if let Some(menu) = household_menu {
        output.push_str(&menu);
    }
    if !has_structured_household_presentation
        && let Some(choice_document) = document.get("choices").and_then(Value::as_object)
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
                "Choose one or more"
            } else {
                "Choose one"
            }
        );
        for (index, choice) in choices.iter().filter_map(Value::as_str).enumerate() {
            let _ = writeln!(output, "{}  {}", index + 1, terminal_safe_text(choice));
        }
        let _ = writeln!(
            output,
            "In chat, enter a number. With ask/reply, send the choice text in the next turn."
        );
    }
    if output.is_empty() {
        let _ = writeln!(output, "{UNRENDERABLE_AGENT_RESULT_MESSAGE}");
    }
    output
}

#[must_use]
pub fn render_item_result(document: &Value, mode: OutputMode) -> String {
    if mode == OutputMode::Json {
        return render_json(document).expect("item result is serializable JSON");
    }
    let Some(private_member_ids) = collect_declared_household_ids(document) else {
        return format!("{UNPRESENTABLE_ITEM_RESULT_MESSAGE}\n");
    };
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
        terminal_safe_text(&item_status_label(&status)),
        terminal_safe_text(summary)
    );
    if let Some(confidence) = document.get("confidence").and_then(Value::as_f64) {
        let _ = writeln!(output, "Confidence: {confidence:.2}");
    }
    let member_id = ["member_id", "affected_member"]
        .into_iter()
        .find_map(|key| document.get(key).and_then(Value::as_str));
    let reviewed_label = ["member_name", "member_label"]
        .into_iter()
        .find_map(|key| document.get(key).and_then(Value::as_str))
        .filter(|label| {
            !contains_private_household_identifier(label)
                && !contains_known_household_identifier(label, &private_member_ids)
        });
    let member = reviewed_label.map(terminal_safe_text).or_else(|| {
        member_id.map(|identifier| {
            if identifier == "_self" {
                "You".to_owned()
            } else {
                "Household member".to_owned()
            }
        })
    });
    if let Some(member) = member {
        let _ = writeln!(output, "Applies to: {member}");
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
    if contains_private_household_identifier(&output)
        || contains_known_household_identifier(&output, &private_member_ids)
    {
        return format!("{UNPRESENTABLE_ITEM_RESULT_MESSAGE}\n");
    }
    output
}

fn item_status_label(value: &str) -> String {
    let normalized = value.trim().to_lowercase().replace(['-', ' '], "_");
    match normalized.as_str() {
        "safe" | "safer" | "generally_safe" | "generally_safer" => "Generally safer".into(),
        "compatible" => "Compatible".into(),
        "risky" | "risk" | "caution" | "needs_review" => "Risky".into(),
        "avoid" | "unsafe" => "Avoid".into(),
        "" | "unknown" | "unable" | "unable_to_evaluate" | "not_evaluated" => {
            "Unable to evaluate".into()
        }
        _ => "Unable to evaluate".into(),
    }
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
    let source = CommandLine::command();
    let mut command = clap::Command::new("heyfood");
    for argument in source.get_arguments() {
        command = command.arg(argument.clone());
    }
    for subcommand in source
        .get_subcommands()
        .filter(|subcommand| subcommand.get_name() != "health")
    {
        command = command.subcommand(subcommand.clone());
    }
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
        let cli = Cli::try_parse_from([
            "heyfood",
            "register",
            "--device",
            "--no-browser",
            "--no-input",
            "--json",
        ])
        .unwrap();
        assert!(cli.machine_output());
        assert!(cli.no_input);
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
        assert_eq!(value["next_command"], "heyfood");
        assert!(!rendered.contains('\u{1b}'));
        assert!(rendered.ends_with('\n'));
    }

    #[test]
    fn logout_is_public_and_renders_one_sanitized_json_value() {
        let cli = Cli::try_parse_from(["heyfood", "--json", "logout"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Logout(_))));
        let rendered = render_logout_success(&LogoutOutcome::already_logged_out(), true).unwrap();
        let value: Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["ok"], true);
        assert_eq!(value["remote_complete"], true);
        assert_eq!(value["local_credentials_cleared"], true);
        assert!(!rendered.contains('\u{1b}'));
        assert!(rendered.ends_with('\n'));
    }

    #[test]
    fn logout_human_output_distinguishes_remote_uncertainty() {
        assert_eq!(
            render_logout_success(&LogoutOutcome::already_logged_out(), false).unwrap(),
            "Logged out.\n"
        );
        assert!(
            render_logout_success(&LogoutOutcome::recovered_local_logout(), false)
                .unwrap()
                .starts_with("Logged out locally.")
        );
        let partial =
            LogoutOutcome::recovered_local_teardown(heyfood_application::HouseholdEraseOutcome {
                household_key_deleted: true,
                household_ciphertext_deleted: true,
                import_snapshot_deleted: true,
                legacy_source_retained: true,
                legacy_credentials_cleared: false,
                legacy_credentials_retained: true,
                local_credentials_cleared: false,
                outcome_uncertain: true,
            });
        assert!(
            render_logout_success(&partial, false)
                .unwrap()
                .starts_with("Logout is incomplete.")
        );
    }

    #[test]
    fn watch_human_output_preserves_source_identity_and_last_change() {
        let watch = MenuWatchSnapshot {
            id: heyfood_core::MenuWatchId::parse("00000000-0000-4000-8000-000000000010").unwrap(),
            restaurant_id: heyfood_core::RestaurantId::parse(
                "0c1cb790-0000-4000-8000-000000000000",
            )
            .unwrap(),
            cadence: heyfood_core::WatchCadenceWire {
                weekday: heyfood_core::WatchWeekday::new(3).unwrap(),
                hour: heyfood_core::WatchHour::new(9).unwrap(),
            },
            tz: "America/Chicago".into(),
            active: true,
            notify: true,
            next_run_at: "2026-07-30T14:00:00Z".into(),
            last_run_at: None,
            last_snapshot_id: Some("snapshot-new".into()),
            created_at: "2026-07-23T12:00:00Z".into(),
            menu_url: Some("https://ordering.example/abby\nforged".into()),
            identity_verdict: Some("verified".into()),
            identity_confidence: Some(0.97),
            identity_reasoning: Some("name and location matched\nforged".into()),
            identity_confirmed: Some(true),
            last_change: Some(heyfood_application::MenuWatchChangeEvent {
                changed_at: "2026-07-24T14:05:00Z".into(),
                previous_snapshot_id: "snapshot-old".into(),
                new_snapshot_id: "snapshot-new".into(),
                summary: heyfood_application::MenuWatchChangeSummary {
                    added: 17,
                    removed: 12,
                    modified: 50,
                    price_increases: 50,
                    price_decreases: 0,
                },
            }),
        };
        let rendered = render_menu_watch(&watch, OutputMode::HumanPlain);
        for expected in [
            "  menu source: https://ordering.example/abby forged",
            "  identity: verified · confidence 0.970",
            "  identity evidence: name and location matched forged",
            "  identity source explicitly confirmed",
            "  last change: 2026-07-24T14:05:00Z",
            "    +17 added · -12 removed · 50 modified",
            "    50 price increases · 0 price decreases",
            "    snapshots: snapshot-old → snapshot-new",
        ] {
            assert!(rendered.lines().any(|line| line == expected), "{rendered}");
        }
        assert!(!rendered.contains("\nforged"));
        assert!(!rendered.contains('\u{1b}'));
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
        for line in [
            "Try soup.",
            "Choose one",
            "1  First",
            "2  Second",
            "In chat, enter a number. With ask/reply, send the choice text in the next turn.",
        ] {
            assert!(rendered.lines().any(|rendered| rendered == line));
        }
    }

    #[test]
    fn agent_human_output_never_dumps_an_unrecognized_structured_result() {
        let document = json!({
            "structured": {
                "type": "future_menu_presentation",
                "sections": [{
                    "name": "Tea",
                    "items": [{
                        "item_id": "18fbb9d6-85a1-4e04-bd44-a8348507048c",
                        "name": "12 oz Chai Latte",
                        "price_cents": 450,
                        "safety": {
                            "_self": {
                                "level": "caution",
                                "reason": "Verify sweetness level."
                            }
                        }
                    }]
                }]
            }
        });

        let rendered = render_agent_result(&document, OutputMode::HumanPlain);
        assert_eq!(
            rendered.trim_end(),
            heyfood_application::household_menu::UNPRESENTABLE_HOUSEHOLD_MENU_MESSAGE
        );
        for protocol_fragment in ["item_id", "\"safety\"", "_self", "{", "}"] {
            assert!(!rendered.contains(protocol_fragment), "{rendered}");
        }

        let machine_output = render_agent_result(&document, OutputMode::Json);
        let decoded: Value = serde_json::from_str(&machine_output).unwrap();
        assert_eq!(decoded, document);
    }

    #[test]
    fn agent_household_evaluation_has_human_privacy_and_complete_json_parity() {
        let fixture: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/contracts/household-backend/v1/fixtures/household_evaluation/founding_scenario_maya_menu.json"
        )))
        .unwrap();
        let document = json!({
            "text": "Unfiltered prose for 3f1c9c2e-2f5a-4a5b-8f1e-9d2b7c6a4e01.",
            "structured_content": fixture["result"].clone()
        });

        let human = render_agent_result(&document, OutputMode::HumanPlain);
        for expected in [
            "Household evaluation at Bistro One",
            "Jordan: Generally safer",
            "Maya: Avoid",
        ] {
            assert!(human.contains(expected), "{human}");
        }
        for forbidden in [
            "3f1c9c2e-2f5a-4a5b-8f1e-9d2b7c6a4e01",
            "54aa3228a67d4e262d383d0cfba6be4f4c0c94f21f5d095f3127d00928586bcb",
            "stub-model-1",
            "dietary-rules-1",
            "member_annotations",
            "context_hash",
            "{\"",
            "Unfiltered prose",
        ] {
            assert!(!human.contains(forbidden), "{human}");
        }

        let machine = render_agent_result(&document, OutputMode::Json);
        assert_eq!(serde_json::from_str::<Value>(&machine).unwrap(), document);
        assert_eq!(
            serde_json::from_str::<Value>(&machine).unwrap()["structured_content"]["items"][0]["member_annotations"],
            fixture["result"]["items"][0]["member_annotations"]
        );
    }

    #[test]
    fn malformed_household_evaluation_hides_unreviewed_prose_but_json_stays_lossless() {
        let fixture: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/contracts/household-backend/v1/fixtures/household_evaluation/founding_scenario_maya_menu.json"
        )))
        .unwrap();
        let mut result = fixture["result"].clone();
        result["items"][0]["member_annotations"][1]
            .as_object_mut()
            .unwrap()
            .remove("label");
        let document = json!({
            "text": "This unreviewed prose must not survive.",
            "structured_content": result
        });

        let human = render_agent_result(&document, OutputMode::HumanPlain);
        assert_eq!(
            human.trim_end(),
            heyfood_application::UNPRESENTABLE_HOUSEHOLD_EVALUATION_MESSAGE
        );
        assert!(!human.contains("unreviewed"));
        assert!(!human.contains("3f1c9c2e"));

        let machine = render_agent_result(&document, OutputMode::Json);
        assert_eq!(serde_json::from_str::<Value>(&machine).unwrap(), document);
    }

    #[test]
    fn partial_household_evaluation_with_a_missing_required_field_fails_closed() {
        let fixture: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/contracts/household-backend/v1/fixtures/household_evaluation/founding_scenario_maya_menu.json"
        )))
        .unwrap();
        let mut result = fixture["result"].clone();
        result.as_object_mut().unwrap().remove("restaurant_name");
        let document = json!({
            "text": "Raw fallback for 3f1c9c2e-2f5a-4a5b-8f1e-9d2b7c6a4e01.",
            "structured_content": result
        });

        let human = render_agent_result(&document, OutputMode::HumanPlain);
        assert_eq!(
            human.trim_end(),
            heyfood_application::UNPRESENTABLE_HOUSEHOLD_EVALUATION_MESSAGE
        );
        assert!(!human.contains("Raw fallback"));
        assert!(!human.contains("3f1c9c2e"));
    }

    #[test]
    fn household_only_truncation_never_falls_back_to_model_prose() {
        let document = json!({
            "text": "Raw household fallback.",
            "structured_content": {
                "household": {
                    "member_count": 2
                }
            }
        });

        let human = render_agent_result(&document, OutputMode::HumanPlain);
        assert_eq!(
            human.trim_end(),
            heyfood_application::UNPRESENTABLE_HOUSEHOLD_EVALUATION_MESSAGE
        );
        assert!(!human.contains("Raw household fallback"));
    }

    #[test]
    fn owner_only_null_labels_preserve_legacy_human_output() {
        let fixture: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/contracts/household-backend/v1/fixtures/household_evaluation/founding_scenario_maya_menu.json"
        )))
        .unwrap();
        let mut result = fixture["result"].clone();
        result["items"][0]["status"] = json!("generally_safer");
        result["items"][0]["confidence"] = json!(0.95);
        result["items"][0]["summary"] = json!("No concerns.");
        for item in result["items"].as_array_mut().unwrap() {
            item["member_annotations"] = Value::Array(vec![item["member_annotations"][0].clone()]);
            item["member_annotations"][0]["label"] = Value::Null;
        }
        result["generally_safer"] = json!(["Garlic Noodles", "Steamed Jasmine Rice"]);
        result["avoid"] = json!([]);
        result["household"]["members"] =
            Value::Array(vec![result["household"]["members"][0].clone()]);
        result["household"]["member_count"] = json!(1);
        let document = json!({
            "text": "Legacy owner guidance.",
            "structured_content": result
        });

        assert_eq!(
            render_agent_result(&document, OutputMode::HumanPlain),
            "Legacy owner guidance.\n"
        );
    }

    #[test]
    fn agent_human_output_includes_the_complete_structured_household_menu() {
        let rendered = render_agent_result(
            &json!({
                "text": "Here are the current options.",
                "structured": {
                    "type": "household_menu",
                    "presentation": "full_menu",
                    "restaurant_name": "Abby Jane Bakeshop",
                    "source_url": "https://example.test/abby-jane",
                    "source_lineage": "hunter_toast_sites",
                    "menu_freshness": "Menu updated 2 hours ago",
                    "captured_at": "2026-07-26T17:27:14Z",
                    "freshness_hours": 2.0,
                    "requested_max_age_seconds": 86400,
                    "is_stale": false,
                    "sections": [{
                        "name": "Pastries",
                        "items": [
                            {
                                "name": "Butter Croissant",
                                "description": "Layers on layers.",
                                "price_cents": 500,
                                "composite_level": "caution",
                                "safety": {
                                    "member-jane": {
                                        "label": "Jane",
                                        "level": "caution",
                                        "reason": "Butter may aggravate symptoms.",
                                        "chips": ["Dairy"],
                                        "conflicts": []
                                    }
                                }
                            },
                            {
                                "name": "Chocolate Croissant",
                                "price_cents": 525,
                                "composite_level": "avoid"
                            }
                        ]
                    }]
                }
            }),
            OutputMode::HumanPlain,
        );

        assert!(rendered.starts_with("Current menu at Abby Jane Bakeshop\n"));
        assert!(!rendered.contains("Here are the current options."));
        for expected in [
            "Current menu at Abby Jane Bakeshop",
            "Source: https://example.test/abby-jane",
            "Freshness: Menu updated 2 hours ago",
            "Captured: 2026-07-26T17:27:14Z",
            "Menu source: Restaurant ordering page",
            "Pastries",
            "• Butter Croissant  $5.00  [caution]",
            "  Layers on layers.",
            "  Why for Jane (caution): Butter may aggravate symptoms.",
            "    Flags: Dairy",
            "• Chocolate Croissant  $5.25  [avoid]",
        ] {
            assert!(rendered.lines().any(|line| line == expected));
        }
        assert_eq!(rendered.matches("• ").count(), 2);
    }

    #[test]
    fn agent_human_output_includes_ranked_restaurant_recommendations() {
        let rendered = render_agent_result(
            &json!({
                "text": "I found several options that fit.",
                "structured": {
                    "type": "household_menu",
                    "restaurant_name": "Harbor Cafe",
                    "menu_freshness": "Menu updated 2 hours ago",
                    "source_url": "https://example.test/menu",
                    "member_summaries": [{
                        "member_id": "_self",
                        "label": null
                    }],
                    "sections": [{
                        "name": "Dinner",
                        "items": [{
                            "item_id": "item-1",
                            "name": "Grilled Fish",
                            "price_cents": 2400,
                            "safety": {
                                "_self": {
                                    "level": "safe",
                                    "reason": "No detected conflicts."
                                }
                            }
                        }]
                    }],
                    "agent_picks": {
                        "_self": [{
                            "item_id": "item-1",
                            "member_id": "_self",
                            "reason": "A simple preparation with no detected conflicts.",
                            "tag": "Top pick"
                        }]
                    }
                }
            }),
            OutputMode::HumanPlain,
        );

        for expected in [
            "Top picks at Harbor Cafe",
            "For you",
            "1. Grilled Fish  $24.00  [generally safer] · Top pick",
            "   A simple preparation with no detected conflicts.",
            "Ask about any pick, or say `show me the full menu` for every evaluated option.",
        ] {
            assert!(rendered.lines().any(|line| line == expected));
        }
        assert!(!rendered.contains("I found several options that fit."));
        assert!(!rendered.contains("_self"));
    }

    #[test]
    fn malformed_household_menu_never_falls_back_to_model_prose() {
        let member_id = "3f1c9c2e-2f5a-4a5b-8f1e-9d2b7c6a4e01";
        let document = json!({
            "text": format!("Unreviewed menu prose for {member_id}."),
            "structured": {
                "presentation": "full_menu",
                "sections": [{
                    "name": "Dinner",
                    "items": [{
                        "name": "Soup",
                        "composite_level": "avoid",
                        "safety": {
                            (member_id): {
                                "level": "future_status",
                                "reason": "Unreviewed reason."
                            }
                        }
                    }]
                }]
            }
        });

        let human = render_agent_result(&document, OutputMode::HumanPlain);
        assert_eq!(
            human.trim_end(),
            heyfood_application::household_menu::UNPRESENTABLE_HOUSEHOLD_MENU_MESSAGE
        );
        assert!(!human.contains("Unreviewed menu prose"));
        assert!(!human.contains(member_id));
    }

    #[test]
    fn roster_aware_agent_renderer_rejects_opaque_ids_omitted_from_the_payload_roster() {
        let member_id = "legacyOpaque7";
        let document = json!({
            "structured": {
                "type": "household_menu",
                "presentation": "full_menu",
                "restaurant_name": member_id,
                "sections": []
            }
        });
        let human = render_agent_result_with_private_household_ids(
            &document,
            OutputMode::HumanPlain,
            &[member_id],
        );
        assert_eq!(
            human.trim_end(),
            heyfood_application::household_menu::UNPRESENTABLE_HOUSEHOLD_MENU_MESSAGE
        );
        assert!(!human.contains(member_id));

        let machine = render_agent_result_with_private_household_ids(
            &document,
            OutputMode::Json,
            &[member_id],
        );
        assert_eq!(serde_json::from_str::<Value>(&machine).unwrap(), document);

        let generic = json!({"message": format!("Prepared for {member_id}.")});
        assert_eq!(
            render_agent_result_with_private_household_ids(
                &generic,
                OutputMode::HumanPlain,
                &[member_id],
            )
            .trim_end(),
            UNRENDERABLE_AGENT_RESULT_MESSAGE
        );
        let protocol_uuid = "3f1c9c2e-2f5a-4a5b-8f1e-9d2b7c6a4e01";
        let generic = json!({"message": format!("Prepared for {protocol_uuid}.")});
        assert_eq!(
            render_agent_result_with_private_household_ids(
                &generic,
                OutputMode::HumanPlain,
                &["_self"],
            )
            .trim_end(),
            UNRENDERABLE_AGENT_RESULT_MESSAGE
        );
    }

    #[test]
    fn roster_aware_agent_renderer_rejects_wrapped_case_and_whitespace_transforms() {
        let long_member_id = format!("legacy{}", "a".repeat(94));
        let wrapped_member_id = format!("{}\n{}", &long_member_id[..50], &long_member_id[50..]);
        for (member_id, rendered_member_id) in [
            (long_member_id.as_str(), wrapped_member_id.as_str()),
            ("legacy  Opaque7", "LEGACY OPAQUE7"),
        ] {
            let document = json!({
                "structured": {
                    "type": "household_menu",
                    "presentation": "full_menu",
                    "restaurant_name": rendered_member_id,
                    "sections": []
                }
            });
            let human = render_agent_result_with_private_household_ids(
                &document,
                OutputMode::HumanPlain,
                &[member_id],
            );
            assert_eq!(
                human.trim_end(),
                heyfood_application::household_menu::UNPRESENTABLE_HOUSEHOLD_MENU_MESSAGE
            );
            assert!(!human.contains(rendered_member_id));

            let machine = render_agent_result_with_private_household_ids(
                &document,
                OutputMode::Json,
                &[member_id],
            );
            assert_eq!(serde_json::from_str::<Value>(&machine).unwrap(), document);
        }
    }

    #[test]
    fn generic_agent_and_choice_documents_cannot_echo_their_declared_household_ids() {
        let member_id = "foreignOpaque7";
        for document in [
            json!({
                "message": format!("Prepared for {member_id}"),
                "member_id": member_id
            }),
            json!({
                "choices": {
                    "choices": [format!("Prepare for {member_id}")],
                    "choice_details": [{
                        "label": "A safe-looking sibling label",
                        "value": member_id
                    }],
                    "allow_multiple": false
                }
            }),
            json!({
                "message": "Prepared for 12345678",
                "choices": {
                    "choices": ["A household member"],
                    "choice_details": [{
                        "label": "A household member",
                        "value": "12345678"
                    }],
                    "allow_multiple": false
                }
            }),
            json!({
                "message": format!("Prepared for {member_id}"),
                "choices": {
                    "choices": ["Maya"],
                    "choice_details": [{"label": "Maya", "value": member_id}],
                    "allow_multiple": false
                }
            }),
        ] {
            let human = render_agent_result(&document, OutputMode::HumanPlain);
            assert_eq!(human.trim_end(), UNRENDERABLE_AGENT_RESULT_MESSAGE);
            assert!(!human.contains(member_id));

            let machine = render_agent_result(&document, OutputMode::Json);
            assert_eq!(serde_json::from_str::<Value>(&machine).unwrap(), document);
        }

        let ordinary_choice = json!({
            "choices": {
                "choices": ["First"],
                "choice_details": [{"label": "First", "value": "1"}],
                "allow_multiple": false
            }
        });
        assert!(render_agent_result(&ordinary_choice, OutputMode::HumanPlain).contains("1  First"));
    }

    #[test]
    fn agent_renderer_rejects_member_ids_transformed_by_menu_humanization() {
        let member_id = "legacy_opaque7";
        let document = json!({
            "structured": {
                "type": "household_menu",
                "presentation": "full_menu",
                "member_summaries": [{"member_id": member_id, "label": "Maya"}],
                "sections": [{
                    "name": "Dinner",
                    "items": [{
                        "name": "Soup",
                        "composite_level": "avoid",
                        "safety": {
                            (member_id): {
                                "label": "Maya",
                                "level": "avoid",
                                "reason": "Contains a restricted ingredient."
                            }
                        },
                        "allergen_detail": [{"allergen_label": member_id}]
                    }]
                }]
            }
        });

        let human = render_agent_result_with_private_household_ids(
            &document,
            OutputMode::HumanPlain,
            &[member_id],
        );
        assert_eq!(
            human.trim_end(),
            heyfood_application::household_menu::UNPRESENTABLE_HOUSEHOLD_MENU_MESSAGE
        );
        assert!(!human.contains(member_id));
        assert!(!human.contains("legacy opaque7"));

        let machine = render_agent_result_with_private_household_ids(
            &document,
            OutputMode::Json,
            &[member_id],
        );
        assert_eq!(serde_json::from_str::<Value>(&machine).unwrap(), document);
    }

    #[test]
    fn item_renderer_does_not_echo_unknown_status_or_transformed_member_identity() {
        let member_id = "legacyopaque7";
        let document = json!({
            "item_name": "soup",
            "status": member_id,
            "summary": "No reviewed summary.",
            "member_id": member_id
        });
        let human = render_item_result(&document, OutputMode::HumanPlain);
        assert!(!human.to_ascii_lowercase().contains(member_id));
        assert!(human.contains("Unable to evaluate"));
        assert!(human.contains("Applies to: Household member"));

        let machine = render_item_result(&document, OutputMode::Json);
        assert_eq!(serde_json::from_str::<Value>(&machine).unwrap(), document);
    }

    #[test]
    fn item_renderer_rejects_nested_content_that_echoes_a_declared_household_id() {
        let member_id = "foreignOpaque7";
        let document = json!({
            "item_name": "Soup",
            "status": "avoid",
            "summary": "Contains a restricted ingredient.",
            "conflicts": [{
                "member_id": member_id,
                "ingredient": member_id,
                "reason": "Restricted."
            }]
        });

        let human = render_item_result(&document, OutputMode::HumanPlain);
        assert_eq!(human.trim_end(), UNPRESENTABLE_ITEM_RESULT_MESSAGE);
        assert!(!human.contains(member_id));

        let machine = render_item_result(&document, OutputMode::Json);
        assert_eq!(serde_json::from_str::<Value>(&machine).unwrap(), document);
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
            "veggie burger  Compatible",
            "This item fits the profile.",
            "Confidence: 0.95",
            "Applies to: Sarah",
        ] {
            assert!(rendered.lines().any(|rendered| rendered == line));
        }
    }

    #[test]
    fn item_human_output_never_uses_a_member_id_as_its_label_but_json_preserves_it() {
        for (member_id, expected) in [
            ("_self", "Applies to: You"),
            (
                "3f1c9c2e-2f5a-4a5b-8f1e-9d2b7c6a4e01",
                "Applies to: Household member",
            ),
            ("legacyOpaque7", "Applies to: Household member"),
        ] {
            let document = json!({
                "item_name": "veggie burger",
                "status": "compatible",
                "summary": "This item fits the profile.",
                "member_id": member_id,
                "member_label": member_id
            });
            let human = render_item_result(&document, OutputMode::HumanPlain);
            assert!(human.lines().any(|line| line == expected), "{human}");
            assert!(!human.contains(member_id), "{human}");

            let machine = render_item_result(&document, OutputMode::Json);
            let decoded: Value = serde_json::from_str(&machine).unwrap();
            assert_eq!(decoded["member_id"], member_id);
            assert_eq!(decoded["member_label"], member_id);
        }

        let document = json!({
            "item_name": "veggie burger",
            "status": "compatible",
            "summary": "Prepared for legacyOpaque7.",
            "member_id": "legacyOpaque7",
            "member_label": "Maya (legacyOpaque7)"
        });
        assert_eq!(
            render_item_result(&document, OutputMode::HumanPlain),
            format!("{UNPRESENTABLE_ITEM_RESULT_MESSAGE}\n")
        );
        let machine = render_item_result(&document, OutputMode::Json);
        assert_eq!(serde_json::from_str::<Value>(&machine).unwrap(), document);
    }
}
