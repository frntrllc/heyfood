"""Command-surface tests for the voice path: stdout purity and `voice devices`."""
from __future__ import annotations

import json

from typer.testing import CliRunner

from heyfood_cli import main
from heyfood_cli import voice_capture
from heyfood_cli.voice_capture import VoiceOutcome


runner = CliRunner(mix_stderr=False)


class _AgentClient:
    """Minimal client so `ask` reaches a JSON result without a network."""

    def saved_location(self):
        return None

    def voice_settings(self):
        return {}

    def stream_agent(self, payload):
        assert payload.get("query") == "spoken request"
        yield "result", {"message": "ok", "conversation_id": "conv-1"}

    def remember_conversation(self, result):
        pass


def test_ask_voice_json_stdout_is_pure(monkeypatch):
    def fake_capture(client, *, purpose, **kwargs):
        # Simulate capture UI noise landing on stderr, never stdout.
        kwargs["stderr_console"].print("[dim]● Recording...[/dim]")
        assert purpose == "ask"
        return VoiceOutcome(transcript="spoken request", source="native")

    monkeypatch.setattr(main, "capture_voice_input", fake_capture)
    monkeypatch.setattr(main, "HelloFoodClient", lambda: _AgentClient())

    result = runner.invoke(
        main.app,
        ["ask", "--voice", "--json"],
        color=False,
    )

    assert result.exit_code == 0
    payload = json.loads(result.stdout)
    assert payload["conversation_id"] == "conv-1"
    assert "Recording" not in result.stdout


def test_ask_without_query_or_voice_is_rejected(monkeypatch):
    monkeypatch.setattr(main, "HelloFoodClient", lambda: _AgentClient())
    result = runner.invoke(main.app, ["ask"], color=False)
    assert result.exit_code != 0


def test_log_voice_routes_through_agent(monkeypatch):
    captured = {}

    def fake_capture(client, *, purpose, **kwargs):
        captured["purpose"] = purpose
        return VoiceOutcome(transcript="two eggs and toast", source="native")

    def fake_ask_agent(text, **kwargs):
        captured["prompt"] = text
        return {"message": "logged"}

    monkeypatch.setattr(main, "capture_voice_input", fake_capture)
    monkeypatch.setattr(main, "HelloFoodClient", lambda: _AgentClient())
    monkeypatch.setattr(main, "_ask_agent", fake_ask_agent)

    result = runner.invoke(main.app, ["log", "--voice"], color=False)

    assert result.exit_code == 0
    assert captured["purpose"] == "log"
    assert "two eggs and toast" in captured["prompt"]


def test_voice_devices_json(monkeypatch):
    monkeypatch.setattr(
        voice_capture,
        "describe_devices",
        lambda backend=None: {
            "available": True,
            "devices": [
                {
                    "index": 0,
                    "name": "Built-in Mic",
                    "max_input_channels": 1,
                    "default_samplerate": 48000.0,
                    "is_default": True,
                }
            ],
        },
    )
    result = runner.invoke(main.app, ["voice", "devices", "--json"], color=False)
    assert result.exit_code == 0
    payload = json.loads(result.stdout)
    assert payload["available"] is True
    assert payload["devices"][0]["name"] == "Built-in Mic"


def test_voice_devices_without_extra_is_graceful(monkeypatch):
    monkeypatch.setattr(
        voice_capture,
        "describe_devices",
        lambda backend=None: {
            "available": False,
            "reason": "extra_not_installed",
            "message": "Native voice capture isn't installed.",
            "devices": [],
        },
    )
    result = runner.invoke(main.app, ["voice", "devices"], color=False)
    assert result.exit_code == 0
    # Graceful notice goes to stderr; stdout stays clean for humans/pipes.
    assert result.stdout.strip() == ""
