use std::collections::BTreeSet;

use clap::{CommandFactory, Parser};
use heyfood_cli::{
    Command, CommandLine, GroceryCommand, GroceryDecisionArgument, OutputMode, render_grocery_list,
    render_json,
};
use heyfood_core::GroceryListWire;
use serde_json::json;

#[test]
fn command_tree_contains_python_parity_and_authorized_phase2_families() {
    let actual = CommandLine::command()
        .get_subcommands()
        .map(|command| command.get_name().to_owned())
        .collect::<BTreeSet<_>>();
    let expected = BTreeSet::from([
        "account",
        "ask",
        "channels",
        "chat",
        "completion",
        "config",
        "context",
        "conversation",
        "daily",
        "doctor",
        "get-menu",
        "grocery",
        "health",
        "household",
        "item",
        "location",
        "log",
        "login",
        "logout",
        "members",
        "menu",
        "onboard",
        "profile",
        "recommend",
        "recipes",
        "register",
        "reply",
        "search",
        "status",
        "voice",
    ])
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(actual, expected);
}

#[test]
fn confirmation_token_is_never_accepted_as_a_command_line_argument() {
    let parsed =
        CommandLine::try_parse_from(["heyfood", "grocery", "confirm", "--decision", "cancel"])
            .unwrap();
    assert!(matches!(
        parsed.command,
        Some(Command::Grocery {
            command: GroceryCommand::Confirm(ref args),
        }) if args.decision == GroceryDecisionArgument::Cancel && args.proposal_stdin
    ));
    assert!(
        CommandLine::try_parse_from([
            "heyfood",
            "grocery",
            "confirm",
            "--decision",
            "accept",
            "secret-token"
        ])
        .is_err()
    );
}

#[test]
fn json_output_is_one_ansi_free_value_even_for_hostile_text() {
    let output = render_json(&json!({"message": "hello\u{1b}[31m\nworld"})).unwrap();
    assert_eq!(output.lines().count(), 1);
    assert!(!output.contains('\u{1b}'));
    let decoded: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(decoded["message"], "hello\u{1b}[31m\nworld");
}

#[test]
fn human_grocery_renderer_removes_terminal_controls() {
    let list: GroceryListWire = serde_json::from_value(json!({
        "id": "list",
        "title": "List\u{1b}[2J",
        "state": "active",
        "version": 1,
        "items": [{
            "id": "item",
            "requested_name": "milk\u{1b}[31m",
            "canonical_name": "milk",
            "quantity": null,
            "unit": null,
            "package_quantity": null,
            "note": null,
            "state": "active",
            "intended_for": null,
            "sources": [],
            "safety": null,
            "created_at": "2026-07-21T12:00:00Z",
            "updated_at": "2026-07-21T12:00:00Z"
        }],
        "created_at": "2026-07-21T12:00:00Z",
        "updated_at": "2026-07-21T12:00:00Z"
    }))
    .unwrap();
    let plain = render_grocery_list(&list, OutputMode::HumanPlain);
    assert!(!plain.contains('\u{1b}'));
    assert!(plain.contains("List[2J"));
    assert!(plain.contains("milk[31m"));
}

#[test]
fn raw_alias_selects_json_but_conflicts_with_json() {
    let parsed = CommandLine::try_parse_from(["heyfood", "--raw", "status"]).unwrap();
    assert_eq!(parsed.output_mode(true), OutputMode::Json);
    assert!(CommandLine::try_parse_from(["heyfood", "--raw", "--json", "status"]).is_err());
}
