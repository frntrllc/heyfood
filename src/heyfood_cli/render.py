from __future__ import annotations

from datetime import datetime, timezone
import shlex
from typing import Any

from rich.console import Console, Group
from rich.markdown import Markdown
from rich.panel import Panel
from rich.table import Table
from rich.text import Text

from . import onboarding
from . import banner
from . import presentation as p


def intro(console: Console) -> None:
    banner.controller.welcome(console)
    console.print(
        Panel.fit(
            "[bold green]heyfood[/bold green]\n"
            "[dim]hello.food intelligence for people who live in terminals[/dim]\n\n"
            "Create an account, sign in, and get useful dietary guidance "
            "without leaving your terminal.",
            border_style="green",
        )
    )


def noninteractive_intro(console: Console) -> None:
    """Plain, side-effect-free bare-command output for pipes and automation."""
    console.print(
        "heyfood - hello.food for your terminal\n"
        "New here: heyfood register\n"
        "Returning user: heyfood login\n"
        "Run heyfood --help for commands."
    )


def status(console: Console, me: dict[str, Any], whoami: dict[str, Any] | None) -> None:
    table = Table(title="heyfood")
    table.add_column("Field", style="bold")
    table.add_column("Value")
    table.add_row("Account", _cell(me.get("user_id") or "unknown"))
    table.add_row("Device", _cell(me.get("device_id") or "unknown"))
    table.add_row("Auth mode", _cell(me.get("auth_mode") or "unknown"))
    table.add_row("Anonymous", _cell(me.get("is_anonymous")))
    if whoami:
        table.add_row("Channel", _cell(whoami.get("channel") or "unknown"))
        table.add_row("Scopes", _cell(", ".join(whoami.get("scopes") or [])))
    console.print(table)


def profile_members(console: Console, data: dict[str, Any]) -> None:
    profiles = data.get("profiles") if isinstance(data.get("profiles"), list) else []
    if not profiles:
        console.print("[dim]No synced member profiles found. Run `heyfood onboard` first.[/dim]")
        return
    table = Table(title="Synced profile members")
    table.add_column("Member id", style="bold")
    table.add_column("Version", justify="right")
    table.add_column("Updated")
    for profile in profiles:
        if not isinstance(profile, dict):
            continue
        table.add_row(
            Text(str(profile.get("member_id") or "")),
            Text(str(profile.get("version") or "")),
            Text(str(profile.get("updated_at") or "")),
        )
    console.print(table)


def household(console: Console, data: dict[str, Any]) -> None:
    members = data.get("members") if isinstance(data.get("members"), list) else []
    active_scope = data.get("active_scope") if isinstance(data.get("active_scope"), dict) else {}
    table = Table(title="hello.food household")
    table.add_column("Active")
    table.add_column("Name", style="bold")
    table.add_column("Relationship")
    table.add_column("Member id")
    table.add_column("Profile")
    for member in members:
        if not isinstance(member, dict) or member.get("archived"):
            continue
        marker = "*" if member.get("active") else ""
        table.add_row(
            marker,
            _cell(member.get("name")),
            _cell(member.get("relationship")),
            _cell(member.get("id")),
            (
                "local-only"
                if member.get("relationship") == "child"
                else ("synced" if member.get("profile_synced") else "local")
            ),
        )
    if data.get("everyone_available"):
        table.add_row(
            "*" if active_scope.get("mode") == "household" else "",
            "Everyone",
            "whole household",
            "everyone",
            "per-member",
        )
    console.print(table)
    imported = [
        member
        for member in members
        if isinstance(member, dict)
        and not member.get("is_owner")
        and member.get("name") == member.get("id")
    ]
    if imported:
        console.print(
            "[dim]Name an imported profile with: "
            "heyfood household label MEMBER_ID --name NAME --relationship RELATIONSHIP[/dim]"
        )


def household_scope(
    console: Console,
    scope: dict[str, Any],
    *,
    changed: bool = False,
) -> None:
    label = str(scope.get("label") or scope.get("id") or "Me")
    prefix = "Now checking for" if changed else "Checking for"
    console.print(Text(f"{prefix} {label}.", style="green"))
    console.print("[dim]Override one turn with --for, or use /for inside chat.[/dim]")


def agent_choices(console: Console, data: dict[str, Any]) -> None:
    choices = data.get("choices") if isinstance(data.get("choices"), list) else []
    if not choices:
        return
    rows = tuple(
        (p.cell(index, "accent", bold=True), p.cell(choice, "bright"))
        for index, choice in enumerate(choices, start=1)
    )
    blocks: list[p.Block] = [
        p.text_line(
            "Choose one or more" if data.get("allow_multiple") else "Choose one",
            "info",
            bold=True,
        ),
        p.Rows(rows=rows, columns=(p.Column(3, no_wrap=True), p.Column(24, ratio=1))),
        p.text_line(
            "In chat, enter a number. With ask/reply, send the choice text in the next turn.",
            "muted",
        ),
    ]
    p.render(console, blocks)


def household_mutation_effect(console: Console, effect: dict[str, Any]) -> None:
    if effect.get("reason") == "synced_profile_cannot_become_child":
        name = effect.get("name") or effect.get("member_id") or "This member"
        console.print(
            Text(
                f"{name} still has a server-synced profile and cannot be changed to child. "
                "Delete its synced dietary data in hello.food first, or keep the member "
                "as an adult.",
                style="yellow",
            )
        )
        return
    if effect.get("applied"):
        name = effect.get("name") or effect.get("member_id") or "member"
        console.print(Text(f"CLI household updated for {name}.", style="green"))
    sync = effect.get("profile_sync")
    if isinstance(sync, dict) and not sync.get("ok"):
        console.print("[yellow]The local roster was saved, but the dietary profile did not sync.[/yellow]")
        if sync.get("repair"):
            console.print(Text(str(sync["repair"]), style="dim"))
    if effect.get("operation") == "add_member":
        name = effect.get("name") or "NAME"
        console.print(
            Text(
                f'Switch with: heyfood household use "{name}" or /for {name} in chat',
                style="dim",
            )
        )


