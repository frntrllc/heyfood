"""Meal-log summary commands."""
from __future__ import annotations

from .. import main
from ..main import (
    HelloFoodError,
    LoginRequired,
    Optional,
    _json_mode,
    _raise_command_error,
    _validated,
    _write_result,
    app,
    date,
    render,
    typer,
    validation,
)

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
    client = main.HelloFoodClient()
    try:
        data = client.daily_summary(target, member_id=member_id)
    except (LoginRequired, HelloFoodError) as exc:
        _raise_command_error(exc, json_mode=json_mode)
    if _write_result(data, json_mode=json_mode):
        return
    render.daily_summary(main.console, data)
