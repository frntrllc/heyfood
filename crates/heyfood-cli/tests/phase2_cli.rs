use std::collections::BTreeSet;

use clap::{CommandFactory, Parser};
use heyfood_application::{
    GroceryDisplayItem, GroceryDisplayList, GroceryDisplayMemberFlag, GroceryDisplaySafety,
    GroceryDisplaySource, GroceryExclusions, MenuWatchList, MenuWatchSnapshot,
};
use heyfood_cli::{
    Command, CommandLine, CompletionShell, GroceryCommand, GroceryDecisionArgument,
    MenuWatchCommand, OutputMode, WatchWeekdayArgument, render_grocery_exclusions,
    render_grocery_list, render_grocery_proposal, render_json, render_menu_watch_list,
    write_completions,
};
use heyfood_core::{
    ExclusionListResponseWire, GroceryListWire, GroceryMutationProposalWire, MenuWatchId,
    RestaurantId, WatchCadenceWire, WatchHour, WatchWeekday,
};
use serde_json::json;

fn display_list(wire: GroceryListWire) -> GroceryDisplayList {
    GroceryDisplayList {
        id: wire.id,
        title: wire.title,
        state: wire.state,
        version: wire.version,
        items: wire
            .items
            .into_iter()
            .map(|item| GroceryDisplayItem {
                id: item.id,
                requested_name: item.requested_name,
                canonical_name: item.canonical_name,
                quantity: item.quantity,
                unit: item.unit,
                package_quantity: item.package_quantity,
                note: item.note,
                state: item.state,
                intended_for: item.intended_for,
                sources: item
                    .sources
                    .into_iter()
                    .map(|source| GroceryDisplaySource {
                        source_type: source.source_type,
                        source_ref: source.source_ref,
                        source_detail: source.source_detail,
                    })
                    .collect(),
                safety: item.safety.map(|safety| GroceryDisplaySafety {
                    basis: safety.basis,
                    status: safety.status,
                    member_flags: safety
                        .member_flags
                        .into_iter()
                        .map(|flag| GroceryDisplayMemberFlag {
                            member_id: flag.member_id,
                            status: flag.status,
                            reason: flag.reason,
                            substitutions: flag.substitutions,
                        })
                        .collect(),
                    model_version: safety.model_version,
                    rules_version: safety.rules_version,
                    confidence: safety.confidence,
                    context_hash: safety.context_hash,
                    context_hash_version: safety.context_hash_version,
                    label_hint: safety.label_hint,
                }),
                created_at: item.created_at,
                updated_at: item.updated_at,
            })
            .collect(),
        created_at: wire.created_at,
        updated_at: wire.updated_at,
    }
}

#[test]
fn command_tree_retains_hidden_compatibility_and_authorized_phase2_families() {
    let actual = CommandLine::command()
        .get_subcommands()
        .map(|command| command.get_name().to_owned())
        .collect::<BTreeSet<_>>();
    let expected = BTreeSet::from([
        "account",
        "agent",
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
        "mcp",
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
        "watch",
    ])
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(actual, expected);
}

#[test]
fn release_help_and_completions_hide_deferred_health_integrations() {
    let mut command = CommandLine::command();
    let root_help = command.render_long_help().to_string();
    assert!(
        !root_help
            .lines()
            .any(|line| line.trim_start().starts_with("health "))
    );

    let health_help = CommandLine::try_parse_from(["heyfood", "health", "--help"])
        .unwrap_err()
        .to_string();
    assert!(health_help.contains("deferred from the supported v0.6.0 contract"));
    assert!(!health_help.contains("connect"));
    assert!(!health_help.contains("sync"));

    for shell in [
        CompletionShell::Bash,
        CompletionShell::Elvish,
        CompletionShell::Fish,
        CompletionShell::PowerShell,
        CompletionShell::Zsh,
    ] {
        let mut completion = Vec::new();
        write_completions(shell, &mut completion);
        let completion = String::from_utf8(completion).unwrap();
        assert!(!completion.contains("health"));
    }
}

