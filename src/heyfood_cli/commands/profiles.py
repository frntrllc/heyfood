"""Dietary profile, onboarding, members, and household commands."""
from __future__ import annotations

from .. import main
from ..main import (
    Any,
    ConfigStore,
    Confirm,
    HelloFoodError,
    LoginRequired,
    Optional,
    Panel,
    Prompt,
    Table,
    _fail,
    _is_profile_sync_consent_required,
    _json_mode,
    _raise_command_error,
    _validated,
    _write_result,
    app,
    household,
    household_app,
    members_app,
    onboarding,
    output,
    personality,
    render,
    typer,
    validation,
)

@members_app.command("list")
def members_list(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """List synced dietary profile member ids."""
    json_mode = _json_mode(json_output, raw)
    client = main.HelloFoodClient()
    try:
        data = client.list_profile_members()
    except (LoginRequired, HelloFoodError) as exc:
        _raise_command_error(exc, json_mode=json_mode)
    if _write_result(data, json_mode=json_mode):
        return
    render.profile_members(main.console, data)


@household_app.command("list")
def household_list(
    local_only: bool = typer.Option(
        False,
        "--local-only",
        help="Do not discover newly synced profile ids from the service.",
    ),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """List household members and the active conversational scope."""
    json_mode = _json_mode(json_output, raw)
    client = main.HelloFoodClient(create_device=not local_only)
    reconciliation: dict[str, str] | None = None
    try:
        state = client.household_state() if local_only else client.refresh_household_state()
    except LoginRequired as exc:
        _raise_command_error(exc, json_mode=json_mode)
    except HelloFoodError as exc:
        if not _is_profile_sync_consent_required(exc):
            _raise_command_error(exc, json_mode=json_mode)
        state = client.household_state()
        reconciliation = {
            "status": "skipped",
            "reason": "profile_sync_consent_required",
            "source": "local_roster",
        }
    document = household.public_document(state)
    if reconciliation is not None:
        document["reconciliation"] = reconciliation
    if _write_result(document, json_mode=json_mode):
        return
    render.household(main.console, document)
    if reconciliation is not None:
        main.console.print(
            "[dim]Showing the local roster. Synced member discovery is available "
            "after profile sync consent is granted through `heyfood onboard`.[/dim]"
        )


@household_app.command("current")
def household_current(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Show the locally active conversational household scope."""
    json_mode = _json_mode(json_output, raw)
    state = main.HelloFoodClient(create_device=False).household_state()
    document = household.public_document(state)
    current = {"ok": True, "active_scope": document["active_scope"]}
    if _write_result(current, json_mode=json_mode):
        return
    render.household_scope(main.console, current["active_scope"])


@household_app.command("use")
def household_use(
    selector: str = typer.Argument(..., help="Member name/id, me, or everyone."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Persist the default household scope for ask, reply, chat, and log."""
    json_mode = _json_mode(json_output, raw)
    client = main.HelloFoodClient()
    try:
        try:
            state = client.set_household_scope(selector)
        except household.HouseholdError:
            client.refresh_household_state()
            state = client.set_household_scope(selector)
    except household.HouseholdError as exc:
        raise typer.BadParameter(str(exc)) from exc
    except (LoginRequired, HelloFoodError) as exc:
        _raise_command_error(exc, json_mode=json_mode)
    document = household.public_document(state)
    selected = {"ok": True, "active_scope": document["active_scope"]}
    if _write_result(selected, json_mode=json_mode):
        return
    render.household_scope(main.console, selected["active_scope"], changed=True)


@household_app.command("label")
def household_label(
    selector: str = typer.Argument(..., help="Existing member name or id."),
    name: str = typer.Option(..., "--name", help="Local display name."),
    relationship: Optional[str] = typer.Option(
        None,
        "--relationship",
        help="spouse, partner, parent, child, sibling, grandparent, friend, or other.",
    ),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Give an imported profile id a local name and relationship."""
    json_mode = _json_mode(json_output, raw)
    client = main.HelloFoodClient(create_device=False)
    try:
        state = client.label_household_member(
            selector,
            name=name,
            relationship=relationship,
        )
    except household.HouseholdError as exc:
        raise typer.BadParameter(str(exc)) from exc
    document = household.public_document(state)
    if _write_result(document, json_mode=json_mode):
        return
    render.household(main.console, document)


@app.command()
def profile(
    member_id: str = typer.Option("_self", "--member-id", help="Synced profile member id."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Show your synced dietary graph."""
    json_mode = _json_mode(json_output, raw)
    member_id = _validated(
        lambda: validation.required_text(member_id, label="Member id", max_length=255)
    )
    client = main.HelloFoodClient()
    try:
        consent = client.profile_consent_status()
        if not consent.get("has_consent"):
            _fail(
                "Profile sync consent has not been granted yet.",
                kind="profile_consent_required",
                json_mode=json_mode,
                hint="Run `heyfood onboard` to build your dietary graph.",
            )
        data = client.download_profile(member_id=member_id)
    except LoginRequired as exc:
        _raise_command_error(exc, json_mode=json_mode)
    except HelloFoodError as exc:
        if str(exc).startswith("404:"):
            _fail(
                "No synced dietary graph found.",
                kind="profile_not_found",
                json_mode=json_mode,
                hint="Run `heyfood onboard` to create one.",
            )
        _raise_command_error(exc, json_mode=json_mode)

    if _write_result(data, json_mode=json_mode):
        return
    render.profile_summary(
        main.console,
        data.get("profile_data") or {},
        member_id=str(data.get("member_id") or member_id),
        version=data.get("version"),
        updated_at=data.get("updated_at"),
    )


@app.command()
def onboard(
    profile_text: Optional[list[str]] = typer.Argument(None, help="Optional natural-language dietary profile."),
    diet: Optional[list[str]] = typer.Option(None, "--diet", "-d", help="Diet style. Repeat or comma-separate."),
    allergy: Optional[list[str]] = typer.Option(None, "--allergy", "-a", help="Allergy or restriction. Repeat or comma-separate."),
    condition: Optional[list[str]] = typer.Option(None, "--condition", "-c", help="Health condition. Repeat or comma-separate."),
    avoid: Optional[list[str]] = typer.Option(None, "--avoid", help="Specific ingredient to avoid. Repeat or comma-separate."),
    cuisine: Optional[list[str]] = typer.Option(None, "--cuisine", help="Cuisine preference. Repeat or comma-separate."),
    activity: Optional[str] = typer.Option(None, "--activity", help="Activity level id or label."),
    notes: Optional[str] = typer.Option(None, "--notes", help="Additional dietary notes."),
    severity: Optional[int] = typer.Option(None, "--severity", min=1, max=5, help="Condition severity from 1-5."),
    member_id: str = typer.Option("_self", "--member-id", help="Synced profile member id."),
    replace: bool = typer.Option(False, "--replace", help="Replace the existing profile instead of merging answered fields."),
    yes: bool = typer.Option(False, "--yes", "-y", help="Grant sync consent and save without confirmation."),
    voice: bool = typer.Option(False, "--voice", help="Capture your dietary profile by voice before extracting it."),
    voice_capture_mode: str = typer.Option("auto", "--voice-capture", help="Voice capture mode: auto, native, browser, or typed."),
    audio_device: Optional[str] = typer.Option(None, "--audio-device", help="Input device id or name for native voice capture."),
    voice_timeout: int = typer.Option(300, "--voice-timeout", help="Seconds to wait for browser voice capture (browser rung only)."),
    no_browser: bool = typer.Option(False, "--no-browser", help="With --voice, print the browser capture URL instead of opening it (browser rung only)."),
    interactive: bool = typer.Option(True, "--interactive/--no-interactive", help="Prompt for missing fields."),
    no_input: bool = typer.Option(False, "--no-input", help="Never prompt; fail if required input or approval is missing."),
    list_options: bool = typer.Option(False, "--list-options", help="Show accepted onboarding labels and ids."),
    dry_run: bool = typer.Option(False, "--dry-run", help="Build and print the profile payload without saving."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Build or update your hello.food dietary graph."""
    json_mode = _json_mode(json_output, raw)
    interactive = (
        interactive is not False
        and no_input is not True
        and not json_mode
        and main._stdin_is_tty()
    )
    if list_options:
        if _write_result(onboarding.option_catalog(), json_mode=json_mode):
            return
        render.onboarding_options(main.console, onboarding.option_catalog())
        return
    member_id = _validated(
        lambda: validation.required_text(member_id, label="Member id", max_length=255)
    )
    notes = _validated(
        lambda: validation.optional_text(notes, label="Notes", max_length=280)
    )
    voice_timeout = _validated(
        lambda: validation.bounded_integer(
            voice_timeout,
            label="Voice timeout",
            minimum=1,
            maximum=900,
        )
    )
    if not dry_run and not yes and not interactive:
        raise typer.BadParameter(
            "Non-interactive profile mutations require --yes. "
            "Use `heyfood onboard --yes --no-input ...`."
        )

    store = ConfigStore()
    configured = store.load()
    configured_household = household.normalize_state(
        configured.get("household"),
        owner_name=configured.get("first_name"),
    )
    target_member = household.find_member(configured_household, member_id)
    is_child_profile = bool(
        target_member is not None and target_member.get("relationship") == "child"
    )
    text_profile = _validated(
        lambda: validation.optional_text(
            " ".join(profile_text or []),
            label="Profile text",
            max_length=1000,
        )
    ) or ""
    if voice:
        text_profile = main._voice_transcript(
            purpose="onboarding",
            capture_mode=voice_capture_mode,
            audio_device=audio_device,
            json_mode=json_mode,
            open_browser=not no_browser,
            browser_timeout=voice_timeout,
        )

    first_name = _resolve_first_name(
        store,
        text_profile=text_profile,
        should_prompt=interactive and not profile_text and not voice,
        persist=not dry_run,
    )
    if first_name and not json_mode:
        message = personality.welcome_message(
            first_name,
            first_time=personality.should_show_first_welcome(store),
        )
        main.console.print(f"[bold green]{message}[/bold green]")
        if not dry_run:
            personality.mark_welcomed(store)

    extracted = onboarding.parse_profile_text(text_profile) if text_profile else {}
    provided = _provided_onboarding_fields(
        profile_text=[text_profile] if text_profile else None,
        diet=diet,
        allergy=allergy,
        condition=condition,
        avoid=avoid,
        cuisine=cuisine,
        activity=activity,
        notes=notes,
        severity=severity,
    )
    if not provided and not interactive:
        raise typer.BadParameter(
            "Provide at least one profile field, or run `heyfood onboard` interactively."
        )

    values = {
        "diets": onboarding.split_values(diet) if diet is not None else extracted.get("diets", []),
        "allergies": onboarding.split_values(allergy) if allergy is not None else extracted.get("allergies", []),
        "conditions": onboarding.split_values(condition) if condition is not None else extracted.get("conditions", []),
        "avoid_ingredients": onboarding.split_values(avoid) if avoid is not None else extracted.get("avoid_ingredients", []),
        "cuisines": onboarding.split_values(cuisine) if cuisine is not None else extracted.get("cuisines", []),
        "activity_level": activity if activity is not None else extracted.get("activity_level"),
        "notes": notes,
        "severity_level": severity,
    }
    answered_sections = set(extracted.get("answered_sections") or [])
    explicit_sections = {
        key
        for key, raw in {
            "diets": diet,
            "allergies": allergy,
            "conditions": condition,
            "avoid_ingredients": avoid,
            "cuisines": cuisine,
            "activity_level": activity,
        }.items()
        if raw is not None
    }
    answered_sections.update(explicit_sections)

    if text_profile and not _has_extracted_onboarding_values(values) and not json_mode:
        main.console.print(
            "[yellow]I couldn't confidently extract dietary details from that text.[/yellow]"
        )

    if interactive and text_profile:
        _print_extracted_onboarding_review(values, answered_sections)
        use_extracted = yes or Confirm.ask(
            "Use these extracted values?",
            default=True,
        )
        if use_extracted:
            prompted = _prompt_missing_onboarding_values(
                values,
                answered_sections=answered_sections,
                notes_answered=notes is not None,
            )
        else:
            prompted = main._prompt_onboarding_values()
            for key in (
                "diets",
                "allergies",
                "conditions",
                "avoid_ingredients",
                "activity_level",
                "cuisines",
                "severity_level",
                "notes",
            ):
                values[key] = None
        for key, value in prompted.items():
            if value is not None:
                values[key] = value

    if interactive and not provided:
        main.console.print(
            Panel(
                "[bold]Let's build your dietary graph.[/bold]\n"
                "Pick by number, range, name, or custom text. Press Enter to skip. "
                "Use commas for multiple answers; type 0 or 'none' to clear.",
                border_style="green",
            )
        )
        values.update(main._prompt_onboarding_values())

    existing_profile: dict[str, Any] | None = None
    expected_version: int | None = None
    client: main.HelloFoodClient | None = None

    if not dry_run:
        client = main.HelloFoodClient()
        if is_child_profile:
            if not replace:
                existing_profile = client.local_household_profiles().get(member_id)
        else:
            _ensure_profile_sync_consent(client, auto_yes=yes, json_mode=json_mode)
            if not replace:
                try:
                    existing = client.download_profile(member_id=member_id)
                    existing_profile = existing.get("profile_data") if isinstance(existing, dict) else None
                    version = existing.get("version") if isinstance(existing, dict) else None
                    expected_version = int(version) if isinstance(version, int) else None
                except HelloFoodError as exc:
                    if not str(exc).startswith("404:"):
                        _raise_command_error(exc, json_mode=json_mode)

    try:
        profile_data = onboarding.build_profile_data(
            existing=existing_profile,
            replace=replace or existing_profile is None,
            diets=values["diets"] if values["diets"] else ([] if provided and diet is not None else None),
            allergies=values["allergies"] if values["allergies"] else ([] if provided and allergy is not None else None),
            conditions=values["conditions"] if values["conditions"] else ([] if provided and condition is not None else None),
            avoid_ingredients=values["avoid_ingredients"] if values["avoid_ingredients"] else ([] if provided and avoid is not None else None),
            activity_level=values["activity_level"],
            cuisines=values["cuisines"] if values["cuisines"] else ([] if provided and cuisine is not None else None),
            notes=values["notes"],
            severity_level=values["severity_level"],
        )
    except ValueError as exc:
        _fail(
            str(exc),
            kind="invalid_input",
            json_mode=json_mode,
            exit_code=2,
        )

    if dry_run:
        output.write_json({"member_id": member_id, "profile_data": profile_data})
        return

    if client is None:
        client = main.HelloFoodClient()

    if not onboarding.profile_has_content(profile_data) and not yes:
        if not Confirm.ask("This dietary graph is empty. Save it anyway?", default=False):
            raise typer.Exit()

    if not yes:
        render.profile_summary(main.console, profile_data, member_id=member_id)
        quip = personality.onboarding_quip(profile_data)
        if quip:
            main.console.print(Panel(quip, title="hello.food", border_style="green"))
        if not Confirm.ask("Save this dietary graph?", default=True):
            raise typer.Exit()

    if is_child_profile:
        try:
            uploaded = client.save_local_child_profile(member_id, profile_data)
        except household.HouseholdError as exc:
            _fail(str(exc), kind="invalid_household_member", json_mode=json_mode, exit_code=2)
    else:
        try:
            uploaded = client.upload_profile(
                profile_data,
                member_id=member_id,
                expected_version=expected_version,
            )
        except (LoginRequired, HelloFoodError) as exc:
            _raise_command_error(exc, json_mode=json_mode)

        if member_id != household.OWNER_ID:
            client.mark_household_profile_synced(member_id)

    if _write_result(uploaded, json_mode=json_mode):
        return
    if is_child_profile:
        main.console.print(
            "[green]Child dietary graph saved locally. It was not sent to profile sync.[/green]"
        )
    else:
        main.console.print("[green]Dietary graph saved. Your CLI agent is now profile-aware.[/green]")
    if yes:
        quip = personality.onboarding_quip(profile_data)
        if quip:
            main.console.print(Panel(quip, title="hello.food", border_style="green"))
    render.profile_summary(
        main.console,
        profile_data,
        member_id=str(uploaded.get("member_id") or member_id),
        version=uploaded.get("version"),
        updated_at=uploaded.get("updated_at"),
    )


def _provided_onboarding_fields(**values: Any) -> bool:
    return any(value is not None and value != [] for value in values.values())


def _resolve_first_name(
    store: ConfigStore,
    *,
    text_profile: str,
    should_prompt: bool,
    persist: bool,
) -> str | None:
    from_text = personality.first_name_from_text(text_profile)
    if from_text:
        if persist:
            personality.save_cli_first_name(store, from_text)
        return from_text

    existing = personality.load_cli_first_name(store)
    if existing:
        return existing

    if not should_prompt:
        return None

    value = Prompt.ask(
        "First name [dim](for your CLI greeting; Enter to skip)[/dim]",
        default="",
    ).strip()
    if not value:
        return None
    cleaned = personality.first_name_from_text(f"my name is {value}") or value
    if persist:
        personality.save_cli_first_name(store, cleaned)
        return personality.load_cli_first_name(store)
    return personality.first_name_from_text(f"my name is {cleaned}") or None


def _has_extracted_onboarding_values(values: dict[str, Any]) -> bool:
    return any(
        bool(values.get(key))
        for key in (
            "diets",
            "allergies",
            "conditions",
            "avoid_ingredients",
            "cuisines",
            "activity_level",
        )
    )


def _prompt_onboarding_values() -> dict[str, Any]:
    diets = main._prompt_multi_options(
        "Diet style",
        onboarding.DIET_STYLES[:8],
    )
    allergies = main._prompt_multi_options(
        "Allergies or restrictions",
        onboarding.ALLERGIES[:10],
    )
    conditions = main._prompt_multi_options(
        "Health conditions",
        onboarding.HEALTH_CONDITIONS[:9],
    )
    avoid = main._prompt_free_text_list("Specific ingredients to avoid", ["raw onion", "garlic", "cilantro"])
    activity = main._prompt_one_option("Activity level", onboarding.ACTIVITY_LEVELS)
    cuisines = main._prompt_multi_options("Cuisines you love", onboarding.CUISINES[:8])
    severity_raw = ""
    if _has_selected_conditions(conditions):
        severity_raw = Prompt.ask("Severity 1-5 [dim](Enter for 3)[/dim]", default="").strip()
    notes = Prompt.ask("Notes [dim](Enter to skip, '-' to clear)[/dim]", default="").strip()
    return {
        "diets": diets,
        "allergies": allergies,
        "conditions": conditions,
        "avoid_ingredients": avoid,
        "activity_level": activity,
        "cuisines": cuisines,
        "severity_level": int(severity_raw) if severity_raw.isdigit() else None,
        "notes": "" if notes == "-" else (notes or None),
    }


def _print_extracted_onboarding_review(
    values: dict[str, Any],
    answered_sections: set[str],
) -> None:
    labels = {
        "diets": "Diet styles",
        "allergies": "Allergies",
        "conditions": "Conditions",
        "avoid_ingredients": "Avoid",
        "activity_level": "Activity",
        "cuisines": "Cuisines",
    }
    table = Table(title="Extracted dietary profile")
    table.add_column("Section", style="bold")
    table.add_column("Extracted value")
    for key, label in labels.items():
        if key not in answered_sections:
            continue
        raw = values.get(key)
        if isinstance(raw, list):
            rendered = ", ".join(str(value) for value in raw) or "None"
        else:
            rendered = str(raw or "None")
        table.add_row(label, rendered)
    if not answered_sections:
        table.add_row("Recognized sections", "None")
    main.console.print(table)


def _prompt_missing_onboarding_values(
    values: dict[str, Any],
    *,
    answered_sections: set[str],
    notes_answered: bool,
) -> dict[str, Any]:
    prompted: dict[str, Any] = {}
    if "diets" not in answered_sections:
        prompted["diets"] = main._prompt_multi_options(
            "Diet style",
            onboarding.DIET_STYLES[:8],
        )
    if "allergies" not in answered_sections:
        prompted["allergies"] = main._prompt_multi_options(
            "Allergies or restrictions",
            onboarding.ALLERGIES[:10],
        )
    if "conditions" not in answered_sections:
        prompted["conditions"] = main._prompt_multi_options(
            "Health conditions",
            onboarding.HEALTH_CONDITIONS[:9],
        )
    if "avoid_ingredients" not in answered_sections:
        prompted["avoid_ingredients"] = main._prompt_free_text_list(
            "Specific ingredients to avoid",
            ["raw onion", "garlic", "cilantro"],
        )
    if "activity_level" not in answered_sections:
        prompted["activity_level"] = main._prompt_one_option(
            "Activity level",
            onboarding.ACTIVITY_LEVELS,
        )
    if "cuisines" not in answered_sections:
        prompted["cuisines"] = main._prompt_multi_options(
            "Cuisines you love",
            onboarding.CUISINES[:8],
        )

    conditions = prompted.get("conditions", values.get("conditions"))
    if values.get("severity_level") is None and _has_selected_conditions(conditions):
        severity_raw = Prompt.ask(
            "Severity 1-5 [dim](Enter for 3)[/dim]",
            default="",
        ).strip()
        prompted["severity_level"] = (
            int(severity_raw) if severity_raw.isdigit() else None
        )
    if not notes_answered:
        notes = Prompt.ask(
            "Notes [dim](Enter to skip, '-' to clear)[/dim]",
            default="",
        ).strip()
        prompted["notes"] = "" if notes == "-" else (notes or None)
    return prompted


def _has_selected_conditions(values: Any) -> bool:
    if not isinstance(values, list):
        return False
    return any(
        str(value).strip().lower() not in {"", "none", "none of these", "no", "skip"}
        for value in values
    )


def _prompt_multi_options(
    label: str,
    options: tuple[onboarding.DietaryOption, ...],
) -> list[str] | None:
    _print_option_table(label, options, none_label="None of these")
    value = Prompt.ask(
        f"{label} [dim](numbers, ranges, names, or custom; Enter to skip)[/dim]",
        default="",
    ).strip()
    if not value:
        return None
    return _parse_option_selection(value, options, multi=True)


def _prompt_one_option(
    label: str,
    options: tuple[onboarding.DietaryOption, ...],
) -> str | None:
    _print_option_table(label, options, none_label="Skip")
    value = Prompt.ask(
        f"{label} [dim](number or name; Enter to skip)[/dim]",
        default="",
    ).strip()
    if not value:
        return None
    selected = _parse_option_selection(value, options, multi=False)
    return selected[0] if selected else None


def _prompt_free_text_list(label: str, examples: list[str]) -> list[str] | None:
    hint = ", ".join(examples)
    value = Prompt.ask(
        f"{label} [dim]({hint}; commas ok; 'none' clears)[/dim]",
        default="",
    ).strip()
    if not value:
        return None
    return onboarding.split_values([value])


def _print_option_table(
    label: str,
    options: tuple[onboarding.DietaryOption, ...],
    *,
    none_label: str,
) -> None:
    table = Table(title=label)
    table.add_column("#", justify="right", style="dim", no_wrap=True)
    table.add_column("Choice")
    for index, option in enumerate(options, start=1):
        table.add_row(str(index), option.label)
    table.add_row("0", none_label)
    main.console.print(table)


def _parse_option_selection(
    value: str,
    options: tuple[onboarding.DietaryOption, ...],
    *,
    multi: bool,
) -> list[str]:
    selected: list[str] = []
    for token in onboarding.split_values([value]):
        expanded = _expand_selection_token(token, options)
        if expanded:
            selected.extend(expanded)
        else:
            selected.append(token)
        if not multi and selected:
            break
    deduped = _dedupe(selected)
    if "none" in {item.strip().lower() for item in deduped}:
        return ["none"]
    return deduped


def _expand_selection_token(
    token: str,
    options: tuple[onboarding.DietaryOption, ...],
) -> list[str]:
    text = token.strip()
    if not text:
        return []
    if text == "0" or text.lower() in {"none", "skip", "no"}:
        return ["none"]
    if text.isdigit():
        index = int(text)
        if 1 <= index <= len(options):
            return [options[index - 1].label]
        return []
    if "-" in text:
        start, _, end = text.partition("-")
        if start.strip().isdigit() and end.strip().isdigit():
            lo = int(start.strip())
            hi = int(end.strip())
            if lo > hi:
                lo, hi = hi, lo
            return [
                options[index - 1].label
                for index in range(lo, hi + 1)
                if 1 <= index <= len(options)
            ]
    return []


def _dedupe(values: list[str]) -> list[str]:
    result: list[str] = []
    for value in values:
        cleaned = value.strip()
        if cleaned and cleaned not in result:
            result.append(cleaned)
    return result


def _ensure_profile_sync_consent(
    client: main.HelloFoodClient,
    *,
    auto_yes: bool,
    json_mode: bool = False,
) -> None:
    try:
        status = client.profile_consent_status()
    except (LoginRequired, HelloFoodError) as exc:
        _raise_command_error(exc, json_mode=json_mode)
    if status.get("has_consent"):
        return

    if not auto_yes:
        main.console.print(
            Panel(
                "hello.food stores your synced dietary graph on the server so the CLI, "
                "mobile apps, recipes, restaurants, and chat all work from the same source of truth.",
                title="Profile Sync",
                border_style="yellow",
            )
        )
        if not Confirm.ask("Allow profile sync for this account?", default=True):
            raise typer.Exit(1)

    try:
        client.grant_profile_consent(consent_version=1)
    except (LoginRequired, HelloFoodError) as exc:
        _raise_command_error(exc, json_mode=json_mode)