def conversations(console: Console, data: dict[str, Any]) -> None:
    conversations = (
        data.get("conversations")
        if isinstance(data.get("conversations"), list)
        else []
    )
    if not conversations:
        console.print("[dim]No conversation is remembered on this machine.[/dim]")
        return
    table = Table(title="Local conversation pointer")
    table.add_column("Conversation id", style="bold")
    table.add_column("Updated")
    for conversation in conversations:
        if not isinstance(conversation, dict):
            continue
        table.add_row(
            Text(str(conversation.get("conversation_id") or "")),
            Text(str(conversation.get("updated_at") or "")),
        )
    console.print(table)
    console.print(
        "[dim]Only the last local id is stored; the service does not expose conversation history listing.[/dim]"
    )


def doctor_config(console: Console, config: dict[str, Any], *, config_path: Any) -> None:
    session = config.get("session") if isinstance(config.get("session"), dict) else {}
    oauth = config.get("oauth") if isinstance(config.get("oauth"), dict) else {}
    table = Table(title="heyfood doctor")
    table.add_column("Check", style="bold")
    table.add_column("Value")
    table.add_row("Config", _cell(config_path))
    table.add_row("API", _cell(config.get("api_url") or "default"))
    table.add_row("Auth", _cell(config.get("auth_url") or "default"))
    table.add_row("API key", "present" if config.get("api_key") else "not stored")
    table.add_row("Device", _cell(config.get("device_id") or "missing"))
    table.add_row("Session token", "present" if session.get("access_token") else "missing")
    table.add_row("Session expires", _format_datetime(session.get("access_expires_at")))
    table.add_row("Channel token", "present" if oauth.get("access_token") else "missing")
    table.add_row("Channel expires", _format_datetime(oauth.get("access_expires_at")))
    table.add_row("Scopes", _cell(oauth.get("scope") or ""))
    console.print(table)


def progress(console: Console, data: dict[str, Any]) -> None:
    p.render(console, progress_blocks(data))


def progress_blocks(data: dict[str, Any]) -> list[p.Block]:
    message = str(data.get("message") or "").strip()
    if not message:
        return []
    label, separator, detail = message.partition(":")
    if separator and detail.strip():
        return [
            p.line(
                p.segment(f"{label.strip()}: ", "muted"),
                p.segment(detail.strip(), "bright"),
            )
        ]
    return [p.text_line(message, "muted")]


def explain_item(console: Console, data: dict[str, Any]) -> None:
    status_value = str(data.get("status") or "unknown").replace("_", " ")
    confidence = data.get("confidence")
    tone = _status_tone(status_value)
    blocks: list[p.Block] = [
        p.line(
            p.segment(str(data.get("item_name") or "Item"), "bright", bold=True),
            p.segment(f"  {_status_label(status_value)}", tone, bold=True),
        ),
        p.text_line(data.get("summary") or "No summary returned."),
    ]
    if confidence is not None:
        blocks.append(p.text_line(f"Confidence: {float(confidence):.2f}", "muted"))
    member = (
        data.get("member_name")
        or data.get("member_label")
        or data.get("member_id")
        or data.get("affected_member")
    )
    if member:
        blocks.append(p.text_line(f"Applies to: {member}", "muted"))
    conflicts = data.get("conflicts") or []
    if conflicts:
        blocks.extend((p.blank(), p.text_line("Conflicts", "warning", bold=True)))
        for conflict in conflicts:
            blocks.append(
                p.line(
                    p.segment(f"{conflict.get('ingredient')}: ", "warning"),
                    p.segment(conflict.get("reason") or ""),
                )
            )
    questions = data.get("questions_to_ask") or []
    if questions:
        blocks.extend((p.blank(), p.text_line("Ask staff", "info", bold=True)))
        for question in questions:
            blocks.append(p.text_line(f"- {question}"))
    for heading, keys in (
        ("Uncertainties", ("uncertainties", "uncertainty")),
        ("Possible modifications", ("modifications", "suggested_modifications")),
        ("Alternatives", ("alternatives",)),
    ):
        values = _first_list(data, *keys)
        if values:
            blocks.extend((p.blank(), p.text_line(heading, "info", bold=True)))
            blocks.extend(p.text_line(f"- {value}") for value in values)
    provenance = data.get("provenance") or data.get("source")
    freshness = data.get("menu_freshness") or data.get("freshness")
    if provenance:
        blocks.append(p.text_line(f"Source: {provenance}", "muted"))
    if freshness:
        blocks.append(p.text_line(f"Freshness: {freshness}", "muted"))
    p.render(console, blocks)


def location(console: Console, loc: dict[str, Any] | None) -> None:
    if not loc:
        console.print(
            "[yellow]No saved location.[/yellow] Set one with "
            "[bold]heyfood location set \"San Luis Obispo, CA\"[/bold] "
            "or [bold]heyfood location set --lat 35.28 --lng -120.66[/bold]."
        )
        return
    table = Table(title="Saved location")
    table.add_column("Field", style="bold")
    table.add_column("Value")
    table.add_row("Label", _cell(loc.get("label") or "-"))
    table.add_row("Latitude", f"{float(loc['latitude']):.5f}")
    table.add_row("Longitude", f"{float(loc['longitude']):.5f}")
    radius = loc.get("radius_miles")
    if isinstance(radius, (int, float)):
        table.add_row("Radius", f"{float(radius):g} mi")
    console.print(table)


def restaurants(console: Console, data: dict[str, Any]) -> None:
    rows = []
    for index, row in enumerate(data.get("restaurants") or [], start=1):
        details = [value for value in (_distance(row.get("distance_miles")), row.get("fit_status")) if value]
        if row.get("has_menu"):
            details.append("menu available")
        name_cell = [p.segment(row.get("name") or "", "bright", bold=True)]
        if row.get("address"):
            name_cell.extend((p.segment("\n"), p.segment(row["address"], "muted")))
        rows.append(
            (
                p.cell(index, "accent", bold=True),
                tuple(name_cell),
                p.cell("  ".join(str(value) for value in details), "muted"),
            )
        )
    blocks: list[p.Block] = [
        p.line(
            p.segment("Restaurants", "bright", bold=True),
            p.segment(f"  {data.get('total_count', len(rows))}", "muted"),
        ),
        p.Rows(
            rows=tuple(rows),
            columns=(p.Column(2, no_wrap=True), p.Column(16, ratio=2), p.Column(10, ratio=1)),
        ),
    ]
    p.render(console, blocks)


