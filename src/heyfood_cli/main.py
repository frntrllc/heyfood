from __future__ import annotations

import time
import sys
from contextlib import nullcontext
from datetime import date
from typing import Any, Callable, Optional, TypeVar

import typer
from rich.console import Console
from rich.panel import Panel
from rich.prompt import Confirm, Prompt
from rich.table import Table
from rich.text import Text

from . import __version__
from .auth import local_urls, perform_device_login, perform_login
from .client import ChannelToolUnavailable, HelloFoodClient, HelloFoodError, LoginRequired
from .config import (
    BUILTIN_CONTEXTS,
    ConfigError,
    ConfigStore,
    DEFAULT_API_KEY,
    configured_config_path,
    configured_contexts,
    is_local_api_url,
    redacted_config,
    resolve_service_urls,
)
from . import diagnostics
from . import onboarding
from . import banner
from . import household
from . import output
from . import personality
from . import render
from .theme import HEYFOOD_THEME
from . import validation
from .voice import VoiceCaptureError, capture_voice_transcript


app = typer.Typer(
    add_completion=True,
    help="heyfood: hello.food for your terminal.",
    invoke_without_command=True,
)
recipes_app = typer.Typer(
    add_completion=False,
    help="Recipe discovery from your terminal.",
)
app.add_typer(recipes_app, name="recipes")
location_app = typer.Typer(
    add_completion=False,
    help="Save a default location for restaurant searches.",
    invoke_without_command=True,
)
app.add_typer(location_app, name="location")
context_app = typer.Typer(add_completion=False, help="Manage named API/auth environments.")
app.add_typer(context_app, name="context")
config_app = typer.Typer(add_completion=False, help="Inspect local CLI configuration safely.")
app.add_typer(config_app, name="config")
members_app = typer.Typer(add_completion=False, help="Discover synced household member ids.")
app.add_typer(members_app, name="members")
household_app = typer.Typer(
    add_completion=False,
    help="Manage the local household roster and active agent scope.",
)
app.add_typer(household_app, name="household")
conversation_app = typer.Typer(
    add_completion=False,
    help="Inspect or manage the locally remembered agent conversation.",
)
app.add_typer(conversation_app, name="conversation")
console = Console(theme=HEYFOOD_THEME)
stderr_console = Console(stderr=True, theme=HEYFOOD_THEME, highlight=False)
MENU_POLL_INTERVAL_SECONDS = 3.0
MENU_POLL_WARNING_SECONDS = 12.0
MENU_POLL_TIMEOUT_SECONDS = 30.0
T = TypeVar("T")


def _json_mode(json_output: object = False, raw: object = False) -> bool:
    raw_enabled = raw is True
    if raw_enabled:
        stderr_console.print(
            "[yellow]--raw is deprecated; use --json.[/yellow]",
        )
    return json_output is True or raw_enabled


def _write_result(data: Any, *, json_mode: bool) -> bool:
    if not json_mode:
        return False
    output.write_json(data)
    return True


def _fail(
    message: str,
    *,
    kind: str,
    json_mode: bool,
    hint: str | None = None,
    exit_code: int = 1,
) -> None:
    if json_mode:
        output.write_json(output.error_document(kind, message, hint=hint))
    else:
        stderr_console.print(f"[red]heyfood error:[/red] {message}")
        if hint:
            stderr_console.print(hint)
    raise typer.Exit(exit_code)


def _validated(callback: Callable[[], T]) -> T:
    try:
        return callback()
    except validation.ValidationError as exc:
        raise typer.BadParameter(str(exc)) from exc


def _stdin_is_tty() -> bool:
    try:
        return bool(sys.stdin.isatty())
    except (AttributeError, OSError):
        return False


@app.callback()
def callback(
    ctx: typer.Context,
    version: bool = typer.Option(False, "--version", help="Show version and exit."),
    no_banner: bool = typer.Option(False, "--no-banner", help="Disable decorative ASCII branding."),
    verbose: bool = typer.Option(False, "--verbose", help="Print safe request diagnostics to stderr."),
) -> None:
    banner.controller.configure(disabled=no_banner)
    diagnostics.reporter.configure(enabled=verbose, console=stderr_console)
    if version:
        console.print(f"heyfood {__version__}")
        raise typer.Exit()
    if ctx.invoked_subcommand is None:
        render.intro(console)


def _validated_service_url(value: str, *, label: str) -> str:
    from urllib.parse import urlparse

    candidate = value.strip().rstrip("/")
    parsed = urlparse(candidate)
    if parsed.scheme not in {"http", "https"} or not parsed.hostname:
        raise typer.BadParameter(f"{label} must be an http:// or https:// URL.")
    return candidate


@context_app.command("list")
def context_list(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
) -> None:
    store = ConfigStore(configured_config_path())
    data = store.load()
    contexts = configured_contexts(data)
    _, _, active = resolve_service_urls(data)
    document = {
        "ok": True,
        "active": active,
        "contexts": [
            {"name": name, "active": name == active, **values}
            for name, values in sorted(contexts.items())
        ],
    }
    if _write_result(document, json_mode=json_output):
        return
    table = Table(title="heyfood contexts")
    table.add_column("Active")
    table.add_column("Name")
    table.add_column("API URL")
    table.add_column("Auth URL")
    for item in document["contexts"]:
        table.add_row("*" if item["active"] else "", item["name"], item["api_url"], item["auth_url"])
    console.print(table)


