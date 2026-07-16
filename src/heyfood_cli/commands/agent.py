"""Conversational agent commands (ask, reply, chat, log, item, conversation)."""
from __future__ import annotations

from .. import main
from ..main import (
    Any,
    Confirm,
    HelloFoodError,
    LoginRequired,
    Optional,
    Prompt,
    Text,
    _fail,
    _geocode_place,
    _json_mode,
    _raise_command_error,
    _validated,
    _write_result,
    app,
    banner,
    conversation_app,
    household,
    output,
    render,
    typer,
    validation,
)

@app.command()
def ask(
    query: Optional[list[str]] = typer.Argument(None, help="Natural-language request."),
    lat: Optional[float] = typer.Option(None, "--lat", help="Latitude for restaurant/location-aware requests."),
    lng: Optional[float] = typer.Option(None, "--lng", help="Longitude for restaurant/location-aware requests."),
    near: Optional[str] = typer.Option(None, "--near", help="Place name for this request."),
    no_location: bool = typer.Option(False, "--no-location", help="Do not send saved location context."),
    conversation_id: Optional[str] = typer.Option(None, "--conversation-id", help="Continue an existing conversation."),
    continue_last: bool = typer.Option(False, "--continue", "-c", help="Continue the last CLI conversation."),
    checking_for: Optional[str] = typer.Option(
        None,
        "--for",
        help="One-turn household scope: member name/id, me, or everyone.",
    ),
    voice: bool = typer.Option(False, "--voice", help="Speak your request instead of typing it."),
    voice_capture_mode: str = typer.Option("auto", "--voice-capture", help="Voice capture mode: auto, native, browser, or typed."),
    audio_device: Optional[str] = typer.Option(None, "--audio-device", help="Input device id or name for native voice capture."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Ask the HelloFood conversational agent."""
    text = " ".join(query or []).strip()
    main._validate_voice_options(
        voice=voice,
        positional_text=text,
        capture_mode=voice_capture_mode,
        audio_device=audio_device,
    )
    if voice:
        text = main._voice_transcript(
            purpose="ask",
            capture_mode=voice_capture_mode,
            audio_device=audio_device,
            json_mode=_json_mode(json_output, raw),
        )
    if not text:
        raise typer.BadParameter("Provide a request, or use --voice to speak one.")
    main._ask_agent(
        text,
        lat=lat,
        lng=lng,
        near=near,
        no_location=no_location,
        conversation_id=conversation_id,
        continue_last=continue_last,
        checking_for=checking_for,
        json_output=json_output,
        raw=raw,
    )


def _ask_agent(
    text: str,
    *,
    lat: Optional[float] = None,
    lng: Optional[float] = None,
    near: Optional[str] = None,
    no_location: bool = False,
    use_saved_location: bool = True,
    conversation_id: Optional[str] = None,
    continue_last: bool = False,
    checking_for: Optional[str] = None,
    json_output: bool = False,
    raw: bool = False,
    show_continue_hint: bool = True,
    client: Optional[main.HelloFoodClient] = None,
) -> dict[str, Any]:
    json_mode = _json_mode(json_output, raw)
    text = _validated(
        lambda: validation.required_text(text, label="Query", max_length=500)
    )
    client = client or main.HelloFoodClient()
    lat, lng = _resolve_agent_location(
        client,
        lat=lat,
        lng=lng,
        near=near,
        no_location=no_location,
        use_saved=use_saved_location,
        json_mode=json_mode,
    )
    banner.controller.loading(main.stderr_console, json_mode=json_mode)
    if continue_last and conversation_id is None:
        conversation_id = client.last_conversation_id()
        if conversation_id is None:
            raise typer.BadParameter("No previous conversation found. Start with `heyfood ask ...` or `heyfood chat`.")

    payload: dict = {"input_mode": "text"}
    if conversation_id:
        payload["conversation_id"] = conversation_id

    pending_confirmation = client.pending_confirmation() if conversation_id else None
    if pending_confirmation and _is_confirmation_reply(text):
        payload["confirm"] = pending_confirmation
    else:
        payload["query"] = text

    scope_context: dict[str, Any] = {}
    context_builder = getattr(client, "agent_household_context", None)
    if callable(context_builder):
        try:
            scope_context = context_builder(checking_for)
        except household.HouseholdError as exc:
            raise typer.BadParameter(str(exc)) from exc
        except (LoginRequired, HelloFoodError) as exc:
            _raise_command_error(exc, json_mode=json_mode)
        for key in ("dietary_context", "device_context", "meal_context"):
            value = scope_context.get(key)
            if isinstance(value, dict):
                payload[key] = value

    request_scope = scope_context.get("scope")
    request_scope_id = (
        request_scope.get("id") if isinstance(request_scope, dict) else None
    )
    if "confirm" in payload:
        remembered_scope_reader = getattr(
            client,
            "last_conversation_household_scope",
            None,
        )
        remembered_scope_id = (
            remembered_scope_reader()
            if callable(remembered_scope_reader)
            else None
        )
        if (
            remembered_scope_id
            and request_scope_id
            and remembered_scope_id != request_scope_id
        ):
            raise typer.BadParameter(
                "The household scope changed after this action was proposed. "
                "Restate the request for the current scope instead of confirming it."
            )

    local_household_effect = None
    if "confirm" in payload:
        apply_pending = getattr(client, "apply_pending_household_confirmation", None)
        if callable(apply_pending):
            local_household_effect = apply_pending()

    if lat is not None:
        payload["lat"] = lat
    if lng is not None:
        payload["lng"] = lng

    final_result = None
    choices_result = None
    partial_chunks: list[str] = []
    status = None if json_mode else main.stderr_console.status("[dim]thinking…[/dim]")
    if status is not None:
        status.start()
    try:
        for event, data in client.stream_agent(payload):
            if event == "thinking":
                message = data.get("message") or _thinking_message(data.get("stage"))
                if status is not None:
                    status.update(f"[dim]{message}[/dim]")
            elif event == "progress":
                if not json_mode:
                    render.progress(main.stderr_console, data)
            elif event == "partial":
                chunk = data.get("text") or data.get("delta") or ""
                if chunk:
                    partial_chunks.append(str(chunk))
            elif event == "choices":
                choices = data.get("choices")
                if isinstance(choices, list) and choices:
                    choices_result = {
                        "choices": [str(choice) for choice in choices],
                        "allow_multiple": bool(data.get("allow_multiple")),
                    }
            elif event == "result":
                final_result = data
            elif event == "error":
                raise HelloFoodError(str(data.get("message") or data))
    except (LoginRequired, HelloFoodError) as exc:
        _raise_command_error(exc, json_mode=json_mode)
    finally:
        if status is not None:
            status.stop()

    if final_result:
        if partial_chunks and not (
            final_result.get("message") or final_result.get("text") or final_result.get("response")
        ):
            final_result["text"] = "".join(partial_chunks)
        if choices_result:
            final_result["choices"] = choices_result
        effects: dict[str, Any] = {}
        if isinstance(local_household_effect, dict):
            effects["household_local_first"] = local_household_effect
        apply_result = getattr(client, "apply_household_result", None)
        if callable(apply_result):
            result_effect = apply_result(final_result)
            if isinstance(result_effect, dict):
                effects["household_result"] = result_effect
        if effects:
            final_result["client_effects"] = effects
        client.remember_conversation(final_result)
        remember_scope = getattr(client, "remember_conversation_household_scope", None)
        if callable(remember_scope):
            remember_scope(str(request_scope_id) if request_scope_id else None)
        if json_mode:
            output.write_json(final_result)
        else:
            render.agent_result(main.console, final_result)
            if choices_result:
                render.agent_choices(main.console, choices_result)
            visible_effect = effects.get("household_result") or effects.get("household_local_first")
            if isinstance(visible_effect, dict):
                render.household_mutation_effect(main.console, visible_effect)
        if show_continue_hint and not json_mode:
            conversation_id = final_result.get("conversation_id")
            if conversation_id:
                main.stderr_console.print(
                    "[dim]Continue with: heyfood reply \"...\" or heyfood chat[/dim]"
                )
        return final_result
    _fail(
        "The hello.food agent returned no final result.",
        kind="empty_agent_result",
        json_mode=json_mode,
    )
    return {}  # pragma: no cover - _fail always raises


def _local_conversation_document(client: main.HelloFoodClient) -> dict[str, Any]:
    remembered = client.last_conversation()
    conversation_id = remembered.get("conversation_id")
    summary = {
        "conversation_id": conversation_id,
        "updated_at": remembered.get("updated_at"),
        "has_pending_confirmation": isinstance(
            remembered.get("pending_confirmation"), dict
        ),
    }
    conversations = (
        [summary]
        if isinstance(conversation_id, str) and conversation_id
        else []
    )
    return {
        "conversations": conversations,
        "total_count": len(conversations),
        "source": "local_last_conversation",
        "history_available": False,
    }


@conversation_app.command("list")
def conversation_list(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Show the last conversation id remembered on this machine."""
    json_mode = _json_mode(json_output, raw)
    data = _local_conversation_document(main.HelloFoodClient(create_device=False))
    if _write_result(data, json_mode=json_mode):
        return
    render.conversations(main.console, data)


@conversation_app.command("resume")
def conversation_resume(
    query: list[str] = typer.Argument(..., help="Follow-up text for the last conversation."),
    lat: Optional[float] = typer.Option(None, "--lat", help="Latitude for location-aware requests."),
    lng: Optional[float] = typer.Option(None, "--lng", help="Longitude for location-aware requests."),
    near: Optional[str] = typer.Option(None, "--near", help="Place name for this request."),
    no_location: bool = typer.Option(False, "--no-location", help="Do not send saved location context."),
    checking_for: Optional[str] = typer.Option(
        None,
        "--for",
        help="Household scope: member name/id, me, or everyone.",
    ),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Continue the last conversation remembered on this machine."""
    main._ask_agent(
        " ".join(query).strip(),
        lat=lat,
        lng=lng,
        near=near,
        no_location=no_location,
        continue_last=True,
        checking_for=checking_for,
        json_output=json_output,
        raw=raw,
    )


@conversation_app.command("clear")
def conversation_clear(
    yes: bool = typer.Option(False, "--yes", "-y", help="Confirm clearing the local pointer."),
    no_input: bool = typer.Option(False, "--no-input", help="Never prompt; requires --yes."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Forget the local last-conversation pointer; server history is unaffected."""
    json_mode = _json_mode(json_output, raw)
    client = main.HelloFoodClient(create_device=False)
    existing = client.last_conversation_id()
    if existing is not None and not yes:
        if json_mode or no_input or not main._stdin_is_tty():
            raise typer.BadParameter("Pass --yes to clear the local conversation pointer.")
        if not Confirm.ask("Forget the locally remembered conversation?", console=main.console):
            data = {
                "ok": True,
                "cleared": False,
                "conversation_id": existing,
                "scope": "local_pointer_only",
            }
            if _write_result(data, json_mode=json_mode):
                return
            main.console.print("[dim]Conversation pointer kept.[/dim]")
            return
    cleared = client.clear_last_conversation()
    data = {
        "ok": True,
        "cleared": cleared,
        "conversation_id": existing,
        "scope": "local_pointer_only",
    }
    if _write_result(data, json_mode=json_mode):
        return
    if cleared:
        main.console.print("[green]Local conversation pointer cleared.[/green]")
    else:
        main.console.print("[dim]No local conversation pointer was stored.[/dim]")


def _thinking_message(stage: Any) -> str:
    return {
        "resolving_restaurant": "resolving restaurant...",
        "loading_menu": "loading menu...",
        "evaluating_menu": "evaluating menu...",
        "applying_dietary_graph": "applying dietary graph...",
        "searching_recipes": "searching recipes...",
        "checking_food": "checking food...",
    }.get(str(stage or ""), "thinking...")


def _is_confirmation_reply(text: str) -> bool:
    normalized = text.strip().lower()
    return normalized in {
        "y",
        "yes",
        "confirm",
        "confirmed",
        "approve",
        "approved",
        "ok",
        "okay",
        "looks good",
        "log it",
        "save it",
        "do it",
    }


def _resolve_chat_choice(text: str, choice_set: dict[str, Any]) -> str:
    choices = choice_set.get("choices")
    if not isinstance(choices, list) or not choices:
        return text
    raw = text.strip()
    tokens = [part.strip() for part in raw.split(",")]
    if not tokens or not all(token.isdigit() for token in tokens):
        return text
    indexes = [int(token) for token in tokens]
    if not choice_set.get("allow_multiple") and len(indexes) > 1:
        raise household.HouseholdError("Choose one number for this question.")
    if any(index < 1 or index > len(choices) for index in indexes):
        raise household.HouseholdError(
            f"Choose a number from 1 to {len(choices)}."
        )
    return ", ".join(str(choices[index - 1]) for index in indexes)


@app.command()
def reply(
    query: list[str] = typer.Argument(..., help="Follow-up text for the last conversation."),
    lat: Optional[float] = typer.Option(None, "--lat", help="Latitude for this follow-up."),
    lng: Optional[float] = typer.Option(None, "--lng", help="Longitude for this follow-up."),
    near: Optional[str] = typer.Option(None, "--near", help="Place name for this follow-up."),
    no_location: bool = typer.Option(False, "--no-location", help="Do not send saved location context."),
    checking_for: Optional[str] = typer.Option(
        None,
        "--for",
        help="Household scope: member name/id, me, or everyone.",
    ),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Reply to the last HelloFood agent conversation."""
    main._ask_agent(
        " ".join(query).strip(),
        lat=lat,
        lng=lng,
        near=near,
        no_location=no_location,
        continue_last=True,
        checking_for=checking_for,
        json_output=json_output,
        raw=raw,
    )


@app.command()
def chat(
    initial: Optional[list[str]] = typer.Argument(None, help="Optional first message."),
    new: bool = typer.Option(False, "--new", help="Start a fresh conversation instead of resuming the last one."),
    no_input: bool = typer.Option(False, "--no-input", help="Never prompt; interactive chat will fail."),
    lat: Optional[float] = typer.Option(None, "--lat", help="Latitude for this chat."),
    lng: Optional[float] = typer.Option(None, "--lng", help="Longitude for this chat."),
    near: Optional[str] = typer.Option(None, "--near", help="Place name for this chat."),
    no_location: bool = typer.Option(False, "--no-location", help="Do not send saved location context."),
    checking_for: Optional[str] = typer.Option(
        None,
        "--for",
        help="Initial household scope: member name/id, me, or everyone.",
    ),
    json_output: bool = typer.Option(False, "--json", help="Reserved; interactive chat has no JSON protocol."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Open an interactive HelloFood agent conversation."""
    if _json_mode(json_output, raw):
        raise typer.BadParameter(
            "Interactive chat does not support --json; use `heyfood ask --json` or `heyfood reply --json`."
        )
    if no_input is True or not main._stdin_is_tty():
        raise typer.BadParameter(
            "Interactive chat requires TTY stdin. Use `heyfood ask` or `heyfood reply` for automation."
        )
    client = main.HelloFoodClient()
    conversation_id = None if new else client.last_conversation_id()
    if new:
        client.clear_last_conversation()

    try:
        initial_scope = client.agent_household_context(checking_for).get("scope") or {}
    except household.HouseholdError as exc:
        raise typer.BadParameter(str(exc)) from exc
    except (LoginRequired, HelloFoodError) as exc:
        _raise_command_error(exc, json_mode=False)
    chat_scope = str(initial_scope.get("id") or checking_for or "") or None
    main.console.print(
        "[bold green]hello.food chat[/bold green] "
        "[dim](type /exit, /new, /household, or /for NAME)[/dim]"
    )
    if initial_scope:
        render.household_scope(main.console, initial_scope)

    first_message = " ".join(initial).strip() if initial else ""
    active_choices: Optional[dict[str, Any]] = None
    while True:
        if first_message:
            text = first_message
            first_message = ""
            line = Text("you", style="bold")
            line.append(f" {text}")
            main.console.print(line)
        else:
            text = Prompt.ask("[bold]you[/bold]").strip()

        if not text:
            continue
        if text in {"/exit", "/quit"}:
            break
        if text == "/new":
            client.clear_last_conversation()
            conversation_id = None
            main.console.print("[dim]Started a fresh conversation.[/dim]")
            continue
        if text == "/household":
            render.household(main.console, household.public_document(client.household_state()))
            continue
        if text == "/for":
            render.household_scope(
                main.console,
                household.public_document(client.household_state())["active_scope"],
            )
            continue
        if text.startswith("/for "):
            selector = text.removeprefix("/for ").strip()
            try:
                state = client.set_household_scope(selector)
            except household.HouseholdError as exc:
                main.stderr_console.print(Text(str(exc), style="red"))
                continue
            chat_scope = str(state["active_scope"])
            client.clear_last_conversation()
            conversation_id = None
            active_choices = None
            render.household_scope(
                main.console,
                household.public_document(state)["active_scope"],
                changed=True,
            )
            main.console.print("[dim]Started a fresh conversation for the new scope.[/dim]")
            continue

        if active_choices:
            try:
                text = _resolve_chat_choice(text, active_choices)
            except household.HouseholdError as exc:
                main.stderr_console.print(Text(str(exc), style="red"))
                continue
            active_choices = None

        result = main._ask_agent(
            text,
            lat=lat,
            lng=lng,
            near=near,
            no_location=no_location,
            conversation_id=conversation_id,
            continue_last=False,
            checking_for=chat_scope,
            show_continue_hint=False,
            client=client,
        )
        conversation_id = client.last_conversation_id()
        choices = result.get("choices")
        active_choices = choices if isinstance(choices, dict) else None


@app.command()
def log(
    meal: Optional[list[str]] = typer.Argument(None, help="Meal text to log."),
    meal_type: Optional[str] = typer.Option(None, "--type", help="breakfast, lunch, dinner, or snack."),
    checking_for: Optional[str] = typer.Option(
        None,
        "--for",
        help="Household scope: member name/id, me, or everyone.",
    ),
    voice: bool = typer.Option(False, "--voice", help="Speak the meal instead of typing it."),
    voice_capture_mode: str = typer.Option("auto", "--voice-capture", help="Voice capture mode: auto, native, browser, or typed."),
    audio_device: Optional[str] = typer.Option(None, "--audio-device", help="Input device id or name for native voice capture."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Log a meal through the conversational agent."""
    text = " ".join(meal or []).strip()
    main._validate_voice_options(
        voice=voice,
        positional_text=text,
        capture_mode=voice_capture_mode,
        audio_device=audio_device,
    )
    if voice:
        text = main._voice_transcript(
            purpose="log",
            capture_mode=voice_capture_mode,
            audio_device=audio_device,
            json_mode=_json_mode(json_output, raw),
        )
    if not text:
        raise typer.BadParameter("Provide a meal to log, or use --voice to speak one.")
    meal_type = _validated(
        lambda: validation.choice(
            meal_type,
            label="Meal type",
            choices={"breakfast", "lunch", "dinner", "snack"},
        )
    )
    prompt = f"Log this meal: {text}"
    if meal_type:
        prompt = f"{prompt}. Meal type: {meal_type}."
    main._ask_agent(
        prompt,
        use_saved_location=False,
        checking_for=checking_for,
        json_output=json_output,
        raw=raw,
    )


@app.command()
def item(
    name: list[str] = typer.Argument(..., help="Food or menu item to evaluate."),
    restaurant: Optional[str] = typer.Option(None, "--restaurant", "-r", help="Restaurant context."),
    at: Optional[str] = typer.Option(None, "--at", help="Restaurant index from the last search."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Assess a food or menu item against your HelloFood profile."""
    json_mode = _json_mode(json_output, raw)
    item_name = _validated(
        lambda: validation.required_text(" ".join(name), label="Item name", max_length=200)
    )
    restaurant = _validated(
        lambda: validation.optional_text(
            restaurant,
            label="Restaurant name",
            max_length=200,
        )
    )
    client = main.HelloFoodClient()
    try:
        if at:
            selected = client.restaurant_from_selector(at)
            if selected and selected.get("name"):
                restaurant = str(selected["name"])
        restaurant = _validated(
            lambda: validation.optional_text(
                restaurant,
                label="Restaurant name",
                max_length=200,
            )
        )
        data = client.channel_tool(
            "explain_item",
            {"item_name": item_name, "restaurant_name": restaurant},
        )
    except (LoginRequired, HelloFoodError) as exc:
        _raise_command_error(exc, json_mode=json_mode)
    if _write_result(data, json_mode=json_mode):
        return
    render.explain_item(main.console, data)


def _resolve_agent_location(
    client: main.HelloFoodClient,
    *,
    lat: Optional[float],
    lng: Optional[float],
    near: Optional[str],
    no_location: bool,
    use_saved: bool,
    json_mode: bool,
) -> tuple[float | None, float | None]:
    lat, lng = _validated(lambda: validation.coordinates(lat, lng))
    near = _validated(
        lambda: validation.optional_text(near, label="Near", max_length=200)
    )
    if no_location:
        if lat is not None or near:
            raise typer.BadParameter(
                "Use either explicit location options or --no-location, not both."
            )
        return None, None
    if lat is not None and lng is not None:
        return lat, lng
    if near:
        resolved_lat, resolved_lng, _ = _geocode_place(
            client,
            near,
            json_mode=json_mode,
        )
        return resolved_lat, resolved_lng
    if use_saved:
        saved = client.saved_location()
        if saved:
            return float(saved["latitude"]), float(saved["longitude"])
    return None, None