def menu(console: Console, data: dict[str, Any]) -> None:
    heading = Text(str(data.get("restaurant_name") or "Menu"), style="bold")
    heading.append(f"\n{data.get('item_count', 0)} items")
    console.print(
        Panel.fit(
            heading,
            border_style="green",
        )
    )
    for section in data.get("sections") or []:
        table = Table(title=Text(str(section.get("name") or "Menu")))
        table.add_column("Item")
        table.add_column("Price")
        table.add_column("Description")
        for item in section.get("items") or []:
            table.add_row(
                _cell(item.get("name")),
                _cell(item.get("price_display")),
                _cell(item.get("description")),
            )
        console.print(table)


def recommendations(console: Console, data: dict[str, Any]) -> None:
    p.render(console, recommendation_blocks(data))


def recommendation_blocks(data: dict[str, Any]) -> list[p.Block]:
    rows = []
    restaurant_selector = data.get("restaurant_id") or data.get("restaurant_name") or "RESTAURANT"
    for item in data.get("recommendations") or []:
        if not isinstance(item, dict):
            continue
        item_name = str(item.get("item_name") or "")
        item_cell = (
            p.segment(item_name, "bright", bold=True),
            p.segment("\n"),
            p.segment(
                _item_check_command(item_name, str(restaurant_selector)),
                "muted",
            ),
        )
        rationale = str(item.get("rationale") or "")
        alternatives = item.get("alternatives")
        if isinstance(alternatives, list) and alternatives:
            rationale = (
                f"{rationale} Alternatives: {', '.join(str(value) for value in alternatives[:3])}."
            ).strip()
        rows.append(
            (
                item_cell,
                p.cell(_format_match(item.get("score")), "accent"),
                p.cell(_format_confidence(item.get("confidence")), "muted"),
                p.cell(rationale),
            )
        )
    blocks: list[p.Block] = [
        p.text_line(
            f"Ranked matches - {data.get('restaurant_name', '')}",
            "bright",
            bold=True,
        ),
        p.text_line(
            "Match ranks relevance; it is not a safety verdict. Use the item command for a safety evaluation.",
            "muted",
        ),
        p.Rows(
            rows=tuple(rows),
            columns=(
                p.Column(24, ratio=2),
                p.Column(7, no_wrap=True),
                p.Column(10, no_wrap=True),
                p.Column(22, ratio=2),
            ),
        ),
    ]
    message = data.get("message")
    if message:
        blocks.append(p.text_line(message, "muted"))
    return blocks


def daily_summary(console: Console, data: dict[str, Any]) -> None:
    table = Table(title=Text(f"Meals for {data.get('date', '')}"))
    table.add_column("Meal")
    table.add_column("Items")
    table.add_column("Calories")
    for entry in data.get("entries") or []:
        names = ", ".join(item.get("name", "") for item in entry.get("items", []))
        nutrition = entry.get("nutrition_totals") or {}
        table.add_row(
            _cell(entry.get("meal_type")),
            _cell(names),
            _cell(nutrition.get("calories")),
        )
    console.print(table)


def agent_result(console: Console, data: dict[str, Any]) -> None:
    structured = data.get("structured")
    structured_type = structured.get("type") if isinstance(structured, dict) else None
    text = data.get("message") or data.get("text") or data.get("response")
    structured_renderers = {
        "action_confirmation",
        "safety_verdict",
        "restaurant_discovery",
        "menu_evaluation",
        "household_menu",
        "restaurant_recommendation",
        "restaurant_pending",
        "recipe_search",
        "recipe_details",
    }
    if text and structured_type not in structured_renderers:
        if structured_type == "general_response" and isinstance(structured, dict) and structured.get("meal_nutrition"):
            p.render(console, [p.text_line(text, "accent", bold=True)])
        else:
            console.print(Markdown(str(text)))
    if isinstance(structured, dict):
        if agent_structured(console, structured):
            return
        console.print_json(data=structured)


def agent_structured(console: Console, structured: dict[str, Any]) -> bool:
    result_type = structured.get("type")
    if result_type == "action_confirmation":
        action_confirmation(console, structured)
        return True
    if result_type == "general_response":
        nutrition = structured.get("meal_nutrition")
        if isinstance(nutrition, dict):
            meal_nutrition(console, nutrition)
        _sideband_notes(console, structured)
        return True
    if result_type == "safety_verdict":
        safety_verdict(console, structured)
        _sideband_notes(console, structured)
        return True
    if result_type == "restaurant_discovery":
        agent_restaurants(console, structured)
        _sideband_notes(console, structured)
        return True
    if result_type in {"menu_evaluation", "household_menu"}:
        agent_menu(console, structured)
        _sideband_notes(console, structured)
        return True
    if result_type == "restaurant_recommendation":
        agent_recommendations(console, structured)
        _sideband_notes(console, structured)
        return True
    if result_type == "restaurant_pending":
        restaurant_pending(console, structured)
        return True
    if result_type == "recipe_search":
        recipe_search(console, structured)
        _sideband_notes(console, structured)
        return True
    if result_type == "recipe_details":
        recipe_details(console, structured)
        _sideband_notes(console, structured)
        return True
    return False


def action_confirmation(console: Console, data: dict[str, Any]) -> None:
    p.render(console, action_confirmation_blocks(data))


