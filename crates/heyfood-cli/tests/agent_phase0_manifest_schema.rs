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
const APPROVAL_PROTOCOL_LIFECYCLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/agent/approval-protocol-v1-lifecycle.json"
));
const APPROVAL_PROTOCOL_CONTRACT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/AGENT_APPROVAL_CONTRACT.md"
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

fn value_has_type(instance: &Value, expected: &str) -> bool {
    match expected {
        "object" => instance.is_object(),
        "array" => instance.is_array(),
        "string" => instance.is_string(),
        "integer" => instance.as_i64().is_some() || instance.as_u64().is_some(),
        "boolean" => instance.is_boolean(),
        "null" => instance.is_null(),
        other => panic!("unsupported fixture-validator JSON type {other}"),
    }
}

fn string_matches_pattern(value: &str, pattern: &str) -> bool {
    match pattern {
        "^[0-9a-f]{64}$" => {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        }
        "^[A-Za-z0-9_-]{43}$" => {
            value.len() == 43
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        }
        "^[a-z][a-z0-9_]{0,63}$" => {
            (1..=64).contains(&value.len())
                && value.as_bytes()[0].is_ascii_lowercase()
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        }
        "^https://auth\\.hello\\.food/agent-approval/[A-Za-z0-9_-]{43}$" => value
            .strip_prefix("https://auth.hello.food/agent-approval/")
            .is_some_and(|reference| {
                reference.len() == 43
                    && reference
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            }),
        "^[^\\u0000-\\u0008\\u000b\\u000c\\u000e-\\u001f\\u007f-\\u009f\\u2028\\u2029]*$" => {
            value.chars().all(|character| {
                !matches!(
                    character,
                    '\u{0000}'..='\u{0008}'
                        | '\u{000b}'
                        | '\u{000c}'
                        | '\u{000e}'..='\u{001f}'
                        | '\u{007f}'..='\u{009f}'
                        | '\u{2028}'
                        | '\u{2029}'
                )
            })
        }
        other => panic!("unsupported fixture-validator pattern {other}"),
    }
}

fn string_has_format(value: &str, format: &str) -> bool {
    match format {
        "uuid" => {
            value.len() == 36
                && value.bytes().enumerate().all(|(index, byte)| match index {
                    8 | 13 | 18 | 23 => byte == b'-',
                    _ => byte.is_ascii_hexdigit(),
                })
        }
        "date-time" => {
            value.len() >= 20
                && value.as_bytes().get(10) == Some(&b'T')
                && (value.ends_with('Z')
                    || value
                        .get(19..)
                        .is_some_and(|suffix| suffix.contains('+') || suffix.contains('-')))
        }
        "uri" => {
            (value.starts_with("https://") || value.starts_with("http://"))
                && !value.chars().any(char::is_whitespace)
        }
        other => panic!("unsupported fixture-validator format {other}"),
    }
}

