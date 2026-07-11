from __future__ import annotations

import json
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path

import pytest
from click.utils import strip_ansi
from rich.console import Console
from typer.testing import CliRunner

from heyfood_cli import main


FIXTURE_ROOT = Path(__file__).parent / "fixtures" / "compat" / "0.1.0"
HELP_ROOT = FIXTURE_ROOT / "help"
RAW_OUTPUTS = json.loads((FIXTURE_ROOT / "raw_outputs.json").read_text())["outputs"]

HELP_COMMANDS = {
    "root": (),
    "login": ("login",),
    "logout": ("logout",),
    "status": ("status",),
    "doctor": ("doctor",),
    "profile": ("profile",),
    "onboard": ("onboard",),
    "ask": ("ask",),
    "reply": ("reply",),
    "chat": ("chat",),
    "log": ("log",),
    "item": ("item",),
    "search": ("search",),
    "menu": ("menu",),
    "get-menu": ("get-menu",),
    "recommend": ("recommend",),
    "daily": ("daily",),
    "recipes": ("recipes",),
    "recipes-search": ("recipes", "search"),
    "recipes-save": ("recipes", "save"),
    "recipes-saved": ("recipes", "saved"),
    "location": ("location",),
    "location-show": ("location", "show"),
    "location-set": ("location", "set"),
    "location-clear": ("location", "clear"),
    "context": ("context",),
    "context-list": ("context", "list"),
    "context-show": ("context", "show"),
    "context-use": ("context", "use"),
    "context-set": ("context", "set"),
    "config": ("config",),
    "config-path": ("config", "path"),
    "config-show": ("config", "show"),
    "config-validate": ("config", "validate"),
    "members": ("members",),
    "members-list": ("members", "list"),
    "conversation": ("conversation",),
    "conversation-list": ("conversation", "list"),
    "conversation-resume": ("conversation", "resume"),
    "conversation-clear": ("conversation", "clear"),
}


def _normalize_help(value: str) -> str:
    lines = [line.rstrip() for line in strip_ansi(value).splitlines()]
    return "\n".join(lines).strip() + "\n"


@pytest.mark.parametrize(("name", "args"), HELP_COMMANDS.items())
def test_help_matches_committed_0_1_0_baseline(name: str, args: tuple[str, ...]) -> None:
    runner = CliRunner(env={"NO_COLOR": "1", "TERM": "dumb", "COLUMNS": "120"})

    result = runner.invoke(
        main.app,
        [*args, "--help"],
        prog_name="heyfood",
        color=False,
        terminal_width=120,
    )

    assert result.exit_code == 0
    assert _normalize_help(result.stdout) == (HELP_ROOT / f"{name}.txt").read_text()


class _FixtureClient:
    api_url = "https://api.hello.food"

    def profile_consent_status(self) -> dict:
        return {"has_consent": True}

    def download_profile(self, *, member_id: str) -> dict:
        assert member_id == "_self"
        return RAW_OUTPUTS["profile"]

    def stream_agent(self, payload: dict):
        assert payload["query"] == "What can I eat?"
        yield "result", RAW_OUTPUTS["ask"]

    def remember_conversation(self, result: dict) -> None:
        assert result == RAW_OUTPUTS["ask"]

    def saved_location(self) -> None:
        return None

    def channel_tool(self, name: str, payload: dict) -> dict:
        responses = {
            "search_restaurant": "search",
            "search_recipes": "recipes_search",
            "save_recipe": "recipes_save",
            "list_saved_recipes": "recipes_saved",
        }
        return RAW_OUTPUTS[responses[name]]

    def remember_restaurant_search(self, result: dict) -> None:
        assert result == RAW_OUTPUTS["search"]

    def remember_recipe_search(self, result: dict) -> None:
        assert result == RAW_OUTPUTS["recipes_search"]

    def recipe_save_payload(self, selector: str) -> dict:
        assert selector == "recipe:thai-basil-chicken"
        return {"recipe_ref": selector}


def _capture_raw(monkeypatch: pytest.MonkeyPatch, callback) -> dict:
    output = StringIO()
    monkeypatch.setattr(main, "HelloFoodClient", _FixtureClient)
    monkeypatch.setattr(
        main,
        "console",
        Console(file=output, force_terminal=False, width=120),
    )

    with redirect_stdout(output):
        callback()

    stdout = output.getvalue()
    assert "\x1b" not in stdout
    return json.loads(stdout)


@pytest.mark.parametrize(
    ("name", "callback"),
    (
        ("profile", lambda: main.profile(member_id="_self", raw=True)),
        ("ask", lambda: main._ask_agent("What can I eat?", raw=True)),
        (
            "search",
            lambda: main.search(
                query="thai",
                lat=35.28,
                lng=-120.66,
                near=None,
                radius=None,
                limit=10,
                raw=True,
            ),
        ),
        (
            "recipes_search",
            lambda: main.recipes_search(
                query=["Thai dinner"],
                cuisine=None,
                meal_type=None,
                max_ready_time=None,
                limit=5,
                raw=True,
            ),
        ),
        (
            "recipes_save",
            lambda: main.recipes_save(
                selector="recipe:thai-basil-chicken",
                notes=None,
                raw=True,
            ),
        ),
        ("recipes_saved", lambda: main.recipes_saved(limit=20, raw=True)),
    ),
)
def test_raw_output_matches_committed_0_1_0_examples(
    monkeypatch: pytest.MonkeyPatch,
    name: str,
    callback,
) -> None:
    assert _capture_raw(monkeypatch, callback) == RAW_OUTPUTS[name]