def action_confirmation_blocks(data: dict[str, Any]) -> list[p.Block]:
    action = str(data.get("action") or "action")
    preview = str(data.get("preview") or _format_action(action))
    structured_preview = data.get("structured_preview")
    if not isinstance(structured_preview, dict):
        structured_preview = {}

    if action == "log_meal":
        title = "Log this meal"
        headline = (
            structured_preview.get("meal_name")
            or preview.removeprefix("Log:").strip()
            or preview
        )
        metadata = (
            ("Type", _titleize(structured_preview.get("meal_type"))),
            ("Restaurant", structured_preview.get("restaurant")),
            ("For", structured_preview.get("member_name")),
            ("Logged", _format_datetime(structured_preview.get("logged_at"))),
        )
    elif action == "add_household_member":
        title = "Add household member"
        headline = str(structured_preview.get("name") or "New member")
        metadata = (
            ("Relationship", _titleize(structured_preview.get("relationship"))),
            ("Diet", _joined_values(structured_preview.get("preferences"))),
            ("Restrictions", _joined_values(structured_preview.get("restrictions"))),
            ("Avoids", _joined_values(structured_preview.get("avoid_ingredients"))),
            ("Condition", _titleize(structured_preview.get("medical_condition_id"))),
        )
    elif action in {"update_household_member", "remove_household_member"}:
        fields = structured_preview.get("fields")
        if not isinstance(fields, dict):
            fields = {}
        title = (
            "Remove household member"
            if action == "remove_household_member"
            else "Update household member"
        )
        headline = str(structured_preview.get("member_id") or preview)
        metadata = tuple(
            (str(key).replace("_", " ").title(), _joined_values(value))
            for key, value in fields.items()
        ) or (("Preview", preview),)
    else:
        title = "Confirmation required"
        headline = _format_action(action)
        metadata = (("Preview", preview),)

    metadata = (*metadata, ("Expires", _format_datetime(data.get("expires_at"))))
    rows = tuple(
        (p.cell(label, "muted"), p.cell(value))
        for label, value in metadata
        if value is not None and str(value).strip()
    )
    return [
        p.text_line(title, "warning", bold=True),
        p.text_line(headline, "bright", bold=True),
        p.Rows(rows=rows, columns=(p.Column(10, no_wrap=True), p.Column(20, ratio=1))),
        p.text_line("Reply with 'confirmed' to run it, or keep chatting to adjust.", "muted"),
    ]


def _joined_values(value: Any) -> str | None:
    if value is None:
        return None
    if isinstance(value, list):
        return ", ".join(str(item) for item in value) or "None"
    if isinstance(value, bool):
        return "Yes" if value else "No"
    return str(value)


def meal_nutrition(console: Console, data: dict[str, Any]) -> None:
    p.render(console, meal_nutrition_blocks(data))


def meal_nutrition_blocks(data: dict[str, Any]) -> list[p.Block]:
    items = data.get("items") if isinstance(data.get("items"), list) else []
    totals = data.get("totals") if isinstance(data.get("totals"), dict) else {}
    rows = []
    for item in items:
        if not isinstance(item, dict):
            continue
        facts = _nutrition_facts(item)
        rows.append(
            (
                p.cell(item.get("name"), "bright", bold=True),
                p.cell("  ".join(facts), "muted"),
            )
        )
    if totals:
        rows.append(
            (
                p.cell("Total", "accent", bold=True),
                p.cell("  ".join(_nutrition_facts(totals)), "accent", bold=True),
            )
        )
    blocks: list[p.Block] = [
        p.text_line("Nutrition estimate", "bright", bold=True),
        p.Rows(rows=tuple(rows), columns=(p.Column(18, ratio=1), p.Column(24, ratio=2))),
    ]
    status = data.get("enrichment_status")
    if status:
        blocks.append(p.text_line(f"Nutrition enrichment: {_titleize(status)}", "muted"))
    return blocks


def safety_verdict(console: Console, data: dict[str, Any]) -> None:
    title = "Safety verdict"
    restaurant = data.get("restaurant_name")
    if restaurant:
        title = f"{title} - {restaurant}"
    rows = []
    for item in data.get("items") or []:
        if not isinstance(item, dict):
            continue
        status_value = str(item.get("status") or "unknown")
        details = _safety_detail(item)
        allergens = _visible_allergens(item.get("allergen_detail"))
        if allergens:
            details = f"{details} Allergens: {allergens}".strip()
        rows.append(
            (
                p.cell(_status_label(status_value), _status_tone(status_value), bold=True),
                p.cell(item.get("food_item") or item.get("name") or item.get("item_name"), "bright", bold=True),
                p.cell(details),
                p.cell(_format_confidence(item.get("confidence")), "muted"),
            )
        )
    p.render(
        console,
        [
            p.text_line(title, "bright", bold=True),
            p.Rows(
                rows=tuple(rows),
                columns=(
                    p.Column(15, no_wrap=True),
                    p.Column(16, ratio=1),
                    p.Column(22, ratio=2),
                    p.Column(5, no_wrap=True, justify="right"),
                ),
            ),
        ],
    )


def agent_restaurants(console: Console, data: dict[str, Any]) -> None:
    rows = data.get("restaurants") or []
    table = Table(title=f"Restaurants ({len(rows)})")
    table.add_column("ID")
    table.add_column("Name")
    table.add_column("Distance")
    table.add_column("Generally safer", justify="right")
    table.add_column("Risky", justify="right")
    table.add_column("Avoid", justify="right")
    for row in rows:
        if not isinstance(row, dict):
            continue
        table.add_row(
            _cell(row.get("id")),
            _cell(row.get("name")),
            _cell(_distance(row.get("distance_miles"))),
            _cell(row.get("safer_item_count")),
            _cell(row.get("risky_item_count")),
            _cell(row.get("avoid_item_count")),
        )
    console.print(table)


