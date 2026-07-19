"""Account-owned AI-channel link management."""
from __future__ import annotations

from rich.table import Table

from .. import main, validation
from ..main import (
    Confirm,
    HelloFoodError,
    LoginRequired,
    _json_mode,
    _raise_command_error,
    _validated,
    _write_result,
    channels_app,
    typer,
)

_PUBLIC_LINK_FIELDS = (
    "id",
    "channel",
    "scopes",
    "status",
    "linked_at",
    "created_at",
)


def _public_links(result: dict) -> list[dict]:
    """Allowlist link metadata so credentials can never reach CLI output."""
    raw_links = result.get("links")
    if not isinstance(raw_links, list):
        return []
    return [
        {key: link[key] for key in _PUBLIC_LINK_FIELDS if key in link}
        for link in raw_links
        if isinstance(link, dict)
    ]


@channels_app.command("list")
def channels_list(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """List account links used by ChatGPT, Gemini, Claude, and other channels."""
    json_mode = _json_mode(json_output, raw)
    client = main.HelloFoodClient(create_device=False)
    try:
        result = client.list_channel_links()
    except (LoginRequired, HelloFoodError) as exc:
        _raise_command_error(exc, json_mode=json_mode)

    links = _public_links(result)
    document = {
        "ok": True,
        "links": links,
        "total_count": len(links),
    }
    if _write_result(document, json_mode=json_mode):
        return

    if not links:
        main.console.print("No linked AI channels.")
        return

    table = Table(title="Linked AI channels")
    table.add_column("Channel")
    table.add_column("Status")
    table.add_column("Scopes")
    table.add_column("Link ID")
    for link in links:
        if not isinstance(link, dict):
            continue
        scopes = link.get("scopes") if isinstance(link.get("scopes"), list) else []
        table.add_row(
            str(link.get("channel") or "unknown"),
            str(link.get("status") or "unknown"),
            ", ".join(str(scope) for scope in scopes),
            str(link.get("id") or ""),
        )
    main.console.print(table)


@channels_app.command("disconnect")
def channels_disconnect(
    link_id: str = typer.Argument(..., help="Link ID from `heyfood channels list`."),
    yes: bool = typer.Option(False, "--yes", "-y", help="Revoke without prompting."),
    no_input: bool = typer.Option(False, "--no-input", help="Never prompt; requires --yes."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Disconnect one AI channel and revoke every token issued for that link."""
    json_mode = _json_mode(json_output, raw)
    normalized_link_id = _validated(
        lambda: validation.required_text(link_id, label="Link ID", max_length=255)
    )

    if not yes:
        if json_mode or no_input or not main._interactive_terminal():
            raise typer.BadParameter(
                "Pass --yes to disconnect an AI channel non-interactively."
            )
        if not Confirm.ask(
            "Disconnect this AI channel and revoke its access?",
            default=False,
            console=main.console,
        ):
            main.console.print("Channel link left connected.")
            return

    client = main.HelloFoodClient(create_device=False)
    try:
        result = client.disconnect_channel_link(normalized_link_id)
    except (LoginRequired, HelloFoodError) as exc:
        _raise_command_error(exc, json_mode=json_mode)

    revoked = result.get("revoked") is True
    document = {
        "ok": revoked,
        "revoked": revoked,
        "link_id": str(result.get("link_id") or normalized_link_id),
    }
    if _write_result(document, json_mode=json_mode):
        return
    main.console.print(f"[green]Disconnected channel link {normalized_link_id}.[/green]")
