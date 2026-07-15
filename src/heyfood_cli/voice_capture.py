"""Capture-mode resolution and the shared capture-then-review voice flow.

Three capture rungs, tried in this order under ``auto``:

    1. native microphone  -> WAV -> transcription endpoint  (most private)
    2. browser capture     (localhost page, third-party speech service)
    3. typed input         (final fallback, always works)

``resolve_capture_mode`` is pure so the whole resolution matrix is unit-tested
without hardware or network. ``capture_voice_input`` runs the chosen rung, keeps
every prompt on stderr (stdout stays a clean data channel), and enforces the
mandatory transcript review before any transcript is returned to a command.
"""
from __future__ import annotations

import os
import select
import sys
import time
from dataclasses import dataclass
from typing import Any, Callable

from rich.panel import Panel
from rich.prompt import Prompt

from .client import (
    HelloFoodError,
    LoginRequired,
    TranscriptionRateLimited,
    TranscriptionRejected,
    TranscriptionScopeRequired,
    TranscriptionUnavailable,
)
from . import voice_native
from .voice import VoiceCaptureError, capture_voice_transcript


AUTO = "auto"
NATIVE = "native"
BROWSER = "browser"
TYPED = "typed"
VALID_MODES = (AUTO, NATIVE, BROWSER, TYPED)

# purpose enum values understood by the transcription endpoint.
PURPOSE_ONBOARDING = "onboarding"
PURPOSE_ASK = "ask"
PURPOSE_LOG = "log"

_INSTALL_HINT = "pipx install 'heyfood-cli[voice]'"


class VoiceInputError(RuntimeError):
    """A terminal, user-facing voice failure a command should surface via _fail."""

    def __init__(self, message: str, *, kind: str, hint: str | None = None):
        super().__init__(message)
        self.kind = kind
        self.hint = hint


@dataclass(frozen=True)
class CapturePlan:
    """The resolved starting rung plus an honest note about why."""

    mode: str
    reason: str
    message: str | None = None


@dataclass
class VoiceOutcome:
    """A reviewed transcript, plus where it came from."""

    transcript: str
    source: str
    model_version: str | None = None
    duration_seconds: float | None = None


def is_ssh_session(env: dict[str, str] | None = None) -> bool:
    """True inside an SSH session, where localhost browser capture can't reach
    the user's microphone without manual port-forwarding."""
    environ = env if env is not None else os.environ
    return bool(environ.get("SSH_TTY") or environ.get("SSH_CONNECTION"))


def resolve_capture_mode(
    requested: str,
    *,
    extra_available: bool,
    has_input_device: bool,
    is_ssh: bool,
    persisted: str | None = None,
) -> CapturePlan:
    """Pick the starting capture rung.

    An explicit ``--voice-capture native`` that cannot run raises
    :class:`VoiceInputError` rather than silently degrading. ``auto`` (or a
    persisted default) walks the chain: native if usable, then browser — skipped
    entirely over SSH — then typed.
    """
    requested = (requested or AUTO).strip().lower()
    if requested not in VALID_MODES:
        raise VoiceInputError(
            f"Unknown capture mode '{requested}'. Choose auto, native, browser, or typed.",
            kind="invalid_voice_mode",
        )

    # An explicit flag wins over any persisted default; a persisted concrete mode
    # only takes effect when the caller left the mode on auto.
    effective = requested
    if requested == AUTO and persisted in (NATIVE, BROWSER, TYPED):
        effective = persisted

    if effective == NATIVE:
        if not extra_available:
            raise VoiceInputError(
                "Native voice capture needs the optional 'voice' extra.",
                kind="voice_capture_unavailable",
                hint=f"Install it with: {_INSTALL_HINT} — or use --voice-capture browser.",
            )
        if not has_input_device:
            raise VoiceInputError(
                "No microphone input device was found on this machine.",
                kind="voice_capture_unavailable",
                hint="Use --voice-capture browser or --voice-capture typed.",
            )
        return CapturePlan(mode=NATIVE, reason="native_selected")

    if effective == BROWSER:
        if is_ssh:
            return CapturePlan(
                mode=BROWSER,
                reason="browser_selected_ssh",
                message=(
                    "Over SSH the browser capture server binds on the remote host; "
                    "you may need to port-forward it to reach your microphone."
                ),
            )
        return CapturePlan(mode=BROWSER, reason="browser_selected")

    if effective == TYPED:
        return CapturePlan(mode=TYPED, reason="typed_selected")

    # auto
    if extra_available and has_input_device:
        return CapturePlan(mode=NATIVE, reason="auto_native")
    if is_ssh:
        return CapturePlan(
            mode=TYPED,
            reason="auto_ssh_typed",
            message=(
                "Over SSH, browser capture can't reach your local microphone, "
                "so voice falls back to typed input."
            ),
        )
    if not extra_available:
        return CapturePlan(
            mode=BROWSER,
            reason="auto_browser_no_extra",
            message=(
                "Native microphone capture isn't installed; using browser capture. "
                f"For local mic capture: {_INSTALL_HINT}."
            ),
        )
    return CapturePlan(
        mode=BROWSER,
        reason="auto_browser_no_device",
        message="No microphone was found; using browser capture.",
    )