def agent_menu(console: Console, data: dict[str, Any]) -> None:
    restaurant = data.get("restaurant_name") or "Menu"
    summary = data.get("summary") if isinstance(data.get("summary"), dict) else {}
    total_items = data.get("total_items")
    if total_items is None:
        total_items = sum(
            len(section.get("items") or [])
            for section in data.get("sections") or []
            if isinstance(section, dict)
        )

    header = _metadata_grid()
    _add_meta(header, "Items", total_items)
    if summary:
        _add_meta(header, "Generally safer", summary.get("safer"))
        _add_meta(header, "Risky", summary.get("risky"))
        _add_meta(header, "Avoid", summary.get("avoid"))
    if data.get("menu_freshness"):
        _add_meta(header, "Freshness", data.get("menu_freshness"))
    console.print(Panel(header, title=Text(str(restaurant)), border_style="green"))

    member_summaries = data.get("member_summaries")
    if isinstance(member_summaries, list) and member_summaries:
        _member_summary(console, member_summaries)

    for section in data.get("sections") or []:
        if not isinstance(section, dict):
            continue
        table = Table(title=Text(str(section.get("name") or "Menu")))
        table.add_column("Item")
        table.add_column("Status")
        table.add_column("Why")
        for item in section.get("items") or []:
            if not isinstance(item, dict):
                continue
            status_value = str(
                item.get("status")
                or item.get("composite_level")
                or item.get("level")
                or "unknown"
            )
            table.add_row(
                _cell(item.get("name")),
                Text(_status_label(status_value), style=_status_color(status_value)),
                _cell(_menu_reason(item)),
            )
        console.print(table)

    conflicts = data.get("conflicts")
    if isinstance(conflicts, list) and conflicts:
        conflict_table = Table(title="Household conflicts")
        conflict_table.add_column("Item")
        conflict_table.add_column("People")
        conflict_table.add_column("Recommendation")
        for conflict in conflicts:
            if not isinstance(conflict, dict):
                continue
            conflict_table.add_row(
                _cell(conflict.get("item_name")),
                _cell(", ".join(conflict.get("involved_members") or [])),
                _cell(conflict.get("recommendation")),
            )
        console.print(conflict_table)


def agent_recommendations(console: Console, data: dict[str, Any]) -> None:
    p.render(console, agent_recommendation_blocks(data))


def agent_recommendation_blocks(data: dict[str, Any]) -> list[p.Block]:
    restaurant = data.get("restaurant_name") or "Restaurant"
    output_rows = []
    for fit, source_rows in (
        ("Generally safer", data.get("safer") or []),
        ("Risky", data.get("risky") or []),
        ("Avoid", data.get("avoid") or []),
    ):
        for row in source_rows:
            if not isinstance(row, dict):
                continue
            item = [p.segment(row.get("name") or "", "bright", bold=True)]
            if row.get("section"):
                item.extend((p.segment("\n"), p.segment(row["section"], "muted")))
            output_rows.append(
                (
                    p.cell(fit, _status_tone(fit), bold=True),
                    tuple(item),
                    p.cell(row.get("explanation") or row.get("description") or ""),
                )
            )
    return [
        p.text_line(f"Recommendations - {restaurant}", "bright", bold=True),
        p.Rows(
            rows=tuple(output_rows),
            columns=(p.Column(15, no_wrap=True), p.Column(18, ratio=1), p.Column(24, ratio=2)),
        ),
    ]


def restaurant_pending(console: Console, data: dict[str, Any]) -> None:
    restaurant = data.get("restaurant_name") or "this restaurant"
    seconds = data.get("estimated_seconds") or 15
    p.render(
        console,
        [
            p.text_line("Restaurant menu", "warning", bold=True),
            p.text_line(f"I'm pulling the menu for {restaurant}."),
            p.text_line(f"Expected in about {seconds} seconds.", "muted"),
        ],
    )


def recipe_search(console: Console, data: dict[str, Any]) -> None:
    p.render(console, recipe_search_blocks(data))


def recipe_search_blocks(data: dict[str, Any]) -> list[p.Block]:
    recipes = data.get("recipes") or []
    title = f"Recipes ({len(recipes)})"
    query = data.get("query_used")
    if query:
        title = f"{title} - {query}"
    rows = []
    for index, recipe in enumerate(recipes, start=1):
        if not isinstance(recipe, dict):
            continue
        recipe_cell = [p.segment(recipe.get("title") or recipe.get("name") or "", "bright", bold=True)]
        tags = ", ".join(_titleize(tag) for tag in (recipe.get("dietary_tags") or [])[:3])
        if tags:
            recipe_cell.extend((p.segment("\n"), p.segment(tags, "muted")))
        facts = [
            _format_minutes(recipe.get("ready_in_minutes") or recipe.get("readyInMinutes")),
            _format_calories(recipe.get("calories_per_serving")),
            f"{_format_match(recipe.get('dietary_match_hint'), recipe.get('partially_compatible'))} match",
            _recipe_ref(recipe),
        ]
        rows.append(
            (
                p.cell(index, "accent", bold=True),
                tuple(recipe_cell),
                p.cell("  ".join(value for value in facts if value), "muted"),
            )
        )
    blocks: list[p.Block] = [
        p.text_line(title, "bright", bold=True),
        p.Rows(
            rows=tuple(rows),
            columns=(p.Column(2, no_wrap=True), p.Column(20, ratio=2), p.Column(22, ratio=2)),
        ),
    ]
    message = data.get("message")
    if message:
        blocks.append(p.text_line(message, "muted"))
    if data.get("personalized") is False:
        blocks.append(p.text_line("No synced dietary profile found; showing general recipe results.", "muted"))
    return blocks


def recipe_details(console: Console, data: dict[str, Any]) -> None:
    title = data.get("title") or data.get("name") or "Recipe"
    body = _metadata_grid()
    _add_meta(body, "Ready", data.get("ready_in_minutes") or data.get("readyInMinutes"))
    _add_meta(body, "Servings", data.get("servings"))
    summary = data.get("summary")
    if summary:
        _add_meta(body, "Summary", str(summary))
    console.print(Panel(body, title=Text(str(title)), border_style="green"))


def saved_recipe_saved(console: Console, data: dict[str, Any]) -> None:
    p.render(console, saved_recipe_saved_blocks(data))


def saved_recipe_saved_blocks(data: dict[str, Any]) -> list[p.Block]:
    recipe = data.get("recipe") if isinstance(data.get("recipe"), dict) else {}
    title = recipe.get("title") or "Recipe"
    message = data.get("message") or f"Saved {title}."
    metadata = (
        ("Ref", _recipe_ref(recipe)),
        ("Ready", _format_minutes(recipe.get("ready_in_minutes"))),
        ("Servings", recipe.get("servings")),
        ("Saved", _format_datetime(recipe.get("updated_at"))),
    )
    rows = tuple(
        (p.cell(label, "muted"), p.cell(value))
        for label, value in metadata
        if value is not None and str(value).strip()
    )
    return [
        p.text_line(str(message), "accent", bold=True),
        p.Rows(rows=rows, columns=(p.Column(9, no_wrap=True), p.Column(20, ratio=1))),
    ]