fn validate_schema_instance(
    schema_document: &Value,
    proposal_schema: &Value,
    schema: &Value,
    instance: &Value,
) -> Result<(), String> {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        if let Some(pointer) = reference.strip_prefix('#') {
            let target = schema_document
                .pointer(pointer)
                .ok_or_else(|| format!("unresolved internal schema reference {reference}"))?;
            return validate_schema_instance(schema_document, proposal_schema, target, instance);
        }
        if reference == "https://hey.food/schemas/v1/agent-proposal-presentation.schema.json" {
            return validate_schema_instance(
                proposal_schema,
                proposal_schema,
                proposal_schema,
                instance,
            );
        }
        return Err(format!("unresolved external schema reference {reference}"));
    }

    if let Some(branches) = schema.get("oneOf").and_then(Value::as_array) {
        let matches = branches
            .iter()
            .filter(|branch| {
                validate_schema_instance(schema_document, proposal_schema, branch, instance).is_ok()
            })
            .count();
        if matches != 1 {
            return Err(format!("oneOf matched {matches} branches"));
        }
    }
    if let Some(branches) = schema.get("allOf").and_then(Value::as_array) {
        for branch in branches {
            validate_schema_instance(schema_document, proposal_schema, branch, instance)?;
        }
    }
    if let Some(condition) = schema.get("if") {
        let condition_matches =
            validate_schema_instance(schema_document, proposal_schema, condition, instance).is_ok();
        let consequence = if condition_matches {
            schema.get("then")
        } else {
            schema.get("else")
        };
        if let Some(consequence) = consequence {
            validate_schema_instance(schema_document, proposal_schema, consequence, instance)?;
        }
    }

    if let Some(expected) = schema.get("type") {
        let matches = match expected {
            Value::String(expected) => value_has_type(instance, expected),
            Value::Array(expected) => expected
                .iter()
                .filter_map(Value::as_str)
                .any(|expected| value_has_type(instance, expected)),
            _ => false,
        };
        if !matches {
            return Err(format!("unexpected instance type for {expected}"));
        }
    }
    if let Some(expected) = schema.get("const")
        && instance != expected
    {
        return Err(format!(
            "const mismatch: expected {expected}, got {instance}"
        ));
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array)
        && !allowed.contains(instance)
    {
        return Err(format!("enum rejects {instance}"));
    }

    if let Some(object) = instance.as_object() {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for field in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(field) {
                    return Err(format!("missing required field {field}"));
                }
            }
        }
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for (field, value) in object {
                if let Some(property_schema) = properties.get(field) {
                    validate_schema_instance(
                        schema_document,
                        proposal_schema,
                        property_schema,
                        value,
                    )
                    .map_err(|error| format!("{field}: {error}"))?;
                } else if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
                    return Err(format!("unknown field {field}"));
                }
            }
        }
    }

    if let Some(array) = instance.as_array() {
        if let Some(maximum) = schema.get("maxItems").and_then(Value::as_u64)
            && array.len() as u64 > maximum
        {
            return Err(format!("array exceeds maxItems {maximum}"));
        }
        if let Some(item_schema) = schema.get("items") {
            for item in array {
                validate_schema_instance(schema_document, proposal_schema, item_schema, item)?;
            }
        }
    }

    if let Some(value) = instance.as_str() {
        let length = value.chars().count() as u64;
        if schema
            .get("minLength")
            .and_then(Value::as_u64)
            .is_some_and(|minimum| length < minimum)
        {
            return Err("string is shorter than minLength".to_owned());
        }
        if schema
            .get("maxLength")
            .and_then(Value::as_u64)
            .is_some_and(|maximum| length > maximum)
        {
            return Err("string is longer than maxLength".to_owned());
        }
        if let Some(pattern) = schema.get("pattern").and_then(Value::as_str)
            && !string_matches_pattern(value, pattern)
        {
            return Err(format!("string does not match {pattern}"));
        }
        if let Some(format) = schema.get("format").and_then(Value::as_str)
            && !string_has_format(value, format)
        {
            return Err(format!("string does not have format {format}"));
        }
    }

    if let Some(minimum) = schema.get("minimum").and_then(Value::as_i64)
        && instance.as_i64().is_some_and(|value| value < minimum)
    {
        return Err(format!("integer is less than {minimum}"));
    }

    Ok(())
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
    let statuses = enum_values(&schema, "/$defs/approval_status/enum");
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

fn schema_definition_for_kind<'a>(schema: &'a Value, kind: &str) -> &'a Value {
    schema["$defs"]
        .as_object()
        .expect("approval definitions")
        .values()
        .find(|definition| {
            definition
                .pointer("/properties/kind/const")
                .and_then(Value::as_str)
                == Some(kind)
        })
        .unwrap_or_else(|| panic!("missing approval schema definition for {kind}"))
}

fn lifecycle_scenario<'a>(fixture: &'a Value, id: &str) -> &'a Value {
    fixture["scenarios"]
        .as_array()
        .expect("approval lifecycle scenarios")
        .iter()
        .find(|scenario| scenario["id"] == id)
        .unwrap_or_else(|| panic!("missing lifecycle scenario {id}"))
}

fn lifecycle_envelopes(fixture: &Value) -> Vec<&Value> {
    fixture["scenarios"]
        .as_array()
        .expect("approval lifecycle scenarios")
        .iter()
        .flat_map(|scenario| {
            scenario["envelopes"]
                .as_array()
                .expect("scenario envelopes")
        })
        .collect()
}

fn envelope_of_kind<'a>(scenario: &'a Value, kind: &str) -> &'a Value {
    scenario["envelopes"]
        .as_array()
        .expect("scenario envelopes")
        .iter()
        .find(|envelope| envelope["kind"] == kind)
        .unwrap_or_else(|| panic!("scenario omits {kind}"))
}