def _default_wait_to_start(stderr_console, *, stdin=None) -> None:
    stream = stdin if stdin is not None else sys.stdin
    stderr_console.print("[bold]Press Enter to start speaking.[/bold]")
    stream.readline()


def _default_wait_to_stop(deadline: float, *, stdin=None) -> None:
    """Return when the user presses Enter or the auto-stop deadline passes.

    Uses ``select`` on the TTY so the hard cap can fire without a thread left
    blocked on ``readline`` (which would then steal the review keystroke).
    """
    stream = stdin if stdin is not None else sys.stdin
    end = time.monotonic() + max(0.0, deadline)
    while True:
        remaining = end - time.monotonic()
        if remaining <= 0:
            return
        try:
            ready, _, _ = select.select([stream], [], [], remaining)
        except (OSError, ValueError):
            # No selectable TTY (piped stdin); fall back to a plain blocking read.
            stream.readline()
            return
        if ready:
            stream.readline()
            return


def _has_input_device(backend: voice_native.MicrophoneBackend) -> bool:
    if not backend.available():
        return False
    try:
        return bool(backend.list_input_devices())
    except Exception:
        return False


def capture_voice_input(
    client: Any,
    *,
    purpose: str,
    requested_mode: str = AUTO,
    device: int | str | None = None,
    language: str | None = None,
    stderr_console,
    is_tty: bool = True,
    browser_timeout: int = 300,
    open_browser: bool = True,
    persisted_mode: str | None = None,
    env: dict[str, str] | None = None,
    backend: voice_native.MicrophoneBackend | None = None,
    browser_capture: Callable[..., Any] | None = None,
    prompt: Callable[..., str] | None = None,
    wait_to_start: Callable[[], None] | None = None,
    wait_to_stop: Callable[[float], None] | None = None,
) -> VoiceOutcome:
    """Capture a transcript through the resolved rung, review it, and return it.

    Raises :class:`VoiceInputError` for terminal failures a command should print,
    and only ever returns a transcript the user has explicitly confirmed.
    """
    if not is_tty:
        raise VoiceInputError(
            "Voice capture needs an interactive terminal.",
            kind="voice_requires_tty",
            hint="Provide the input as text instead.",
        )

    backend = backend or voice_native.SoundDeviceBackend()
    browser_capture = browser_capture or capture_voice_transcript
    ssh = is_ssh_session(env)

    def _ask(message: str, *, default: str = "") -> str:
        if prompt is not None:
            return prompt(message, default=default)
        return Prompt.ask(message, console=stderr_console, default=default)

    plan = resolve_capture_mode(
        requested_mode,
        extra_available=backend.available(),
        has_input_device=_has_input_device(backend),
        is_ssh=ssh,
        persisted=persisted_mode,
    )
    current = plan.mode
    notice = plan.message

    while True:
        if notice:
            stderr_console.print(f"[dim]{notice}[/dim]")
            notice = None

        if current == NATIVE:
            transcript, meta, retry = _run_native(
                client,
                backend=backend,
                device=device,
                purpose=purpose,
                language=language,
                stderr_console=stderr_console,
                requested_native=(requested_mode.strip().lower() == NATIVE),
                wait_to_start=wait_to_start,
                wait_to_stop=wait_to_stop,
            )
            if retry is not None:
                stderr_console.print(f"[yellow]{retry}[/yellow]")
                current = TYPED if ssh else BROWSER
                continue
            source = NATIVE
        elif current == BROWSER:
            transcript, fell_back = _run_browser(
                browser_capture,
                stderr_console=stderr_console,
                timeout=browser_timeout,
                open_browser=open_browser,
            )
            if fell_back:
                current = TYPED
                continue
            meta = {}
            source = BROWSER
        else:  # TYPED
            transcript = _run_typed(_ask)
            meta = {}
            source = TYPED

        if not transcript.strip():
            stderr_console.print("[yellow]No transcript captured. Let's try again.[/yellow]")
            continue

        action, text = _review(transcript, stderr_console=stderr_console, ask=_ask)
        if action == "retry":
            continue
        return VoiceOutcome(
            transcript=text,
            source=source,
            model_version=(meta.get("model_version") if isinstance(meta, dict) else None),
            duration_seconds=(
                meta.get("duration_seconds") if isinstance(meta, dict) else None
            ),
        )


