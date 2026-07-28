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

fn argument_labels(command: &Command) -> BTreeSet<String> {
    command
        .get_arguments()
        .filter(|argument| !argument.is_global_set())
        .flat_map(|argument| {
            let mut labels = Vec::new();
            if let Some(long) = argument.get_long() {
                labels.push(format!("--{long}"));
                labels.extend(
                    argument
                        .get_all_aliases()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|alias| format!("--{alias}")),
                );
            }
            if let Some(short) = argument.get_short() {
                labels.push(format!("-{short}"));
            }
            if argument.get_long().is_none() && argument.get_short().is_none() {
                labels.push(format!("<{}>", argument.get_id()));
            }
            labels
        })
        .collect()
}

fn command_metadata(
    command: &Command,
    prefix: &str,
    arguments: &mut BTreeMap<String, BTreeSet<String>>,
    aliases: &mut BTreeMap<String, BTreeSet<String>>,
) {
    for subcommand in command.get_subcommands() {
        let path = if prefix.is_empty() {
            subcommand.get_name().to_owned()
        } else {
            format!("{prefix} {}", subcommand.get_name())
        };
        let local_arguments = argument_labels(subcommand);
        if !local_arguments.is_empty() {
            arguments.insert(path.clone(), local_arguments);
        }
        let command_aliases = subcommand
            .get_all_aliases()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        if !command_aliases.is_empty() {
            aliases.insert(path.clone(), command_aliases);
        }
        command_metadata(subcommand, &path, arguments, aliases);
    }
}

fn parsed_inventory() -> Value {
    serde_json::from_str(INVENTORY).expect("Phase 0 command inventory must be valid JSON")
}

#[test]
fn inventory_covers_global_controls_arguments_and_aliases() {
    let document = parsed_inventory();
    let command = CommandLine::command_tree();
    let mut global_controls = command
        .get_arguments()
        .filter(|argument| argument.is_global_set())
        .filter_map(|argument| argument.get_long().map(|long| format!("--{long}")))
        .collect::<BTreeSet<_>>();
    global_controls.extend(["--help".to_owned(), "--version".to_owned()]);
    let inventoried_globals = document["global_process_controls"]
        .as_array()
        .expect("global_process_controls")
        .iter()
        .map(|value| value.as_str().expect("global control").to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(inventoried_globals, global_controls);
    let inventoried_global_effects = document["global_process_control_effects"]
        .as_object()
        .expect("global_process_control_effects")
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(inventoried_global_effects, global_controls);

    let mut clap_arguments = BTreeMap::new();
    let mut clap_aliases = BTreeMap::new();
    command_metadata(&command, "", &mut clap_arguments, &mut clap_aliases);

    let inventoried_arguments = document["command_arguments"]
        .as_object()
        .expect("command_arguments must be an object")
        .iter()
        .map(|(path, values)| {
            (
                path.clone(),
                values
                    .as_array()
                    .expect("command arguments must be arrays")
                    .iter()
                    .map(|value| value.as_str().expect("argument").to_owned())
                    .collect::<BTreeSet<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(inventoried_arguments, clap_arguments);

    let inventoried_aliases = document["commands"]
        .as_array()
        .expect("commands")
        .iter()
        .filter_map(|row| {
            let aliases = row.get("aliases")?.as_array()?;
            Some((
                row["path"].as_str().expect("path").to_owned(),
                aliases
                    .iter()
                    .map(|alias| {
                        alias
                            .as_str()
                            .expect("alias path")
                            .rsplit_once(' ')
                            .map_or_else(
                                || alias.as_str().unwrap().to_owned(),
                                |(_, leaf)| leaf.to_owned(),
                            )
                    })
                    .collect::<BTreeSet<_>>(),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(inventoried_aliases, clap_aliases);
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
        "network_calls",
        "retry",
        "product_state",
        "required_scopes",
        "json_output_family",
        "error_output_family",
        "error_types",
        "local_side_effects",
        "remote_side_effects",
        "controlling_terminal",
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
        assert!(
            policy["required_scopes"].is_array(),
            "policy {policy_name} scopes must be explicit"
        );
        assert!(
            policy["network_calls"].is_array(),
            "policy {policy_name} network calls must be explicit"
        );
        assert!(
            policy["error_types"]
                .as_array()
                .is_some_and(|errors| !errors.is_empty()),
            "policy {policy_name} needs an error taxonomy"
        );
        if policy["network"] != "none" {
            assert!(
                policy["network_calls"]
                    .as_array()
                    .is_some_and(|calls| !calls.is_empty()),
                "networked policy {policy_name} must inventory calls"
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
fn reviewed_policy_corrections_remain_frozen() {
    let document = parsed_inventory();
    let policies = document["policies"].as_object().expect("policies");
    let commands = document["commands"].as_array().expect("commands");
    let policy_for = |path: &str| {
        commands
            .iter()
            .find(|command| command["path"] == path)
            .and_then(|command| command["policy"].as_str())
            .unwrap_or_else(|| panic!("missing command {path}"))
    };

    let item = &policies[policy_for("item")];
    assert_eq!(item["audience"], "agent_unsupported");
    assert_eq!(item["network"], "post_as_read");
    assert_eq!(
        item["network_calls"],
        serde_json::json!(["POST /v1/channel/tools/explain_item"])
    );

    let export = &policies[policy_for("grocery export")];
    assert_eq!(export["audience"], "agent_unsupported");
    assert_eq!(export["controlling_terminal"], "not_used");
    assert_eq!(
        export["local_side_effects"],
        "optional_owner_only_atomic_file_write_and_session_rotation"
    );
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
