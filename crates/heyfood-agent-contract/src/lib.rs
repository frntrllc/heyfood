//! Deterministic, network-free contracts embedded in the heyfood executable.

#![forbid(unsafe_code)]

use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const GUIDE: &str = include_str!("../../../docs/AGENT_INTEGRATION.md");
pub const SAFETY: &str = include_str!("../../../docs/AGENT_SAFETY.md");
pub const MANIFEST_SCHEMA: &str =
    include_str!("../../../schemas/v1/heyfood-agent-manifest.schema.json");
pub const SCHEMA_INDEX_SCHEMA: &str =
    include_str!("../../../schemas/v1/heyfood-agent-schema-index.schema.json");
pub const DOCTOR_SCHEMA: &str =
    include_str!("../../../schemas/v1/heyfood-agent-doctor.schema.json");
pub const PUBLIC_OUTPUT_SCHEMA: &str =
    include_str!("../../../schemas/v1/heyfood-output.schema.json");
pub const PROPOSAL_PRESENTATION_SCHEMA: &str =
    include_str!("../../../schemas/v1/agent-proposal-presentation.schema.json");

pub const MANIFEST_SCHEMA_ID: &str =
    "https://hey.food/schemas/v1/heyfood-agent-manifest.schema.json";
pub const SCHEMA_INDEX_SCHEMA_ID: &str =
    "https://hey.food/schemas/v1/heyfood-agent-schema-index.schema.json";
pub const DOCTOR_SCHEMA_ID: &str = "https://hey.food/schemas/v1/heyfood-agent-doctor.schema.json";
pub const PUBLIC_OUTPUT_SCHEMA_ID: &str =
    "https://github.com/frntrllc/heyfood/blob/main/schemas/v1/heyfood-output.schema.json";
pub const PROPOSAL_PRESENTATION_SCHEMA_ID: &str =
    "https://hey.food/schemas/v1/agent-proposal-presentation.schema.json";

pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
pub const MAX_SCHEMA_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbeddedSchema {
    Manifest,
    SchemaIndex,
    Doctor,
    PublicOutput,
    ProposalPresentation,
}

impl EmbeddedSchema {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Manifest => "manifest",
            Self::SchemaIndex => "schema-index",
            Self::Doctor => "doctor",
            Self::PublicOutput => "output",
            Self::ProposalPresentation => "proposal-presentation",
        }
    }

    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Manifest => MANIFEST_SCHEMA_ID,
            Self::SchemaIndex => SCHEMA_INDEX_SCHEMA_ID,
            Self::Doctor => DOCTOR_SCHEMA_ID,
            Self::PublicOutput => PUBLIC_OUTPUT_SCHEMA_ID,
            Self::ProposalPresentation => PROPOSAL_PRESENTATION_SCHEMA_ID,
        }
    }

    #[must_use]
    pub const fn document(self) -> &'static str {
        match self {
            Self::Manifest => MANIFEST_SCHEMA,
            Self::SchemaIndex => SCHEMA_INDEX_SCHEMA,
            Self::Doctor => DOCTOR_SCHEMA,
            Self::PublicOutput => PUBLIC_OUTPUT_SCHEMA,
            Self::ProposalPresentation => PROPOSAL_PRESENTATION_SCHEMA,
        }
    }
}

