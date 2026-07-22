use std::collections::BTreeSet;

use clap::{CommandFactory, Parser};
use heyfood_cli::{
    Command, CommandLine, GroceryCommand, GroceryDecisionArgument, OutputMode,
    render_grocery_exclusions, render_grocery_list, render_grocery_proposal, render_json,
};
use heyfood_core::{ExclusionListResponseWire, GroceryListWire, GroceryMutationProposalWire};
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
            command: Some(GroceryCommand::Confirm(ref args)),
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
fn grocery_show_exclusions_and_never_commands_are_typed() {
    let parsed = CommandLine::try_parse_from(["heyfood", "grocery"]).unwrap();
    assert!(matches!(
        parsed.command,
        Some(Command::Grocery { command: None })
    ));
    for alias in ["list", "show"] {
        let parsed = CommandLine::try_parse_from(["heyfood", "grocery", alias]).unwrap();
        assert!(matches!(
            parsed.command,
            Some(Command::Grocery {
                command: Some(GroceryCommand::List),
            })
        ));
    }
    let parsed = CommandLine::try_parse_from(["heyfood", "grocery", "exclusions"]).unwrap();
    assert!(matches!(
        parsed.command,
        Some(Command::Grocery {
            command: Some(GroceryCommand::Exclusions),
        })
    ));
    let parsed = CommandLine::try_parse_from([
        "heyfood",
        "grocery",
        "never",
        "--list-id",
        "00000000-0000-4000-8000-000000000123",
        "--version",
        "4",
        "--remove",
        "raw onion",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Some(Command::Grocery {
            command: Some(GroceryCommand::Never(ref arguments)),
        }) if arguments.remove && arguments.item == "raw onion"
    ));
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
fn grocery_renderer_surfaces_stable_ids_provenance_member_flags_and_substitutions() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/contracts/grocery-backend/phase-a/fixtures/grocery/founding_scenario_maya.json"
    ))
    .unwrap();
    let list: GroceryListWire = serde_json::from_value(fixture["list"].clone()).unwrap();
    let output = render_grocery_list(&list, OutputMode::HumanPlain);
    assert!(output.contains("id:i2"));
    assert!(output.contains("source: recipe:dahl-001"));
    assert!(output.contains("maya-uuid: risky"));
    assert!(output.contains("try: green parts of scallion, garlic-infused oil"));
    assert!(output.contains("Screened at ingredient level — verify the product label."));

    let exclusions = ExclusionListResponseWire {
        exclusions: vec!["pork\u{1b}[2J".into(), "raw onion".into()],
    };
    let rendered = render_grocery_exclusions(&exclusions, OutputMode::HumanPlain);
    assert!(!rendered.contains('\u{1b}'));
    assert!(rendered.contains("pork[2J"));
}

#[test]
fn human_grocery_proposal_is_a_reviewable_non_mutating_card() {
    let proposal: GroceryMutationProposalWire = serde_json::from_value(json!({
        "confirmation_id": "00000000-0000-4000-8000-000000000001",
        "idempotency_key": "00000000-0000-4000-8000-000000000002",
        "operation": "add_items",
        "expires_at": "2026-07-22T12:05:00Z",
        "structured_preview": {
            "items": [{
                "requested_name": "onion",
                "intended_for": "maya",
                "safety": {
                    "status": "risky",
                    "member_flags": [{
                        "member_id": "maya",
                        "status": "risky",
                        "substitutions": ["green parts of scallion"]
                    }],
                    "label_hint": "Screened at ingredient level — verify the product label."
                }
            }]
        },
        "preconditions": [{"type": "list_version", "expected_version": 4}],
        "confirmation_token": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }))
    .unwrap();
    let output = render_grocery_proposal(&proposal, OutputMode::HumanPlain);
    assert!(output.contains("Review add_items"));
    assert!(output.contains("1. onion for maya"));
    assert!(output.contains("maya: risky"));
    assert!(output.contains("try: green parts of scallion"));
    assert!(output.contains("Nothing has changed"));
    assert!(!output.contains("aaaaaaaa"));
}

#[test]
fn raw_alias_selects_json_but_conflicts_with_json() {
    let parsed = CommandLine::try_parse_from(["heyfood", "--raw", "status"]).unwrap();
    assert_eq!(parsed.output_mode(true), OutputMode::Json);
    assert!(CommandLine::try_parse_from(["heyfood", "--raw", "--json", "status"]).is_err());
}

#[test]
fn coordinates_preserve_short_names_aliases_and_validate_domains() {
    for arguments in [
        [
            "heyfood", "ask", "lunch", "--lat", "34.1", "--lng", "-118.2",
        ],
        [
            "heyfood",
            "ask",
            "lunch",
            "--latitude",
            "34.1",
            "--longitude",
            "-118.2",
        ],
    ] {
        let parsed = CommandLine::try_parse_from(arguments).unwrap();
        assert!(matches!(
            parsed.command,
            Some(Command::Ask(ref ask))
                if ask.latitude == Some(34.1) && ask.longitude == Some(-118.2)
        ));
    }

    for arguments in [
        vec!["heyfood", "ask", "lunch", "--lat", "91", "--lng", "0"],
        vec!["heyfood", "ask", "lunch", "--lat", "0", "--lng", "181"],
        vec!["heyfood", "ask", "lunch", "--lat", "NaN", "--lng", "0"],
        vec!["heyfood", "ask", "lunch", "--lat", "0", "--lng", "inf"],
    ] {
        assert!(CommandLine::try_parse_from(arguments).is_err());
    }
}
