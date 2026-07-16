"""Voice utility commands: device discovery and preference management."""
from __future__ import annotations

from typing import Optional

from .. import main
from ..main import (
    HelloFoodClient,
    Table,
    _fail,
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


def _voice_status_payload(client: "HelloFoodClient") -> dict:
    settings = client.voice_settings()
    mode = settings.get("capture_mode")
    return {
        # ``mode_set`` distinguishes an omitted preference from an explicit
        # 'auto' selection: an omitted mode leaves auto behavior free to change
        # with the environment, while an explicit 'auto' is a recorded choice.
        "mode_set": mode is not None,
        "capture_mode": mode if mode is not None else voice_capture.AUTO,
        "device": settings.get("device"),
    }


@voice_app.command("status")
def voice_status(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Show the persisted voice capture preferences for this machine."""
    json_mode = _json_mode(json_output, raw)
    payload = _voice_status_payload(HelloFoodClient())
    if _write_result(payload, json_mode=json_mode):
        return
    if payload["mode_set"]:
        main.console.print(f"Capture mode: {payload['capture_mode']} (explicitly set)")
    else:
        main.console.print("Capture mode: auto (default; not explicitly set)")
    device = payload.get("device")
    main.console.print(f"Audio device: {device if device is not None else '(default)'}")


@voice_app.command("set")
def voice_set(
    mode: Optional[str] = typer.Option(
        None, "--mode", help="Persist a capture mode: auto, native, browser, or typed."
    ),
    device: Optional[str] = typer.Option(
        None, "--device", help="Persist a default input device id or name."
    ),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Persist a voice capture preference (mode and/or device)."""
    json_mode = _json_mode(json_output, raw)
    if mode is None and device is None:
        _fail(
            "Provide --mode and/or --device to set a preference.",
            kind="nothing_to_set",
            json_mode=json_mode,
        )
    if mode is not None:
        normalized = mode.strip().lower()
        if normalized not in voice_capture.VALID_MODES:
            _fail(
                f"Unknown capture mode '{mode}'. Choose auto, native, browser, or typed.",
                kind="invalid_voice_mode",
                json_mode=json_mode,
            )
        mode = normalized
    client = HelloFoodClient()
    client.remember_voice_settings(capture_mode=mode, device=device)
    payload = _voice_status_payload(client)
    if _write_result(payload, json_mode=json_mode):
        return
    main.stderr_console.print("[green]Voice preferences updated.[/green]")
    if payload["mode_set"]:
        main.console.print(f"Capture mode: {payload['capture_mode']} (explicitly set)")
    else:
        main.console.print("Capture mode: auto (default; not explicitly set)")
    device_value = payload.get("device")
    main.console.print(
        f"Audio device: {device_value if device_value is not None else '(default)'}"
    )


@voice_app.command("reset")
def voice_reset(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Clear all persisted voice capture preferences for this machine."""
    json_mode = _json_mode(json_output, raw)
    client = HelloFoodClient()
    client.remember_voice_settings(capture_mode=None, device=None, clear=True)
    payload = _voice_status_payload(client)
    if _write_result(payload, json_mode=json_mode):
        return
    main.stderr_console.print("[green]Voice preferences reset to defaults.[/green]")
