//! Deterministic, network-free contracts embedded in the heyfood executable.

#![forbid(unsafe_code)]

use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const GUIDE: &str = include_str!("../../../docs/AGENT_INTEGRATION.md");
pub const SAFETY: &str = include_str!("../../../docs/AGENT_SAFETY.md");
pub const MANIFEST_SCHEMA: &str =
    include_str!("../../../schemas/v1/heyfood-agent-manifest.schema.json");
pub const MANIFEST_V2_SCHEMA: &str =
    include_str!("../../../schemas/v2/heyfood-agent-manifest.schema.json");
pub const SCHEMA_INDEX_SCHEMA: &str =
    include_str!("../../../schemas/v1/heyfood-agent-schema-index.schema.json");
pub const DOCTOR_SCHEMA: &str =
    include_str!("../../../schemas/v1/heyfood-agent-doctor.schema.json");
pub const DOCTOR_V2_SCHEMA: &str =
    include_str!("../../../schemas/v2/heyfood-agent-doctor.schema.json");
pub const GUIDE_SCHEMA: &str = include_str!("../../../schemas/v1/heyfood-agent-guide.schema.json");
pub const SCHEMA_RESULT_SCHEMA: &str =
    include_str!("../../../schemas/v1/heyfood-agent-schema-result.schema.json");
pub const CLI_ERROR_SCHEMA: &str =
    include_str!("../../../schemas/v1/heyfood-cli-error.schema.json");
pub const PUBLIC_OUTPUT_SCHEMA: &str =
    include_str!("../../../schemas/v1/heyfood-output.schema.json");
pub const PROPOSAL_PRESENTATION_SCHEMA: &str =
    include_str!("../../../schemas/v1/agent-proposal-presentation.schema.json");
pub const SETUP_PLAN_SCHEMA: &str =
    include_str!("../../../schemas/v1/heyfood-agent-setup-plan.schema.json");

pub const MANIFEST_SCHEMA_ID: &str =
    "https://hey.food/schemas/v1/heyfood-agent-manifest.schema.json";
pub const MANIFEST_V2_SCHEMA_ID: &str =
    "https://hey.food/schemas/v2/heyfood-agent-manifest.schema.json";
pub const SCHEMA_INDEX_SCHEMA_ID: &str =
    "https://hey.food/schemas/v1/heyfood-agent-schema-index.schema.json";
pub const DOCTOR_SCHEMA_ID: &str = "https://hey.food/schemas/v1/heyfood-agent-doctor.schema.json";
pub const DOCTOR_V2_SCHEMA_ID: &str =
    "https://hey.food/schemas/v2/heyfood-agent-doctor.schema.json";
pub const GUIDE_SCHEMA_ID: &str = "https://hey.food/schemas/v1/heyfood-agent-guide.schema.json";
pub const SCHEMA_RESULT_SCHEMA_ID: &str =
    "https://hey.food/schemas/v1/heyfood-agent-schema-result.schema.json";
pub const CLI_ERROR_SCHEMA_ID: &str = "https://hey.food/schemas/v1/heyfood-cli-error.schema.json";
pub const PUBLIC_OUTPUT_SCHEMA_ID: &str =
    "https://github.com/frntrllc/heyfood/blob/main/schemas/v1/heyfood-output.schema.json";
pub const PROPOSAL_PRESENTATION_SCHEMA_ID: &str =
    "https://hey.food/schemas/v1/agent-proposal-presentation.schema.json";
pub const SETUP_PLAN_SCHEMA_ID: &str =
    "https://hey.food/schemas/v1/heyfood-agent-setup-plan.schema.json";

pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
pub const MAX_SCHEMA_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbeddedSchema {
    Manifest,
    ManifestV2,
    SchemaIndex,
    Doctor,
    DoctorV2,
    Guide,
    SchemaResult,
    CliError,
    PublicOutput,
    ProposalPresentation,
    SetupPlan,
}