pub const PUBLIC_SCHEMAS: [EmbeddedSchema; 5] = [
    EmbeddedSchema::Manifest,
    EmbeddedSchema::SchemaIndex,
    EmbeddedSchema::Doctor,
    EmbeddedSchema::PublicOutput,
    EmbeddedSchema::ProposalPresentation,
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
    output_schema_sha256: Option<&'static str>,
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

#[allow(clippy::too_many_arguments)]
const fn command(
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
        ),
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
        ),
        command(
            "agent guide",
            "Print the embedded agent integration guide.",
            "agent_safe",
            "none",
            "agent_guide_v1",
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
            &["heyfood agent guide"],
        ),
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
            &["heyfood agent schema manifest"],
        ),
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
            "data_stdin_plus_controlling_terminal",
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
            "no_blind_retry",
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
            "no_blind_retry",
            "none",
            "independent_browser_or_device",
            "required",
            NONE,
        ),
        command(
            "grocery",
            "Read the active Grocery list.",
            "agent_unsupported",
            "none",
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
            "none",
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
            "none",
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
            "data_stdin_plus_controlling_terminal",
            "grocery_mutation_proposal_v1",
            "one_json_value",
            "prepare",
            true,
            true,
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
            "data_stdin_plus_controlling_terminal",
            "grocery_mutation_proposal_v1",
            "one_json_value",
            "prepare",
            true,
            true,
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
            "data_stdin_plus_controlling_terminal",
            "grocery_mutation_proposal_v1",
            "one_json_value",
            "prepare",
            true,
            true,
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
            "data_stdin_plus_controlling_terminal",
            "grocery_mutation_proposal_v1",
            "one_json_value",
            "prepare",
            true,
            true,
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
            "grocery_export_v1",
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
            "data_stdin_plus_controlling_terminal",
            "grocery_mutation_result_v1",
            "one_json_value",
            "confirm",
            true,
            true,
            true,
            GROCERY_WRITE,
            "no_blind_retry",
            "attached_terminal",
            "controlling_terminal",
            "none",
            NONE,
        ),
        command(
            "watch",
            "Read Menu Watch subscriptions.",
            "agent_unsupported",
            "none",
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
            "none",
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
            "data_stdin_plus_controlling_terminal",
            "menu_watch_v1",
            "one_json_value",
            "mutation",
            true,
            true,
            true,
            WATCH,
            "no_blind_retry",
            "attached_terminal",
            "controlling_terminal",
            "none",
            NONE,
        ),
        command(
            "watch remove",
            "Remove a Menu Watch after human terminal review.",
            "human_terminal_only",
            "data_stdin_plus_controlling_terminal",
            "menu_watch_result_v1",
            "one_json_value",
            "mutation",
            true,
            true,
            true,
            WATCH,
            "no_blind_retry",
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
            "additive_optional_fields": true
        },
        "automation_surfaces": {
            "one_shot_json": "active",
            "mcp_stdio": "deferred",
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
            {"id": "agent-mcp", "status": "deferred", "summary": "The local typed MCP server is not active in this phase.", "contract_version": null},
            {"id": "health", "status": "deferred", "summary": "Health is outside the supported release contract.", "contract_version": null},
            {"id": "native-voice", "status": "deferred", "summary": "Native voice is not enabled in the default artifact.", "contract_version": null},
            {"id": "windows-distribution", "status": "deferred", "summary": "Windows source CI is active; public Windows distribution is deferred.", "contract_version": null}
        ],
        "commands": commands()
    });
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
    json!({
        "guide_sha256": sha256_hex(GUIDE.as_bytes()),
        "safety_sha256": sha256_hex(SAFETY.as_bytes()),
        "manifest_schema_sha256": sha256_hex(MANIFEST_SCHEMA.as_bytes()),
        "schema_index_schema_sha256": sha256_hex(SCHEMA_INDEX_SCHEMA.as_bytes()),
        "doctor_schema_sha256": sha256_hex(DOCTOR_SCHEMA.as_bytes()),
        "public_output_schema_sha256": sha256_hex(PUBLIC_OUTPUT_SCHEMA.as_bytes()),
        "proposal_presentation_schema_sha256": sha256_hex(PROPOSAL_PRESENTATION_SCHEMA.as_bytes()),
    })
}

#[must_use]
pub fn doctor_document() -> Value {
    let manifest = manifest();
    json!({
        "schema_version": 1,
        "ok": true,
        "binary_version": env!("CARGO_PKG_VERSION"),
        "target": env!("HEYFOOD_BUILD_TARGET"),
        "manifest_schema_version": manifest["schema_version"],
        "manifest_sha256": sha256_hex(canonical_json(&manifest).as_bytes()),
        "embedded": embedded_digests(),
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
    fn embedded_documents_are_valid_and_bounded() {
        for schema in [
            EmbeddedSchema::Manifest,
            EmbeddedSchema::SchemaIndex,
            EmbeddedSchema::Doctor,
            EmbeddedSchema::PublicOutput,
            EmbeddedSchema::ProposalPresentation,
        ] {
            assert!(schema.document().len() <= MAX_SCHEMA_BYTES);
            let parsed: Value = serde_json::from_str(schema.document()).unwrap();
            assert_eq!(parsed["$id"], schema.id());
        }
        assert!(GUIDE.len() <= MAX_SCHEMA_BYTES);
        assert!(SAFETY.len() <= MAX_SCHEMA_BYTES);
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
                "schema-index",
                "doctor",
                "output",
                "proposal-presentation"
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
