"""Capture-mode resolution matrix and the capture-then-review flow."""
from __future__ import annotations

from io import StringIO

import pytest
from rich.console import Console

from heyfood_cli import voice_capture
from heyfood_cli.client import (
    TranscriptionRateLimited,
    TranscriptionRejected,
    TranscriptionScopeRequired,
    TranscriptionUnavailable,
)
from heyfood_cli.voice import VoiceCaptureError, VoiceCaptureResult
from heyfood_cli.voice_capture import (
    BROWSER,
    NATIVE,
    TYPED,
    VoiceCancelled,
    VoiceInputError,
    capture_voice_input,
    resolve_capture_mode,
)
from heyfood_cli.voice_native import InputDevice, NativeCaptureFailed


# --------------------------------------------------------------------------- #
# resolve_capture_mode matrix
# --------------------------------------------------------------------------- #


def test_auto_prefers_native_when_extra_and_device_present():
    plan = resolve_capture_mode(
        "auto", extra_available=True, has_input_device=True, is_ssh=False
    )
    assert plan.mode == NATIVE


def test_auto_falls_to_browser_without_extra():
    plan = resolve_capture_mode(
        "auto", extra_available=False, has_input_device=False, is_ssh=False
    )
    assert plan.mode == BROWSER
    assert "isn't installed" in plan.message


def test_auto_falls_to_browser_when_extra_but_no_device():
    plan = resolve_capture_mode(
        "auto", extra_available=True, has_input_device=False, is_ssh=False
    )
    assert plan.mode == BROWSER


def test_ssh_skips_browser_to_typed():
    plan = resolve_capture_mode(
        "auto", extra_available=False, has_input_device=False, is_ssh=True
    )
    assert plan.mode == TYPED
    assert "SSH" in plan.message


def test_ssh_still_uses_native_when_a_device_exists():
    plan = resolve_capture_mode(
        "auto", extra_available=True, has_input_device=True, is_ssh=True
    )
    assert plan.mode == NATIVE


def test_explicit_native_without_extra_errors():
    with pytest.raises(VoiceInputError) as excinfo:
        resolve_capture_mode(
            "native", extra_available=False, has_input_device=False, is_ssh=False
        )
    assert excinfo.value.kind == "voice_capture_unavailable"


def test_explicit_native_without_device_errors():
    with pytest.raises(VoiceInputError):
        resolve_capture_mode(
            "native", extra_available=True, has_input_device=False, is_ssh=False
        )


def test_explicit_browser_and_typed_pass_through():
    assert resolve_capture_mode(
        "browser", extra_available=True, has_input_device=True, is_ssh=False
    ).mode == BROWSER
    assert resolve_capture_mode(
        "typed", extra_available=True, has_input_device=True, is_ssh=False
    ).mode == TYPED


def test_persisted_mode_used_only_when_request_is_auto():
    persisted_native = resolve_capture_mode(
        "auto",
        extra_available=True,
        has_input_device=True,
        is_ssh=False,
        persisted="typed",
    )
    assert persisted_native.mode == TYPED
    explicit_wins = resolve_capture_mode(
        "browser",
        extra_available=True,
        has_input_device=True,
        is_ssh=False,
        persisted="typed",
    )
    assert explicit_wins.mode == BROWSER


def test_invalid_mode_errors():
    with pytest.raises(VoiceInputError):
        resolve_capture_mode(
            "shout", extra_available=True, has_input_device=True, is_ssh=False
        )


def test_is_ssh_session_reads_env():
    assert voice_capture.is_ssh_session({"SSH_TTY": "/dev/pts/0"}) is True
    assert voice_capture.is_ssh_session({"SSH_CONNECTION": "1 2 3 4"}) is True
    assert voice_capture.is_ssh_session({}) is False


# --------------------------------------------------------------------------- #
# capture_voice_input flow
# --------------------------------------------------------------------------- #


class FakeStream:
    def __init__(self, pcm, *, overflowed=False):
        self._pcm = pcm
        self.overflowed = overflowed
        self.stopped = False

    def start(self):
        pass

    def stop(self):
        self.stopped = True

    def drain(self):
        return self._pcm

    def close(self):
        pass


class FakeBackend:
    def __init__(self, *, available=True, has_device=True, open_fails=False, overflowed=False):
        self._available = available
        self._has_device = has_device
        self._open_fails = open_fails
        self._overflowed = overflowed

    def available(self):
        return self._available

    def list_input_devices(self):
        if not self._has_device:
            return []
        return [InputDevice(0, "Fake Mic", 1, 16_000.0, is_default=True)]

    def resolve_device(self, selector):
        from heyfood_cli.voice_native import NativeCaptureUnavailable

        if not self._has_device:
            raise NativeCaptureUnavailable("no device")
        return InputDevice(0, "Fake Mic", 1, 16_000.0, is_default=True)

    def open(self, *, sample_rate, channels, device):
        from heyfood_cli.voice_native import PortAudioError

        if self._open_fails:
            raise PortAudioError("cannot open")
        return FakeStream(b"\x00\x00" * 1000, overflowed=self._overflowed)