impl EmbeddedSchema {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Manifest => "manifest",
            Self::ManifestV2 => "manifest-v2",
            Self::SchemaIndex => "schema-index",
            Self::Doctor => "doctor",
            Self::DoctorV2 => "doctor-v2",
            Self::Guide => "guide",
            Self::SchemaResult => "schema-result",
            Self::CliError => "error",
            Self::PublicOutput => "output",
            Self::ProposalPresentation => "proposal-presentation",
            Self::SetupPlan => "setup-plan",
        }
    }

    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Manifest => MANIFEST_SCHEMA_ID,
            Self::ManifestV2 => MANIFEST_V2_SCHEMA_ID,
            Self::SchemaIndex => SCHEMA_INDEX_SCHEMA_ID,
            Self::Doctor => DOCTOR_SCHEMA_ID,
            Self::DoctorV2 => DOCTOR_V2_SCHEMA_ID,
            Self::Guide => GUIDE_SCHEMA_ID,
            Self::SchemaResult => SCHEMA_RESULT_SCHEMA_ID,
            Self::CliError => CLI_ERROR_SCHEMA_ID,
            Self::PublicOutput => PUBLIC_OUTPUT_SCHEMA_ID,
            Self::ProposalPresentation => PROPOSAL_PRESENTATION_SCHEMA_ID,
            Self::SetupPlan => SETUP_PLAN_SCHEMA_ID,
        }
    }

    #[must_use]
    pub const fn document(self) -> &'static str {
        match self {
            Self::Manifest => MANIFEST_SCHEMA,
            Self::ManifestV2 => MANIFEST_V2_SCHEMA,
            Self::SchemaIndex => SCHEMA_INDEX_SCHEMA,
            Self::Doctor => DOCTOR_SCHEMA,
            Self::DoctorV2 => DOCTOR_V2_SCHEMA,
            Self::Guide => GUIDE_SCHEMA,
            Self::SchemaResult => SCHEMA_RESULT_SCHEMA,
            Self::CliError => CLI_ERROR_SCHEMA,
            Self::PublicOutput => PUBLIC_OUTPUT_SCHEMA,
            Self::ProposalPresentation => PROPOSAL_PRESENTATION_SCHEMA,
            Self::SetupPlan => SETUP_PLAN_SCHEMA,
        }
    }
}

pub const PUBLIC_SCHEMAS: [EmbeddedSchema; 11] = [
    EmbeddedSchema::Manifest,
    EmbeddedSchema::ManifestV2,
    EmbeddedSchema::SchemaIndex,
    EmbeddedSchema::Doctor,
    EmbeddedSchema::DoctorV2,
    EmbeddedSchema::Guide,
    EmbeddedSchema::SchemaResult,
    EmbeddedSchema::CliError,
    EmbeddedSchema::PublicOutput,
    EmbeddedSchema::ProposalPresentation,
    EmbeddedSchema::SetupPlan,
];

#[derive(Serialize)]
struct CommandContract {
    path: &'static str,
    purpose: &'static str,
    status: &'static str,
    audience: &'static str,
    input_channel: &'static str,
    output_family: &'static str,
    output_schema_id: Option<&'static str>,
    output_schema_sha256: Option<String>,
    error_schema_id: &'static str,
    error_schema_sha256: String,
    exit_behavior: &'static str,
    operation_class: &'static str,
    network: bool,
    product_state_mutation: bool,
    credential_side_effect_possible: bool,
    required_scopes: &'static [&'static str],
    authorization_upgrade_command: Option<&'static str>,
    retry_class: &'static str,
    reconciliation_command: Option<&'static str>,
    human_confirmation: &'static str,
    interactivity: &'static str,
    browser_handoff: &'static str,
    examples: &'static [&'static str],
}

impl CommandContract {
    fn with_output_schema(mut self, schema: EmbeddedSchema) -> Self {
        self.output_schema_id = Some(schema.id());
        self.output_schema_sha256 = Some(sha256_hex(schema.document().as_bytes()));
        self
    }

    fn with_reconciliation(mut self, command: &'static str) -> Self {
        self.reconciliation_command = Some(command);
        self
    }
}

#[allow(clippy::too_many_arguments)]
fn command(
    path: &'static str,
    purpose: &'static str,
    audience: &'static str,
    input_channel: &'static str,
    output_family: &'static str,
    exit_behavior: &'static str,
    operation_class: &'static str,
    network: bool,
    mutation: bool,
    credential_side_effect_possible: bool,
    scopes: &'static [&'static str],
    retry_class: &'static str,
    human_confirmation: &'static str,
    interactivity: &'static str,
    browser_handoff: &'static str,
    examples: &'static [&'static str],
) -> CommandContract {
    CommandContract {
        path,
        purpose,
        status: "active",
        audience,
        input_channel,
        output_family,
        output_schema_id: None,
        output_schema_sha256: None,
        error_schema_id: CLI_ERROR_SCHEMA_ID,
        error_schema_sha256: sha256_hex(CLI_ERROR_SCHEMA.as_bytes()),
        exit_behavior,
        operation_class,
        network,
        product_state_mutation: mutation,
        credential_side_effect_possible,
        required_scopes: scopes,
        authorization_upgrade_command: if scopes.is_empty() {
            None
        } else {
            Some("heyfood login")
        },
        retry_class,
        reconciliation_command: None,
        human_confirmation,
        interactivity,
        browser_handoff,
        examples,
    }
}

