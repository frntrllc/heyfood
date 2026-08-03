use std::net::TcpListener;
use std::process::Command;

use serde_json::Value;

fn agent(arguments: &[&str], service: &TcpListener) -> std::process::Output {
    let address = service.local_addr().expect("local fixture address");
    Command::new(env!("CARGO_BIN_EXE_heyfood"))
        .args(arguments)
        .env("HEYFOOD_API_URL", format!("http://{address}/"))
        .env("HEYFOOD_AUTH_URL", format!("http://{address}/"))
        .env("HEYFOOD_API_KEY", "must-not-be-read")
        .env("HEYFOOD_CREDENTIAL_STORE", "invalid-must-not-be-read")
        .output()
        .expect("run installed discovery command")
}

fn assert_no_network(service: &TcpListener) {
    service.set_nonblocking(true).unwrap();
    let error = service
        .accept()
        .expect_err("offline discovery must not open a socket");
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
}

#[test]
fn describe_is_deterministic_ansi_free_and_offline() {
    let service = TcpListener::bind("127.0.0.1:0").unwrap();
    let first = agent(&["agent", "describe"], &service);
    let second = agent(&["--json", "agent", "describe"], &service);
    let bare = agent(&["agent"], &service);

    assert!(first.status.success(), "{:?}", first.stderr);
    assert!(second.status.success(), "{:?}", second.stderr);
    assert!(bare.status.success(), "{:?}", bare.stderr);
    assert!(first.stderr.is_empty());
    assert!(second.stderr.is_empty());
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(first.stdout, bare.stdout);
    assert!(!first.stdout.contains(&0x1b));
    let manifest: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(manifest["schema_version"], 3);
    assert_eq!(manifest["product"], "heyfood");
    assert_eq!(manifest["binary_version"], heyfood_core::VERSION);
    assert_eq!(
        manifest["automation_surfaces"]["tui_automation"],
        "unsupported"
    );
    assert_eq!(
        manifest["native_state_compatibility"]["maximum_native_state_version"],
        3
    );
    assert_eq!(manifest, heyfood_agent_contract::manifest());
    assert_no_network(&service);
}

