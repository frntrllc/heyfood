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
pub enum AgentCompatibilitySemanticError {
    InvalidDocument,
    InvertedSupportedRange,
    ManifestOutsideSupportedRange,
}

/// Enforce cross-field compatibility invariants that JSON Schema cannot
/// express. The binary-owned compatibility emitter must call this before it
/// serializes a result.
pub fn validate_agent_compatibility_semantics(
    value: &Value,
) -> Result<(), AgentCompatibilitySemanticError> {
    let manifest_version = value
        .get("manifest_schema_version")
        .and_then(Value::as_u64)
        .ok_or(AgentCompatibilitySemanticError::InvalidDocument)?;
    let compatible = value
        .get("compatible")
        .and_then(Value::as_bool)
        .ok_or(AgentCompatibilitySemanticError::InvalidDocument)?;
    let installations = value
        .get("installations")
        .and_then(Value::as_array)
        .ok_or(AgentCompatibilitySemanticError::InvalidDocument)?;
    for installation in installations {
        let minimum = installation
            .get("supported_manifest_minimum")
            .and_then(Value::as_u64);
        let maximum = installation
            .get("supported_manifest_maximum")
            .and_then(Value::as_u64);
        if let (Some(minimum), Some(maximum)) = (minimum, maximum) {
            if minimum > maximum {
                return Err(AgentCompatibilitySemanticError::InvertedSupportedRange);
            }
            let claims_compatible =
                installation.get("status").and_then(Value::as_str) == Some("compatible");
            if (compatible || claims_compatible) && !(minimum..=maximum).contains(&manifest_version)
            {
                return Err(AgentCompatibilitySemanticError::ManifestOutsideSupportedRange);
            }
        } else if compatible {
            return Err(AgentCompatibilitySemanticError::InvalidDocument);
        }
    }
    Ok(())
}

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

#[cfg(test)]
mod household_phase0_contract_tests {
    use std::collections::BTreeSet;

    use serde_json::Value;

    use super::{
        PUBLIC_SCHEMAS, canonical_json, manifest, manifest_v2, sha256_hex,
        validate_agent_compatibility_semantics,
    };