fn commands() -> Vec<CommandContract> {
    const NONE: &[&str] = &[];
    const GROCERY_READ: &[&str] = &["grocery:read"];
    const GROCERY_WRITE: &[&str] = &["grocery:read", "grocery:write"];
    const MENU_READ: &[&str] = &["menu:read"];
    const WATCH: &[&str] = &["menu:watch"];
    const MEALS_WRITE: &[&str] = &["meals:write"];

    vec![
        command(
            "agent",
            "Describe the exact installed agent contract.",
            "agent_safe",
            "none",
            "heyfood_agent_manifest_v1",
            "one_json_value",
            "local_read",
            false,
            false,
            false,
            NONE,
            "not_applicable",
            "none",
            "none",
            "none",
            &["heyfood agent"],
        )
        .with_output_schema(EmbeddedSchema::Manifest),
        command(
            "agent describe",
            "Describe the exact installed agent contract.",
            "agent_safe",
            "none",
            "heyfood_agent_manifest_v1",
            "one_json_value",
            "local_read",
            false,
            false,
            false,
            NONE,
            "not_applicable",
            "none",
            "none",
            "none",
            &["heyfood agent describe"],
        )
        .with_output_schema(EmbeddedSchema::Manifest),
        command(
            "agent guide",
            "Print the embedded agent integration or safety guide.",
            "agent_safe",
            "arguments",
            "markdown_or_agent_guide_v1",
            "raw_markdown_or_one_json_value",
            "local_read",
            false,
            false,
            false,
            NONE,
            "not_applicable",
            "none",
            "none",
            "none",
            &[
                "heyfood agent guide --format markdown",
                "heyfood --json agent guide --format markdown --safety",
            ],
        )
        .with_output_schema(EmbeddedSchema::Guide),
        command(
            "agent schema",
            "Print one embedded agent JSON Schema.",
            "agent_safe",
            "arguments",
            "json_schema_2020_12",
            "one_json_value",
            "local_read",
            false,
            false,
            false,
            NONE,
            "not_applicable",
            "none",
            "none",
            "none",
            &[
                "heyfood agent schema --list",
                "heyfood agent schema manifest",
            ],
        )
        .with_output_schema(EmbeddedSchema::SchemaResult),
        command(
            "agent doctor",
            "Inspect the local integration without credentials or network.",
            "agent_safe",
            "none",
            "agent_doctor_v1",
            "one_json_value",
            "local_read",
            false,
            false,
            false,
            NONE,
            "not_applicable",
            "none",
            "none",
            "none",
            &["heyfood agent doctor"],
        )
        .with_output_schema(EmbeddedSchema::Doctor),
        command(
            "agent setup",
            "Plan or install the canonical heyfood Agent Skill and read-only MCP registration.",
            "agent_unsupported",
            "arguments",
            "agent_setup_plan_v1",
            "one_json_value",
            "mutation",
            false,
            false,
            false,
            NONE,
            "no_blind_retry",
            "none",
            "none",
            "none",
            &["heyfood --json agent setup --target all --scope user --dry-run"],
        )
        .with_output_schema(EmbeddedSchema::SetupPlan)
        .with_reconciliation("heyfood agent setup --target TARGET --scope SCOPE --dry-run"),
        command(
            "agent uninstall",
            "Plan or remove an exact receipt-bound heyfood Agent Skill and MCP registration.",
            "agent_unsupported",
            "arguments",
            "agent_setup_plan_v1",
            "one_json_value",
            "mutation",
            false,
            false,
            false,
            NONE,
            "no_blind_retry",
            "none",
            "none",
            "none",
            &["heyfood --json agent uninstall --target all --scope user --dry-run"],
        )
        .with_output_schema(EmbeddedSchema::SetupPlan)
        .with_reconciliation("heyfood agent uninstall --target TARGET --scope SCOPE --dry-run"),
        command(
            "mcp serve",
            "Serve six bounded read/discovery tools over local MCP stdio.",
            "agent_safe",
            "mcp_stdio",
            "mcp_json_rpc",
            "long_lived_json_rpc_frames",
            "remote_read",
            true,
            false,
            true,
            NONE,
            "tool_specific",
            "none",
            "none",
            "none",
            &["/absolute/path/to/heyfood mcp serve"],
        ),
        command(
            "ask",
            "Ask the hosted conversational service.",
            "agent_unsupported",
            "arguments_or_utf8_stdin",
            "agent_turn_result_v1",
            "one_json_value",
            "remote_read",
            true,
            false,
            true,
            NONE,
            "reconcile_before_retry",
            "none",
            "none",
            "none",
            NONE,
        ),
        command(
            "reply",
            "Continue one hosted conversation.",
            "agent_unsupported",
            "arguments_or_utf8_stdin",
            "agent_turn_result_v1",
            "one_json_value",
            "remote_read",
            true,
            false,
            true,
            NONE,
            "reconcile_before_retry",
            "none",
            "none",
            "none",
            NONE,
        ),
        command(
            "chat",
            "Open the interactive human terminal.",
            "human_terminal_only",
            "attached_terminal",
            "human_terminal",
            "human_terminal",
            "interactive",
            true,
            false,
            true,
            NONE,
            "no_blind_retry",
            "none",
            "attached_terminal",
            "none",
            NONE,
        ),
        command(
            "log",
            "Log a meal after a human terminal decision.",
            "human_terminal_only",
            "arguments_or_utf8_stdin_plus_controlling_terminal",
            "agent_turn_result_v1",
            "one_json_value",
            "mutation",
            true,
            true,
            true,
            MEALS_WRITE,
            "reconcile_before_retry",
            "attached_terminal",
            "controlling_terminal",
            "none",
            NONE,
        ),
        command(
            "item",
            "Assess a food or menu item.",
            "agent_unsupported",
            "arguments",
            "item_explanation_v1",
            "one_json_value",
            "remote_read",
            true,
            false,
            true,
            MENU_READ,
            "no_blind_retry",
            "none",
            "none",
            "none",
            NONE,
        ),
        command(
            "login",
            "Connect or replace the human account authorization.",
            "human_terminal_only",
            "attached_terminal",
            "login_result_v1",
            "one_json_value",
            "authorization",
            true,
            false,
            true,
            NONE,
            "reconcile_before_retry",
            "none",
            "independent_browser_or_device",
            "required",
            NONE,
        ),
        command(
            "register",
            "Create and connect a human account.",
            "human_terminal_only",
            "attached_terminal",
            "registration_result_v1",
            "one_json_value",
            "authorization",
            true,
            false,
            true,
            NONE,
            "reconcile_before_retry",
            "none",
            "independent_browser_or_device",
            "required",
            NONE,
        ),
        command(
            "logout",
            "Revoke this device's hosted authority and clear its local credentials.",
            "agent_unsupported",
            "arguments",
            "logout_result_v1",
            "one_json_value",
            "authorization",
            true,
            false,
            true,
            NONE,
            "no_blind_retry",
            "none",
            "none",
            "none",
            NONE,
        )
        .with_reconciliation("heyfood logout"),
        command(
            "grocery",
            "Read the active Grocery list.",
            "agent_unsupported",
            "arguments",
            "grocery_list_v1",
            "one_json_value",
            "remote_read",
            true,
            false,
            true,
            GROCERY_READ,
            "safe_read",
            "none",
            "none",
            "none",
            NONE,
        ),
        command(
            "grocery show",
            "Read the active Grocery list.",
            "agent_unsupported",
            "arguments",
            "grocery_list_v1",
            "one_json_value",
            "remote_read",
            true,
            false,
            true,
            GROCERY_READ,
            "safe_read",
            "none",
            "none",
            "none",
            NONE,
        ),
        command(
            "grocery exclusions",
            "Read Grocery exclusions.",
            "agent_unsupported",
            "arguments",
            "grocery_exclusions_v1",
            "one_json_value",
            "remote_read",
            true,
            false,
            true,
            GROCERY_READ,
            "safe_read",
            "none",
            "none",
            "none",
            NONE,
        ),
        command(
            "grocery add",
            "Prepare Grocery additions for human review.",
            "human_terminal_only",
            "arguments_plus_controlling_terminal",
            "grocery_mutation_proposal_v1",
            "one_json_value",
            "prepare",
            true,
            false,
            true,
            GROCERY_WRITE,
            "no_blind_retry",
            "attached_terminal",
            "controlling_terminal",
            "none",
            NONE,
        ),
        command(
            "grocery remove",
            "Prepare Grocery removals for human review.",
            "human_terminal_only",
            "arguments_plus_controlling_terminal",
            "grocery_mutation_proposal_v1",
            "one_json_value",
            "prepare",
            true,
            false,
            true,
            GROCERY_WRITE,
            "no_blind_retry",
            "attached_terminal",
            "controlling_terminal",
            "none",
            NONE,
        ),
        command(
            "grocery state",
            "Prepare Grocery state changes for human review.",
            "human_terminal_only",
            "arguments_plus_controlling_terminal",
            "grocery_mutation_proposal_v1",
            "one_json_value",
            "prepare",
            true,
            false,
            true,
            GROCERY_WRITE,
            "no_blind_retry",
            "attached_terminal",
            "controlling_terminal",
            "none",
            NONE,
        ),
        command(
            "grocery never",
            "Prepare an exclusion change for human review.",
            "human_terminal_only",
            "arguments_plus_controlling_terminal",
            "grocery_mutation_proposal_v1",
            "one_json_value",
            "prepare",
            true,
            false,
            true,
            GROCERY_WRITE,
            "no_blind_retry",
            "attached_terminal",
            "controlling_terminal",
            "none",
            NONE,
        ),
        command(
            "grocery export",
            "Export a Grocery list to a human-selected path.",
            "agent_unsupported",
            "arguments",
            "grocery_export_or_write_receipt_v1",
            "one_json_value",
            "remote_read",
            true,
            false,
            true,
            GROCERY_READ,
            "safe_read",
            "none",
            "none",
            "none",
            NONE,
        ),
        command(
            "grocery confirm",
            "Commit or cancel an exact Grocery proposal after human terminal review.",
            "human_terminal_only",
            "json_stdin_plus_controlling_terminal",
            "grocery_mutation_result_v1",
            "one_json_value",
            "confirm",
            true,
            true,
            true,
            GROCERY_WRITE,
            "reconcile_before_retry",
            "attached_terminal",
            "controlling_terminal",
            "none",
            NONE,
        ),
        command(
            "watch",
            "Read Menu Watch subscriptions.",
            "agent_unsupported",
            "arguments",
            "menu_watch_list_v1",
            "one_json_value",
            "remote_read",
            true,
            false,
            true,
            WATCH,
            "safe_read",
            "none",
            "none",
            "none",
            NONE,
        ),
        command(
            "watch show",
            "Read Menu Watch subscriptions.",
            "agent_unsupported",
            "arguments",
            "menu_watch_list_v1",
            "one_json_value",
            "remote_read",
            true,
            false,
            true,
            WATCH,
            "safe_read",
            "none",
            "none",
            "none",
            NONE,
        ),
        command(
            "watch add",
            "Create a Menu Watch after human terminal review.",
            "human_terminal_only",
            "arguments_plus_controlling_terminal",
            "menu_watch_snapshot_v1",
            "one_json_value",
            "mutation",
            true,
            true,
            true,
            WATCH,
            "reconcile_before_retry",
            "attached_terminal",
            "controlling_terminal",
            "none",
            NONE,
        ),
        command(
            "watch remove",
            "Remove a Menu Watch after human terminal review.",
            "human_terminal_only",
            "arguments_plus_controlling_terminal",
            "menu_watch_delete_receipt_v1",
            "one_json_value",
            "mutation",
            true,
            true,
            true,
            WATCH,
            "reconcile_before_retry",
            "attached_terminal",
            "controlling_terminal",
            "none",
            NONE,
        ),
        command(
            "completion",
            "Print human shell completion syntax.",
            "agent_unsupported",
            "arguments",
            "shell_completion",
            "shell_source",
            "local_read",
            false,
            false,
            false,
            NONE,
            "not_applicable",
            "none",
            "none",
            "none",
            NONE,
        ),
    ]
}