def saved_recipes(console: Console, data: dict[str, Any]) -> None:
    recipes = data.get("recipes") or []
    if not recipes:
        console.print(
            Panel(
                "No saved recipes yet. Try [bold]heyfood recipes search \"dinner ideas\"[/bold].",
                title="Saved Recipes",
                border_style="yellow",
            )
        )
        return

    table = Table(title=f"Saved Recipes ({data.get('total_count', len(recipes))})")
    table.add_column("#", justify="right")
    table.add_column("Recipe")
    table.add_column("Ready")
    table.add_column("Calories")
    table.add_column("Cooked")
    table.add_column("Ref")
    table.add_column("Tags")
    for index, recipe in enumerate(recipes, start=1):
        if not isinstance(recipe, dict):
            continue
        table.add_row(
            str(index),
            _cell(recipe.get("title") or recipe.get("name")),
            _cell(_format_minutes(recipe.get("ready_in_minutes"))),
            _cell(_format_calories(recipe.get("calories_per_serving"))),
            _cell(recipe.get("times_cooked")),
            _cell(_recipe_ref(recipe)),
            _cell(", ".join((recipe.get("dietary_tags") or [])[:3])),
        )
    console.print(table)


def profile_summary(
    console: Console,
    profile_data: dict[str, Any],
    *,
    member_id: str = "_self",
    version: int | None = None,
    updated_at: str | None = None,
) -> None:
    profile = _normalize_profile(profile_data)
    title = f"Dietary Graph - {member_id}"
    if version:
        title = f"{title} v{version}"

    has_visible_content = any(
        bool(profile.get(key))
        for key in (
            "health_condition_ids",
            "custom_health_conditions",
            "diet_style_ids",
            "allergy_ids",
            "preferences",
            "custom_diet_styles",
            "restrictions",
            "custom_restrictions",
            "avoid_ingredients",
            "activity_level",
            "cuisine_preferences",
            "custom_cuisines",
            "notes",
        )
    )

    fields = _metadata_grid()
    _add_meta(fields, "Conditions", _join_labels(profile["health_condition_ids"], _CONDITION_LABELS))
    if profile["custom_health_conditions"]:
        _add_meta(fields, "Custom conditions", ", ".join(profile["custom_health_conditions"]))
    if profile["diet_style_ids"]:
        _add_meta(fields, "Diet styles", _join_labels(profile["diet_style_ids"], _DIET_STYLE_LABELS))
    else:
        _add_meta(fields, "Preferences", _join_labels(profile["preferences"], _PREFERENCE_LABELS))
    custom_diets = [
        value
        for value in profile["custom_diet_styles"]
        if value not in {_DIET_STYLE_LABELS.get(item) for item in profile["diet_style_ids"]}
    ]
    if custom_diets:
        _add_meta(fields, "Custom diets", ", ".join(custom_diets))
    _add_meta(fields, "Allergies", _join_labels(profile["allergy_ids"], _ALLERGY_LABELS))
    _add_meta(fields, "Restrictions", _join_labels(profile["restrictions"], _RESTRICTION_LABELS))
    custom_restrictions = [
        value
        for value in profile["custom_restrictions"]
        if value not in {_ALLERGY_LABELS.get(item) for item in profile["allergy_ids"]}
    ]
    if custom_restrictions:
        _add_meta(fields, "Custom restrictions", ", ".join(custom_restrictions))
    _add_meta(fields, "Avoid", ", ".join(profile["avoid_ingredients"]))
    _add_meta(fields, "Activity", _ACTIVITY_LABELS.get(profile["activity_level"], profile["activity_level"]))
    _add_meta(fields, "Cuisines", _join_labels(profile["cuisine_preferences"], _CUISINE_LABELS))
    if profile["custom_cuisines"]:
        _add_meta(fields, "Custom cuisines", ", ".join(profile["custom_cuisines"]))
    if profile["health_condition_ids"]:
        _add_meta(fields, "Severity", profile["severity_level"])
    _add_meta(fields, "Notes", profile["notes"])
    _add_meta(fields, "Updated", _format_datetime(updated_at))

    if not has_visible_content:
        console.print(
            Panel(
                "No dietary graph yet. Run [bold]heyfood onboard[/bold] to build one.",
                title=title,
                border_style="yellow",
            )
        )
        return

    console.print(Panel(fields, title=title, border_style="green"))


def onboarding_options(console: Console, catalog: dict[str, tuple[Any, ...]]) -> None:
    for title, options in catalog.items():
        table = Table(title=_titleize(title))
        table.add_column("Label")
        table.add_column("ID")
        for option in options:
            table.add_row(str(getattr(option, "label", "")), str(getattr(option, "id", "")))
        console.print(table)


def _status_color(status_value: str) -> str:
    canonical = _canonical_status(status_value)
    if canonical == "avoid":
        return "red"
    if canonical == "risky":
        return "yellow"
    if canonical == "generally_safer":
        return "green"
    return "blue"


def _status_tone(status_value: str) -> p.Tone:
    color = _status_color(status_value.lower())
    return {
        "green": "accent",
        "yellow": "warning",
        "red": "danger",
        "blue": "info",
    }[color]  # type: ignore[return-value]


def _nutrition_facts(data: dict[str, Any]) -> list[str]:
    values = (
        data.get("portion"),
        f"{_format_number(data.get('calories'))} cal" if _format_number(data.get("calories")) else None,
        f"{_format_grams(data.get('protein_g'))} protein" if _format_grams(data.get("protein_g")) else None,
        f"{_format_grams(data.get('carbs_g'))} carbs" if _format_grams(data.get("carbs_g")) else None,
        f"{_format_grams(data.get('fat_g'))} fat" if _format_grams(data.get("fat_g")) else None,
    )
    return [str(value) for value in values if value is not None and str(value).strip()]


def _distance(value: Any) -> str:
    if value is None:
        return ""
    return f"{float(value):.1f} mi"


