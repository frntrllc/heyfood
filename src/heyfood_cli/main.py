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
from .auth import LoginInterrupted, local_urls, perform_device_login, perform_login
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


# ---------------------------------------------------------------------------
# Command registration and backward-compatible re-exports.
# Importing these modules registers their commands on the Typer apps above
# and re-exports their callables so ``heyfood_cli.main.<command>`` keeps working.
# ---------------------------------------------------------------------------
from .commands.auth import (  # noqa: E402,F401
    login,
    logout,
    status,
    doctor,
    _first_name_from_account,
)
from .commands.profiles import (  # noqa: E402,F401
    members_list,
    household_list,
    household_current,
    household_use,
    household_label,
    profile,
    onboard,
    _provided_onboarding_fields,
    _resolve_first_name,
    _has_extracted_onboarding_values,
    _prompt_onboarding_values,
    _print_extracted_onboarding_review,
    _prompt_missing_onboarding_values,
    _has_selected_conditions,
    _prompt_multi_options,
    _prompt_one_option,
    _prompt_free_text_list,
    _print_option_table,
    _parse_option_selection,
    _expand_selection_token,
    _dedupe,
    _ensure_profile_sync_consent,
)
from .commands.agent import (  # noqa: E402,F401
    ask,
    _ask_agent,
    _local_conversation_document,
    conversation_list,
    conversation_resume,
    conversation_clear,
    _thinking_message,
    _is_confirmation_reply,
    _resolve_chat_choice,
    reply,
    chat,
    log,
    item,
    _resolve_agent_location,
)
from .commands.restaurants import (  # noqa: E402,F401
    resolve_location,
    search,
    _show_location,
    location_callback,
    location_show,
    location_set,
    location_clear,
    menu,
    _poll_menu_until_terminal,
    get_menu_command,
    recommend,
)
from .commands.meals import (  # noqa: E402,F401
    daily_summary,
)
from .commands.recipes import (  # noqa: E402,F401
    recipes_search,
    _is_recipe_provider_unavailable,
    recipes_save,
    recipes_saved,
)
from .commands.config import (  # noqa: E402,F401
    _validated_service_url,
    context_list,
    context_show,
    context_use,
    context_set,
    config_path,
    config_show,
    config_validate,
)