#[test]
fn approval_lifecycle_fixture_covers_every_strict_wire_envelope() {
    let schema: Value =
        serde_json::from_str(APPROVAL_PROTOCOL_SCHEMA).expect("approval protocol schema JSON");
    let proposal_schema: Value =
        serde_json::from_str(PROPOSAL_PRESENTATION_SCHEMA).expect("proposal schema JSON");
    let fixture: Value =
        serde_json::from_str(APPROVAL_PROTOCOL_LIFECYCLE).expect("approval lifecycle JSON");

    let registered = schema["oneOf"]
        .as_array()
        .expect("approval envelope registry")
        .iter()
        .map(|entry| {
            entry["$ref"]
                .as_str()
                .expect("envelope reference")
                .trim_start_matches("#/$defs/")
                .to_owned()
        })
        .collect::<BTreeSet<_>>();
    let envelopes = lifecycle_envelopes(&fixture);
    let fixture_kinds = envelopes
        .iter()
        .map(|envelope| {
            envelope["kind"]
                .as_str()
                .expect("fixture envelope kind")
                .to_owned()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(fixture_kinds, registered);

    for envelope in envelopes {
        let kind = envelope["kind"].as_str().expect("kind");
        validate_schema_instance(&schema, &proposal_schema, &schema, envelope)
            .unwrap_or_else(|error| panic!("{kind} fixture violates the complete schema: {error}"));
        let definition = schema_definition_for_kind(&schema, kind);
        assert_eq!(
            definition["additionalProperties"],
            Value::Bool(false),
            "{kind} must reject unknown fields"
        );
        assert_required_object(definition, "/required", envelope);
        let allowed = definition["properties"]
            .as_object()
            .expect("envelope properties")
            .keys()
            .collect::<BTreeSet<_>>();
        let actual = envelope
            .as_object()
            .expect("envelope object")
            .keys()
            .collect::<BTreeSet<_>>();
        assert_eq!(
            actual, allowed,
            "{kind} fixture must freeze every wire field"
        );
        assert_eq!(
            definition["properties"]["kind"]["const"], envelope["kind"],
            "{kind} must select one unambiguous schema"
        );
    }
}

#[test]
fn approval_lifecycles_preserve_identity_and_only_use_legal_transitions() {
    let fixture: Value =
        serde_json::from_str(APPROVAL_PROTOCOL_LIFECYCLE).expect("approval lifecycle JSON");
    let legal_transitions = fixture["legal_transitions"]
        .as_array()
        .expect("legal transitions")
        .iter()
        .map(|transition| {
            let transition = transition.as_array().expect("transition pair");
            (
                transition[0].as_str().expect("from state"),
                transition[1].as_str().expect("to state"),
            )
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        legal_transitions,
        BTreeSet::from([
            ("approved", "committing"),
            ("awaiting_human", "approved"),
            ("awaiting_human", "cancelled"),
            ("awaiting_human", "declined"),
            ("awaiting_human", "expired"),
            ("awaiting_human", "invalidated"),
            ("committing", "committed"),
            ("committing", "reconciliation_required"),
            ("prepared", "awaiting_human"),
            ("prepared", "cancelled"),
            ("prepared", "expired"),
            ("prepared", "invalidated"),
            ("reconciliation_required", "committed"),
            ("reconciliation_required", "invalidated"),
        ]),
        "the fixture must freeze exactly the normative state machine"
    );
    let terminal_states = BTreeSet::from([
        "cancelled",
        "committed",
        "declined",
        "expired",
        "invalidated",
    ]);
    for scenario in fixture["scenarios"].as_array().expect("scenarios") {
        let states = scenario["expected_states"]
            .as_array()
            .expect("expected states")
            .iter()
            .map(|state| state.as_str().expect("state"))
            .collect::<Vec<_>>();
        for transition in states.windows(2) {
            assert!(
                legal_transitions.contains(&(transition[0], transition[1])),
                "scenario {} contains illegal transition {} -> {}",
                scenario["id"].as_str().expect("scenario id"),
                transition[0],
                transition[1]
            );
        }
        let terminal = states.last().copied().expect("terminal state");
        assert!(
            terminal_states.contains(terminal),
            "scenario {} must end in one terminal state",
            scenario["id"].as_str().expect("scenario id")
        );
        assert!(
            !legal_transitions.iter().any(|(from, _)| *from == terminal),
            "terminal state {terminal} must have no outgoing transition"
        );
    }

    let committed = lifecycle_scenario(&fixture, "approved_commit_after_uncertain_observation");
    let proposal_request = envelope_of_kind(committed, "proposal_create_request");
    let proposal_created = envelope_of_kind(committed, "proposal_created");
    let human_view = envelope_of_kind(committed, "human_proposal_view");
    let decision_request = envelope_of_kind(committed, "human_decision_request");
    let decision_result = envelope_of_kind(committed, "human_decision_result");
    let commit_request = envelope_of_kind(committed, "commit_request");
    let commit_receipt = envelope_of_kind(committed, "commit_receipt");
    let observations = committed["envelopes"]
        .as_array()
        .expect("commit envelopes")
        .iter()
        .filter(|envelope| envelope["kind"] == "approval_observation")
        .collect::<Vec<_>>();

    assert_eq!(proposal_created["presentation"], human_view["presentation"]);
    assert_eq!(
        proposal_created["presentation"]["proposal_digest_sha256"],
        proposal_request["expected_proposal_digest_sha256"]
    );
    assert_eq!(
        proposal_created["presentation"]["approval_reference"],
        proposal_created["approval_url"]
            .as_str()
            .expect("approval URL")
            .rsplit_once('/')
            .expect("approval URL reference")
            .1
    );
    assert_eq!(
        proposal_created["proposal_request_id"],
        proposal_request["proposal_request_id"]
    );
    assert_eq!(
        proposal_created["approval_session_id"],
        proposal_request["approval_session_id"]
    );
    assert_eq!(
        proposal_created["operation_id"],
        proposal_request["operation_id"]
    );
    assert_eq!(
        decision_request["approval_id"],
        proposal_created["approval_id"]
    );
    assert_eq!(
        decision_result["approval_id"],
        proposal_created["approval_id"]
    );
    assert_eq!(
        decision_request["proposal_digest_sha256"],
        proposal_created["presentation"]["proposal_digest_sha256"]
    );
    assert_eq!(
        decision_result["proposal_digest_sha256"],
        proposal_created["presentation"]["proposal_digest_sha256"]
    );
    assert_eq!(
        decision_request["decision_nonce"],
        human_view["decision_nonce"]
    );
    assert_eq!(
        commit_request["approval_session_id"],
        proposal_created["approval_session_id"]
    );
    assert_eq!(
        commit_request["operation_id"],
        proposal_created["operation_id"]
    );
    assert_eq!(
        commit_request["proposal_digest_sha256"],
        proposal_created["presentation"]["proposal_digest_sha256"]
    );
    assert_ne!(
        proposal_request["proposal_request_id"], commit_request["commit_request_id"],
        "proposal and commit use distinct idempotency identities"
    );
    assert_ne!(
        commit_request["operation_id"], commit_request["commit_request_id"],
        "semantic operation identity is not idempotency authority"
    );
    assert_eq!(
        commit_receipt["commit_request_id"],
        commit_request["commit_request_id"]
    );
    assert_eq!(
        commit_receipt["approval_id"],
        proposal_created["approval_id"]
    );
    assert_eq!(
        commit_receipt["operation_id"],
        proposal_created["operation_id"]
    );
    assert_eq!(
        commit_receipt["proposal_digest_sha256"],
        proposal_created["presentation"]["proposal_digest_sha256"]
    );
    for observation in &observations {
        assert_eq!(observation["approval_id"], proposal_created["approval_id"]);
        assert_eq!(
            observation["approval_session_id"],
            proposal_created["approval_session_id"]
        );
        assert_eq!(
            observation["operation_id"],
            proposal_created["operation_id"]
        );
        assert_eq!(
            observation["proposal_digest_sha256"],
            proposal_created["presentation"]["proposal_digest_sha256"]
        );
        assert_eq!(observation["cancellation_receipt"], Value::Null);
    }
    let committed_observation = observations
        .iter()
        .find(|observation| observation["status"] == "committed")
        .expect("committed observation");
    assert_eq!(committed_observation["commit_receipt"], *commit_receipt);
    let uncertain = envelope_of_kind(committed, "protocol_error");
    assert_eq!(uncertain["approval_id"], proposal_created["approval_id"]);
    assert_eq!(uncertain["operation_id"], proposal_created["operation_id"]);

    let cancelled = lifecycle_scenario(&fixture, "cancelled_before_human_decision");
    let cancelled_proposal_request = envelope_of_kind(cancelled, "proposal_create_request");
    let cancelled_proposal = envelope_of_kind(cancelled, "proposal_created");
    let cancel_request = envelope_of_kind(cancelled, "cancel_request");
    let cancellation_receipt = envelope_of_kind(cancelled, "cancellation_receipt");
    let cancelled_observation = envelope_of_kind(cancelled, "approval_observation");
    assert_ne!(
        cancelled_proposal["approval_id"], proposal_created["approval_id"],
        "commit and cancel paths must use distinct approvals"
    );
    assert_ne!(
        cancelled_proposal["presentation"]["proposal_digest_sha256"],
        proposal_created["presentation"]["proposal_digest_sha256"]
    );
    assert_eq!(
        cancelled_proposal["proposal_request_id"],
        cancelled_proposal_request["proposal_request_id"]
    );
    assert_eq!(
        cancelled_proposal["operation_id"],
        cancelled_proposal_request["operation_id"]
    );
    assert_eq!(
        cancelled_proposal["approval_session_id"],
        cancelled_proposal_request["approval_session_id"]
    );
    assert_eq!(
        cancelled_proposal["presentation"]["proposal_digest_sha256"],
        cancelled_proposal_request["expected_proposal_digest_sha256"]
    );
    assert_eq!(
        cancel_request["approval_session_id"],
        cancelled_proposal["approval_session_id"]
    );
    assert_eq!(
        cancel_request["proposal_digest_sha256"],
        cancelled_proposal["presentation"]["proposal_digest_sha256"]
    );
    assert_eq!(
        cancellation_receipt["cancel_request_id"],
        cancel_request["cancel_request_id"]
    );
    assert_eq!(
        cancellation_receipt["approval_id"],
        cancelled_proposal["approval_id"]
    );
    assert_eq!(
        cancellation_receipt["proposal_digest_sha256"],
        cancelled_proposal["presentation"]["proposal_digest_sha256"]
    );
    assert_eq!(
        cancelled_observation["cancellation_receipt"],
        *cancellation_receipt
    );
    assert_eq!(
        cancelled_observation["approval_id"],
        cancelled_proposal["approval_id"]
    );
    assert_eq!(
        cancelled_observation["approval_session_id"],
        cancelled_proposal["approval_session_id"]
    );
    assert_eq!(
        cancelled_observation["operation_id"],
        cancelled_proposal["operation_id"]
    );
    assert_eq!(
        cancelled_observation["proposal_digest_sha256"],
        cancelled_proposal["presentation"]["proposal_digest_sha256"]
    );
    assert_eq!(cancelled_observation["commit_receipt"], Value::Null);

    let declined = lifecycle_scenario(&fixture, "declined_by_authenticated_human");
    let declined_proposal_request = envelope_of_kind(declined, "proposal_create_request");
    let declined_proposal = envelope_of_kind(declined, "proposal_created");
    let declined_view = envelope_of_kind(declined, "human_proposal_view");
    let declined_request = envelope_of_kind(declined, "human_decision_request");
    let declined_result = envelope_of_kind(declined, "human_decision_result");
    let declined_observation = envelope_of_kind(declined, "approval_observation");
    assert_eq!(
        declined_proposal["presentation"],
        declined_view["presentation"]
    );
    assert_eq!(declined_request["decision"], "decline");
    assert_eq!(declined_result["status"], "declined");
    assert_ne!(
        declined_proposal["approval_id"],
        proposal_created["approval_id"]
    );
    assert_ne!(
        declined_proposal["approval_id"],
        cancelled_proposal["approval_id"]
    );
    assert_eq!(
        declined_proposal["proposal_request_id"],
        declined_proposal_request["proposal_request_id"]
    );
    assert_eq!(
        declined_proposal["approval_session_id"],
        declined_proposal_request["approval_session_id"]
    );
    assert_eq!(
        declined_proposal["operation_id"],
        declined_proposal_request["operation_id"]
    );
    assert_eq!(
        declined_proposal["presentation"]["proposal_digest_sha256"],
        declined_proposal_request["expected_proposal_digest_sha256"]
    );
    for envelope in [
        declined_view,
        declined_request,
        declined_result,
        declined_observation,
    ] {
        assert_eq!(envelope["approval_id"], declined_proposal["approval_id"]);
    }
    assert_eq!(
        declined_request["decision_nonce"],
        declined_view["decision_nonce"]
    );
    for envelope in [declined_request, declined_result, declined_observation] {
        assert_eq!(
            envelope["proposal_digest_sha256"],
            declined_proposal["presentation"]["proposal_digest_sha256"]
        );
    }
    assert_eq!(
        declined_observation["approval_session_id"],
        declined_proposal["approval_session_id"]
    );
    assert_eq!(
        declined_observation["operation_id"],
        declined_proposal["operation_id"]
    );
    assert_eq!(declined_observation["commit_receipt"], Value::Null);
    assert_eq!(declined_observation["cancellation_receipt"], Value::Null);
}

#[test]
fn approval_observation_schema_enforces_terminal_receipt_invariants() {
    let schema: Value =
        serde_json::from_str(APPROVAL_PROTOCOL_SCHEMA).expect("approval protocol schema JSON");
    let proposal_schema: Value =
        serde_json::from_str(PROPOSAL_PRESENTATION_SCHEMA).expect("proposal schema JSON");
    let fixture: Value =
        serde_json::from_str(APPROVAL_PROTOCOL_LIFECYCLE).expect("approval lifecycle JSON");
    let observation_schema = &schema["$defs"]["approval_observation"];

    let committed = lifecycle_scenario(&fixture, "approved_commit_after_uncertain_observation");
    let mut invalid_committed = committed["envelopes"]
        .as_array()
        .expect("commit envelopes")
        .iter()
        .find(|envelope| {
            envelope["kind"] == "approval_observation" && envelope["status"] == "committed"
        })
        .expect("committed observation")
        .clone();
    invalid_committed["commit_receipt"] = Value::Null;
    assert!(
        validate_schema_instance(
            &schema,
            &proposal_schema,
            observation_schema,
            &invalid_committed,
        )
        .is_err(),
        "committed observations require the exact commit receipt"
    );

    let cancelled = lifecycle_scenario(&fixture, "cancelled_before_human_decision");
    let mut invalid_cancelled = envelope_of_kind(cancelled, "approval_observation").clone();
    invalid_cancelled["cancellation_receipt"] = Value::Null;
    assert!(
        validate_schema_instance(
            &schema,
            &proposal_schema,
            observation_schema,
            &invalid_cancelled,
        )
        .is_err(),
        "cancelled observations require the exact cancellation receipt"
    );

    let mut invalid_approved = envelope_of_kind(committed, "approval_observation").clone();
    invalid_approved["cancellation_receipt"] =
        envelope_of_kind(cancelled, "cancellation_receipt").clone();
    assert!(
        validate_schema_instance(
            &schema,
            &proposal_schema,
            observation_schema,
            &invalid_approved,
        )
        .is_err(),
        "nonterminal observations cannot carry a terminal receipt"
    );
}

#[test]
fn approval_transport_and_negative_cases_freeze_the_authority_boundary() {
    let schema: Value =
        serde_json::from_str(APPROVAL_PROTOCOL_SCHEMA).expect("approval protocol schema JSON");
    let proposal_schema: Value =
        serde_json::from_str(PROPOSAL_PRESENTATION_SCHEMA).expect("proposal schema JSON");
    let fixture: Value =
        serde_json::from_str(APPROVAL_PROTOCOL_LIFECYCLE).expect("approval lifecycle JSON");

    for (definition_name, fixture_name) in [
        ("session_create_headers", "session_create_headers"),
        ("bound_mcp_headers", "bound_mcp_headers"),
        ("bound_mcp_mutation_headers", "proposal_create_headers"),
        ("bound_mcp_mutation_headers", "commit_headers"),
        ("bound_mcp_mutation_headers", "cancel_headers"),
        ("browser_decision_headers", "browser_decision_headers"),
    ] {
        let definition = &schema["$defs"][definition_name];
        let instance = &fixture["transport_examples"][fixture_name];
        validate_schema_instance(&schema, &proposal_schema, definition, instance)
            .unwrap_or_else(|error| panic!("{fixture_name} violates its schema: {error}"));
        assert_eq!(definition["additionalProperties"], Value::Bool(false));
        assert_required_object(definition, "/required", instance);
        assert_eq!(
            definition["properties"]
                .as_object()
                .expect("header properties")
                .keys()
                .collect::<BTreeSet<_>>(),
            instance
                .as_object()
                .expect("header fixture")
                .keys()
                .collect::<BTreeSet<_>>()
        );
    }

    let commit_scenario =
        lifecycle_scenario(&fixture, "approved_commit_after_uncertain_observation");
    let proposal = envelope_of_kind(commit_scenario, "proposal_created");
    let proposal_request = envelope_of_kind(commit_scenario, "proposal_create_request");
    let commit_request = envelope_of_kind(commit_scenario, "commit_request");
    let cancel_scenario = lifecycle_scenario(&fixture, "cancelled_before_human_decision");
    let cancel_request = envelope_of_kind(cancel_scenario, "cancel_request");
    assert_eq!(
        fixture["transport_examples"]["proposal_create_headers"]["idempotency_key"],
        proposal_request["proposal_request_id"]
    );
    assert_eq!(
        fixture["transport_examples"]["commit_headers"]["idempotency_key"],
        commit_request["commit_request_id"]
    );
    assert_eq!(
        fixture["transport_examples"]["cancel_headers"]["idempotency_key"],
        cancel_request["cancel_request_id"]
    );
    let endpoint_keys = BTreeSet::from([
        proposal_request["proposal_request_id"]
            .as_str()
            .expect("proposal request ID"),
        commit_request["commit_request_id"]
            .as_str()
            .expect("commit request ID"),
        cancel_request["cancel_request_id"]
            .as_str()
            .expect("cancel request ID"),
    ]);
    assert_eq!(
        endpoint_keys.len(),
        3,
        "normal lifecycle endpoints require distinct idempotency identities"
    );
    let idempotency_case = |id: &str| {
        fixture["idempotency_cases"]
            .as_array()
            .expect("idempotency cases")
            .iter()
            .find(|case| case["id"] == id)
            .unwrap_or_else(|| panic!("missing idempotency case {id}"))
    };
    let normal = idempotency_case("normal_proposal_then_commit");
    assert_ne!(
        normal["proposal"]["idempotency_key"],
        normal["commit"]["idempotency_key"]
    );
    assert_ne!(normal["proposal"]["path"], normal["commit"]["path"]);
    assert_eq!(normal["expected"], "both_accepted_once");
    let identical = idempotency_case("same_endpoint_identical_replay");
    assert_eq!(
        identical["first_jcs_sha256"],
        identical["replay_jcs_sha256"]
    );
    assert_eq!(identical["expected"], "original_result");
    let mismatched = idempotency_case("same_endpoint_mismatched_replay");
    assert_eq!(mismatched["method"], identical["method"]);
    assert_eq!(mismatched["path"], identical["path"]);
    assert_eq!(mismatched["idempotency_key"], identical["idempotency_key"]);
    assert_ne!(
        mismatched["first_jcs_sha256"],
        mismatched["replay_jcs_sha256"]
    );
    assert_eq!(mismatched["expected"], "approval_conflict");
    let approval_url = proposal["approval_url"].as_str().expect("approval URL");
    assert!(approval_url.starts_with("https://auth.hello.food/agent-approval/"));
    assert!(!approval_url.contains('?'));
    assert!(!approval_url.contains('#'));
    assert_eq!(
        schema
            .pointer("/$defs/proposal_created/properties/presentation/$ref")
            .and_then(Value::as_str),
        Some("https://hey.food/schemas/v1/agent-proposal-presentation.schema.json")
    );

    let invalid_values = fixture["negative_cases"]
        .as_array()
        .expect("negative cases");
    let value_for = |id: &str| {
        invalid_values
            .iter()
            .find(|case| case["id"] == id)
            .and_then(|case| case["value"].as_str())
            .unwrap_or_else(|| panic!("missing negative value for {id}"))
    };
    for id in ["alternate_approval_origin", "approval_url_query_injection"] {
        let mut invalid = proposal.clone();
        invalid["approval_url"] = Value::String(value_for(id).to_owned());
        assert!(
            validate_schema_instance(&schema, &proposal_schema, &schema, &invalid).is_err(),
            "{id} must fail full schema conformance"
        );
    }

    let commit = commit_request;
    for field in ["decision", "session_binding_token"] {
        let mut invalid = commit.clone();
        invalid
            .as_object_mut()
            .expect("commit object")
            .insert(field.to_owned(), Value::String("forged".to_owned()));
        assert!(
            validate_schema_instance(&schema, &proposal_schema, &schema, &invalid).is_err(),
            "{field} must be rejected from commit JSON"
        );
    }

    let mut missing_csrf = fixture["transport_examples"]["browser_decision_headers"].clone();
    missing_csrf
        .as_object_mut()
        .expect("browser header fixture")
        .remove("x_heyfood_csrf_token");
    assert!(
        validate_schema_instance(
            &schema,
            &proposal_schema,
            &schema["$defs"]["browser_decision_headers"],
            &missing_csrf,
        )
        .is_err(),
        "browser decisions without CSRF authority must fail"
    );

    let negative_ids = fixture["negative_cases"]
        .as_array()
        .expect("negative cases")
        .iter()
        .map(|case| case["id"].as_str().expect("negative case id"))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        negative_ids,
        BTreeSet::from([
            "alternate_approval_origin",
            "approval_url_query_injection",
            "binding_token_in_json",
            "blind_retry_after_uncertain_commit",
            "changed_frozen_presentation",
            "commit_and_cancel_same_approval",
            "committed_without_receipt",
            "conflicting_decision_replay",
            "cross_session_binding",
            "missing_browser_csrf_header",
            "model_supplied_commit_decision",
            "same_endpoint_idempotency_mismatch",
        ])
    );

    for request in [
        "proposal_create_request",
        "human_decision_request",
        "commit_request",
        "cancel_request",
    ] {
        let properties = schema["$defs"][request]["properties"]
            .as_object()
            .expect("request properties");
        assert!(
            !properties.contains_key("session_binding_token"),
            "{request} must receive binding authority only through its header"
        );
        assert!(
            !properties.contains_key("commit_token"),
            "{request} must not accept reusable commit authority"
        );
    }
    assert!(
        !schema["$defs"]["commit_request"]["properties"]
            .as_object()
            .expect("commit request properties")
            .contains_key("decision"),
        "the model cannot convey semantic approval through commit JSON"
    );
    for required_rule in [
        "binding is account and session scoped",
        "decision_nonce",
        "never retry a POST",
        "X-Heyfood-Agent-Approval-Binding",
        "__Host-heyfood-agent-approval",
        "(account subject, backend",
        "Proposal, commit, and cancel request IDs are independently",
        "Reuse within that namespace with different content",
    ] {
        assert!(
            APPROVAL_PROTOCOL_CONTRACT.contains(required_rule),
            "approval contract omits negative-path rule {required_rule}"
        );
    }
}

#[test]
fn approval_error_schema_binds_every_code_to_one_outcome_contract() {
    let schema: Value =
        serde_json::from_str(APPROVAL_PROTOCOL_SCHEMA).expect("approval protocol schema JSON");
    let variants = schema
        .pointer("/$defs/protocol_error/allOf/0/oneOf")
        .and_then(Value::as_array)
        .expect("error variants");
    let mappings = variants
        .iter()
        .map(|variant| {
            let properties = &variant["properties"];
            (
                properties["code"]["const"].as_str().expect("error code"),
                properties["http_status"]["const"]
                    .as_u64()
                    .expect("HTTP status"),
                properties["outcome_uncertain"]["const"]
                    .as_bool()
                    .expect("outcome certainty"),
            )
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        mappings,
        BTreeSet::from([
            ("approval_conflict", 409, false),
            ("approval_expired", 410, false),
            ("approval_not_found", 404, false),
            ("forbidden", 403, false),
            ("internal_before_dispatch", 500, false),
            ("invalid_request", 400, false),
            ("outcome_uncertain", 503, true),
            ("rate_limited", 429, false),
            ("unauthenticated", 401, false),
        ])
    );
    assert_eq!(
        schema["$defs"]["protocol_error"]["properties"]["retry_allowed"]["const"],
        Value::Bool(false)
    );
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
