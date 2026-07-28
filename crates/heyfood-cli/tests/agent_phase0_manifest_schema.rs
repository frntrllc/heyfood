use std::collections::BTreeSet;

use serde_json::Value;

const SCHEMA: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../schemas/v1/heyfood-agent-manifest.schema.json"
));
const GOLDEN: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/agent/manifest-v1-golden.json"
));
const PROPOSAL_PRESENTATION_SCHEMA: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../schemas/v1/agent-proposal-presentation.schema.json"
));
const APPROVAL_PROTOCOL_SCHEMA: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../schemas/v1/agent-approval-protocol.schema.json"
));
const MCP_ENVIRONMENT_POLICY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/release-evidence/agent-native-phase0/mcp-environment-policy.json"
));

fn enum_values<'a>(schema: &'a Value, pointer: &str) -> BTreeSet<&'a str> {
    schema
        .pointer(pointer)
        .and_then(|value| value.as_array())
        .unwrap_or_else(|| panic!("missing enum at {pointer}"))
        .iter()
        .map(|value| value.as_str().expect("enum values are strings"))
        .collect()
}

#[test]
fn manifest_schema_freezes_public_status_and_audience_vocabulary() {
    let schema: Value = serde_json::from_str(SCHEMA).expect("manifest schema JSON");
    assert_eq!(
        enum_values(&schema, "/$defs/capability/properties/status/enum"),
        BTreeSet::from(["active", "deferred", "unavailable"])
    );
    assert_eq!(
        enum_values(&schema, "/$defs/command/properties/audience/enum"),
        BTreeSet::from(["agent_safe", "agent_unsupported", "human_terminal_only"])
    );
    assert!(
        !SCHEMA.contains("\"hidden\""),
        "hidden topology is never a public manifest status"
    );
}

#[test]
fn embedded_build_provenance_cannot_claim_container_artifact_digests() {
    let schema: Value = serde_json::from_str(SCHEMA).expect("manifest schema JSON");
    let build = schema
        .pointer("/$defs/build/properties")
        .and_then(Value::as_object)
        .expect("build properties");

    assert!(build.contains_key("build_input_digest_sha256"));
    for forbidden in [
        "executable_digest",
        "executable_sha256",
        "archive_digest",
        "archive_sha256",
        "artifact_digest",
    ] {
        assert!(
            !build.contains_key(forbidden),
            "{forbidden} would be self-referential embedded provenance"
        );
    }
}

#[test]
fn mcp_resource_limits_match_the_phase0_protocol_freeze() {
    let schema: Value = serde_json::from_str(SCHEMA).expect("manifest schema JSON");
    let limits = schema
        .pointer("/$defs/limits/properties")
        .and_then(Value::as_object)
        .expect("limit properties");

    let expected = [
        ("mcp_inbound_frame_bytes", 1_048_576),
        ("mcp_tool_arguments_bytes", 1_048_576),
        ("mcp_structured_result_bytes", 4_194_304),
        ("sse_line_bytes", 65_536),
        ("sse_event_bytes", 1_048_576),
        ("stream_event_count", 4_096),
        ("stream_total_bytes", 4_194_304),
        ("outstanding_requests", 8),
        ("remote_in_flight", 1),
        ("queued_requests", 7),
        ("page_records", 100),
    ];
    for (name, value) in expected {
        assert_eq!(limits[name]["const"].as_u64(), Some(value), "limit {name}");
    }
}

#[test]
fn command_contract_requires_authority_retry_and_side_effect_metadata() {
    let schema: Value = serde_json::from_str(SCHEMA).expect("manifest schema JSON");
    let required = schema
        .pointer("/$defs/command/required")
        .and_then(Value::as_array)
        .expect("command required fields")
        .iter()
        .map(|value| value.as_str().expect("required field"))
        .collect::<BTreeSet<_>>();

    for field in [
        "audience",
        "input_channel",
        "output_schema_id",
        "output_schema_sha256",
        "operation_class",
        "product_state_mutation",
        "credential_side_effect_possible",
        "required_scopes",
        "authorization_upgrade_command",
        "retry_class",
        "reconciliation_command",
        "human_confirmation",
        "interactivity",
        "browser_handoff",
    ] {
        assert!(required.contains(field), "missing required field {field}");
    }
}

fn required_fields(schema: &Value, pointer: &str) -> BTreeSet<String> {
    schema
        .pointer(pointer)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("missing required array at {pointer}"))
        .iter()
        .map(|value| value.as_str().expect("required field").to_owned())
        .collect()
}

fn assert_required_object(schema: &Value, pointer: &str, instance: &Value) {
    let object = instance
        .as_object()
        .unwrap_or_else(|| panic!("fixture at {pointer} must be an object"));
    for field in required_fields(schema, pointer) {
        assert!(
            object.contains_key(&field),
            "golden manifest omits required field {field} from {pointer}"
        );
    }
}