#[test]
fn menu_watch_commands_are_typed_and_bounded() {
    let parsed = CommandLine::try_parse_from(["heyfood", "watch"]).unwrap();
    assert!(matches!(
        parsed.command,
        Some(Command::Watch { command: None })
    ));
    let parsed = CommandLine::try_parse_from([
        "heyfood",
        "watch",
        "add",
        "0c1cb790-0000-4000-8000-000000000000",
        "--weekday",
        "thursday",
        "--hour",
        "9",
        "--notify",
        "--tz",
        "America/Chicago",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Some(Command::Watch {
            command: Some(MenuWatchCommand::Add(ref arguments)),
        }) if arguments.weekday == WatchWeekdayArgument::Thursday
            && arguments.hour == 9
            && arguments.notify
    ));
    assert!(
        CommandLine::try_parse_from([
            "heyfood",
            "watch",
            "add",
            "0c1cb790-0000-4000-8000-000000000000",
            "--weekday",
            "thursday",
            "--hour",
            "24",
        ])
        .is_err()
    );
    let parsed = CommandLine::try_parse_from([
        "heyfood",
        "watch",
        "rm",
        "00000000-0000-4000-8000-000000000010",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Some(Command::Watch {
            command: Some(MenuWatchCommand::Remove(_)),
        })
    ));
}

#[test]
fn menu_watch_renderer_surfaces_schedule_baseline_and_identity_evidence() {
    let response = MenuWatchList {
        watches: vec![MenuWatchSnapshot {
            id: MenuWatchId::parse("00000000-0000-4000-8000-000000000010").unwrap(),
            restaurant_id: RestaurantId::parse("0c1cb790-0000-4000-8000-000000000000").unwrap(),
            cadence: WatchCadenceWire {
                weekday: WatchWeekday::new(3).unwrap(),
                hour: WatchHour::new(9).unwrap(),
            },
            tz: "America/Chicago\u{1b}[2J".into(),
            active: true,
            notify: true,
            next_run_at: "2026-07-30T14:00:00Z".into(),
            last_run_at: None,
            last_snapshot_id: None,
            created_at: "2026-07-23T12:00:00Z".into(),
            menu_url: None,
            identity_verdict: Some("verified".into()),
            identity_confidence: Some(0.92),
            identity_reasoning: None,
            identity_confirmed: None,
            last_change: None,
        }],
        count: 1,
    };
    let output = render_menu_watch_list(&response, OutputMode::HumanPlain);
    assert!(output.contains("Thursday 09:00 · active"));
    assert!(output.contains("awaiting first successful baseline"));
    assert!(output.contains("identity: verified · confidence 0.920"));
    assert!(output.contains("America/Chicago[2J"));
    assert!(!output.contains('\u{1b}'));
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

    let parsed = CommandLine::try_parse_from([
        "heyfood",
        "grocery",
        "export",
        "00000000-0000-4000-8000-000000000123",
        "--format",
        "json",
        "--out",
        "grocery.json",
        "--overwrite",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Some(Command::Grocery {
            command: Some(GroceryCommand::Export(ref arguments)),
        }) if arguments.out.as_deref() == Some(std::path::Path::new("grocery.json"))
            && arguments.overwrite
    ));
    assert!(
        CommandLine::try_parse_from([
            "heyfood",
            "grocery",
            "export",
            "00000000-0000-4000-8000-000000000123",
            "--overwrite",
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
    let plain = render_grocery_list(&display_list(list), OutputMode::HumanPlain);
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
    let output = render_grocery_list(&display_list(list), OutputMode::HumanPlain);
    assert!(output.contains("id:i2"));
    assert!(output.contains("source: recipe:dahl-001"));
    assert!(output.contains("maya-uuid: risky"));
    assert!(output.contains("try: green parts of scallion, garlic-infused oil"));
    assert!(output.contains("Screened at ingredient level — verify the product label."));

    let exclusions = ExclusionListResponseWire {
        exclusions: vec!["pork\u{1b}[2J".into(), "raw onion".into()],
    };
    let rendered = render_grocery_exclusions(
        &GroceryExclusions {
            exclusions: exclusions.exclusions,
        },
        OutputMode::HumanPlain,
    );
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
