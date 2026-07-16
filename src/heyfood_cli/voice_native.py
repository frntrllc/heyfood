"""Native microphone capture: record a short WAV in-terminal, no browser.

The recorder is hidden behind the :class:`MicrophoneBackend` seam so the whole
pipeline can be exercised with a fake in tests — zero audio hardware, zero
network. The concrete :class:`SoundDeviceBackend` is the only place that touches
the optional ``sounddevice`` dependency, and it imports it lazily so the core
CLI stays dependency-light and Linux hosts without ``libportaudio2`` degrade to
the browser/typed fallbacks instead of crashing at import time.

Every byte captured here stays in memory: PCM frames are assembled into a WAV
and handed back to the caller. Nothing is written to disk.
"""
from __future__ import annotations

import io
import queue
import wave
from dataclasses import dataclass
from typing import Callable, Protocol


# Capture format and limits. All of these are sourced from the single canonical
# transcription contract (see ``transcription_contract`` and
# ``schemas/v1/transcription.schema.json``) so the recorder can never negotiate a
# rate, duration, or size the server would reject. The sample rate is negotiated
# at open time (preferred rate first, then the device default, then the window
# ceiling), but a rate outside the accepted window is never opened.
from .transcription_contract import (  # noqa: E402
    CHANNELS as CHANNELS,
    MAX_AUDIO_BYTES as MAX_WAV_BYTES,
    MAX_DURATION_SECONDS as MAX_DURATION_SECONDS,
    PREFERRED_SAMPLE_RATE_HZ as DEFAULT_SAMPLE_RATE,
    SAMPLE_RATE_MAX_HZ as SAMPLE_RATE_MAX_HZ,
    SAMPLE_RATE_MIN_HZ as SAMPLE_RATE_MIN_HZ,
    SAMPLE_WIDTH_BYTES as SAMPLE_WIDTH,
    WAV_HEADER_BYTES as WAV_HEADER_BYTES,
)


class NativeCaptureError(RuntimeError):
    """Base error for native microphone capture."""


class NativeCaptureUnavailable(NativeCaptureError):
    """The native path cannot run here (extra missing or no input device).

    Distinct from :class:`NativeCaptureFailed` because ``auto`` mode treats this
    as "try the next rung", whereas an explicit ``--voice-capture native`` turns
    it into a clear error rather than a silent downgrade.
    """


class NativeCaptureFailed(NativeCaptureError):
    """Capture started but could not complete (device/stream failure)."""


class PortAudioError(NativeCaptureError):
    """A backend stream-open failure, retryable at a different sample rate.

    ``SoundDeviceBackend`` maps ``sounddevice.PortAudioError`` onto this so the
    negotiation loop never has to import the optional dependency to catch it.
    """


@dataclass(frozen=True)
class InputDevice:
    """A selectable microphone as reported by the backend."""

    index: int
    name: str
    max_input_channels: int
    default_samplerate: float
    is_default: bool = False


@dataclass(frozen=True)
class Recording:
    """A completed capture, ready to upload."""

    wav_bytes: bytes
    sample_rate: int
    duration_seconds: float
    truncated: bool
    overflowed: bool


class MicrophoneStream(Protocol):
    """An open input stream. Frames accumulate off the callback thread."""

    @property
    def overflowed(self) -> bool:
        """True if the driver reported at least one dropped/overflowed block."""

    def start(self) -> None:
        ...

    def stop(self) -> None:
        """Halt capture before draining. Optional; fakes may omit it."""

    def drain(self) -> bytes:
        """Return every buffered PCM frame captured so far and clear the buffer."""

    def close(self) -> None:
        ...


class MicrophoneBackend(Protocol):
    """Seam over the audio library. Tests provide a hardware-free fake."""

    def available(self) -> bool:
        """True if the capture dependency is importable on this host."""

    def list_input_devices(self) -> list[InputDevice]:
        ...

    def resolve_device(self, selector: int | str | None) -> InputDevice:
        """Resolve a selector (index, name, or None for default) to a device.

        Raises :class:`NativeCaptureUnavailable` when no input device matches.
        """

    def open(
        self,
        *,
        sample_rate: int,
        channels: int,
        device: int | None,
    ) -> MicrophoneStream:
        """Open a stream. Raises :class:`PortAudioError` on an unsupported rate."""


def effective_capacity_seconds(
    sample_rate: int,
    *,
    channels: int = CHANNELS,
    max_bytes: int = MAX_WAV_BYTES,
    max_duration_seconds: int = MAX_DURATION_SECONDS,
) -> float:
    """Largest recording length that stays under both the time and byte caps.

    At 16-48 kHz the duration cap binds (a 120 s mono int16 clip at 48 kHz is
    ~11.5 MB, under the 12.5 MB wire limit). Above 48 kHz the byte cap binds and
    this returns a shorter length so the finished WAV never 413s the endpoint.
    """
    usable = max(0, max_bytes - WAV_HEADER_BYTES)
    frames_by_bytes = usable // (channels * SAMPLE_WIDTH)
    seconds_by_bytes = frames_by_bytes / sample_rate if sample_rate > 0 else 0.0
    return min(float(max_duration_seconds), seconds_by_bytes)