class FakeClient:
    def __init__(self, *, result=None, error=None, scope=True):
        self._result = result or {"transcript": "spoken text", "model_version": "hf-transcribe-1"}
        self._error = error
        self._scope = scope
        self.calls = []

    def voice_settings(self):
        return {}

    def has_transcribe_scope(self):
        return self._scope

    def transcribe_audio(self, wav_bytes, *, purpose, language=None):
        self.calls.append({"purpose": purpose, "language": language, "bytes": len(wav_bytes)})
        if self._error is not None:
            raise self._error
        return self._result


def _prompter(answers):
    seq = list(answers)

    def _ask(message, *, default=""):
        return seq.pop(0) if seq else default

    return _ask


def _stderr():
    buffer = StringIO()
    return Console(file=buffer, force_terminal=False, width=100), buffer


def _run(client, *, backend, prompt, **kwargs):
    console, buffer = _stderr()
    outcome = capture_voice_input(
        client,
        purpose="ask",
        stderr_console=console,
        backend=backend,
        prompt=prompt,
        wait_to_start=lambda: None,
        wait_to_stop=lambda deadline: None,
        **kwargs,
    )
    return outcome, buffer.getvalue()


def _explode(**kw):  # a browser_capture that must never be called
    raise AssertionError("browser capture must not run without consent")


def test_native_success_accepts_transcript():
    client = FakeClient()
    outcome, _ = _run(
        client,
        backend=FakeBackend(),
        prompt=_prompter(["a"]),
    )
    assert outcome.transcript == "spoken text"
    assert outcome.source == NATIVE
    assert outcome.model_version == "hf-transcribe-1"
    assert client.calls[0]["purpose"] == "ask"


def test_native_success_accepts_on_enter_default():
    # An empty answer at review accepts a good-quality capture.
    client = FakeClient()
    outcome, _ = _run(client, backend=FakeBackend(), prompt=_prompter([]))
    assert outcome.transcript == "spoken text"


def test_review_edit_replaces_transcript():
    client = FakeClient()
    outcome, _ = _run(
        client,
        backend=FakeBackend(),
        prompt=_prompter(["e", "corrected words"]),
    )
    assert outcome.transcript == "corrected words"


def test_review_record_again_then_accepts():
    client = FakeClient()
    outcome, _ = _run(
        client,
        backend=FakeBackend(),
        prompt=_prompter(["r", "a"]),
    )
    assert outcome.transcript == "spoken text"
    assert len(client.calls) == 2  # captured twice


def test_review_cancel_raises_voice_cancelled():
    client = FakeClient()
    with pytest.raises(VoiceCancelled):
        _run(client, backend=FakeBackend(), prompt=_prompter(["c"]))


def test_low_quality_capture_cannot_be_accepted_but_can_be_edited():
    client = FakeClient()
    # Overflowed capture: 'accept' is refused; only edit/record/type/cancel.
    outcome, log = _run(
        client,
        backend=FakeBackend(overflowed=True),
        prompt=_prompter(["a", "e", "hand corrected"]),
    )
    # The first 'a' is rejected (quality bad), then edit yields verified text.
    assert outcome.transcript == "hand corrected"
    assert "dropped audio" in log


def test_scope_preflight_blocks_before_recording():
    client = FakeClient(scope=False)
    with pytest.raises(VoiceInputError) as excinfo:
        _run(client, backend=FakeBackend(), prompt=_prompter([]), browser_capture=_explode)
    assert excinfo.value.kind == "insufficient_scope"
    assert "heyfood login" in (excinfo.value.hint or "")
    # Critically, no recording/upload happened before the scope check.
    assert client.calls == []


def test_auto_native_failure_requires_consent_before_browser():
    # Declining the browser consent (default no) goes to typed, never browser.
    client = FakeClient()
    outcome, _ = _run(
        client,
        backend=FakeBackend(open_fails=True),  # native record fails
        prompt=_prompter(["typed fallback", "a"]),
        confirm=lambda message: False,  # decline browser
        browser_capture=_explode,
    )
    assert outcome.source == TYPED
    assert outcome.transcript == "typed fallback"