def _metadata_grid() -> Table:
    table = Table.grid(padding=(0, 2))
    table.add_column(style="dim", no_wrap=True)
    table.add_column()
    return table


def _add_meta(table: Table, label: str, value: Any) -> None:
    if value is None:
        return
    text = str(value).strip()
    if not text:
        return
    table.add_row(Text(label), Text(text))


def _cell(value: Any) -> Text:
    if value is None:
        return Text("")
    return Text(str(value))


def _format_action(value: Any) -> str:
    return _titleize(str(value or "action").replace("_", " "))


def _titleize(value: Any) -> str:
    if value is None:
        return ""
    text = str(value).replace("_", " ").strip()
    return text[:1].upper() + text[1:] if text else ""


def _status_label(value: Any) -> str:
    canonical = _canonical_status(value)
    labels = {
        "generally_safer": "Generally safer",
        "risky": "Risky",
        "avoid": "Avoid",
        "unable_to_evaluate": "Unable to evaluate",
    }
    return labels.get(canonical, _titleize(value))


def _canonical_status(value: Any) -> str:
    normalized = str(value or "").strip().lower().replace("-", "_").replace(" ", "_")
    if normalized in {"safe", "safer", "generally_safe", "generally_safer"}:
        return "generally_safer"
    if normalized in {"risky", "risk", "caution", "needs_review"}:
        return "risky"
    if normalized in {"avoid", "unsafe"}:
        return "avoid"
    if normalized in {"", "unknown", "unable", "unable_to_evaluate", "not_evaluated"}:
        return "unable_to_evaluate"
    return normalized


def _format_number(value: Any) -> str:
    number = _to_float(value)
    if number is None:
        return ""
    if number.is_integer():
        return str(int(number))
    return f"{number:.1f}"


def _format_grams(value: Any) -> str:
    number = _format_number(value)
    return f"{number}g" if number else ""


def _format_confidence(value: Any) -> str:
    number = _to_float(value)
    if number is None:
        return ""
    if number <= 1:
        number *= 100
    return f"{number:.0f}%"


def _format_match(value: Any, partial: Any = False) -> str:
    number = _to_float(value)
    if number is None:
        return "partial" if partial else ""
    label = f"{number * 100:.0f}%" if number <= 1 else f"{number:.0f}%"
    return f"{label} partial" if partial else label


def _format_minutes(value: Any) -> str:
    number = _to_float(value)
    if number is None:
        return ""
    return f"{int(number)} min"


def _format_calories(value: Any) -> str:
    number = _to_float(value)
    if number is None:
        return ""
    return f"{int(number)} cal"


def _recipe_ref(recipe: dict[str, Any]) -> str:
    ref = recipe.get("recipe_ref")
    if isinstance(ref, dict):
        provider = ref.get("provider") or recipe.get("provider")
        external_id = ref.get("external_id") or recipe.get("external_recipe_id")
        if provider and external_id:
            return f"{provider}:{external_id}"
    provider = recipe.get("provider")
    external_id = recipe.get("external_recipe_id") or recipe.get("spoonacular_id")
    if provider and external_id:
        return f"{provider}:{external_id}"
    return str(external_id or "")


def _to_float(value: Any) -> float | None:
    if value is None or value == "":
        return None
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


def _format_datetime(value: Any) -> str:
    if not isinstance(value, str) or not value:
        return ""
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return value
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    parsed = parsed.astimezone()
    hour = parsed.strftime("%I").lstrip("0") or "0"
    return f"{parsed.strftime('%b')} {parsed.day}, {parsed.year} {hour}:{parsed.strftime('%M %p')}"


def _visible_allergens(value: Any) -> str:
    if not isinstance(value, list):
        return ""
    names: list[str] = []
    for detail in value:
        if not isinstance(detail, dict):
            continue
        if str(detail.get("confidence") or "").lower() == "low":
            continue
        allergen = detail.get("allergen")
        if allergen:
            names.append(str(allergen))
    return ", ".join(names)


def _first_list(data: dict[str, Any], *keys: str) -> list[str]:
    for key in keys:
        value = data.get(key)
        if isinstance(value, list):
            return [str(item).strip() for item in value if str(item).strip()]
        if isinstance(value, str) and value.strip():
            return [value.strip()]
    return []


def _guidance_supplement(data: dict[str, Any]) -> str:
    parts: list[str] = []
    member = (
        data.get("member_name")
        or data.get("member_label")
        or data.get("member_id")
        or data.get("affected_member")
    )
    if member:
        parts.append(f"Applies to: {member}.")
    for label, keys in (
        ("Uncertainty", ("uncertainties", "uncertainty")),
        ("Modifications", ("modifications", "suggested_modifications")),
        ("Ask staff", ("questions_to_ask", "questions_to_ask_staff")),
        ("Alternatives", ("alternatives",)),
    ):
        values = _first_list(data, *keys)
        if values:
            parts.append(f"{label}: {', '.join(values)}.")
    freshness = data.get("menu_freshness") or data.get("freshness")
    provenance = data.get("provenance") or data.get("source")
    if freshness:
        parts.append(f"Freshness: {freshness}.")
    if provenance:
        parts.append(f"Source: {provenance}.")
    return " ".join(parts)


def _safety_detail(item: dict[str, Any]) -> str:
    base = str(item.get("reason") or item.get("explanation") or "").strip()
    supplement = _guidance_supplement(item)
    return " ".join(value for value in (base, supplement) if value).strip()


def _menu_reason(item: dict[str, Any]) -> str:
    base = ""
    if item.get("explanation"):
        base = str(item["explanation"])
    else:
        safety = item.get("safety")
        if isinstance(safety, dict):
            reasons = []
            for entry in safety.values():
                if isinstance(entry, dict) and entry.get("reason"):
                    label = entry.get("label") or entry.get("member_id") or "Member"
                    reasons.append(f"{label}: {entry['reason']}")
            if reasons:
                base = "; ".join(reasons[:2])
        if not base and item.get("description"):
            base = str(item["description"])
        if not base:
            base = _visible_allergens(item.get("allergen_detail"))
    supplement = _guidance_supplement(item)
    return " ".join(value for value in (base, supplement) if value).strip()


