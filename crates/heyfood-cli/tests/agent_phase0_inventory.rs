use std::collections::{BTreeMap, BTreeSet};

use clap::Command;
use heyfood_cli::CommandLine;
use serde_json::Value;

const INVENTORY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/release-evidence/agent-native-phase0/command-authority-inventory.json"
));

fn command_paths(command: &Command, prefix: &str, paths: &mut BTreeSet<String>) {
    for subcommand in command.get_subcommands() {
        let path = if prefix.is_empty() {
            subcommand.get_name().to_owned()
        } else {
            format!("{prefix} {}", subcommand.get_name())
        };
        assert!(paths.insert(path.clone()), "duplicate Clap path {path}");
        command_paths(subcommand, &path, paths);
    }
}

fn parsed_inventory() -> Value {
    serde_json::from_str(INVENTORY).expect("Phase 0 command inventory must be valid JSON")
}

#[test]
fn inventory_covers_the_exact_clap_command_tree() {
    let document = parsed_inventory();
    let inventory_paths = document["commands"]
        .as_array()
        .expect("commands must be an array")
        .iter()
        .map(|command| {
            command["path"]
                .as_str()
                .expect("every command requires a path")
                .to_owned()
        })
        .collect::<BTreeSet<_>>();

    let mut clap_paths = BTreeSet::new();
    command_paths(&CommandLine::command_tree(), "", &mut clap_paths);

    assert_eq!(
        inventory_paths, clap_paths,
        "every primary Clap path, including hidden topology, must have one authority row"
    );
}

#[test]
fn every_command_references_a_complete_policy() {
    let document = parsed_inventory();
    let policies = document["policies"]
        .as_object()
        .expect("policies must be an object");
    let required_fields = [
        "audience",
        "operation_class",
        "input_transport",
        "output_transport",
        "network",
        "retry",
        "product_state",
        "phase0_gap",
    ];

    let mut usages = BTreeMap::<&str, usize>::new();
    for command in document["commands"]
        .as_array()
        .expect("commands must be an array")
    {
        let policy_name = command["policy"]
            .as_str()
            .expect("every command requires a policy");
        let policy = policies
            .get(policy_name)
            .unwrap_or_else(|| panic!("unknown policy {policy_name}"));
        let policy = policy
            .as_object()
            .unwrap_or_else(|| panic!("policy {policy_name} must be an object"));
        for field in required_fields {
            assert!(
                policy.contains_key(field),
                "policy {policy_name} is missing {field}"
            );
        }
        *usages.entry(policy_name).or_default() += 1;
    }

    let unused = policies
        .keys()
        .filter(|name| !usages.contains_key(name.as_str()))
        .collect::<Vec<_>>();
    assert!(unused.is_empty(), "unused policies: {unused:?}");
}

#[test]
fn phase0_exposes_no_agent_safe_product_mutation() {
    let document = parsed_inventory();
    let policies = document["policies"]
        .as_object()
        .expect("policies must be an object");

    for command in document["commands"]
        .as_array()
        .expect("commands must be an array")
    {
        let path = command["path"].as_str().expect("path");
        let policy_name = command["policy"].as_str().expect("policy");
        let policy = policies[policy_name].as_object().expect("policy object");
        if policy["audience"] == "agent_safe" {
            assert!(
                !matches!(
                    policy["operation_class"].as_str(),
                    Some("mutation" | "mutation_via_conversation" | "confirm_or_cancel")
                ),
                "{path} must not be an agent-safe mutation"
            );
            assert!(
                !policy["product_state"]
                    .as_str()
                    .is_some_and(|state| state.contains("mutation")),
                "{path} must not hide mutation behind an agent-safe policy"
            );
        }
    }
}

#[test]
fn deferred_and_hidden_topology_is_never_agent_safe() {
    let document = parsed_inventory();
    let policies = document["policies"]
        .as_object()
        .expect("policies must be an object");

    for command in document["commands"]
        .as_array()
        .expect("commands must be an array")
    {
        let visibility = command["visibility"].as_str().expect("visibility");
        let policy_name = command["policy"].as_str().expect("policy");
        let audience = policies[policy_name]["audience"]
            .as_str()
            .expect("audience");
        if visibility == "hidden" {
            assert_ne!(
                audience,
                "agent_safe",
                "hidden command {} cannot be agent-safe",
                command["path"].as_str().expect("path")
            );
        }
    }
}

#[test]
fn phase0_does_not_publish_agent_or_mcp_commands() {
    let command = CommandLine::command_tree();
    assert!(command.find_subcommand("agent").is_none());
    assert!(command.find_subcommand("mcp").is_none());
}