#[test]
fn explicitly_versioned_describe_exposes_strict_native_state_metadata_offline() {
    let service = TcpListener::bind("127.0.0.1:0").unwrap();
    let output = agent(&["agent", "describe", "--schema-version", "2"], &service);

    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stderr.is_empty());
    let manifest: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(manifest["schema_version"], 2);
    assert_eq!(
        manifest["native_state_compatibility"],
        serde_json::json!({
            "binary_version": heyfood_core::VERSION,
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
    assert_eq!(manifest, heyfood_agent_contract::manifest_v2());
    assert_no_network(&service);
}

#[test]
fn explicit_v1_describe_remains_a_frozen_offline_compatibility_view() {
    let service = TcpListener::bind("127.0.0.1:0").unwrap();
    let output = agent(&["agent", "describe", "--schema-version", "1"], &service);

    assert!(output.status.success(), "{:?}", output.stderr);
    let manifest: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(manifest["schema_version"], 1);
    assert!(manifest.get("native_state_compatibility").is_none());
    assert_eq!(manifest, heyfood_agent_contract::manifest_v1());
    assert_no_network(&service);
}

#[test]
fn compatibility_bootstrap_fails_closed_without_receipts_and_stays_offline() {
    let service = TcpListener::bind("127.0.0.1:0").unwrap();
    let isolated_home = std::env::temp_dir().join(format!(
        "heyfood-agent-compatibility-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&isolated_home).unwrap();
    let address = service.local_addr().expect("local fixture address");
    let output = Command::new(env!("CARGO_BIN_EXE_heyfood"))
        .args(["--json", "--no-input", "agent", "compatibility"])
        .env("HOME", &isolated_home)
        .env("CODEX_HOME", isolated_home.join(".codex"))
        .env("PATH", "")
        .env("HEYFOOD_API_URL", format!("http://{address}/"))
        .env("HEYFOOD_AUTH_URL", format!("http://{address}/"))
        .output()
        .expect("run isolated compatibility command");
    assert!(output.status.success(), "{:?}", output.stderr);
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["schema_version"], 1);
    assert_eq!(result["manifest_schema_version"], 3);
    assert_eq!(result["compatible"], false);
    assert_eq!(result["network_accessed"], false);
    assert_eq!(result["credentials_accessed"], false);
    assert_eq!(result["product_state_mutated"], false);
    assert_eq!(
        result["installations"][0]["status"],
        "skill_identity_unknown"
    );
    let remediation = result["installations"][0]["remediation"]["arguments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect::<Vec<_>>();
    let remediation_output = Command::new(env!("CARGO_BIN_EXE_heyfood"))
        .args(remediation)
        .env("HOME", &isolated_home)
        .env("CODEX_HOME", isolated_home.join(".codex"))
        .env("PATH", "")
        .output()
        .expect("run emitted compatibility remediation");
    assert!(
        remediation_output.status.success(),
        "emitted remediation must be an executable dry-run: {:?}",
        remediation_output.stderr
    );
    std::fs::remove_dir_all(&isolated_home).unwrap();
    assert_no_network(&service);
}

#[test]
fn guide_and_safety_are_exact_embedded_bytes_without_network() {
    let service = TcpListener::bind("127.0.0.1:0").unwrap();
    let guide = agent(&["agent", "guide", "--format", "markdown"], &service);
    let safety = agent(
        &["agent", "guide", "--format", "markdown", "--safety"],
        &service,
    );
    assert!(guide.status.success());
    assert!(safety.status.success());
    assert_eq!(guide.stdout, heyfood_agent_contract::GUIDE.as_bytes());
    assert_eq!(safety.stdout, heyfood_agent_contract::SAFETY.as_bytes());

    let machine = agent(&["--json", "agent", "guide"], &service);
    let document: Value = serde_json::from_slice(&machine.stdout).unwrap();
    assert_eq!(document["content"], heyfood_agent_contract::GUIDE);
    assert_eq!(
        document["sha256"],
        heyfood_agent_contract::sha256_hex(heyfood_agent_contract::GUIDE.as_bytes())
    );
    assert_no_network(&service);
}

#[test]
fn schemas_are_exact_embedded_bytes_without_network() {
    let cases = [
        ("manifest", heyfood_agent_contract::EmbeddedSchema::Manifest),
        (
            "manifest-v2",
            heyfood_agent_contract::EmbeddedSchema::ManifestV2,
        ),
        (
            "manifest-v3",
            heyfood_agent_contract::EmbeddedSchema::ManifestV3,
        ),
        (
            "schema-index",
            heyfood_agent_contract::EmbeddedSchema::SchemaIndex,
        ),
        ("doctor", heyfood_agent_contract::EmbeddedSchema::Doctor),
        (
            "doctor-v2",
            heyfood_agent_contract::EmbeddedSchema::DoctorV2,
        ),
        ("guide", heyfood_agent_contract::EmbeddedSchema::Guide),
        (
            "schema-result",
            heyfood_agent_contract::EmbeddedSchema::SchemaResult,
        ),
        ("error", heyfood_agent_contract::EmbeddedSchema::CliError),
        (
            "output",
            heyfood_agent_contract::EmbeddedSchema::PublicOutput,
        ),
        (
            "proposal-presentation",
            heyfood_agent_contract::EmbeddedSchema::ProposalPresentation,
        ),
        (
            "setup-plan",
            heyfood_agent_contract::EmbeddedSchema::SetupPlan,
        ),
        (
            "agent-compatibility",
            heyfood_agent_contract::EmbeddedSchema::AgentCompatibility,
        ),
        (
            "household-context-input",
            heyfood_agent_contract::EmbeddedSchema::HouseholdContextInput,
        ),
        (
            "household-member-input",
            heyfood_agent_contract::EmbeddedSchema::HouseholdMemberInput,
        ),
        (
            "household-read",
            heyfood_agent_contract::EmbeddedSchema::HouseholdRead,
        ),
    ];
    for (name, schema) in cases {
        let service = TcpListener::bind("127.0.0.1:0").unwrap();
        let output = agent(&["agent", "schema", name], &service);
        assert!(output.status.success(), "{name}: {:?}", output.stderr);
        assert_eq!(output.stdout, schema.document().as_bytes(), "{name}");
        let document: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(document["$id"], schema.id());
        assert_no_network(&service);
    }

    let service = TcpListener::bind("127.0.0.1:0").unwrap();
    let output = agent(&["agent", "schema", "--list"], &service);
    assert!(output.status.success(), "{:?}", output.stderr);
    let index: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(index, heyfood_agent_contract::schema_index());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("approval-protocol"));
    assert_no_network(&service);
}

#[test]
fn unknown_schema_is_a_typed_runtime_error_without_clap_topology_leakage() {
    let service = TcpListener::bind("127.0.0.1:0").unwrap();
    let output = agent(
        &["--json", "agent", "schema", "approval-protocol"],
        &service,
    );
    assert!(!output.status.success());
    assert!(output.stderr.is_empty());
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["ok"], false);
    assert_eq!(error["error"]["type"], "agent_schema_unknown");
    assert!(error["error"]["hint"].as_str().is_some());
    assert!(error["error"].get("code").is_none());
    assert!(error["error"].get("action").is_none());
    assert!(error["error"].get("retryable").is_none());
    assert!(error["error"].get("outcome_uncertain").is_none());
    assert_no_network(&service);
}

#[test]
fn doctor_is_local_bounded_and_credential_free() {
    let service = TcpListener::bind("127.0.0.1:0").unwrap();
    let output = agent(&["agent", "doctor"], &service);
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stderr.is_empty());
    assert!(output.stdout.len() < 16 * 1024);
    let doctor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doctor["schema_version"], 3);
    assert_eq!(doctor["manifest_schema_version"], 3);
    assert_eq!(doctor["ok"], true);
    assert_eq!(doctor["network_accessed"], false);
    assert_eq!(doctor["credentials_accessed"], false);
    assert_eq!(doctor["product_state_mutated"], false);
    assert_eq!(doctor["tui_automation_supported"], false);
    assert!(
        doctor["checks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|check| check["status"] == "pass")
    );
    assert!(doctor.get("executable").is_none());
    assert_no_network(&service);
}

#[test]
fn explicitly_versioned_doctor_is_bound_to_the_v2_manifest_offline() {
    let service = TcpListener::bind("127.0.0.1:0").unwrap();
    let output = agent(&["agent", "doctor", "--schema-version", "2"], &service);
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stderr.is_empty());
    let doctor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doctor["schema_version"], 2);
    assert_eq!(doctor["manifest_schema_version"], 2);
    assert_eq!(doctor["ok"], true);
    assert_eq!(doctor["network_accessed"], false);
    assert_eq!(doctor["credentials_accessed"], false);
    assert_no_network(&service);
}