def test_auto_native_failure_crosses_to_browser_only_with_consent():
    client = FakeClient()
    browser = lambda **kw: VoiceCaptureResult(transcript="from browser")
    outcome, _ = _run(
        client,
        backend=FakeBackend(open_fails=True),
        prompt=_prompter(["a"]),
        confirm=lambda message: True,  # explicit consent
        browser_capture=browser,
    )
    assert outcome.source == BROWSER
    assert outcome.transcript == "from browser"


def test_endpoint_unavailable_recovers_via_consent():
    client = FakeClient(error=TranscriptionUnavailable("503"))
    browser = lambda **kw: VoiceCaptureResult(transcript="browser text")
    outcome, _ = _run(
        client,
        backend=FakeBackend(),
        prompt=_prompter(["a"]),
        confirm=lambda message: True,
        browser_capture=browser,
    )
    assert outcome.source == BROWSER


def test_browser_failure_falls_back_to_typed():
    client = FakeClient()

    def browser(**kw):
        raise VoiceCaptureError("browser timed out")

    outcome, _ = _run(
        client,
        backend=FakeBackend(available=False),  # forces browser rung
        prompt=_prompter(["typed answer", "a"]),
        confirm=lambda message: True,
        browser_capture=browser,
    )
    assert outcome.source == TYPED
    assert outcome.transcript == "typed answer"


def test_explicit_native_failure_recovers_to_typed_never_browser():
    client = FakeClient()
    outcome, _ = _run(
        client,
        backend=FakeBackend(open_fails=True),
        prompt=_prompter(["typed recovery", "a"]),
        requested_mode="native",
        confirm=lambda message: (_ for _ in ()).throw(
            AssertionError("native must never prompt for browser")
        ),
        browser_capture=_explode,
    )
    assert outcome.source == TYPED
    assert outcome.transcript == "typed recovery"


def test_insufficient_scope_from_endpoint_names_relogin():
    # Even if the preflight were bypassed, an endpoint 403 stays terminal.
    client = FakeClient(error=TranscriptionScopeRequired("insufficient_scope"))
    with pytest.raises(VoiceInputError) as excinfo:
        _run(client, backend=FakeBackend(), prompt=_prompter([]))
    assert excinfo.value.kind == "insufficient_scope"
    assert "heyfood login" in (excinfo.value.hint or "")


def test_malformed_success_body_is_terminal_not_a_loop():
    client = FakeClient(result={"transcript": "   "})  # whitespace only
    with pytest.raises(VoiceInputError) as excinfo:
        _run(client, backend=FakeBackend(), prompt=_prompter([]), browser_capture=_explode)
    assert excinfo.value.kind == "transcription_contract_error"


def test_rate_limited_surfaces_retry_and_limit():
    client = FakeClient(error=TranscriptionRateLimited("rate_limited", retry_after=42))
    with pytest.raises(VoiceInputError) as excinfo:
        _run(client, backend=FakeBackend(), prompt=_prompter([]))
    assert excinfo.value.kind == "rate_limited"
    assert "42" in str(excinfo.value)


def test_rejected_audio_surfaces_limits():
    client = FakeClient(error=TranscriptionRejected("audio_too_long"))
    with pytest.raises(VoiceInputError) as excinfo:
        _run(client, backend=FakeBackend(), prompt=_prompter([]))
    assert excinfo.value.kind == "audio_rejected"
    assert "120" in (excinfo.value.hint or "")


def test_typed_mode_never_records_or_transcribes():
    client = FakeClient()
    outcome, _ = _run(
        client,
        backend=FakeBackend(),
        prompt=_prompter(["typed only", "a"]),
        requested_mode="typed",
    )
    assert outcome.source == TYPED
    assert client.calls == []


def test_non_tty_is_rejected():
    client = FakeClient()
    console, _ = _stderr()
    with pytest.raises(VoiceInputError) as excinfo:
        capture_voice_input(
            client,
            purpose="ask",
            stderr_console=console,
            backend=FakeBackend(),
            prompt=_prompter(["a"]),
            is_tty=False,
        )
    assert excinfo.value.kind == "voice_requires_tty"


def test_capture_ui_stays_off_stdout(capsys):
    client = FakeClient()
    outcome, _ = _run(client, backend=FakeBackend(), prompt=_prompter(["a"]))
    captured = capsys.readouterr()
    # Every prompt/status line went to the injected stderr console, not stdout.
    assert captured.out == ""


def test_describe_devices_without_extra_is_graceful():
    payload = voice_capture.describe_devices(FakeBackend(available=False))
    assert payload["available"] is False
    assert payload["devices"] == []


def test_describe_devices_lists_inputs():
    payload = voice_capture.describe_devices(FakeBackend())
    assert payload["available"] is True
    assert payload["devices"][0]["name"] == "Fake Mic"
    assert payload["devices"][0]["is_default"] is True
