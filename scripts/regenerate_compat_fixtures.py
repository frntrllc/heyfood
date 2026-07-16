#!/usr/bin/env python3
"""Regenerate a command-help compatibility baseline from the current source.

This is the repo's canonical fixture tooling: it reproduces, byte-for-byte, the
normalization that ``tests/test_compatibility_fixtures.py`` asserts against, so a
baseline directory can be regenerated deterministically from whatever CLI source
is currently checked out. Run it from a clean worktree pinned at the exact commit
whose behavior you want to capture.

Usage::

    python scripts/regenerate_compat_fixtures.py <baseline-dir>

It writes ``<baseline-dir>/help/<name>.txt`` for every command in the pinned
help map. It never touches ``raw_outputs.json`` — those are hand-authored stub
fixtures, not runtime captures, and must be copied deliberately.
"""
from __future__ import annotations

import sys
from pathlib import Path

import typer.rich_utils
from click.utils import strip_ansi
from typer.testing import CliRunner

from heyfood_cli import main


# The command -> argv map. Kept in lockstep with the compat test's HELP_COMMANDS.
HELP_COMMANDS: dict[str, tuple[str, ...]] = {
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
    # Voice commands (present from the native-voice baseline onward). Commands
    # absent from the checked-out source simply exit non-zero and are skipped.
    "voice": ("voice",),
    "voice-devices": ("voice", "devices"),
    "voice-status": ("voice", "status"),
    "voice-set": ("voice", "set"),
    "voice-reset": ("voice", "reset"),
}


def _normalize_help(value: str) -> str:
    lines = [line.rstrip() for line in strip_ansi(value).splitlines()]
    return "\n".join(lines).strip() + "\n"


def _render_help(args: tuple[str, ...]) -> str | None:
    typer.rich_utils.MAX_WIDTH = 120
    typer.rich_utils.FORCE_TERMINAL = False
    runner = CliRunner(env={"NO_COLOR": "1", "TERM": "dumb", "COLUMNS": "120"})
    result = runner.invoke(main.app, [*args, "--help"], prog_name="heyfood", color=False)
    if result.exit_code != 0:
        return None
    return _normalize_help(result.stdout)


def main_cli() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: regenerate_compat_fixtures.py <baseline-dir>")
    baseline = Path(sys.argv[1])
    help_dir = baseline / "help"
    help_dir.mkdir(parents=True, exist_ok=True)
    written = 0
    skipped: list[str] = []
    for name, args in HELP_COMMANDS.items():
        rendered = _render_help(args)
        if rendered is None:
            skipped.append(name)
            continue
        (help_dir / f"{name}.txt").write_text(rendered)
        written += 1
    print(f"wrote {written} help fixtures to {help_dir}")
    if skipped:
        print(f"skipped (not in this source): {', '.join(skipped)}")


if __name__ == "__main__":
    main_cli()