#[must_use]
pub fn manifest() -> Value {
    let features = env!("HEYFOOD_BUILD_FEATURES")
        .split(',')
        .filter(|feature| !feature.is_empty())
        .collect::<Vec<_>>();
    let manifest = json!({
        "schema_version": 1,
        "product": "heyfood",
        "binary_version": env!("CARGO_PKG_VERSION"),
        "build": {
            "source_commit": env!("HEYFOOD_BUILD_SOURCE_COMMIT"),
            "source_tree": env!("HEYFOOD_BUILD_SOURCE_TREE"),
            "dirty": env!("HEYFOOD_BUILD_DIRTY") == "true",
            "toolchain": env!("HEYFOOD_BUILD_TOOLCHAIN"),
            "distribution_channel": env!("HEYFOOD_BUILD_DISTRIBUTION_CHANNEL"),
            "target": env!("HEYFOOD_BUILD_TARGET"),
            "features": features,
            "build_input_digest_sha256": env!("HEYFOOD_BUILD_INPUT_DIGEST")
        },
        "compatibility": {
            "guide_version": 1,
            "mcp_protocol_version": 1,
            "minimum_skill_manifest_version": 1,
            "maximum_skill_manifest_version": 1,
            "additive_optional_fields": false
        },
        "automation_surfaces": {
            "one_shot_json": "active",
            "mcp_stdio": "active",
            "tui_automation": "unsupported"
        },
        "limits": {
            "manifest_bytes": MAX_MANIFEST_BYTES,
            "schema_bytes": MAX_SCHEMA_BYTES,
            "mcp_inbound_frame_bytes": 1048576,
            "mcp_tool_arguments_bytes": 1048576,
            "mcp_structured_result_bytes": 4194304,
            "sse_line_bytes": 65536,
            "sse_event_bytes": 1048576,
            "stream_event_count": 4096,
            "stream_total_bytes": 4194304,
            "outstanding_requests": 8,
            "remote_in_flight": 1,
            "queued_requests": 7,
            "page_records": 100
        },
        "capabilities": [
            {"id": "agent-self-description", "status": "active", "summary": "Offline manifest, guide, schemas, and doctor are embedded.", "contract_version": "v1"},
            {"id": "grocery", "status": "active", "summary": "Human Grocery workflows are active; agent access follows per-command audience.", "contract_version": "v1"},
            {"id": "menu-watch", "status": "active", "summary": "Human Menu Watch management is active; agent access follows per-command audience.", "contract_version": "v1"},
            {"id": "agent-mcp", "status": "active", "summary": "Six bounded read/discovery tools are available over local stdio with native account credentials.", "contract_version": "v1"},
            {"id": "health", "status": "deferred", "summary": "Health is outside the supported release contract.", "contract_version": null},
            {"id": "native-voice", "status": "deferred", "summary": "Native voice is not enabled in the default artifact.", "contract_version": null},
            {"id": "windows-distribution", "status": "deferred", "summary": "Windows source CI is active; public Windows distribution is deferred.", "contract_version": null}
        ],
        "commands": commands()
    });
    debug_assert!(canonical_json(&manifest).len() <= MAX_MANIFEST_BYTES);
    manifest
}

