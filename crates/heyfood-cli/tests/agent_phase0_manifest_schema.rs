use std::collections::BTreeSet;

use serde_json::Value;

const SCHEMA: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../schemas/v1/heyfood-agent-manifest.schema.json"
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
        "operation_class",
        "product_state_mutation",
        "credential_side_effect_possible",
        "required_scopes",
        "retry_class",
        "human_confirmation",
    ] {
        assert!(required.contains(field), "missing required field {field}");
    }
}
