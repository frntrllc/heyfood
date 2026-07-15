"""Voice utility commands (device discovery for native capture)."""
from __future__ import annotations

from .. import main
from ..main import (
    Table,
    _json_mode,
    _write_result,
    typer,
    voice_app,
    voice_capture,
)


@voice_app.command("devices")
def voice_devices(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """List microphones available for native voice capture."""
    json_mode = _json_mode(json_output, raw)
    data = voice_capture.describe_devices()
    if _write_result(data, json_mode=json_mode):
        return
    if not data.get("available"):
        main.stderr_console.print(f"[yellow]{data.get('message')}[/yellow]")
        return
    devices = data.get("devices") or []
    if not devices:
        main.stderr_console.print(
            "[yellow]No microphone input devices were found.[/yellow]"
        )
        return
    table = Table(title="Voice input devices")
    table.add_column("Id", justify="right", style="dim", no_wrap=True)
    table.add_column("Name")
    table.add_column("Channels", justify="right")
    table.add_column("Default", justify="center")
    for device in devices:
        table.add_row(
            str(device.get("index")),
            str(device.get("name")),
            str(device.get("max_input_channels")),
            "●" if device.get("is_default") else "",
        )
    main.console.print(table)
    main.stderr_console.print(
        "[dim]Use --audio-device <id-or-name> on onboard, ask, or log to pick one.[/dim]"
    )