/// Return the explicit v2 discovery contract used by the v0.7.1 native-state
/// release verifier. The default [`manifest`] remains the closed v1 contract
/// consumed by already-installed v0.6.2 Agent Skills.
#[must_use]
pub fn manifest_v2() -> Value {
    let mut manifest = manifest();
    manifest["schema_version"] = Value::from(2);
    manifest
        .as_object_mut()
        .expect("manifest is an object")
        .insert(
            "native_state_compatibility".to_owned(),
            json!({
                "binary_version": env!("CARGO_PKG_VERSION"),
                "maximum_native_state_version": 2,
                "native_state_capabilities": [
                    "household-account-slot-v1",
                    "household-lifecycle-lock-v1",
                    "household-migration-guard-v1",
                    "household-teardown-journal-v1"
                ],
                "schema_version": 1
            }),
        );

    for command in manifest["commands"]
        .as_array_mut()
        .expect("manifest commands are an array")
    {
        match command["path"].as_str().expect("command path is a string") {
            "agent describe" => {
                command["purpose"] = Value::from(
                    "Describe the exact installed agent contract using v1 by default or an explicitly requested v2.",
                );
                command["input_channel"] = Value::from("arguments");
                command["output_family"] = Value::from("heyfood_agent_manifest_v1_or_v2");
                command["output_schema_id"] = Value::Null;
                command["output_schema_sha256"] = Value::Null;
                command["examples"] = json!([
                    "heyfood agent describe",
                    "heyfood agent describe --schema-version 2"
                ]);
            }
            "agent doctor" => {
                command["purpose"] = Value::from(
                    "Inspect the local integration using v1 by default or an explicitly requested v2.",
                );
                command["input_channel"] = Value::from("arguments");
                command["output_family"] = Value::from("agent_doctor_v1_or_v2");
                command["output_schema_id"] = Value::Null;
                command["output_schema_sha256"] = Value::Null;
                command["examples"] = json!([
                    "heyfood agent doctor",
                    "heyfood agent doctor --schema-version 2"
                ]);
            }
            _ => {}
        }
    }

    debug_assert!(canonical_json(&manifest).len() <= MAX_MANIFEST_BYTES);
    manifest
}