    struct ContractCase {
        name: &'static str,
        schema: &'static str,
        fixtures: &'static [&'static str],
    }

    const READ_SCHEMA: &str = include_str!("../../../schemas/v1/agent-household-read.schema.json");
    const CONTEXT_INPUT_SCHEMA: &str =
        include_str!("../../../schemas/v1/agent-household-context-input.schema.json");
    const MEMBER_INPUT_SCHEMA: &str =
        include_str!("../../../schemas/v1/agent-household-member-input.schema.json");
    const ACTION_SCHEMA: &str =
        include_str!("../../../schemas/v1/agent-household-action.schema.json");
    const GET_CHANGE_INPUT_SCHEMA: &str =
        include_str!("../../../schemas/v1/agent-household-get-change-input.schema.json");
    const CANCEL_INPUT_SCHEMA: &str =
        include_str!("../../../schemas/v1/agent-household-cancel-input.schema.json");
    const RECONCILE_INPUT_SCHEMA: &str =
        include_str!("../../../schemas/v1/agent-household-reconcile-input.schema.json");
    const PRESENTATION_SCHEMA: &str =
        include_str!("../../../schemas/v1/agent-household-proposal-presentation.schema.json");
    const OUTCOME_SCHEMA: &str =
        include_str!("../../../schemas/v1/agent-household-outcome.schema.json");
    const LOCAL_APPROVAL_SCHEMA: &str =
        include_str!("../../../schemas/v1/local-household-approval-protocol.schema.json");
    const DISCLOSURE_SCHEMA: &str =
        include_str!("../../../schemas/v1/household-agent-disclosure.schema.json");
    const COMPATIBILITY_SCHEMA: &str =
        include_str!("../../../schemas/v1/heyfood-agent-compatibility.schema.json");
    const NATIVE_STATE_SCHEMA: &str =
        include_str!("../../../schemas/v1/agent-household-native-state.schema.json");
    const MANIFEST_V3_SCHEMA: &str =
        include_str!("../../../schemas/v3/heyfood-agent-manifest.schema.json");

    const CONTEXT_INPUT: &str =
        include_str!("../../../fixtures/agent/household-phase0/context-input.json");
    const MEMBER_INPUT: &str =
        include_str!("../../../fixtures/agent/household-phase0/member-input.json");
    const READ_PROFILE: &str =
        include_str!("../../../fixtures/agent/household-phase0/read-result-profile.json");
    const READ_CONTENT_FREE: &str =
        include_str!("../../../fixtures/agent/household-phase0/read-result-content-free.json");
    const PREPARE_REQUEST: &str =
        include_str!("../../../fixtures/agent/household-phase0/prepare-request.json");
    const CANCEL_REQUEST: &str =
        include_str!("../../../fixtures/agent/household-phase0/cancel-request.json");
    const GET_CHANGE_INPUT: &str =
        include_str!("../../../fixtures/agent/household-phase0/get-change-input.json");
    const RECONCILE_INPUT: &str =
        include_str!("../../../fixtures/agent/household-phase0/reconcile-input.json");
    const PROPOSAL_CONTENT_FREE: &str =
        include_str!("../../../fixtures/agent/household-phase0/proposal-content-free.json");
    const PROPOSAL_ROSTER: &str =
        include_str!("../../../fixtures/agent/household-phase0/proposal-roster.json");
    const PROPOSAL_PROFILE: &str =
        include_str!("../../../fixtures/agent/household-phase0/proposal-profile.json");
    const CANCEL_OUTCOME: &str =
        include_str!("../../../fixtures/agent/household-phase0/cancel-outcome.json");
    const RECONCILIATION_OUTCOME: &str =
        include_str!("../../../fixtures/agent/household-phase0/reconciliation-outcome.json");
    const LOCAL_APPROVAL: &str =
        include_str!("../../../fixtures/agent/household-phase0/local-approval-lifecycle.json");
    const DISCLOSURE: &str =
        include_str!("../../../fixtures/agent/household-phase0/disclosure-cases.json");
    const COMPATIBILITY_KNOWN: &str =
        include_str!("../../../fixtures/agent/household-phase0/compatibility-known.json");
    const COMPATIBILITY_UNKNOWN: &str =
        include_str!("../../../fixtures/agent/household-phase0/compatibility-unknown.json");
    const NATIVE_STATE: &str =
        include_str!("../../../fixtures/agent/household-phase0/native-state-migration.json");
    const MANIFEST_V3: &str =
        include_str!("../../../fixtures/agent/household-phase0/manifest-v3-contract.json");
    const COMMAND_TOOL_MATRIX: &str =
        include_str!("../../../fixtures/agent/household-phase0/command-tool-matrix.json");
    const DG_R2: &str = include_str!("../../../fixtures/agent/household-phase0/dg-r2.json");
    const TUI_GRAMMAR: &str =
        include_str!("../../../fixtures/agent/household-phase0/tui-grammar.json");
    const APPLIED_COMMIT_PROOF: &str =
        include_str!("../../../fixtures/agent/household-phase0/applied-commit-proof.json");

    fn parse(value: &str) -> Value {
        serde_json::from_str(value).expect("phase0 contract JSON")
    }

    fn complete_manifest_v3_contract() -> Value {
        let overlay = parse(MANIFEST_V3);
        assert_eq!(overlay["fixture_kind"], "manifest_v3_additions_overlay");
        let v2 = manifest_v2();
        let mut v3 = v2.clone();
        v3["schema_version"] = serde_json::json!(3);
        v3["binary_version"] = overlay["binary_version"].clone();
        v3["build"] = overlay["build"].clone();
        v3["compatibility"] = overlay["compatibility"].clone();
        v3["native_state_compatibility"] = overlay["native_state_compatibility"].clone();
        v3["limits"]["proposal_lifetime_seconds"] = serde_json::json!(600);
        v3["mcp_inventory"] = overlay["mcp_inventory"].clone();
        v3["household_contracts"] = overlay["household_contracts"].clone();
        v3["frozen_compatibility_views"] = overlay["frozen_compatibility_views"].clone();

        let commands = v3["commands"].as_array_mut().expect("v3 commands");
        for command in commands.iter_mut() {
            command["input_schema_id"] = Value::Null;
            command["input_schema_sha256"] = Value::Null;
            match command["path"].as_str().expect("preserved command path") {
                "agent" => {
                    command["output_family"] = Value::from("heyfood_agent_manifest_v3");
                    command["output_schema_id"] = Value::from(
                        "https://hey.food/schemas/v3/heyfood-agent-manifest.schema.json",
                    );
                    command["output_schema_sha256"] =
                        Value::from(sha256_hex(MANIFEST_V3_SCHEMA.as_bytes()));
                }
                "agent describe" => {
                    command["purpose"] = Value::from(
                        "Describe the exact installed agent contract using v3 by default or an explicitly requested frozen compatibility view.",
                    );
                    command["output_family"] = Value::from("heyfood_agent_manifest_v1_v2_or_v3");
                    command["examples"] = serde_json::json!([
                        "heyfood agent describe",
                        "heyfood agent describe --schema-version 1",
                        "heyfood agent describe --schema-version 2",
                        "heyfood agent describe --schema-version 3"
                    ]);
                }
                "agent doctor" => {
                    command["purpose"] = Value::from(
                        "Inspect the local integration using v3 by default or an explicitly requested frozen compatibility view.",
                    );
                    command["output_family"] = Value::from("agent_doctor_v1_v2_or_v3");
                    command["examples"] = serde_json::json!([
                        "heyfood agent doctor",
                        "heyfood agent doctor --schema-version 1",
                        "heyfood agent doctor --schema-version 2",
                        "heyfood agent doctor --schema-version 3"
                    ]);
                }
                _ => {}
            }
        }
        let base = commands
            .iter()
            .find(|command| command["path"] == "agent guide")
            .expect("local-read command template")
            .clone();
        for addition in overlay["command_additions"]
            .as_array()
            .expect("command additions")
        {
            let path = addition["path"].as_str().expect("future command path");
            let (purpose, family, example) = match path {
                "agent compatibility" => (
                    "Diagnose installed Agent Skill compatibility without network, credentials, or product-state access.",
                    "heyfood_agent_compatibility_v1",
                    "heyfood agent compatibility --json --no-input",
                ),
                "household show" => (
                    "Read the locally authorized household context using stable subjects.",
                    "agent_household_read_result_v1",
                    "heyfood household show --json --no-input",
                ),
                "household member" => (
                    "Read one locally authorized household member using a stable member reference.",
                    "agent_household_read_result_v1",
                    "heyfood household member --member-ref MEMBER_REF --json --no-input",
                ),
                other => panic!("unexpected future command {other}"),
            };
            let mut command = base.clone();
            command["path"] = Value::from(path);
            command["purpose"] = Value::from(purpose);
            command["input_channel"] = Value::from("arguments");
            command["input_schema_id"] = addition["input_schema"]["id"].clone();
            command["input_schema_sha256"] = addition["input_schema"]["sha256"].clone();
            command["output_family"] = Value::from(family);
            command["output_schema_id"] = addition["result_schema"]["id"].clone();
            command["output_schema_sha256"] = addition["result_schema"]["sha256"].clone();
            command["retry_class"] = Value::from("safe_read");
            command["examples"] = serde_json::json!([example]);
            commands.push(command);
        }

        v3["capabilities"]
            .as_array_mut()
            .expect("v3 capabilities")
            .extend(
                overlay["capability_additions"]
                    .as_array()
                    .expect("capability additions")
                    .iter()
                    .cloned(),
            );
        let agent_mcp = v3["capabilities"]
            .as_array_mut()
            .expect("v3 capabilities")
            .iter_mut()
            .find(|capability| capability["id"] == "agent-mcp")
            .expect("agent MCP capability");
        agent_mcp["summary"] = Value::from(
            "Twelve bounded discovery, read, and household preparation/status tools are available over local stdio with native account credentials.",
        );
        agent_mcp["contract_version"] = Value::from("v2");
        v3
    }

    #[test]
    fn closed_phase0_schemas_validate_their_fixtures() {
        let cases = [
            ContractCase {
                name: "household context input",
                schema: CONTEXT_INPUT_SCHEMA,
                fixtures: &[CONTEXT_INPUT],
            },
            ContractCase {
                name: "household member input",
                schema: MEMBER_INPUT_SCHEMA,
                fixtures: &[MEMBER_INPUT],
            },
            ContractCase {
                name: "household read result",
                schema: READ_SCHEMA,
                fixtures: &[READ_PROFILE, READ_CONTENT_FREE],
            },
            ContractCase {
                name: "household prepare input",
                schema: ACTION_SCHEMA,
                fixtures: &[PREPARE_REQUEST],
            },
            ContractCase {
                name: "household get-change input",
                schema: GET_CHANGE_INPUT_SCHEMA,
                fixtures: &[GET_CHANGE_INPUT],
            },
            ContractCase {
                name: "household cancel input",
                schema: CANCEL_INPUT_SCHEMA,
                fixtures: &[CANCEL_REQUEST],
            },
            ContractCase {
                name: "household reconcile input",
                schema: RECONCILE_INPUT_SCHEMA,
                fixtures: &[RECONCILE_INPUT],
            },
            ContractCase {
                name: "proposal presentation",
                schema: PRESENTATION_SCHEMA,
                fixtures: &[PROPOSAL_CONTENT_FREE, PROPOSAL_ROSTER, PROPOSAL_PROFILE],
            },
            ContractCase {
                name: "outcome receipt",
                schema: OUTCOME_SCHEMA,
                fixtures: &[CANCEL_OUTCOME, RECONCILIATION_OUTCOME],
            },
            ContractCase {
                name: "local approval",
                schema: LOCAL_APPROVAL_SCHEMA,
                fixtures: &[LOCAL_APPROVAL],
            },
            ContractCase {
                name: "disclosure",
                schema: DISCLOSURE_SCHEMA,
                fixtures: &[DISCLOSURE],
            },
            ContractCase {
                name: "compatibility bootstrap",
                schema: COMPATIBILITY_SCHEMA,
                fixtures: &[COMPATIBILITY_KNOWN, COMPATIBILITY_UNKNOWN],
            },
            ContractCase {
                name: "native state",
                schema: NATIVE_STATE_SCHEMA,
                fixtures: &[NATIVE_STATE],
            },
        ];

        for case in cases {
            let schema = parse(case.schema);
            jsonschema::draft202012::meta::validate(&schema)
                .unwrap_or_else(|error| panic!("{} meta-schema: {error}", case.name));
            for fixture in case.fixtures {
                let instance = parse(fixture);
                jsonschema::draft202012::validate(&schema, &instance)
                    .unwrap_or_else(|error| panic!("{} fixture: {error}", case.name));
            }
        }
        let schema = parse(MANIFEST_V3_SCHEMA);
        jsonschema::draft202012::meta::validate(&schema)
            .unwrap_or_else(|error| panic!("manifest v3 meta-schema: {error}"));
        jsonschema::draft202012::validate(&schema, &complete_manifest_v3_contract())
            .unwrap_or_else(|error| panic!("complete manifest v3: {error}"));
    }

    #[test]
    fn household_action_schema_rejects_ambiguous_or_authority_bearing_shapes() {
        let schema = parse(ACTION_SCHEMA);
        let valid = parse(PREPARE_REQUEST);
        for invalid in [
            {
                let mut value = valid.clone();
                value["operation"] = serde_json::json!("scope");
                value["bundled_scope"] = Value::Null;
                value
            },
            {
                let mut value = valid.clone();
                value["operation"] = serde_json::json!("edit");
                value["affected_member_ref"] = Value::Null;
                value
            },
            {
                let mut value = valid.clone();
                value["commit_id"] = serde_json::json!("forbidden");
                value
            },
            {
                let mut value = valid.clone();
                value["kind"] = serde_json::json!("confirm_household_change");
                value
            },
        ] {
            assert!(
                jsonschema::draft202012::validate(&schema, &invalid).is_err(),
                "invalid action was accepted: {}",
                canonical_json(&invalid)
            );
        }
    }

    #[test]
    fn every_household_surface_rejects_other_surface_inputs_and_results() {
        let surfaces = [
            (CONTEXT_INPUT_SCHEMA, CONTEXT_INPUT),
            (MEMBER_INPUT_SCHEMA, MEMBER_INPUT),
            (ACTION_SCHEMA, PREPARE_REQUEST),
            (GET_CHANGE_INPUT_SCHEMA, GET_CHANGE_INPUT),
            (CANCEL_INPUT_SCHEMA, CANCEL_REQUEST),
            (RECONCILE_INPUT_SCHEMA, RECONCILE_INPUT),
        ];
        for (index, (schema_source, own_fixture)) in surfaces.iter().enumerate() {
            let schema = parse(schema_source);
            jsonschema::draft202012::validate(&schema, &parse(own_fixture))
                .expect("own surface fixture");
            for (other_index, (_, other_fixture)) in surfaces.iter().enumerate() {
                if index != other_index {
                    assert!(
                        jsonschema::draft202012::validate(&schema, &parse(other_fixture)).is_err(),
                        "surface {index} accepted input for surface {other_index}"
                    );
                }
            }
            for output in [READ_PROFILE, PROPOSAL_PROFILE, CANCEL_OUTCOME] {
                assert!(jsonschema::draft202012::validate(&schema, &parse(output)).is_err());
            }
        }
    }

    #[test]
    fn compatibility_success_requires_verified_identity_range_and_exact_host_remediation() {
        let schema = parse(COMPATIBILITY_SCHEMA);
        let known = parse(COMPATIBILITY_KNOWN);
        let unknown = parse(COMPATIBILITY_UNKNOWN);
        jsonschema::draft202012::validate(&schema, &known).expect("known compatibility schema");
        validate_agent_compatibility_semantics(&known).expect("known compatibility semantics");
        for invalid in [
            {
                let mut value = unknown.clone();
                value["compatible"] = Value::Bool(true);
                value
            },
            {
                let mut value = known.clone();
                value["installations"][0]["receipt_state"] = serde_json::json!("missing");
                value
            },
            {
                let mut value = known.clone();
                value["installations"][0]["skill_sha256"] = Value::Null;
                value
            },
            {
                let mut value = known.clone();
                value["installations"][0]["supported_manifest_maximum"] = Value::Null;
                value
            },
            {
                let mut value = known.clone();
                value["installations"][0]["remediation"]["arguments"][3] =
                    serde_json::json!("openclaw");
                value
            },
            {
                let mut value = known.clone();
                value["compatible"] = Value::Bool(false);
                value
            },
        ] {
            assert!(
                jsonschema::draft202012::validate(&schema, &invalid).is_err(),
                "contradictory compatibility result was accepted: {}",
                canonical_json(&invalid)
            );
        }

        for (minimum, maximum) in [(4, 5), (1, 2), (5, 4)] {
            let mut invalid = known.clone();
            invalid["installations"][0]["supported_manifest_minimum"] = serde_json::json!(minimum);
            invalid["installations"][0]["supported_manifest_maximum"] = serde_json::json!(maximum);
            jsonschema::draft202012::validate(&schema, &invalid)
                .expect("cross-field contradiction is structurally valid");
            assert!(
                validate_agent_compatibility_semantics(&invalid).is_err(),
                "out-of-range compatibility result was accepted: {}",
                canonical_json(&invalid)
            );
        }
    }

    #[test]
    fn phase0_keeps_v1_v2_and_public_routes_frozen() {
        let v1 = manifest();
        let v2 = manifest_v2();
        let v3 = complete_manifest_v3_contract();
        assert_eq!(v1["schema_version"], 1);
        assert_eq!(v2["schema_version"], 2);
        assert_eq!(v1["commands"].as_array().map(Vec::len), Some(30));
        assert_eq!(v2["commands"].as_array().map(Vec::len), Some(30));
        assert_eq!(PUBLIC_SCHEMAS.len(), 11);

        let v2_commands = v2["commands"].as_array().expect("v2 commands");
        let v3_commands = v3["commands"].as_array().expect("v3 commands");
        assert_eq!(v3_commands.len(), 33);
        for (legacy, successor) in v2_commands.iter().zip(v3_commands.iter()) {
            let mut successor = successor.clone();
            successor
                .as_object_mut()
                .expect("successor command")
                .remove("input_schema_id");
            successor
                .as_object_mut()
                .expect("successor command")
                .remove("input_schema_sha256");
            let path = legacy["path"].as_str().expect("legacy path");
            if matches!(path, "agent" | "agent describe" | "agent doctor") {
                let mut legacy = legacy.clone();
                for field in [
                    "purpose",
                    "output_family",
                    "output_schema_id",
                    "output_schema_sha256",
                    "examples",
                ] {
                    successor
                        .as_object_mut()
                        .expect("successor command")
                        .remove(field);
                    legacy
                        .as_object_mut()
                        .expect("legacy command")
                        .remove(field);
                }
                assert_eq!(
                    successor, legacy,
                    "v3 changed a non-descriptive command field"
                );
            } else {
                assert_eq!(&successor, legacy, "v3 changed a preserved command row");
            }
        }
        let v2_capabilities = v2["capabilities"].as_array().expect("v2 capabilities");
        let v3_capabilities = v3["capabilities"].as_array().expect("v3 capabilities");
        for (legacy, successor) in v2_capabilities.iter().zip(v3_capabilities.iter()) {
            if legacy["id"] == "agent-mcp" {
                let mut legacy = legacy.clone();
                let mut successor = successor.clone();
                for field in ["summary", "contract_version"] {
                    legacy
                        .as_object_mut()
                        .expect("legacy capability")
                        .remove(field);
                    successor
                        .as_object_mut()
                        .expect("successor capability")
                        .remove(field);
                }
                assert_eq!(successor, legacy);
            } else {
                assert_eq!(successor, legacy);
            }
        }
        let v2_paths = v2_commands
            .iter()
            .map(|command| command["path"].as_str().expect("v2 path"))
            .collect::<BTreeSet<_>>();
        let v3_paths = v3_commands
            .iter()
            .map(|command| command["path"].as_str().expect("v3 path"))
            .collect::<BTreeSet<_>>();
        let expected_paths = v2_paths
            .into_iter()
            .chain(["agent compatibility", "household show", "household member"])
            .collect::<BTreeSet<_>>();
        assert_eq!(v3_paths, expected_paths);
        assert_eq!(v3["capabilities"].as_array().map(Vec::len), Some(11));
        assert_eq!(
            v3["mcp_inventory"]["tools"].as_array().map(Vec::len),
            Some(12)
        );

        for encoded in [canonical_json(&v1), canonical_json(&v2)] {
            for forbidden in [
                "\"id\":\"household-roster\"",
                "\"id\":\"household-profile\"",
                "\"id\":\"household-lifecycle\"",
                "heyfood_get_household",
                "prepare_household_change",
                "agent compatibility",
            ] {
                assert!(
                    !encoded.contains(forbidden),
                    "public compatibility view gained {forbidden}"
                );
            }
        }
        assert_eq!(
            sha256_hex(super::MANIFEST_SCHEMA.as_bytes()),
            "056011fb36521e89fae5540eda25c8e895df8a0f6e104df3a842821f3c672839"
        );
        assert_eq!(
            sha256_hex(super::MANIFEST_V2_SCHEMA.as_bytes()),
            "ed28909d5f7bd3afc296acac5af07e7f67acf146427fa566394cc2331e4b65fc"
        );
    }

    #[test]
    fn agent_visible_proposals_and_outcomes_contain_no_commit_authority() {
        for fixture in [
            PROPOSAL_CONTENT_FREE,
            PROPOSAL_ROSTER,
            PROPOSAL_PROFILE,
            CANCEL_OUTCOME,
            RECONCILIATION_OUTCOME,
        ] {
            let encoded = canonical_json(&parse(fixture));
            for forbidden in [
                "account_binding",
                "commit_credential",
                "commit_id",
                "effect_fingerprint",
                "lifecycle_generation",
                "operation_id",
                "proposal_digest",
                "repository_path",
                "single_use_nonce",
            ] {
                assert!(
                    !encoded.contains(forbidden),
                    "agent result exposed {forbidden}"
                );
            }
        }
    }

    #[test]
    fn matrix_and_policy_fixtures_freeze_the_phase0_boundary() {
        for (schema, expected) in [
            (
                CONTEXT_INPUT_SCHEMA,
                "e832c0e64e13bf3d91e59a339d795c03b1c9b45f5b2e4d6f03fe00d8dbaf8342",
            ),
            (
                MEMBER_INPUT_SCHEMA,
                "c9f4d347345f9dad7d308ad0c14a42abd4287a79156f43a42b5dbef2a058ecc8",
            ),
            (
                READ_SCHEMA,
                "9ab5a881284a18ca73346dbb5bacefb155230e8931eea87698bda722e0759618",
            ),
            (
                ACTION_SCHEMA,
                "eeec44e7cbb790c7de66e86821371e0c287b7b2b59597f140194b548eb0ca55b",
            ),
            (
                GET_CHANGE_INPUT_SCHEMA,
                "28601c8d46c80e440523d50632b307bf9710243fd33185cd7169731d5c8c7da4",
            ),
            (
                CANCEL_INPUT_SCHEMA,
                "6470d8f6ca6b97c01c080f9138bb43e920181eebd971db003dd2e507b3ba21be",
            ),
            (
                RECONCILE_INPUT_SCHEMA,
                "9d6caa7514e1308db3ce7b2f318c6b69eb03d504def7626b9e048f0c72c6b008",
            ),
            (
                PRESENTATION_SCHEMA,
                "9488244a01f7aef1b9116cd7c4a77e0fe017ccb6aadd1fe3099dc6b25aecd7ba",
            ),
            (
                OUTCOME_SCHEMA,
                "64df217e4ba7e1563df3b70da97c6c4a21c3ecbcdc03659da431440db0d9ddb0",
            ),
            (
                LOCAL_APPROVAL_SCHEMA,
                "64433ae9a5471308d7dce51c4b43e3f72a5903311953c02e1161c63c3200b5b3",
            ),
            (
                DISCLOSURE_SCHEMA,
                "c9acedd06068eba99fdb19c58300a3ed1c86fe023d83d9c78272bd3d8bd7cc36",
            ),
            (
                COMPATIBILITY_SCHEMA,
                "c2fccf908a221fdce96131fbf675705b2a222a26d64cfe954d4c177785b2ae97",
            ),
            (
                NATIVE_STATE_SCHEMA,
                "690c3480c9b4719482063665a49f6d70969ece2eb5e0ee29ee88cc8e1a494403",
            ),
            (
                MANIFEST_V3_SCHEMA,
                "0f9f078b541c34cd0159cd90390b44643e26f4a5961776e854a265edef048d2b",
            ),
        ] {
            assert_eq!(sha256_hex(schema.as_bytes()), expected);
        }

        let matrix = parse(COMMAND_TOOL_MATRIX);
        assert_eq!(matrix["baseline"]["command_count"], 30);
        assert_eq!(matrix["baseline"]["mcp_tool_count"], 6);
        assert_eq!(matrix["future_v3"]["command_count"], 33);
        assert_eq!(matrix["future_v3"]["mcp_tool_count"], 12);
        let tools = matrix["future_v3"]["mcp_tools"]
            .as_array()
            .expect("tool list");
        let names = tools
            .iter()
            .map(|tool| tool["name"].as_str().expect("tool name"))
            .collect::<BTreeSet<_>>();
        assert_eq!(names.len(), 12);
        assert!(
            !names
                .iter()
                .any(|name| name.contains("confirm") || name.contains("erase"))
        );

        let dg_r2 = parse(DG_R2);
        assert_eq!(dg_r2["rules"]["blind_retry_after_dispatch"], false);
        assert_eq!(dg_r2["rules"]["agent_commit_authority"], false);
        assert_eq!(dg_r2["boundaries"].as_array().map(Vec::len), Some(12));

        let tui = parse(TUI_GRAMMAR);
        assert_eq!(tui["activation"], "phase0_contract_only");
        assert_eq!(tui["commands"].as_array().map(Vec::len), Some(8));
        assert_eq!(tui["help_completion_registry_must_match"], true);

        let ledger = parse(APPLIED_COMMIT_PROOF);
        assert_eq!(
            ledger["executable_repository_test"]["name"],
            "phase0_agent_effects_execute_all_five_exact_once_repository_paths"
        );
        assert_eq!(
            ledger["executable_repository_test"]["resolves_before_persistence"],
            true
        );
        assert_eq!(
            ledger["executable_repository_test"]["co_committed_ledger_readback"],
            true
        );
        assert_eq!(
            ledger["executable_repository_test"]["exact_replay_after_persistence"],
            true
        );
        assert_eq!(
            ledger["executable_repository_test"]["conflicting_fingerprint_rejected"],
            true
        );
        assert_eq!(ledger["operations"].as_array().map(Vec::len), Some(5));
        assert!(
            ledger["operations"]
                .as_array()
                .expect("operations")
                .iter()
                .all(|operation| operation["fingerprint_frozen_after_complete_input"] == true)
        );
    }
}
