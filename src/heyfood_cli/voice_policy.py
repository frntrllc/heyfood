"""One central interactive-capability policy for voice capture.

Voice is an interactive-only input method. Every command that accepts ``--voice``
routes through :func:`ensure_voice_interactive` *before* any microphone device is
opened or any browser is launched, so a machine/automation context produces one
stable, deterministic error instead of blocking on a prompt or opening hardware.

The rejected contexts are: ``--json``/``--raw`` output, ``--no-input``, a
non-interactive stdin or stderr, ``CI``, and ``TERM=dumb``. ``voice devices
--json`` does not go through here — it is a read-only machine command.
"""
from __future__ import annotations

import os
import sys
from typing import Callable


# One stable error kind for every noninteractive rejection, so automation can
# branch on a single value regardless of which specific condition tripped.
VOICE_NOT_INTERACTIVE = "voice_not_interactive"


class VoiceNotInteractiveError(RuntimeError):
    """Voice capture was requested in a context where it is not permitted."""

    kind = VOICE_NOT_INTERACTIVE

    def __init__(self, message: str, *, hint: str | None = None):
        super().__init__(message)
        self.hint = hint


def _is_truthy_env(value: str | None) -> bool:
    return bool(value) and value.strip().lower() not in {"", "0", "false", "no"}


def ensure_voice_interactive(
    *,
    json_mode: bool,
    no_input: bool = False,
    env: dict[str, str] | None = None,
    stdin_isatty: Callable[[], bool] | None = None,
    stderr_isatty: Callable[[], bool] | None = None,
) -> None:
    """Raise :class:`VoiceNotInteractiveError` if voice capture is not allowed.

    Kept pure and fully injectable so the whole rejection matrix is unit-tested
    without a real terminal, environment, or hardware.
    """
    environ = env if env is not None else os.environ
    stdin_tty = stdin_isatty or _stdin_isatty
    stderr_tty = stderr_isatty or _stderr_isatty

    if json_mode:
        raise VoiceNotInteractiveError(
            "Voice capture is interactive-only and cannot be combined with "
            "--json or --raw.",
            hint="Pass the request as text, or drop --json/--raw to use voice.",
        )
    if no_input:
        raise VoiceNotInteractiveError(
            "Voice capture is interactive-only and cannot be combined with "
            "--no-input.",
            hint="Provide the input as text instead.",
        )
    if _is_truthy_env(environ.get("CI")):
        raise VoiceNotInteractiveError(
            "Voice capture is interactive-only and is disabled in CI.",
            hint="Provide the input as text instead.",
        )
    if (environ.get("TERM") or "").strip().lower() == "dumb":
        raise VoiceNotInteractiveError(
            "Voice capture needs an interactive terminal (TERM=dumb).",
            hint="Provide the input as text instead.",
        )
    if not stdin_tty() or not stderr_tty():
        raise VoiceNotInteractiveError(
            "Voice capture needs an interactive terminal.",
            hint="Provide the input as text instead.",
        )


def _stdin_isatty() -> bool:
    try:
        return bool(sys.stdin.isatty())
    except (AttributeError, OSError, ValueError):
        return False


def _stderr_isatty() -> bool:
    try:
        return bool(sys.stderr.isatty())
    except (AttributeError, OSError, ValueError):
        return False