def _run_native(
    client: Any,
    *,
    backend: voice_native.MicrophoneBackend,
    device: int | str | None,
    purpose: str,
    language: str | None,
    stderr_console,
    requested_native: bool,
    wait_to_start: Callable[[], None] | None,
    wait_to_stop: Callable[[float], None] | None,
) -> tuple[str, dict[str, Any], str | None]:
    """Record + transcribe. Returns (transcript, meta, retry_notice).

    ``retry_notice`` is non-None when the caller should fall back a rung (auto
    only). Terminal, user-facing failures raise :class:`VoiceInputError`.
    """
    start = wait_to_start or (lambda: _default_wait_to_start(stderr_console))
    stop = wait_to_stop or (lambda deadline: _default_wait_to_stop(deadline))

    def _on_record_start(rate: int, deadline: float) -> None:
        stderr_console.print(
            f"[bold]● Recording...[/bold] press Enter to stop "
            f"[dim](auto-stops at {int(deadline)}s)[/dim]"
        )

    try:
        recording = voice_native.capture_recording(
            backend=backend,
            device=device,
            wait_to_start=start,
            wait_to_stop=stop,
            on_record_start=_on_record_start,
        )
    except voice_native.NativeCaptureUnavailable as exc:
        if requested_native:
            raise VoiceInputError(
                str(exc),
                kind="voice_capture_unavailable",
                hint=f"Install with {_INSTALL_HINT}, or use --voice-capture browser.",
            ) from exc
        return "", {}, f"{exc} Falling back."
    except voice_native.NativeCaptureFailed as exc:
        if requested_native:
            raise VoiceInputError(
                str(exc),
                kind="voice_capture_failed",
                hint="Try --voice-capture browser or --voice-capture typed.",
            ) from exc
        return "", {}, f"{exc} Falling back."

    if recording.truncated:
        stderr_console.print(
            "[yellow]Recording reached the length limit and was trimmed.[/yellow]"
        )
    stderr_console.print("[dim]Transcribing...[/dim]")
    try:
        result = client.transcribe_audio(
            recording.wav_bytes,
            purpose=purpose,
            language=language,
        )
    except TranscriptionScopeRequired as exc:
        raise VoiceInputError(
            "Your login is missing voice permission.",
            kind="insufficient_scope",
            hint="Run `heyfood login` again to grant voice access.",
        ) from exc
    except TranscriptionRateLimited as exc:
        detail = str(exc)
        if exc.retry_after:
            detail = f"{detail} Try again in {exc.retry_after}s."
        raise VoiceInputError(
            detail,
            kind="rate_limited",
            hint="The transcription limit is 20 recordings per hour.",
        ) from exc
    except TranscriptionRejected as exc:
        raise VoiceInputError(
            str(exc),
            kind="audio_rejected",
            hint="Recordings are limited to 120 seconds and 12.5 MB.",
        ) from exc
    except TranscriptionUnavailable as exc:
        return "", {}, f"{exc} Falling back to browser capture."
    except LoginRequired as exc:
        raise VoiceInputError(
            str(exc),
            kind="login_required",
            hint="Run `heyfood login` and retry.",
        ) from exc
    except HelloFoodError as exc:
        raise VoiceInputError(
            str(exc),
            kind="transcription_error",
        ) from exc

    transcript = str(result.get("transcript") or "").strip()
    return transcript, result, None


def _run_browser(
    browser_capture: Callable[..., Any],
    *,
    stderr_console,
    timeout: int,
    open_browser: bool,
) -> tuple[str, bool]:
    """Run browser capture. Returns (transcript, fell_back_to_typed)."""
    try:
        result = browser_capture(
            timeout_seconds=timeout,
            open_browser=open_browser,
            url_callback=lambda url: stderr_console.print(
                f"[dim]Voice capture URL:[/dim] {url}"
            ),
        )
    except VoiceCaptureError as exc:
        stderr_console.print(
            f"[yellow]{exc} Falling back to typed input.[/yellow]"
        )
        return "", True
    transcript = getattr(result, "transcript", "") or ""
    return str(transcript), False


def _run_typed(ask: Callable[..., str]) -> str:
    return ask("Type your input", default="").strip()


def _review(
    transcript: str,
    *,
    stderr_console,
    ask: Callable[..., str],
) -> tuple[str, str]:
    """Mandatory transcript review. Returns ('accept', text) or ('retry', '')."""
    stderr_console.print(
        Panel(transcript, title="Transcript", border_style="green")
    )
    answer = ask("Use this transcript? [Y/n/e]", default="y").strip().lower()
    if answer in ("", "y", "yes"):
        return "accept", transcript
    if answer in ("e", "edit"):
        edited = ask("Edit transcript", default=transcript).strip()
        if not edited:
            return "retry", ""
        return "accept", edited
    return "retry", ""


def describe_devices(backend: voice_native.MicrophoneBackend | None = None) -> dict[str, Any]:
    """Build the ``voice devices`` payload, or a graceful not-installed notice."""
    backend = backend or voice_native.SoundDeviceBackend()
    if not backend.available():
        return {
            "available": False,
            "reason": "extra_not_installed",
            "message": (
                "Native voice capture isn't installed. Enable it with: "
                f"{_INSTALL_HINT}"
            ),
            "devices": [],
        }
    devices = backend.list_input_devices()
    return {
        "available": True,
        "devices": [
            {
                "index": device.index,
                "name": device.name,
                "max_input_channels": device.max_input_channels,
                "default_samplerate": device.default_samplerate,
                "is_default": device.is_default,
            }
            for device in devices
        ],
    }
