use std::collections::BTreeSet;

use clap::{CommandFactory, Parser};
use heyfood_application::{
    GroceryDisplayItem, GroceryDisplayList, GroceryDisplayMemberFlag, GroceryDisplaySafety,
    GroceryDisplaySource, GroceryExclusions, MenuWatchList, MenuWatchSnapshot,
};
use heyfood_cli::{
    Command, CommandLine, CompletionShell, GroceryCommand, GroceryDecisionArgument,
    MenuWatchCommand, OutputMode, WatchWeekdayArgument, render_grocery_exclusions,
    render_grocery_list, render_grocery_mutation_result, render_grocery_proposal, render_json,
    render_menu_watch_list, write_completions,
};
use heyfood_core::{
    ExclusionListResponseWire, GroceryListWire, GroceryMutationProposalWire,
    GroceryMutationResultWire, MenuWatchId, RestaurantId, WatchCadenceWire, WatchHour,
    WatchWeekday,
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
    assert!(health_help.contains("deferred from the supported v0.7.1 contract"));
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
fn grocery_renderer_hides_member_ids_but_preserves_item_ids_provenance_and_json() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/contracts/grocery-backend/phase-a/fixtures/grocery/founding_scenario_maya.json"
    ))
    .unwrap();
    let list: GroceryListWire = serde_json::from_value(fixture["list"].clone()).unwrap();
    let list = display_list(list);
    let output = render_grocery_list(&list, OutputMode::HumanPlain);
    assert!(output.contains("id:i2"));
    assert!(output.contains("source: recipe:dahl-001"));
    assert!(output.contains("Household member: risky"));
    assert!(!output.contains("maya-uuid"));
    assert!(output.contains("try: green parts of scallion, garlic-infused oil"));
    assert!(output.contains("Screened at ingredient level — verify the product label."));
    let machine: serde_json::Value =
        serde_json::from_str(&render_grocery_list(&list, OutputMode::Json)).unwrap();
    assert_eq!(
        machine["items"][1]["safety"]["member_flags"][0]["member_id"],
        "maya-uuid"
    );
    let private_uuid = "3f1c9c2e-2f5a-4a5b-8f1e-9d2b7c6a4e01";
    let mut uuid_list = list.clone();
    uuid_list.items[1].intended_for = Some(private_uuid.into());
    uuid_list.items[1].safety.as_mut().unwrap().member_flags[0].member_id = private_uuid.into();
    let uuid_output = render_grocery_list(&uuid_list, OutputMode::HumanPlain);
    assert!(uuid_output.contains("onion for a household member"));
    assert!(uuid_output.contains("Household member: risky · intended"));
    assert!(!uuid_output.contains(private_uuid));
    uuid_list.items[1].safety.as_mut().unwrap().member_flags[0].reason =
        Some(format!("Risk applies to {private_uuid}."));
    let refused = render_grocery_list(&uuid_list, OutputMode::HumanPlain);
    assert_eq!(
        refused,
        "hey.food returned a Grocery list this version can’t display safely. Refresh the list and try again.\n"
    );
    assert!(!refused.contains(private_uuid));
    let machine: serde_json::Value =
        serde_json::from_str(&render_grocery_list(&uuid_list, OutputMode::Json)).unwrap();
    assert_eq!(
        machine["items"][1]["safety"]["member_flags"][0]["reason"],
        format!("Risk applies to {private_uuid}.")
    );

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
fn grocery_renderer_presents_self_as_you_without_changing_json() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/contracts/grocery-backend/phase-a/fixtures/grocery/founding_scenario_maya.json"
    ))
    .unwrap();
    let mut list: GroceryListWire = serde_json::from_value(fixture["list"].clone()).unwrap();
    list.items[1].intended_for = Some("_self".into());
    list.items[1].safety.as_mut().unwrap().member_flags[0].member_id = "_self".into();
    let list = display_list(list);

    let output = render_grocery_list(&list, OutputMode::HumanPlain);
    assert!(output.contains("onion for you"));
    assert!(output.contains("You: risky · intended"));
    assert!(!output.contains("_self"));

    let machine: serde_json::Value =
        serde_json::from_str(&render_grocery_list(&list, OutputMode::Json)).unwrap();
    assert_eq!(machine["items"][1]["intended_for"], "_self");
    assert_eq!(
        machine["items"][1]["safety"]["member_flags"][0]["member_id"],
        "_self"
    );
}