def pcm_to_wav(
    pcm: bytes,
    *,
    sample_rate: int,
    channels: int = CHANNELS,
    sample_width: int = SAMPLE_WIDTH,
) -> bytes:
    """Wrap raw PCM frames in a WAV container, entirely in memory."""
    buffer = io.BytesIO()
    with wave.open(buffer, "wb") as wav_file:
        wav_file.setnchannels(channels)
        wav_file.setsampwidth(sample_width)
        wav_file.setframerate(sample_rate)
        wav_file.writeframes(pcm)
    return buffer.getvalue()


def _open_negotiated(
    backend: MicrophoneBackend,
    *,
    device_index: int | None,
    requested_sample_rate: int | None,
    channels: int,
    default_sample_rate: int,
) -> tuple[MicrophoneStream, int]:
    """Open the stream at a rate inside the accepted 8-48 kHz window.

    Candidate rates are the requested rate, the preferred rate, the device
    default, and the window ceiling — but only those inside the server's accepted
    window are ever tried. A device whose native rate is above 48 kHz (many pro
    interfaces default to 96 kHz) is negotiated down to an in-window rate rather
    than producing a WAV the server would reject; if no in-window rate opens, the
    capture fails cleanly instead of uploading an out-of-contract clip. A
    ``PortAudioError`` at one rate is an expected condition, not a bug — try the
    next in-window candidate.
    """
    candidates: list[int] = []
    for rate in (
        requested_sample_rate,
        DEFAULT_SAMPLE_RATE,
        default_sample_rate,
        SAMPLE_RATE_MAX_HZ,
    ):
        if not rate:
            continue
        value = int(rate)
        if not (SAMPLE_RATE_MIN_HZ <= value <= SAMPLE_RATE_MAX_HZ):
            # Never open outside the accepted window — a 96 kHz stream would
            # yield a WAV the endpoint rejects.
            continue
        if value not in candidates:
            candidates.append(value)
    if not candidates:
        raise NativeCaptureFailed(
            "This microphone has no supported sample rate within the accepted "
            f"{SAMPLE_RATE_MIN_HZ // 1000}-{SAMPLE_RATE_MAX_HZ // 1000} kHz range."
        )
    last_error: Exception | None = None
    for rate in candidates:
        try:
            stream = backend.open(
                sample_rate=rate,
                channels=channels,
                device=device_index,
            )
        except PortAudioError as exc:
            last_error = exc
            continue
        return stream, rate
    raise NativeCaptureFailed(
        "Could not open the microphone at any supported sample rate."
    ) from last_error


def _stop_stream(stream: "MicrophoneStream") -> None:
    """Stop a stream before draining, if it exposes an optional ``stop``.

    Kept tolerant so hardware-free fakes that only implement start/drain/close
    still work; the concrete backend defines ``stop`` and halts the callback
    before the buffer is drained.
    """
    stopper = getattr(stream, "stop", None)
    if callable(stopper):
        try:
            stopper()
        except Exception:  # pragma: no cover - stop is best-effort before drain
            pass


