"""The central interactive-capability policy for voice (blocker 3)."""
from __future__ import annotations

import pytest

from heyfood_cli.voice_policy import (
    VOICE_NOT_INTERACTIVE,
    VoiceNotInteractiveError,
    ensure_voice_interactive,
)


def _tty(value: bool):
    return lambda: value


def _ok(**overrides):
    kwargs = dict(
        json_mode=False,
        no_input=False,
        env={},
        stdin_isatty=_tty(True),
        stderr_isatty=_tty(True),
    )
    kwargs.update(overrides)
    return kwargs


def test_interactive_tty_is_allowed():
    # Does not raise.
    ensure_voice_interactive(**_ok())


@pytest.mark.parametrize(
    "overrides",
    [
        {"json_mode": True},
        {"no_input": True},
        {"env": {"CI": "true"}},
        {"env": {"CI": "1"}},
        {"env": {"TERM": "dumb"}},
        {"stdin_isatty": lambda: False},
        {"stderr_isatty": lambda: False},
    ],
)
def test_noninteractive_contexts_are_rejected(overrides):
    with pytest.raises(VoiceNotInteractiveError) as excinfo:
        ensure_voice_interactive(**_ok(**overrides))
    # One stable kind for every rejection reason.
    assert excinfo.value.kind == VOICE_NOT_INTERACTIVE


def test_ci_falsey_values_do_not_trip():
    ensure_voice_interactive(**_ok(env={"CI": "0"}))
    ensure_voice_interactive(**_ok(env={"CI": "false"}))
    ensure_voice_interactive(**_ok(env={"CI": ""}))