def _item_check_command(item_name: str, restaurant: str) -> str:
    return (
        f"heyfood item {shlex.quote(item_name)} "
        f"--restaurant {shlex.quote(restaurant)}"
    )


def _member_summary(console: Console, rows: list[Any]) -> None:
    table = Table(title="Household fit")
    table.add_column("Member")
    table.add_column("Diet")
    table.add_column("Generally safer", justify="right")
    table.add_column("Caution", justify="right")
    table.add_column("Avoid", justify="right")
    for row in rows:
        if not isinstance(row, dict):
            continue
        table.add_row(
            _cell(row.get("label") or row.get("member_id")),
            _cell(row.get("diet_label") or row.get("top_restriction")),
            _cell(row.get("safe_count")),
            _cell(row.get("caution_count")),
            _cell(row.get("avoid_count")),
        )
    console.print(table)


def _normalize_profile(data: dict[str, Any]) -> dict[str, Any]:
    aliases = {
        "healthConditionIds": "health_condition_ids",
        "customHealthConditions": "custom_health_conditions",
        "customDietStyles": "custom_diet_styles",
        "customRestrictions": "custom_restrictions",
        "customCuisines": "custom_cuisines",
        "dietStyleIds": "diet_style_ids",
        "allergyIds": "allergy_ids",
    }
    profile = dict(data or {})
    for alias, key in aliases.items():
        if alias in profile and key not in profile:
            profile[key] = profile[alias]
    defaults = {
        "preferences": [],
        "restrictions": [],
        "avoid_ingredients": [],
        "medical_constraints": [],
        "activity_level": None,
        "cuisine_preferences": [],
        "health_condition_ids": [],
        "custom_health_conditions": [],
        "custom_diet_styles": [],
        "custom_restrictions": [],
        "custom_cuisines": [],
        "diet_style_ids": [],
        "allergy_ids": [],
        "severity_level": None,
        "notes": None,
    }
    for key, default in defaults.items():
        profile.setdefault(key, default)
    return profile


def _join_labels(values: Any, labels: dict[str, str]) -> str:
    if not isinstance(values, list):
        return ""
    return ", ".join(labels.get(str(value), _titleize(value)) for value in values)


def _sideband_notes(console: Console, structured: dict[str, Any]) -> None:
    captured_name = structured.get("captured_user_name")
    if captured_name:
        console.print(Text(f"Remembered name: {captured_name}", style="dim"))
    if structured.get("homescreen_mutation"):
        console.print("[dim]Home screen updates are queued for the mobile app.[/dim]")
    if structured.get("household_mutation"):
        console.print("[dim]Household updates are queued for the mobile app.[/dim]")


_CONDITION_LABELS = {
    "celiac": "Celiac disease",
    "diabetes_type_1": "Type 1 diabetes",
    "diabetes_type_2": "Type 2 diabetes",
    "ibs": "IBS",
    "crohns": "Crohn's disease",
    "gastroparesis": "Gastroparesis",
    "gerd": "GERD / Acid reflux",
    "food_allergies_general": "Food allergies (general)",
    "colitis": "Ulcerative colitis",
    "diverticulitis": "Diverticulitis",
    "eoe": "Eosinophilic esophagitis",
    "ckd": "Kidney disease / CKD",
    "pku": "PKU",
    "alpha_gal": "Alpha-gal syndrome",
    "mcas": "MCAS",
    "histamine_intolerance": "Histamine intolerance",
    "fructose_malabsorption": "Fructose malabsorption",
    "sibo": "SIBO",
    "hypertension": "Hypertension / high blood pressure",
    "arfid": "ARFID",
    "autism_sensory": "Autism / sensory food needs",
}
_DIET_STYLE_LABELS = {
    option.id: option.label for option in onboarding.DIET_STYLES
}
_ALLERGY_LABELS = {
    option.id: option.label for option in onboarding.ALLERGIES
}
_PREFERENCE_LABELS = {
    "keto": "Keto",
    "vegan": "Vegan",
    "vegetarian": "Vegetarian",
    "paleo": "Paleo",
    "mediterranean": "Mediterranean",
    "lowCarb": "Low carb",
    "whole30": "Whole30",
    "pescatarian": "Pescatarian",
    "low_fodmap": "Low-FODMAP",
    "high_protein": "High protein",
}
_RESTRICTION_LABELS = {
    "glutenFree": "Gluten-free",
    "dairyFree": "Dairy-free",
    "nutFree": "Nut-free",
    "peanutFree": "Peanut-free",
    "treeNutFree": "Tree nut-free",
    "shellfishFree": "Shellfish-free",
    "fishFree": "Fish-free",
    "soyFree": "Soy-free",
    "eggFree": "Egg-free",
    "sesameFree": "Sesame-free",
    "lactoseIntolerant": "Lactose intolerant",
    "halal": "Halal",
    "kosher": "Kosher",
}
_ACTIVITY_LABELS = {
    "very_active": "Very active (5+/week)",
    "moderate": "Moderate (2-4/week)",
    "light": "Light activity",
    "sedentary": "Sedentary",
    "prefer_not_to_say": "Prefer not to say",
}
_CUISINE_LABELS = {
    "mexican": "Mexican",
    "italian": "Italian",
    "japanese": "Japanese",
    "thai": "Thai",
    "indian": "Indian",
    "mediterranean": "Mediterranean",
    "korean": "Korean",
    "american": "American",
    "chinese": "Chinese",
    "vietnamese": "Vietnamese",
    "ethiopian": "Ethiopian",
    "french": "French",
    "greek": "Greek",
    "middle_eastern": "Middle Eastern",
    "spanish": "Spanish / Tapas",
    "brazilian": "Brazilian",
    "peruvian": "Peruvian",
    "turkish": "Turkish",
    "caribbean": "Jamaican / Caribbean",
    "southern": "Southern / Soul food",
    "cajun_creole": "Cajun / Creole",
    "german": "German",
    "british": "British",
    "filipino": "Filipino",
    "malaysian": "Malaysian",
    "indonesian": "Indonesian",
    "hawaiian": "Hawaiian / Polynesian",
    "georgian": "Georgian",
}