def capture_recording(
    *,
    backend: MicrophoneBackend,
    wait_to_start: Callable[[], None],
    wait_to_stop: Callable[[float], None],
    device: int | str | None = None,
    requested_sample_rate: int | None = DEFAULT_SAMPLE_RATE,
    channels: int = CHANNELS,
    max_duration_seconds: int = MAX_DURATION_SECONDS,
    max_bytes: int = MAX_WAV_BYTES,
    on_record_start: Callable[[int, float], None] | None = None,
) -> Recording:
    """Record one clip: wait for start, capture until stop, return WAV bytes.

    ``wait_to_start`` blocks until the user is ready (Enter). ``wait_to_stop`` is
    given the auto-stop deadline in seconds and must return when the user stops
    (Enter) or the deadline passes. The stream is always torn down, including on
    ``KeyboardInterrupt``, so Ctrl-C never leaves the device open.
    """
    if not backend.available():
        raise NativeCaptureUnavailable(
            "Native voice capture needs the optional 'voice' extra."
        )
    info = backend.resolve_device(device)

    # The explicit start acknowledgement happens BEFORE the device is opened, so
    # no microphone stream is created and no OS permission prompt is triggered
    # until the user has taken an affirmative action.
    wait_to_start()

    stream, used_rate = _open_negotiated(
        backend,
        device_index=info.index,
        requested_sample_rate=requested_sample_rate,
        channels=channels,
        default_sample_rate=int(info.default_samplerate or DEFAULT_SAMPLE_RATE),
    )
    deadline = effective_capacity_seconds(
        used_rate,
        channels=channels,
        max_bytes=max_bytes,
        max_duration_seconds=max_duration_seconds,
    )
    overflowed = False
    try:
        stream.start()
        if on_record_start is not None:
            on_record_start(used_rate, deadline)
        wait_to_stop(deadline)
        # Stop the stream before draining so no frames arrive mid-drain and the
        # captured buffer is deterministic.
        _stop_stream(stream)
        pcm = stream.drain()
        overflowed = stream.overflowed
    finally:
        stream.close()

    frame_bytes = channels * SAMPLE_WIDTH
    max_frames = int(deadline * used_rate)
    truncated = len(pcm) > max_frames * frame_bytes
    if truncated:
        pcm = pcm[: max_frames * frame_bytes]
    # Never emit a partial trailing frame.
    pcm = pcm[: (len(pcm) // frame_bytes) * frame_bytes]
    frames = len(pcm) // frame_bytes
    if frames == 0:
        raise NativeCaptureFailed(
            "No audio was captured. Try again, or type your input instead."
        )
    return Recording(
        wav_bytes=pcm_to_wav(pcm, sample_rate=used_rate, channels=channels),
        sample_rate=used_rate,
        duration_seconds=frames / used_rate,
        truncated=truncated,
        overflowed=overflowed,
    )


def list_input_devices(backend: MicrophoneBackend | None = None) -> list[InputDevice]:
    """Enumerate input devices, or raise if the extra is not installed."""
    backend = backend or SoundDeviceBackend()
    if not backend.available():
        raise NativeCaptureUnavailable(
            "Native voice capture needs the optional 'voice' extra."
        )
    return backend.list_input_devices()


class _SoundDeviceStream:
    """Concrete stream backed by ``sounddevice.RawInputStream``."""

    def __init__(self, sd_module, *, sample_rate: int, channels: int, device: int | None):
        self._queue: "queue.Queue[bytes]" = queue.Queue()
        self._overflowed = False
        self._stream = sd_module.RawInputStream(
            samplerate=sample_rate,
            channels=channels,
            dtype="int16",
            device=device,
            callback=self._callback,
        )

    def _callback(self, indata, frames, time_info, status) -> None:  # noqa: ANN001
        # The callback runs on the driver thread: it must not block and must copy
        # the buffer, since the driver reuses it after the call returns.
        if status:
            self._overflowed = True
        self._queue.put(bytes(indata))

    @property
    def overflowed(self) -> bool:
        return self._overflowed

    def start(self) -> None:
        self._stream.start()

    def stop(self) -> None:
        try:
            self._stream.stop()
        except Exception:  # pragma: no cover - stop is best-effort before drain
            pass

    def drain(self) -> bytes:
        chunks: list[bytes] = []
        while True:
            try:
                chunks.append(self._queue.get_nowait())
            except queue.Empty:
                break
        return b"".join(chunks)

    def close(self) -> None:
        try:
            self._stream.stop()
            self._stream.close()
        except Exception:  # pragma: no cover - teardown must never raise
            pass


class SoundDeviceBackend:
    """Default :class:`MicrophoneBackend`, lazily bound to ``sounddevice``."""

    def __init__(self) -> None:
        self._sd = None

    def _module(self):
        if self._sd is None:
            import sounddevice  # noqa: PLC0415 - optional, imported on demand

            self._sd = sounddevice
        return self._sd

    def available(self) -> bool:
        try:
            self._module()
        except Exception:
            # ImportError (extra missing) or OSError (no libportaudio2) both mean
            # "native capture cannot run here" — an expected runtime condition.
            return False
        return True

    def list_input_devices(self) -> list[InputDevice]:
        sd = self._module()
        default_input = None
        try:
            default_input = sd.default.device[0]
        except Exception:
            default_input = None
        devices: list[InputDevice] = []
        for index, info in enumerate(sd.query_devices()):
            if int(info.get("max_input_channels", 0)) <= 0:
                continue
            devices.append(
                InputDevice(
                    index=index,
                    name=str(info.get("name", f"device {index}")),
                    max_input_channels=int(info.get("max_input_channels", 0)),
                    default_samplerate=float(
                        info.get("default_samplerate", DEFAULT_SAMPLE_RATE)
                    ),
                    is_default=(index == default_input),
                )
            )
        return devices

    def resolve_device(self, selector: int | str | None) -> InputDevice:
        devices = self.list_input_devices()
        if not devices:
            raise NativeCaptureUnavailable(
                "No microphone input device was found on this machine."
            )
        if selector is None or selector == "":
            for device in devices:
                if device.is_default:
                    return device
            return devices[0]
        if isinstance(selector, int) or (
            isinstance(selector, str) and selector.isdigit()
        ):
            wanted = int(selector)
            for device in devices:
                if device.index == wanted:
                    return device
            raise NativeCaptureUnavailable(
                f"No input device with index {wanted}."
            )
        needle = str(selector).strip().lower()
        for device in devices:
            if needle in device.name.lower():
                return device
        raise NativeCaptureUnavailable(
            f"No input device matching '{selector}'."
        )

    def open(
        self,
        *,
        sample_rate: int,
        channels: int,
        device: int | None,
    ) -> MicrophoneStream:
        sd = self._module()
        try:
            return _SoundDeviceStream(
                sd,
                sample_rate=sample_rate,
                channels=channels,
                device=device,
            )
        except sd.PortAudioError as exc:
            raise PortAudioError(str(exc)) from exc