@context_app.command("show")
def context_show(
    name: Optional[str] = typer.Argument(None, help="Context name; defaults to active."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
) -> None:
    data = ConfigStore(configured_config_path()).load()
    contexts = configured_contexts(data)
    _, _, active = resolve_service_urls(data)
    selected = name or active
    if selected not in contexts:
        raise typer.BadParameter(f"Unknown context '{selected}'. Run `heyfood context list`.")
    document = {"ok": True, "name": selected, "active": selected == active, **contexts[selected]}
    if _write_result(document, json_mode=json_output):
        return
    console.print(f"[bold]{selected}[/bold]{' [green](active)[/green]' if selected == active else ''}")
    console.print(f"API:  {document['api_url']}")
    console.print(f"Auth: {document['auth_url']}")


@context_app.command("use")
def context_use(name: str = typer.Argument(..., help="Context name.")) -> None:
    store = ConfigStore()
    data = store.load()
    contexts = configured_contexts(data)
    if name not in contexts:
        raise typer.BadParameter(f"Unknown context '{name}'. Run `heyfood context list`.")
    data["active_context"] = name
    # Remove the legacy single-environment override so the selected context
    # becomes authoritative. Environment variables still take precedence.
    data.pop("api_url", None)
    data.pop("auth_url", None)
    store.save(data)
    console.print(f"[green]Using context '{name}'.[/green]")


@context_app.command("set")
def context_set(
    name: str = typer.Argument(..., help="New custom context name."),
    api_url: str = typer.Option(..., "--api-url", help="API base URL."),
    auth_url: str = typer.Option(..., "--auth-url", help="Auth authorize URL."),
    use: bool = typer.Option(False, "--use", help="Make this context active."),
) -> None:
    normalized_name = name.strip().lower()
    if not normalized_name or normalized_name in BUILTIN_CONTEXTS:
        raise typer.BadParameter("Choose a non-empty custom name other than production or local.")
    store = ConfigStore()
    data = store.load()
    contexts = data.setdefault("contexts", {})
    if not isinstance(contexts, dict):
        contexts = {}
        data["contexts"] = contexts
    contexts[normalized_name] = {
        "api_url": _validated_service_url(api_url, label="API URL"),
        "auth_url": _validated_service_url(auth_url, label="Auth URL"),
    }
    if use:
        data["active_context"] = normalized_name
        data.pop("api_url", None)
        data.pop("auth_url", None)
    store.save(data)
    console.print(f"[green]Saved context '{normalized_name}'.[/green]")


@config_app.command("path")
def config_path(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
) -> None:
    path = configured_config_path()
    if _write_result({"ok": True, "path": str(path)}, json_mode=json_output):
        return
    console.print(str(path))


@config_app.command("show")
def config_show(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
) -> None:
    store = ConfigStore(configured_config_path())
    try:
        data = store.load()
    except ConfigError as exc:
        _fail(str(exc), kind="invalid_config", json_mode=json_output, exit_code=2)
    effective_api, effective_auth, active = resolve_service_urls(data)
    document = {
        "ok": True,
        "path": str(store.path),
        "active_context": active,
        "effective": {"api_url": effective_api, "auth_url": effective_auth},
        "config": redacted_config(data),
    }
    if _write_result(document, json_mode=json_output):
        return
    output.write_json(document)


@config_app.command("validate")
def config_validate(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
) -> None:
    store = ConfigStore(configured_config_path())
    try:
        store.load()
    except ConfigError as exc:
        document = {
            "ok": False,
            "path": str(store.path),
            "error": {"type": "invalid_config", "message": str(exc)},
            "repair": "Move the invalid file aside, then rerun `heyfood login`.",
        }
        if json_output:
            output.write_json(document)
        else:
            stderr_console.print(f"[red]{exc}[/red]")
            stderr_console.print(
                f"Move it aside with: mv '{store.path}' '{store.path}.invalid'"
            )
        raise typer.Exit(2)
    document = {"ok": True, "path": str(store.path), "valid": True}
    if _write_result(document, json_mode=json_output):
        return
    console.print("[green]Configuration is valid.[/green]")


@app.command()
def login(
    api_url: Optional[str] = typer.Option(None, "--api-url", help="Override the active context API URL."),
    auth_url: Optional[str] = typer.Option(None, "--auth-url", help="Override the active context auth URL."),
    api_key: str = typer.Option(DEFAULT_API_KEY, "--api-key", help="API key for first-party app session refresh."),
    local: bool = typer.Option(False, "--local", help="Use the local Docker dev URLs."),
    device: bool = typer.Option(False, "--device", help="Use short-code login for SSH/headless systems."),
    no_browser: bool = typer.Option(False, "--no-browser", help="Print the login URL instead of opening a browser."),
    timeout: int = typer.Option(180, "--timeout", help="Seconds to wait for browser callback."),
) -> None:
    """Authenticate this machine with HelloFood."""
    store = ConfigStore()
    configured_api, configured_auth, _ = resolve_service_urls(store.load())
    if local:
        api_url, auth_url = local_urls()
    else:
        api_url = api_url or configured_api
        auth_url = auth_url or configured_auth
    banner.controller.loading(stderr_console)
    stderr_console.print("[bold]Opening hello.food login...[/bold]")
    try:
        if device:
            perform_device_login(
                store=store,
                api_url=api_url,
                auth_url=auth_url,
                api_key=api_key or None,
                open_browser=not no_browser,
                timeout_seconds=timeout,
                authorization_callback=lambda url, code: stderr_console.print(
                    f"Open this URL:\n{url}\n\nEnter code: [bold]{code}[/bold]"
                ),
            )
        else:
            perform_login(
                store=store,
                api_url=api_url,
                auth_url=auth_url,
                api_key=api_key or None,
                open_browser=not no_browser,
                timeout_seconds=timeout,
                authorize_url_callback=lambda url: stderr_console.print(f"Open this URL:\n{url}"),
            )
    except Exception as exc:
        message = str(exc)
        stderr_console.print(f"[red]Login failed:[/red] {message}")
        if "Channel OAuth is disabled" in message:
            stderr_console.print(
                "For local development, set [bold]CHANNEL_OAUTH_ENABLED=true[/bold] "
                "in [bold]backend/.env[/bold], restart the API, then run "
                "[bold]heyfood login --local[/bold]."
            )
        raise typer.Exit(1) from exc
    first_name = _first_name_from_account(store)
    if first_name:
        console.print(f"[green]Connected. Welcome, {first_name}. The CLI is ready.[/green]")
    else:
        console.print("[green]Connected. The CLI is ready.[/green]")


@app.command()
def logout(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON teardown results."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Forget local credentials and revoke the current app session."""
    client = HelloFoodClient(create_device=False)
    result = client.revoke_local_session()
    if _write_result(result, json_mode=_json_mode(json_output, raw)):
        return
    if result["remote_complete"]:
        console.print("[green]Logged out.[/green]")
    else:
        console.print(
            "[yellow]Logged out locally. Some server cleanup could not be confirmed; "
            "remaining sessions will expire automatically.[/yellow]"
        )


@app.command()
def status(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Show the active HelloFood account and channel scopes."""
    json_mode = _json_mode(json_output, raw)
    client = HelloFoodClient(create_device=False)
    try:
        me = client.me()
        whoami = client.channel_whoami()
    except (LoginRequired, HelloFoodError) as exc:
        _raise_command_error(exc, json_mode=json_mode)
    if _write_result({"ok": True, "account": me, "channel": whoami}, json_mode=json_mode):
        return
    render.status(console, me, whoami)


@app.command()
def doctor(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON diagnostics."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Check local config, auth refresh, and API reachability."""
    json_mode = _json_mode(json_output, raw)
    client = HelloFoodClient(
        store=ConfigStore(configured_config_path()),
        create_device=False,
    )
    config_check = {
        "ok": True,
        "path": str(client.store.path),
        "api_url": client.api_url,
        "auth_url": client.auth_url,
        "context": client.context_name,
        "has_api_key": bool(client.config.get("api_key")),
        "has_session": isinstance(client.config.get("session"), dict),
        "has_channel": isinstance(client.config.get("oauth"), dict),
    }
    checks: dict[str, Any] = {"config": config_check}

    try:
        me = client.me()
    except (LoginRequired, HelloFoodError) as exc:
        checks["session"] = {
            "ok": False,
            "error": str(exc),
            "type": _error_kind(exc),
        }
    else:
        checks["session"] = {"ok": True, "user_id": me.get("user_id")}

    try:
        whoami = client.channel_whoami()
    except (LoginRequired, HelloFoodError) as exc:
        checks["channel"] = {
            "ok": False,
            "error": str(exc),
            "type": _error_kind(exc),
        }
    else:
        checks["channel"] = {
            "ok": True,
            "channel": whoami.get("channel"),
            "scopes": whoami.get("scopes") or [],
        }

    ok = all(bool(value.get("ok")) for value in checks.values())
    document = {"ok": ok, "checks": checks}
    if json_mode:
        output.write_json(document)
    else:
        render.doctor_config(stderr_console, client.config, config_path=client.store.path)
        session = checks["session"]
        channel = checks["channel"]
        if session["ok"]:
            stderr_console.print(
                f"[green]Session OK[/green] user={session.get('user_id') or 'unknown'}"
            )
        else:
            stderr_console.print(f"[red]Session:[/red] {session['error']}")
        if channel["ok"]:
            scopes = ", ".join(channel.get("scopes") or [])
            stderr_console.print(
                f"[green]Channel OK[/green] {channel.get('channel') or 'unknown'} [{scopes}]"
            )
        else:
            stderr_console.print(f"[red]Channel:[/red] {channel['error']}")
    if not ok:
        raise typer.Exit(1)


@members_app.command("list")
def members_list(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """List synced dietary profile member ids."""
    json_mode = _json_mode(json_output, raw)
    client = HelloFoodClient()
    try:
        data = client.list_profile_members()
    except (LoginRequired, HelloFoodError) as exc:
        _raise_command_error(exc, json_mode=json_mode)
    if _write_result(data, json_mode=json_mode):
        return
    render.profile_members(console, data)


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
    client = HelloFoodClient(create_device=not local_only)
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
    render.household(console, document)
    if reconciliation is not None:
        console.print(
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
    state = HelloFoodClient(create_device=False).household_state()
    document = household.public_document(state)
    current = {"ok": True, "active_scope": document["active_scope"]}
    if _write_result(current, json_mode=json_mode):
        return
    render.household_scope(console, current["active_scope"])


@household_app.command("use")
def household_use(
    selector: str = typer.Argument(..., help="Member name/id, me, or everyone."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Persist the default household scope for ask, reply, chat, and log."""
    json_mode = _json_mode(json_output, raw)
    client = HelloFoodClient()
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
    render.household_scope(console, selected["active_scope"], changed=True)


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
    client = HelloFoodClient(create_device=False)
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
    render.household(console, document)


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
    client = HelloFoodClient()
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
        console,
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
    voice: bool = typer.Option(False, "--voice", help="Open a browser voice capture session before extracting the profile."),
    voice_timeout: int = typer.Option(300, "--voice-timeout", help="Seconds to wait for browser voice capture."),
    no_browser: bool = typer.Option(False, "--no-browser", help="With --voice, print the capture URL instead of opening it."),
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
        and _stdin_is_tty()
    )
    if list_options:
        if _write_result(onboarding.option_catalog(), json_mode=json_mode):
            return
        render.onboarding_options(console, onboarding.option_catalog())
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
        try:
            result = capture_voice_transcript(
                timeout_seconds=voice_timeout,
                open_browser=not no_browser,
                url_callback=lambda url: stderr_console.print(f"Voice capture URL:\n{url}"),
            )
        except VoiceCaptureError as exc:
            _fail(
                str(exc),
                kind="voice_capture_error",
                json_mode=json_mode,
            )
        text_profile = result.transcript
        if not json_mode:
            console.print(Panel(text_profile, title="Voice transcript", border_style="green"))

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
        console.print(f"[bold green]{message}[/bold green]")
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
        console.print(
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
            prompted = _prompt_onboarding_values()
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
        console.print(
            Panel(
                "[bold]Let's build your dietary graph.[/bold]\n"
                "Pick by number, range, name, or custom text. Press Enter to skip. "
                "Use commas for multiple answers; type 0 or 'none' to clear.",
                border_style="green",
            )
        )
        values.update(_prompt_onboarding_values())

    existing_profile: dict[str, Any] | None = None
    expected_version: int | None = None
    client: HelloFoodClient | None = None

    if not dry_run:
        client = HelloFoodClient()
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
        client = HelloFoodClient()

    if not onboarding.profile_has_content(profile_data) and not yes:
        if not Confirm.ask("This dietary graph is empty. Save it anyway?", default=False):
            raise typer.Exit()

    if not yes:
        render.profile_summary(console, profile_data, member_id=member_id)
        quip = personality.onboarding_quip(profile_data)
        if quip:
            console.print(Panel(quip, title="hello.food", border_style="green"))
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
        console.print(
            "[green]Child dietary graph saved locally. It was not sent to profile sync.[/green]"
        )
    else:
        console.print("[green]Dietary graph saved. Your CLI agent is now profile-aware.[/green]")
    if yes:
        quip = personality.onboarding_quip(profile_data)
        if quip:
            console.print(Panel(quip, title="hello.food", border_style="green"))
    render.profile_summary(
        console,
        profile_data,
        member_id=str(uploaded.get("member_id") or member_id),
        version=uploaded.get("version"),
        updated_at=uploaded.get("updated_at"),
    )


@app.command()
def ask(
    query: list[str] = typer.Argument(..., help="Natural-language request."),
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
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Ask the HelloFood conversational agent."""
    text = " ".join(query).strip()
    _ask_agent(
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
    client: Optional[HelloFoodClient] = None,
) -> dict[str, Any]:
    json_mode = _json_mode(json_output, raw)
    text = _validated(
        lambda: validation.required_text(text, label="Query", max_length=500)
    )
    client = client or HelloFoodClient()
    lat, lng = _resolve_agent_location(
        client,
        lat=lat,
        lng=lng,
        near=near,
        no_location=no_location,
        use_saved=use_saved_location,
        json_mode=json_mode,
    )
    banner.controller.loading(stderr_console, json_mode=json_mode)
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
    status = None if json_mode else stderr_console.status("[dim]thinking…[/dim]")
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
                    render.progress(stderr_console, data)
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
            render.agent_result(console, final_result)
            if choices_result:
                render.agent_choices(console, choices_result)
            visible_effect = effects.get("household_result") or effects.get("household_local_first")
            if isinstance(visible_effect, dict):
                render.household_mutation_effect(console, visible_effect)
        if show_continue_hint and not json_mode:
            conversation_id = final_result.get("conversation_id")
            if conversation_id:
                stderr_console.print(
                    "[dim]Continue with: heyfood reply \"...\" or heyfood chat[/dim]"
                )
        return final_result
    _fail(
        "The hello.food agent returned no final result.",
        kind="empty_agent_result",
        json_mode=json_mode,
    )
    return {}  # pragma: no cover - _fail always raises


def _local_conversation_document(client: HelloFoodClient) -> dict[str, Any]:
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
    data = _local_conversation_document(HelloFoodClient(create_device=False))
    if _write_result(data, json_mode=json_mode):
        return
    render.conversations(console, data)


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
    _ask_agent(
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
    client = HelloFoodClient(create_device=False)
    existing = client.last_conversation_id()
    if existing is not None and not yes:
        if json_mode or no_input or not _stdin_is_tty():
            raise typer.BadParameter("Pass --yes to clear the local conversation pointer.")
        if not Confirm.ask("Forget the locally remembered conversation?", console=console):
            data = {
                "ok": True,
                "cleared": False,
                "conversation_id": existing,
                "scope": "local_pointer_only",
            }
            if _write_result(data, json_mode=json_mode):
                return
            console.print("[dim]Conversation pointer kept.[/dim]")
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
        console.print("[green]Local conversation pointer cleared.[/green]")
    else:
        console.print("[dim]No local conversation pointer was stored.[/dim]")


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
    _ask_agent(
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
    if no_input is True or not _stdin_is_tty():
        raise typer.BadParameter(
            "Interactive chat requires TTY stdin. Use `heyfood ask` or `heyfood reply` for automation."
        )
    client = HelloFoodClient()
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
    console.print(
        "[bold green]hello.food chat[/bold green] "
        "[dim](type /exit, /new, /household, or /for NAME)[/dim]"
    )
    if initial_scope:
        render.household_scope(console, initial_scope)

    first_message = " ".join(initial).strip() if initial else ""
    active_choices: Optional[dict[str, Any]] = None
    while True:
        if first_message:
            text = first_message
            first_message = ""
            line = Text("you", style="bold")
            line.append(f" {text}")
            console.print(line)
        else:
            text = Prompt.ask("[bold]you[/bold]").strip()

        if not text:
            continue
        if text in {"/exit", "/quit"}:
            break
        if text == "/new":
            client.clear_last_conversation()
            conversation_id = None
            console.print("[dim]Started a fresh conversation.[/dim]")
            continue
        if text == "/household":
            render.household(console, household.public_document(client.household_state()))
            continue
        if text == "/for":
            render.household_scope(
                console,
                household.public_document(client.household_state())["active_scope"],
            )
            continue
        if text.startswith("/for "):
            selector = text.removeprefix("/for ").strip()
            try:
                state = client.set_household_scope(selector)
            except household.HouseholdError as exc:
                stderr_console.print(Text(str(exc), style="red"))
                continue
            chat_scope = str(state["active_scope"])
            client.clear_last_conversation()
            conversation_id = None
            active_choices = None
            render.household_scope(
                console,
                household.public_document(state)["active_scope"],
                changed=True,
            )
            console.print("[dim]Started a fresh conversation for the new scope.[/dim]")
            continue

        if active_choices:
            try:
                text = _resolve_chat_choice(text, active_choices)
            except household.HouseholdError as exc:
                stderr_console.print(Text(str(exc), style="red"))
                continue
            active_choices = None

        result = _ask_agent(
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
    meal: list[str] = typer.Argument(..., help="Meal text to log."),
    meal_type: Optional[str] = typer.Option(None, "--type", help="breakfast, lunch, dinner, or snack."),
    checking_for: Optional[str] = typer.Option(
        None,
        "--for",
        help="Household scope: member name/id, me, or everyone.",
    ),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Log a meal through the conversational agent."""
    text = " ".join(meal).strip()
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
    _ask_agent(
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
    client = HelloFoodClient()
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
    render.explain_item(console, data)


def _raise_geocode_error(
    exc: HelloFoodError,
    place: str | None = None,
    *,
    json_mode: bool = False,
) -> None:
    """Surface a geocode channel-tool failure as a friendly message, then exit.

    Discriminates on the error DETAIL token, not the bare status: the CLI's
    channel_tool() only maps the exact string "404: Not Found" to
    ChannelToolUnavailable, so a genuine "404: location_not_found" arrives here
    as a plain HelloFoodError.
    """
    if isinstance(exc, LoginRequired):
        _raise_command_error(exc, json_mode=json_mode)
    message = str(exc).strip()
    where = f' for "{place}"' if place else ""
    if "location_not_found" in message:
        _fail(
            f"Couldn't find a location{where}.",
            kind="location_not_found",
            json_mode=json_mode,
            hint='Try adding a state, e.g. "San Luis Obispo, CA".',
        )
    elif "geocoding_unavailable" in message:
        _fail(
            "Location lookup isn't available right now.",
            kind="geocoding_unavailable",
            json_mode=json_mode,
            hint="You can still set a location directly with --lat/--lng.",
        )
    elif "geocoding_upstream_error" in message:
        _fail(
            "Location lookup failed upstream.",
            kind="geocoding_upstream_error",
            json_mode=json_mode,
            hint="Try again in a moment.",
        )
    else:
        _raise_command_error(exc, json_mode=json_mode)


def _geocode_place(
    client: HelloFoodClient,
    place: str,
    *,
    json_mode: bool = False,
) -> tuple[float, float, str]:
    """Resolve a place name to (lat, lng, label) via the backend, or exit friendly."""
    try:
        data = client.geocode_location(place)
    except ChannelToolUnavailable as exc:
        _raise_command_error(exc, json_mode=json_mode)
    except (LoginRequired, HelloFoodError) as exc:
        _raise_geocode_error(exc, place, json_mode=json_mode)
    lat_v = data.get("latitude")
    lng_v = data.get("longitude")
    if not isinstance(lat_v, (int, float)) or isinstance(lat_v, bool) or (
        not isinstance(lng_v, (int, float)) or isinstance(lng_v, bool)
    ):
        _fail(
            f'Location lookup for "{place}" returned no coordinates.',
            kind="invalid_geocode_response",
            json_mode=json_mode,
        )
    label = str(data.get("label") or place)
    return float(lat_v), float(lng_v), label


def _resolve_agent_location(
    client: HelloFoodClient,
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


def resolve_location(
    client: HelloFoodClient,
    *,
    lat: Optional[float],
    lng: Optional[float],
    near: Optional[str],
    radius: Optional[float],
    json_mode: bool = False,
) -> tuple[float, float, float]:
    """Resolve search coordinates + radius by precedence: explicit > --near > saved."""
    lat, lng = _validated(lambda: validation.coordinates(lat, lng))
    validated_radius = (
        None
        if radius is None
        else _validated(
            lambda: validation.bounded_number(
                radius,
                label="Radius",
                minimum=0.1,
                maximum=50.0,
            )
        )
    )
    if lat is not None and lng is not None:
        return lat, lng, validated_radius if validated_radius is not None else 5.0
    if near:
        near = _validated(
            lambda: validation.required_text(near, label="Near", max_length=200)
        )
        resolved_lat, resolved_lng, resolved_label = _geocode_place(
            client,
            near,
            json_mode=json_mode,
        )
        if not json_mode:
            stderr_console.print(f"[dim]Searching near {resolved_label}[/dim]")
        return (
            resolved_lat,
            resolved_lng,
            validated_radius if validated_radius is not None else 5.0,
        )
    saved = client.saved_location()
    if saved:
        saved_radius = saved.get("radius_miles")
        saved_lat, saved_lng = _validated(
            lambda: validation.coordinates(
                saved.get("latitude"),
                saved.get("longitude"),
                required=True,
            )
        )
        effective = validated_radius
        if effective is None:
            effective = _validated(
                lambda: validation.bounded_number(
                    float(saved_radius) if isinstance(saved_radius, (int, float)) else 5.0,
                    label="Saved radius",
                    minimum=0.1,
                    maximum=50.0,
                )
            )
        return float(saved_lat), float(saved_lng), effective
    raise typer.BadParameter(
        'No location. Pass --lat/--lng, --near "City, ST", or run `heyfood location set`.'
    )


@app.command()
def search(
    query: Optional[str] = typer.Option(None, "--query", "-q", help="Restaurant search text."),
    lat: Optional[float] = typer.Option(None, "--lat", help="Latitude (with --lng). Overrides saved location."),
    lng: Optional[float] = typer.Option(None, "--lng", help="Longitude (with --lat). Overrides saved location."),
    near: Optional[str] = typer.Option(None, "--near", help='Place name to search near, e.g. "Fresno, CA".'),
    radius: Optional[float] = typer.Option(None, "--radius", help="Search radius in miles (default 5, or the saved location's radius)."),
    limit: int = typer.Option(10, "--limit", help="Maximum restaurants."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Search restaurants near coordinates, a saved default location, or a named place."""
    json_mode = _json_mode(json_output, raw)
    query = _validated(
        lambda: validation.optional_text(query, label="Query", max_length=200)
    )
    limit = _validated(
        lambda: validation.bounded_integer(
            limit,
            label="Limit",
            minimum=1,
            maximum=50,
        )
    )
    lat, lng = _validated(lambda: validation.coordinates(lat, lng))
    if radius is not None:
        radius = _validated(
            lambda: validation.bounded_number(
                radius,
                label="Radius",
                minimum=0.1,
                maximum=50.0,
            )
        )
    client = HelloFoodClient()
    resolved_lat, resolved_lng, resolved_radius = resolve_location(
        client, lat=lat, lng=lng, near=near, radius=radius, json_mode=json_mode
    )
    try:
        data = client.channel_tool(
            "search_restaurant",
            {
                "latitude": resolved_lat,
                "longitude": resolved_lng,
                "radius_miles": resolved_radius,
                "limit": limit,
                "query": query,
            },
        )
    except (LoginRequired, HelloFoodError) as exc:
        _raise_command_error(exc, json_mode=json_mode)
    client.remember_restaurant_search(data)
    if _write_result(data, json_mode=json_mode):
        return
    render.restaurants(console, data)


def _show_location(*, json_mode: bool = False) -> None:
    client = HelloFoodClient()
    location = client.saved_location()
    if _write_result({"location": location}, json_mode=json_mode):
        return
    render.location(console, location)


@location_app.callback(invoke_without_command=True)
def location_callback(
    ctx: typer.Context,
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Show the saved location when run with no subcommand."""
    if ctx.invoked_subcommand is None:
        _show_location(json_mode=_json_mode(json_output, raw))


@location_app.command("show")
def location_show(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Show the saved default location."""
    _show_location(json_mode=_json_mode(json_output, raw))


@location_app.command("set")
def location_set(
    place: Optional[str] = typer.Argument(None, help='Place name to geocode, e.g. "San Luis Obispo, CA".'),
    lat: Optional[float] = typer.Option(None, "--lat", help="Latitude (with --lng; skips geocoding)."),
    lng: Optional[float] = typer.Option(None, "--lng", help="Longitude (with --lat; skips geocoding)."),
    label: Optional[str] = typer.Option(None, "--label", help="Label for a coordinate location."),
    radius: float = typer.Option(5.0, "--radius", help="Default search radius in miles."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Save a default location by place name, or directly with --lat/--lng."""
    json_mode = _json_mode(json_output, raw)
    place = _validated(
        lambda: validation.optional_text(place, label="Place", max_length=200)
    )
    label = _validated(
        lambda: validation.optional_text(label, label="Label", max_length=100)
    )
    lat, lng = _validated(lambda: validation.coordinates(lat, lng))
    radius = _validated(
        lambda: validation.bounded_number(
            radius,
            label="Radius",
            minimum=0.1,
            maximum=50.0,
        )
    )
    if lat is not None and lng is not None:
        if place:
            raise typer.BadParameter("Pass either a place name or --lat/--lng, not both.")
        client = HelloFoodClient()
        resolved_label = label or f"{lat:.4f}, {lng:.4f}"
        client.save_location(label=resolved_label, latitude=lat, longitude=lng, radius_miles=radius)
        saved = client.saved_location()
        if _write_result({"location": saved}, json_mode=json_mode):
            return
        render.location(console, saved)
        return
    if not place:
        raise typer.BadParameter("Provide a place name, or --lat and --lng.")
    client = HelloFoodClient()
    resolved_lat, resolved_lng, resolved_label = _geocode_place(
        client,
        place,
        json_mode=json_mode,
    )
    # Always echo Google's formatted_address so an ambiguous input (e.g.
    # "Springfield" landing in the wrong state) is visible before it sticks.
    if not json_mode:
        stderr_console.print(f"[green]Resolved to[/green] [bold]{resolved_label}[/bold]")
    client.save_location(
        label=label or resolved_label,
        latitude=resolved_lat,
        longitude=resolved_lng,
        radius_miles=radius,
    )
    saved = client.saved_location()
    if _write_result({"location": saved}, json_mode=json_mode):
        return
    render.location(console, saved)


@location_app.command("clear")
def location_clear(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Forget the saved default location."""
    json_mode = _json_mode(json_output, raw)
    client = HelloFoodClient()
    cleared = client.clear_location()
    if _write_result({"cleared": cleared}, json_mode=json_mode):
        return
    if cleared:
        console.print("[green]Saved location cleared.[/green]")
    else:
        console.print("[dim]No saved location to clear.[/dim]")


@app.command()
def menu(
    restaurant_id: str = typer.Argument(..., help="HelloFood restaurant id or index from the last search."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Browse a full restaurant menu."""
    json_mode = _json_mode(json_output, raw)
    restaurant_id = _validated(
        lambda: validation.required_text(
            restaurant_id,
            label="Restaurant id",
            max_length=255,
        )
    )
    client = HelloFoodClient()
    try:
        resolved_restaurant_id = client.restaurant_id_from_selector(restaurant_id)
        data = client.channel_tool(
            "get_menu",
            {"restaurant_id": resolved_restaurant_id},
        )
        data = _poll_menu_until_terminal(
            client,
            resolved_restaurant_id,
            data,
            show_progress=not json_mode,
            json_mode=json_mode,
        )
    except ChannelToolUnavailable as exc:
        _raise_command_error(exc, json_mode=json_mode)
    except (LoginRequired, HelloFoodError) as exc:
        _raise_command_error(exc, json_mode=json_mode)
    status = str(data.get("status") or "ready")
    if json_mode:
        output.write_json(data)
        if status != "ready":
            raise typer.Exit(1)
        return
    if status == "ready":
        render.menu(console, data)
    elif status == "unavailable":
        stderr_console.print(f"[yellow]{data.get('message') or 'No public menu is available for this restaurant.'}[/yellow]")
        raise typer.Exit(1)
    elif status == "rate_limited":
        retry_after = data.get("retry_after_seconds")
        hint = f" Try again in about {int(retry_after)} seconds." if retry_after else " Try again later."
        stderr_console.print(f"[yellow]Menu fetch limit reached.{hint}[/yellow]")
        raise typer.Exit(1)
    elif status == "timed_out":
        job_id = data.get("job_id")
        stderr_console.print(
            f"[yellow]{data.get('message') or 'Menu is still being fetched after 30 seconds.'}[/yellow]"
        )
        if job_id:
            stderr_console.print(
                f"[dim]Job {job_id}. Rerun: heyfood menu {resolved_restaurant_id}[/dim]"
            )
        raise typer.Exit(1)
    else:
        stderr_console.print(f"[red]{data.get('reason') or data.get('message') or 'Menu fetching could not be completed.'}[/red]")
        raise typer.Exit(1)


def _poll_menu_until_terminal(
    client: HelloFoodClient,
    restaurant_id: str,
    initial: dict[str, Any],
    *,
    show_progress: bool = True,
    json_mode: bool = False,
) -> dict[str, Any]:
    """Poll an acquire-on-miss job until the server reports a terminal state."""
    if str(initial.get("status") or "ready") != "acquiring":
        return initial
    job_id = initial.get("job_id")
    if not isinstance(job_id, str) or not job_id:
        return {
            **initial,
            "status": "failed",
            "message": "The server did not return a menu job id.",
        }

    started_at = time.monotonic()
    deadline = started_at + MENU_POLL_TIMEOUT_SECONDS
    latest = initial
    banner.controller.loading(stderr_console, json_mode=json_mode)
    warned = False
    status_context = (
        stderr_console.status("[dim]Fetching and checking the menu…[/dim]")
        if show_progress
        else nullcontext(None)
    )
    with status_context as status:
        while str(latest.get("status") or "") == "acquiring":
            now = time.monotonic()
            remaining = deadline - now
            if remaining <= 0:
                return {
                    **latest,
                    "status": "timed_out",
                    "message": "Menu is still being fetched after 30 seconds.",
                    "poll_timeout_seconds": MENU_POLL_TIMEOUT_SECONDS,
                }
            if status is not None and not warned and now - started_at >= MENU_POLL_WARNING_SECONDS:
                warned = True
                status.update(
                    "[yellow]Still fetching the menu — returning control by 30 seconds…[/yellow]"
                )
            time.sleep(min(MENU_POLL_INTERVAL_SECONDS, remaining))
            try:
                latest = client.get_menu_status(
                    restaurant_id=restaurant_id,
                    job_id=job_id,
                )
            except ChannelToolUnavailable:
                return {
                    **latest,
                    "status": "timed_out",
                    "message": (
                        "This API does not expose menu-status polling yet. "
                        "Retry the menu command with the same restaurant id."
                    ),
                    "polling_supported": False,
                    "poll_timeout_seconds": 0,
                }
    return latest


@app.command("get-menu")
def get_menu_command(
    restaurant_id: str = typer.Argument(
        ...,
        help="HelloFood restaurant id or index from the last search.",
    ),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Fetch or browse a full restaurant menu (alias for `heyfood menu`)."""
    menu(restaurant_id, json_output=json_output, raw=raw)


@app.command()
def recommend(
    restaurant_id: str = typer.Argument(..., help="HelloFood restaurant id or index from the last search."),
    query: Optional[str] = typer.Option(None, "--query", "-q", help="What you are in the mood for."),
    limit: int = typer.Option(5, "--limit", help="Maximum recommendations."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Recommend menu items for your profile."""
    json_mode = _json_mode(json_output, raw)
    restaurant_id = _validated(
        lambda: validation.required_text(
            restaurant_id,
            label="Restaurant id",
            max_length=255,
        )
    )
    query = _validated(
        lambda: validation.optional_text(query, label="Query", max_length=200)
    )
    limit = _validated(
        lambda: validation.bounded_integer(
            limit,
            label="Limit",
            minimum=1,
            maximum=20,
        )
    )
    client = HelloFoodClient()
    try:
        selected = client.restaurant_from_selector(restaurant_id)
        if selected and selected.get("has_menu") is False:
            name = selected.get("name") or "that restaurant"
            _fail(
                f"No menu is available for {name} yet.",
                kind="menu_unavailable",
                json_mode=json_mode,
                hint="Try another result with a menu or run `heyfood item` for a specific food.",
            )
        data = client.channel_tool(
            "recommend_items",
            {
                "restaurant_id": client.restaurant_id_from_selector(restaurant_id),
                "query": query,
                "limit": limit,
            },
        )
    except ChannelToolUnavailable as exc:
        _raise_command_error(exc, json_mode=json_mode)
    except (LoginRequired, HelloFoodError) as exc:
        _raise_command_error(exc, json_mode=json_mode)
    if _write_result(data, json_mode=json_mode):
        return
    render.recommendations(console, data)


@recipes_app.command("search")
def recipes_search(
    query: list[str] = typer.Argument(..., help="Recipe search text."),
    cuisine: Optional[str] = typer.Option(None, "--cuisine", "-c", help="Cuisine filter."),
    meal_type: Optional[str] = typer.Option(None, "--type", "-t", help="Override inferred meal type: breakfast, lunch, dinner, snack, or dessert."),
    max_ready_time: Optional[int] = typer.Option(None, "--max-ready-time", help="Maximum ready time in minutes."),
    limit: int = typer.Option(5, "--limit", "-n", help="Maximum recipes to show."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Search real recipes matched to your hello.food profile."""
    json_mode = _json_mode(json_output, raw)
    text = _validated(
        lambda: validation.required_text(
            " ".join(query),
            label="Recipe query",
            max_length=200,
        )
    )
    cuisine = _validated(
        lambda: validation.optional_text(cuisine, label="Cuisine", max_length=80)
    )
    meal_type = _validated(
        lambda: validation.choice(
            meal_type,
            label="Meal type",
            choices={"breakfast", "lunch", "dinner", "snack", "dessert"},
        )
    )
    if max_ready_time is not None:
        max_ready_time = _validated(
            lambda: validation.bounded_integer(
                max_ready_time,
                label="Maximum ready time",
                minimum=1,
            )
        )
    limit = _validated(
        lambda: validation.bounded_integer(
            limit,
            label="Limit",
            minimum=1,
            maximum=20,
        )
    )
    payload: dict = {
        "query": text,
        "limit": limit,
    }
    if cuisine:
        payload["cuisine"] = cuisine
    if meal_type:
        payload["meal_type"] = meal_type
    if max_ready_time is not None:
        payload["max_ready_time"] = max_ready_time

    client = HelloFoodClient()
    try:
        data = client.channel_tool("search_recipes", payload)
    except LoginRequired as exc:
        _raise_command_error(exc, json_mode=json_mode)
    except ChannelToolUnavailable as exc:
        _raise_command_error(exc, json_mode=json_mode)
    except HelloFoodError as exc:
        message = str(exc)
        if _is_recipe_provider_unavailable(message):
            hint = None
            if is_local_api_url(client.api_url):
                hint = "The configured local recipe provider is unavailable."
            _fail(
                "Recipe search is temporarily unavailable.",
                kind="recipe_provider_unavailable",
                json_mode=json_mode,
                hint=hint or message,
            )
        _raise_command_error(exc, json_mode=json_mode)

    client.remember_recipe_search(data)
    if _write_result(data, json_mode=json_mode):
        return
    render.recipe_search(console, data)


def _is_recipe_provider_unavailable(message: str) -> bool:
    normalized = message.lower()
    return (
        normalized.startswith("503:")
        or "recipe service temporarily unavailable" in normalized
        or "recipe provider unavailable" in normalized
    )


@recipes_app.command("save")
def recipes_save(
    selector: str = typer.Argument(..., help="Recipe ref, Spoonacular id, or index from the last recipe search."),
    notes: Optional[str] = typer.Option(None, "--notes", help="Optional cookbook note."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Save a recipe to your hello.food cookbook."""
    json_mode = _json_mode(json_output, raw)
    selector = _validated(
        lambda: validation.required_text(selector, label="Recipe selector", max_length=300)
    )
    notes = _validated(
        lambda: validation.optional_text(notes, label="Notes", max_length=1000)
    )
    client = HelloFoodClient()
    try:
        payload = client.recipe_save_payload(selector)
        if notes:
            payload["notes"] = notes
        data = client.channel_tool("save_recipe", payload)
    except ChannelToolUnavailable as exc:
        _raise_command_error(exc, json_mode=json_mode)
    except (LoginRequired, HelloFoodError) as exc:
        _raise_command_error(exc, json_mode=json_mode)

    if _write_result(data, json_mode=json_mode):
        return
    render.saved_recipe_saved(console, data)


@recipes_app.command("saved")
def recipes_saved(
    limit: int = typer.Option(20, "--limit", "-n", help="Maximum saved recipes to show."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Show your saved recipe cookbook."""
    json_mode = _json_mode(json_output, raw)
    limit = _validated(
        lambda: validation.bounded_integer(
            limit,
            label="Limit",
            minimum=1,
            maximum=100,
        )
    )
    client = HelloFoodClient()
    try:
        data = client.channel_tool("list_saved_recipes", {"limit": limit})
    except ChannelToolUnavailable as exc:
        _raise_command_error(exc, json_mode=json_mode)
    except (LoginRequired, HelloFoodError) as exc:
        _raise_command_error(exc, json_mode=json_mode)

    if _write_result(data, json_mode=json_mode):
        return
    render.saved_recipes(console, data)


@app.command("daily")
def daily_summary(
    day: str = typer.Argument("today", help="Date as YYYY-MM-DD or 'today'."),
    member_id: Optional[str] = typer.Option(None, "--member-id", help="Household member id."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Show meal log summary for a day."""
    json_mode = _json_mode(json_output, raw)
    target = date.today().isoformat() if day == "today" else day
    target = _validated(lambda: validation.iso_date(target))
    member_id = _validated(
        lambda: validation.optional_text(member_id, label="Member id", max_length=255)
    )
    client = HelloFoodClient()
    try:
        data = client.daily_summary(target, member_id=member_id)
    except (LoginRequired, HelloFoodError) as exc:
        _raise_command_error(exc, json_mode=json_mode)
    if _write_result(data, json_mode=json_mode):
        return
    render.daily_summary(console, data)


def _error_kind(exc: HelloFoodError) -> str:
    if isinstance(exc, LoginRequired):
        return "login_required"
    if isinstance(exc, ChannelToolUnavailable):
        return "channel_tool_unavailable"
    return "api_error"


def _is_profile_sync_consent_required(exc: HelloFoodError) -> bool:
    message = str(exc).strip().casefold()
    return message.startswith("403:") and "sync consent" in message and "required" in message


def _raise_command_error(exc: HelloFoodError, *, json_mode: bool = False) -> None:
    hint = None
    if isinstance(exc, LoginRequired):
        hint = "Run `heyfood login` and retry."
    elif isinstance(exc, ChannelToolUnavailable):
        hint = "The CLI and connected API may be on different versions."
    elif "Missing API key" in str(exc):
        hint = "Run `heyfood login` again, or `heyfood doctor` for details."
    _fail(
        str(exc),
        kind=_error_kind(exc),
        json_mode=json_mode,
        hint=hint,
    )


def _print_command_error(exc: HelloFoodError) -> None:
    stderr_console.print(f"[red]heyfood error:[/red] {exc}")
    if "Missing API key" in str(exc):
        stderr_console.print(
            "This session could not refresh through the current API. "
            "Run [bold]heyfood login[/bold] again, or [bold]heyfood doctor[/bold] for details."
        )


def _print_tool_unavailable(exc: ChannelToolUnavailable) -> None:
    stderr_console.print(f"[yellow]{exc}[/yellow]")
    stderr_console.print(
        "Your CLI is newer than the connected API, or the API deploy is missing this tool. "
        "Try [bold]heyfood ask[/bold] for the same task, or rerun after the API deploy finishes."
    )


def _print_no_menu(restaurant: dict[str, Any]) -> None:
    name = restaurant.get("name") or "that restaurant"
    stderr_console.print(f"[yellow]No menu is available for {name} yet.[/yellow]")
    stderr_console.print(
        "Try another search result with [bold]Menu=yes[/bold], or assess a specific item with "
        f"[bold]heyfood item \"pad thai\" --restaurant \"{name}\"[/bold]."
    )


def _provided_onboarding_fields(**values: Any) -> bool:
    return any(value is not None and value != [] for value in values.values())


def _first_name_from_account(store: ConfigStore) -> str | None:
    try:
        me = HelloFoodClient(store=store).me()
    except HelloFoodError:
        return None
    first_name = personality.first_name_from_account(me)
    if first_name:
        personality.save_cli_first_name(store, first_name)
    return first_name


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
    diets = _prompt_multi_options(
        "Diet style",
        onboarding.DIET_STYLES[:8],
    )
    allergies = _prompt_multi_options(
        "Allergies or restrictions",
        onboarding.ALLERGIES[:10],
    )
    conditions = _prompt_multi_options(
        "Health conditions",
        onboarding.HEALTH_CONDITIONS[:9],
    )
    avoid = _prompt_free_text_list("Specific ingredients to avoid", ["raw onion", "garlic", "cilantro"])
    activity = _prompt_one_option("Activity level", onboarding.ACTIVITY_LEVELS)
    cuisines = _prompt_multi_options("Cuisines you love", onboarding.CUISINES[:8])
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
    console.print(table)


def _prompt_missing_onboarding_values(
    values: dict[str, Any],
    *,
    answered_sections: set[str],
    notes_answered: bool,
) -> dict[str, Any]:
    prompted: dict[str, Any] = {}
    if "diets" not in answered_sections:
        prompted["diets"] = _prompt_multi_options(
            "Diet style",
            onboarding.DIET_STYLES[:8],
        )
    if "allergies" not in answered_sections:
        prompted["allergies"] = _prompt_multi_options(
            "Allergies or restrictions",
            onboarding.ALLERGIES[:10],
        )
    if "conditions" not in answered_sections:
        prompted["conditions"] = _prompt_multi_options(
            "Health conditions",
            onboarding.HEALTH_CONDITIONS[:9],
        )
    if "avoid_ingredients" not in answered_sections:
        prompted["avoid_ingredients"] = _prompt_free_text_list(
            "Specific ingredients to avoid",
            ["raw onion", "garlic", "cilantro"],
        )
    if "activity_level" not in answered_sections:
        prompted["activity_level"] = _prompt_one_option(
            "Activity level",
            onboarding.ACTIVITY_LEVELS,
        )
    if "cuisines" not in answered_sections:
        prompted["cuisines"] = _prompt_multi_options(
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
    console.print(table)


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
    client: HelloFoodClient,
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
        console.print(
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