#[must_use]
pub fn canonical_json(value: &Value) -> String {
    serde_json::to_string(value).expect("embedded contract is serializable")
}

#[must_use]
pub fn pretty_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).expect("embedded contract is serializable")
}

#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[must_use]
pub fn guide_document() -> Value {
    json!({
        "schema_version": 1,
        "media_type": "text/markdown",
        "sha256": sha256_hex(GUIDE.as_bytes()),
        "content": GUIDE
    })
}

#[must_use]
pub fn safety_document() -> Value {
    json!({
        "schema_version": 1,
        "media_type": "text/markdown",
        "sha256": sha256_hex(SAFETY.as_bytes()),
        "content": SAFETY
    })
}

#[must_use]
pub fn schema_document(schema: EmbeddedSchema) -> Value {
    serde_json::from_str(schema.document()).expect("embedded schema is valid JSON")
}

#[must_use]
pub fn schema_by_name(name: &str) -> Option<EmbeddedSchema> {
    PUBLIC_SCHEMAS
        .into_iter()
        .find(|schema| schema.name() == name || schema.id() == name)
}

#[must_use]
pub fn schema_index() -> Value {
    let schemas = PUBLIC_SCHEMAS
        .into_iter()
        .map(|schema| {
            json!({
                "name": schema.name(),
                "id": schema.id(),
                "sha256": sha256_hex(schema.document().as_bytes()),
                "bytes": schema.document().len()
            })
        })
        .collect::<Vec<_>>();
    json!({
        "schema_version": 1,
        "schemas": schemas
    })
}

#[must_use]
pub fn embedded_digests() -> Value {
    embedded_digests_for(MANIFEST_SCHEMA, DOCTOR_SCHEMA)
}

#[must_use]
pub fn embedded_digests_v2() -> Value {
    embedded_digests_for(MANIFEST_V2_SCHEMA, DOCTOR_V2_SCHEMA)
}