#[test]
fn human_grocery_proposal_is_a_private_reviewable_non_mutating_card() {
    let mut proposal: GroceryMutationProposalWire = serde_json::from_value(json!({
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
    assert!(output.contains("1. onion for a household member"));
    assert!(output.contains("Household member: risky"));
    assert!(!output.contains("maya"));
    assert!(output.contains("try: green parts of scallion"));
    assert!(output.contains("Nothing has changed"));
    assert!(!output.contains("aaaaaaaa"));

    let machine: serde_json::Value =
        serde_json::from_str(&render_grocery_proposal(&proposal, OutputMode::Json)).unwrap();
    assert_eq!(
        machine["structured_preview"]["items"][0]["intended_for"],
        "maya"
    );
    assert_eq!(
        machine["structured_preview"]["items"][0]["safety"]["member_flags"][0]["member_id"],
        "maya"
    );

    proposal.structured_preview["items"][0]["intended_for"] = json!("_self");
    proposal.structured_preview["items"][0]["safety"]["member_flags"][0]["member_id"] =
        json!("_self");
    let owner_output = render_grocery_proposal(&proposal, OutputMode::HumanPlain);
    assert!(owner_output.contains("1. onion for you"));
    assert!(owner_output.contains("You: risky"));
    assert!(!owner_output.contains("_self"));

    let private_id = "legacyOpaque7";
    proposal.structured_preview["items"][0]["intended_for"] = json!(private_id);
    proposal.structured_preview["items"][0]["safety"]["member_flags"][0]["member_id"] =
        json!(private_id);
    proposal.structured_preview["items"][0]["safety"]["member_flags"][0]["substitutions"] =
        json!([format!("use {private_id}")]);
    let refused = render_grocery_proposal(&proposal, OutputMode::HumanPlain);
    assert_eq!(
        refused,
        "hey.food returned a Grocery change this version can’t display safely. Nothing changed.\n"
    );
    assert!(!refused.contains(private_id));
    let machine: serde_json::Value =
        serde_json::from_str(&render_grocery_proposal(&proposal, OutputMode::Json)).unwrap();
    assert_eq!(
        machine["structured_preview"]["items"][0]["safety"]["member_flags"][0]["substitutions"][0],
        format!("use {private_id}")
    );
}

#[test]
fn grocery_proposal_rejects_additive_item_identity_echo_but_preserves_json() {
    let private_id = "foreignOpaque7";
    let proposal: GroceryMutationProposalWire = serde_json::from_value(json!({
        "confirmation_id": "00000000-0000-4000-8000-000000000001",
        "idempotency_key": "00000000-0000-4000-8000-000000000002",
        "operation": "add_items",
        "expires_at": "2026-07-22T12:05:00Z",
        "structured_preview": {
            "items": [{
                "name": private_id,
                "member_id": private_id
            }]
        },
        "preconditions": [{"type": "list_version", "expected_version": 4}],
        "confirmation_token": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }))
    .unwrap();

    let human = render_grocery_proposal(&proposal, OutputMode::HumanPlain);
    assert_eq!(
        human,
        "hey.food returned a Grocery change this version can’t display safely. Nothing changed.\n"
    );
    assert!(!human.contains(private_id));

    let machine: serde_json::Value =
        serde_json::from_str(&render_grocery_proposal(&proposal, OutputMode::Json)).unwrap();
    assert_eq!(
        machine["structured_preview"]["items"][0]["name"],
        private_id
    );
    assert_eq!(
        machine["structured_preview"]["items"][0]["member_id"],
        private_id
    );
}

#[test]
fn grocery_mutation_receipt_hides_household_protocol_but_preserves_json() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/contracts/grocery-backend/phase-a/fixtures/grocery/founding_scenario_maya.json"
    ))
    .unwrap();
    let result_value = json!({
        "status": "committed",
        "operation": "add_items",
        "confirmation_id": "00000000-0000-4000-8000-000000000001",
        "list": fixture["list"],
        "exclusions": null
    });
    let result: GroceryMutationResultWire = serde_json::from_value(result_value.clone()).unwrap();

    let human = render_grocery_mutation_result(&result, OutputMode::HumanPlain);
    assert!(human.contains("Grocery change confirmed: items added."));
    assert!(human.contains("List version"));
    assert!(!human.contains("maya-uuid"));
    assert!(!human.contains("00000000-0000-4000-8000-000000000001"));

    let machine: serde_json::Value =
        serde_json::from_str(&render_grocery_mutation_result(&result, OutputMode::Json)).unwrap();
    assert_eq!(machine, result_value);
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