#[test]
fn golden_manifest_contains_every_required_nullable_and_surface_field() {
    let schema: Value = serde_json::from_str(SCHEMA).expect("manifest schema JSON");
    let golden: Value = serde_json::from_str(GOLDEN).expect("golden manifest JSON");

    assert_required_object(&schema, "/required", &golden);
    assert_required_object(&schema, "/$defs/build/required", &golden["build"]);
    assert_required_object(
        &schema,
        "/$defs/compatibility/required",
        &golden["compatibility"],
    );
    assert_required_object(
        &schema,
        "/$defs/automation_surfaces/required",
        &golden["automation_surfaces"],
    );
    assert_required_object(&schema, "/$defs/limits/required", &golden["limits"]);
    for capability in golden["capabilities"].as_array().expect("capabilities") {
        assert_required_object(&schema, "/$defs/capability/required", capability);
    }
    for command in golden["commands"].as_array().expect("commands") {
        assert_required_object(&schema, "/$defs/command/required", command);
    }

    assert_eq!(golden["automation_surfaces"]["mcp_stdio"], "deferred");
    assert_eq!(
        golden["automation_surfaces"]["tui_automation"],
        "unsupported"
    );
}

#[test]
fn schema_freezes_cross_field_authority_invariants() {
    let schema: Value = serde_json::from_str(SCHEMA).expect("manifest schema JSON");
    let rules = schema
        .pointer("/$defs/command/allOf")
        .and_then(Value::as_array)
        .expect("command cross-field rules");
    assert_eq!(rules.len(), 4);

    let encoded = serde_json::to_string(rules).unwrap();
    for invariant in [
        "\"agent_safe\"",
        "\"product_state_mutation\":{\"const\":false}",
        "\"human_confirmation\":{\"const\":\"none\"}",
        "\"no_blind_retry\"",
        "\"reconcile_before_retry\"",
        "\"human_terminal_only\"",
        "\"safe_read\"",
        "\"operation_class\":{\"enum\":[\"local_read\",\"remote_read\"]}",
    ] {
        assert!(
            encoded.contains(invariant),
            "schema omits cross-field invariant {invariant}"
        );
    }
}

#[test]
fn proposal_presentation_is_allowlisted_and_contains_no_commit_capability_field() {
    let schema: Value =
        serde_json::from_str(PROPOSAL_PRESENTATION_SCHEMA).expect("proposal schema JSON");
    let required = required_fields(&schema, "/required");
    for field in [
        "mutation_family",
        "operation",
        "approval_reference",
        "proposal_digest_sha256",
        "expires_at",
        "display",
        "resource_references",
        "preconditions",
        "items",
    ] {
        assert!(required.contains(field), "missing proposal field {field}");
    }
    assert_eq!(schema["additionalProperties"], false);
    for forbidden in [
        "confirmation_token",
        "idempotency_key",
        "commit_token",
        "session_binding_token",
        "access_token",
        "refresh_token",
    ] {
        assert!(
            !PROPOSAL_PRESENTATION_SCHEMA.contains(forbidden),
            "agent presentation schema exposes {forbidden}"
        );
    }
}

#[test]
fn approval_protocol_freezes_backend_nonce_and_single_use_states() {
    let schema: Value =
        serde_json::from_str(APPROVAL_PROTOCOL_SCHEMA).expect("approval protocol schema JSON");
    assert_eq!(
        schema
            .pointer("/$defs/opaque_256/pattern")
            .and_then(Value::as_str),
        Some("^[A-Za-z0-9_-]{43}$")
    );
    let session_required = required_fields(&schema, "/$defs/session_created/required");
    assert!(session_required.contains("approval_session_id"));
    assert!(session_required.contains("session_binding_token"));
    let statuses = enum_values(
        &schema,
        "/$defs/approval_observation/properties/status/enum",
    );
    for status in [
        "prepared",
        "awaiting_human",
        "approved",
        "declined",
        "expired",
        "invalidated",
        "cancelled",
        "committing",
        "committed",
        "reconciliation_required",
    ] {
        assert!(statuses.contains(status), "missing approval state {status}");
    }
}

#[test]
fn mcp_environment_policy_rejects_overrides_before_credentials() {
    let policy: Value =
        serde_json::from_str(MCP_ENVIRONMENT_POLICY).expect("MCP environment policy JSON");
    assert_eq!(policy["service"]["origin"], "https://api.hello.food");
    assert_eq!(policy["service"]["network_policy"], "production");
    assert_eq!(policy["service"]["api_key_source"], "none");
    assert_eq!(policy["credentials"]["store"], "native_account_bound");
    assert_eq!(policy["credentials"]["legacy_file_fallback"], false);
    assert_eq!(
        policy["environment"]["reject_prefixes_before_credential_access"],
        serde_json::json!(["HEYFOOD_"])
    );
    assert_eq!(
        policy["environment"]["host_registration_environment"],
        serde_json::json!({})
    );
    assert_eq!(
        policy["environment"]["inherited_heyfood_prefix_allowed"],
        serde_json::json!([])
    );
}