fn embedded_digests_for(manifest_schema: &str, doctor_schema: &str) -> Value {
    json!({
        "guide_sha256": sha256_hex(GUIDE.as_bytes()),
        "safety_sha256": sha256_hex(SAFETY.as_bytes()),
        "manifest_schema_sha256": sha256_hex(manifest_schema.as_bytes()),
        "schema_index_schema_sha256": sha256_hex(SCHEMA_INDEX_SCHEMA.as_bytes()),
        "doctor_schema_sha256": sha256_hex(doctor_schema.as_bytes()),
        "guide_schema_sha256": sha256_hex(GUIDE_SCHEMA.as_bytes()),
        "schema_result_schema_sha256": sha256_hex(SCHEMA_RESULT_SCHEMA.as_bytes()),
        "cli_error_schema_sha256": sha256_hex(CLI_ERROR_SCHEMA.as_bytes()),
        "public_output_schema_sha256": sha256_hex(PUBLIC_OUTPUT_SCHEMA.as_bytes()),
        "proposal_presentation_schema_sha256": sha256_hex(PROPOSAL_PRESENTATION_SCHEMA.as_bytes()),
    })
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn doctor_check(id: &'static str, passed: bool) -> Value {
    json!({
        "id": id,
        "status": if passed { "pass" } else { "fail" }
    })
}

#[must_use]
pub fn doctor_document() -> Value {
    doctor_document_for(manifest(), 1, embedded_digests())
}

/// Return diagnostics bound to the explicit v2 manifest and schemas.
#[must_use]
pub fn doctor_document_v2() -> Value {
    doctor_document_for(manifest_v2(), 2, embedded_digests_v2())
}

fn doctor_document_for(manifest: Value, schema_version: u16, embedded: Value) -> Value {
    let canonical_manifest = canonical_json(&manifest);
    let manifest_round_trip = serde_json::from_str::<Value>(&canonical_manifest)
        .is_ok_and(|decoded| decoded == manifest)
        && canonical_manifest.len() <= MAX_MANIFEST_BYTES;

    let schemas_valid = PUBLIC_SCHEMAS.into_iter().all(|schema| {
        schema.document().len() <= MAX_SCHEMA_BYTES
            && serde_json::from_str::<Value>(schema.document()).is_ok_and(|document| {
                document["$schema"] == "https://json-schema.org/draft/2020-12/schema"
                    && document["$id"] == schema.id()
            })
    });

    let index = schema_index();
    let index_exact = index["schemas"].as_array().is_some_and(|entries| {
        entries.len() == PUBLIC_SCHEMAS.len()
            && entries.iter().zip(PUBLIC_SCHEMAS).all(|(entry, schema)| {
                entry["name"] == schema.name()
                    && entry["id"] == schema.id()
                    && entry["sha256"] == sha256_hex(schema.document().as_bytes())
                    && entry["bytes"] == schema.document().len()
            })
    });

    let command_bindings = manifest["commands"].as_array().is_some_and(|contracts| {
        contracts.iter().all(|contract| {
            let error_bound = contract["error_schema_id"] == CLI_ERROR_SCHEMA_ID
                && contract["error_schema_sha256"] == sha256_hex(CLI_ERROR_SCHEMA.as_bytes());
            let output_bound = if contract["output_schema_id"].is_null() {
                contract["output_schema_sha256"].is_null()
            } else {
                contract["output_schema_id"]
                    .as_str()
                    .and_then(schema_by_name)
                    .is_some_and(|schema| {
                        contract["output_schema_sha256"] == sha256_hex(schema.document().as_bytes())
                    })
            };
            error_bound && output_bound
        })
    });

    let embedded_guides = !GUIDE.is_empty()
        && !SAFETY.is_empty()
        && GUIDE.len() <= MAX_SCHEMA_BYTES
        && SAFETY.len() <= MAX_SCHEMA_BYTES;
    let build = &manifest["build"];
    let build_identity = build["source_commit"]
        .as_str()
        .is_some_and(|value| is_lower_hex(value, 40))
        && build["source_tree"]
            .as_str()
            .is_some_and(|value| is_lower_hex(value, 40))
        && build["build_input_digest_sha256"]
            .as_str()
            .is_some_and(|value| is_lower_hex(value, 64))
        && matches!(
            build["distribution_channel"].as_str(),
            Some("development" | "candidate" | "release")
        );

    let checks = vec![
        doctor_check("manifest_round_trip", manifest_round_trip),
        doctor_check("public_schemas", schemas_valid),
        doctor_check("schema_index", index_exact),
        doctor_check("command_schema_bindings", command_bindings),
        doctor_check("embedded_guides", embedded_guides),
        doctor_check("build_identity", build_identity),
    ];
    let ok = checks.iter().all(|check| check["status"] == "pass");
    json!({
        "schema_version": schema_version,
        "ok": ok,
        "binary_version": env!("CARGO_PKG_VERSION"),
        "target": env!("HEYFOOD_BUILD_TARGET"),
        "manifest_schema_version": manifest["schema_version"],
        "manifest_sha256": sha256_hex(canonical_manifest.as_bytes()),
        "embedded": embedded,
        "checks": checks,
        "network_accessed": false,
        "credentials_accessed": false,
        "product_state_mutated": false,
        "tui_automation_supported": false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_bounded_deterministic_and_contains_no_commit_authority() {
        let first = canonical_json(&manifest());
        let second = canonical_json(&manifest());
        assert_eq!(first, second);
        assert!(first.len() <= MAX_MANIFEST_BYTES);
        for forbidden in [
            "confirmation_token",
            "idempotency_key",
            "commit_credential",
            "serialized_proposal",
        ] {
            assert!(!first.contains(forbidden), "{forbidden}");
        }
    }

    #[test]
    fn manifest_declares_exact_native_state_compatibility() {
        let default_manifest = manifest();
        assert_eq!(default_manifest["schema_version"], 1);
        assert!(default_manifest.get("native_state_compatibility").is_none());

        let manifest = manifest_v2();
        assert_eq!(manifest["schema_version"], 2);
        assert_eq!(
            manifest["native_state_compatibility"],
            json!({
                "binary_version": env!("CARGO_PKG_VERSION"),
                "maximum_native_state_version": 2,
                "native_state_capabilities": [
                    "household-account-slot-v1",
                    "household-lifecycle-lock-v1",
                    "household-migration-guard-v1",
                    "household-teardown-journal-v1"
                ],
                "schema_version": 1
            })
        );
    }

    #[test]
    fn embedded_documents_are_valid_and_bounded() {
        for schema in [
            EmbeddedSchema::Manifest,
            EmbeddedSchema::ManifestV2,
            EmbeddedSchema::SchemaIndex,
            EmbeddedSchema::Doctor,
            EmbeddedSchema::DoctorV2,
            EmbeddedSchema::Guide,
            EmbeddedSchema::SchemaResult,
            EmbeddedSchema::CliError,
            EmbeddedSchema::PublicOutput,
            EmbeddedSchema::ProposalPresentation,
            EmbeddedSchema::SetupPlan,
        ] {
            assert!(schema.document().len() <= MAX_SCHEMA_BYTES);
            let parsed: Value = serde_json::from_str(schema.document()).unwrap();
            assert_eq!(parsed["$id"], schema.id());
        }
        assert!(GUIDE.len() <= MAX_SCHEMA_BYTES);
        assert!(SAFETY.len() <= MAX_SCHEMA_BYTES);
    }

    #[test]
    fn every_public_schema_and_generated_instance_passes_draft_2020_12_validation() {
        for schema in PUBLIC_SCHEMAS {
            let document: Value = serde_json::from_str(schema.document()).unwrap();
            jsonschema::draft202012::meta::validate(&document).unwrap_or_else(|error| {
                panic!("{} meta-schema validation: {error}", schema.name())
            });
        }

        let cases = [
            (EmbeddedSchema::Manifest, manifest()),
            (EmbeddedSchema::ManifestV2, manifest_v2()),
            (EmbeddedSchema::SchemaIndex, schema_index()),
            (EmbeddedSchema::Doctor, doctor_document()),
            (EmbeddedSchema::DoctorV2, doctor_document_v2()),
            (EmbeddedSchema::Guide, guide_document()),
            (EmbeddedSchema::Guide, safety_document()),
            (EmbeddedSchema::SchemaResult, schema_index()),
            (
                EmbeddedSchema::SchemaResult,
                schema_document(EmbeddedSchema::Manifest),
            ),
            (
                EmbeddedSchema::CliError,
                json!({
                    "ok": false,
                    "error": {
                        "type": "agent_schema_unknown",
                        "message": "The requested schema is not public.",
                        "hint": "List public schemas first."
                    }
                }),
            ),
            (
                EmbeddedSchema::SetupPlan,
                json!({
                    "schema_version": 1,
                    "operation": "install",
                    "mode": "dry_run",
                    "target": "codex",
                    "scope": "user",
                    "project_root": null,
                    "binary": {
                        "path": "/absolute/heyfood",
                        "sha256": "0".repeat(64),
                        "version": "0.7.1"
                    },
                    "package": {
                        "name": "heyfood",
                        "version": "0.7.1",
                        "sha256": "1".repeat(64),
                        "files": 6
                    },
                    "plan_sha256": "2".repeat(64),
                    "ready": true,
                    "changed": false,
                    "hosts": [{
                        "host": "codex",
                        "host_executable": "/absolute/codex",
                        "host_version": "codex-cli 0.145.0-alpha.18",
                        "compatible_version": "codex-cli 0.145.0-alpha.18",
                        "compatibility": "compatible",
                        "skill_path": "/home/user/.agents/skills/heyfood",
                        "receipt_path": "/state/receipts/receipt.json",
                        "mcp": {
                            "name": "heyfood",
                            "transport": "stdio",
                            "command": "/absolute/heyfood",
                            "arguments": ["mcp", "serve"],
                            "environment": {},
                            "environment_policy_sha256": "3".repeat(64),
                            "configuration_scope": "user",
                            "action": "install"
                        },
                        "action": "install",
                        "conflicts": [],
                        "user_actions": []
                    }]
                }),
            ),
        ];
        for (schema, instance) in cases {
            let document: Value = serde_json::from_str(schema.document()).unwrap();
            jsonschema::draft202012::validate(&document, &instance).unwrap_or_else(|error| {
                panic!("{} generated-instance validation: {error}", schema.name())
            });
        }
    }

    #[test]
    fn public_schema_index_is_sorted_and_omits_commit_protocol() {
        let index = schema_index();
        let names = index["schemas"]
            .as_array()
            .unwrap()
            .iter()
            .map(|schema| schema["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "manifest",
                "manifest-v2",
                "schema-index",
                "doctor",
                "doctor-v2",
                "guide",
                "schema-result",
                "error",
                "output",
                "proposal-presentation",
                "setup-plan"
            ]
        );
        let encoded = canonical_json(&index);
        assert!(!encoded.contains("approval-protocol"));
        assert!(!encoded.contains("commit_request"));
    }

    #[test]
    fn agent_examples_never_include_human_only_commands() {
        for contract in commands() {
            if contract.audience == "human_terminal_only" {
                assert!(contract.examples.is_empty(), "{}", contract.path);
            }
        }
    }
}
