from __future__ import annotations

import json
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path

import pytest
import typer.rich_utils
from click.utils import strip_ansi
from rich.console import Console
from typer.testing import CliRunner

from heyfood_cli import main


# Pinned to the CURRENT baseline (0.3.0): the live CLI's help and raw output must
# match these fixtures byte-for-byte. 0.3.0 adds native voice (--voice /
# --voice-capture / --audio-device on ask/log/onboard, the `voice` group with
# devices/status/set/reset). The 0.1.0 and 0.2.0 baselines are kept on disk as
# immutable historical evidence and are guarded by the historical tests below;
# see COMPAT_ROOT / "0.2.0" for the reconstructed released-v0.2.0 (household, no
# voice) snapshot.
COMPAT_ROOT = Path(__file__).parent / "fixtures" / "compat"
CURRENT_VERSION = "0.3.0"
FIXTURE_ROOT = COMPAT_ROOT / CURRENT_VERSION
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
    "household": ("household",),
    "household-list": ("household", "list"),
    "household-current": ("household", "current"),
    "household-use": ("household", "use"),
    "household-label": ("household", "label"),
    "conversation": ("conversation",),
    "conversation-list": ("conversation", "list"),
    "conversation-resume": ("conversation", "resume"),
    "conversation-clear": ("conversation", "clear"),
    "voice": ("voice",),
    "voice-devices": ("voice", "devices"),
    "voice-status": ("voice", "status"),
    "voice-set": ("voice", "set"),
    "voice-reset": ("voice", "reset"),
}


def _normalize_help(value: str) -> str:
    lines = [line.rstrip() for line in strip_ansi(value).splitlines()]
    return "\n".join(lines).strip() + "\n"


@pytest.mark.parametrize(("name", "args"), HELP_COMMANDS.items())
def test_help_matches_current_baseline(
    name: str,
    args: tuple[str, ...],
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(typer.rich_utils, "MAX_WIDTH", 120)
    monkeypatch.setattr(typer.rich_utils, "FORCE_TERMINAL", False)
    runner = CliRunner(env={"NO_COLOR": "1", "TERM": "dumb", "COLUMNS": "120"})

    result = runner.invoke(
        main.app,
        [*args, "--help"],
        prog_name="heyfood",
        color=False,
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
def test_raw_output_matches_current_examples(
    monkeypatch: pytest.MonkeyPatch,
    name: str,
    callback,
) -> None:
    assert _capture_raw(monkeypatch, callback) == RAW_OUTPUTS[name]


# --------------------------------------------------------------------------- #
# Historical baselines: immutable release evidence, guarded so they cannot rot.
# --------------------------------------------------------------------------- #

# Commands present in the pre-voice releases (0.1.0 and the reconstructed 0.2.0).
_PRE_VOICE_COMMANDS = {
    name for name in HELP_COMMANDS if not name.startswith("voice")
}


def _help_files(version: str) -> dict[str, str]:
    root = COMPAT_ROOT / version / "help"
    return {path.stem: path.read_text() for path in sorted(root.glob("*.txt"))}


def test_reconstructed_0_2_0_matches_the_0_1_0_baseline() -> None:
    # The released v0.2.0 (household support) never changed any command's help
    # from the 0.1.0 baseline. The reconstruction from the exact published commit
    # 583ced64 proved this; freezing the equality here keeps that evidence honest.
    v010 = _help_files("0.1.0")
    v020 = _help_files("0.2.0")
    assert set(v020) == set(v010) == _PRE_VOICE_COMMANDS
    for name in v010:
        assert v020[name] == v010[name], f"0.2.0 help drifted from 0.1.0 for {name}"


def test_reconstructed_0_2_0_has_no_voice_surface() -> None:
    # Voice shipped in 0.3.0, not 0.2.0. The historical baseline must not contain
    # any voice command help — that was the whole point of reconstructing it.
    v020_help = COMPAT_ROOT / "0.2.0" / "help"
    assert not list(v020_help.glob("voice*.txt"))


def test_0_2_0_raw_outputs_match_the_0_1_0_release() -> None:
    v010 = json.loads((COMPAT_ROOT / "0.1.0" / "raw_outputs.json").read_text())
    v020 = json.loads((COMPAT_ROOT / "0.2.0" / "raw_outputs.json").read_text())
    assert v020 == v010


def test_current_baseline_only_adds_voice_over_0_2_0() -> None:
    # The 0.2.0 -> 0.3.0 diff must be exactly the voice surface: new voice
    # commands plus the four commands that gained --voice options.
    v020 = _help_files("0.2.0")
    v030 = _help_files("0.3.0")
    assert set(v020).issubset(set(v030))
    new_commands = set(v030) - set(v020)
    assert new_commands == {"voice", "voice-devices", "voice-status", "voice-set", "voice-reset"}
    changed = {name for name in v020 if v030[name] != v020[name]}
    assert changed == {"root", "ask", "log", "onboard"}
