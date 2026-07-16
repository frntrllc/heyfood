"""Native capture tests. No audio hardware: the microphone is a fake backend."""
from __future__ import annotations

import io
import wave

import pytest

from heyfood_cli import voice_native
from heyfood_cli.voice_native import (
    InputDevice,
    NativeCaptureFailed,
    NativeCaptureUnavailable,
    PortAudioError,
    capture_recording,
    effective_capacity_seconds,
    pcm_to_wav,
)


class FakeStream:
    def __init__(self, pcm: bytes, *, overflowed: bool = False):
        self._pcm = pcm
        self._drained = False
        self.overflowed = overflowed
        self.started = False
        self.stopped = False
        self.closed = False

    def start(self) -> None:
        self.started = True

    def stop(self) -> None:
        self.stopped = True

    def drain(self) -> bytes:
        if self._drained:
            return b""
        self._drained = True
        return self._pcm

    def close(self) -> None:
        self.closed = True


class FakeBackend:
    def __init__(
        self,
        *,
        available: bool = True,
        devices=None,
        pcm: bytes = b"",
        overflowed: bool = False,
        unsupported_rates=(),
    ):
        self._available = available
        self._devices = (
            devices
            if devices is not None
            else [InputDevice(0, "Fake Mic", 1, 16_000.0, is_default=True)]
        )
        self._pcm = pcm
        self._overflowed = overflowed
        self._unsupported = set(unsupported_rates)
        self.opened_rates: list[int] = []
        self.streams: list[FakeStream] = []

    def available(self) -> bool:
        return self._available

    def list_input_devices(self):
        return list(self._devices)

    def resolve_device(self, selector):
        if not self._devices:
            raise NativeCaptureUnavailable("no device")
        if selector in (None, ""):
            return self._devices[0]
        if isinstance(selector, int) or str(selector).isdigit():
            for device in self._devices:
                if device.index == int(selector):
                    return device
            raise NativeCaptureUnavailable(f"no device {selector}")
        for device in self._devices:
            if str(selector).lower() in device.name.lower():
                return device
        raise NativeCaptureUnavailable(f"no device {selector}")

    def open(self, *, sample_rate, channels, device):
        self.opened_rates.append(sample_rate)
        if sample_rate in self._unsupported:
            raise PortAudioError(f"rate {sample_rate} unsupported")
        stream = FakeStream(self._pcm, overflowed=self._overflowed)
        self.streams.append(stream)
        return stream


def _silence(frames: int) -> bytes:
    return b"\x00\x00" * frames


def test_out_of_window_device_rate_is_negotiated_down_never_opened():
    # A 96 kHz-default device is negotiated to an in-window rate (48 kHz), and
    # the out-of-window rate is never opened (a 96 kHz WAV would be rejected).
    backend = FakeBackend(
        pcm=_silence(10),
        devices=[InputDevice(0, "Pro Interface", 1, 96_000.0, is_default=True)],
    )
    recording = capture_recording(
        backend=backend,
        wait_to_start=lambda: None,
        wait_to_stop=lambda deadline: None,
    )
    assert 96_000 not in backend.opened_rates
    assert all(8_000 <= rate <= 48_000 for rate in backend.opened_rates)
    assert 8_000 <= recording.sample_rate <= 48_000


def test_out_of_window_only_device_fails_rather_than_uploading():
    # If the only rate the device supports is above the window, capture fails
    # cleanly instead of producing a WAV the server rejects.
    backend = FakeBackend(
        pcm=_silence(10),
        devices=[InputDevice(0, "192k only", 1, 96_000.0, is_default=True)],
        unsupported_rates=(8_000, 16_000, 44_100, 48_000),
    )
    with pytest.raises(NativeCaptureFailed):
        capture_recording(
            backend=backend,
            wait_to_start=lambda: None,
            wait_to_stop=lambda deadline: None,
        )
    assert all(rate <= 48_000 for rate in backend.opened_rates)


def test_stream_is_stopped_before_drain():
    backend = FakeBackend(pcm=_silence(100))
    capture_recording(
        backend=backend,
        wait_to_start=lambda: None,
        wait_to_stop=lambda deadline: None,
    )
    # The concrete fake exposes a stop() the recorder must call before draining.
    assert backend.streams[0].stopped is True


def test_start_acknowledgement_happens_before_device_open():
    order: list[str] = []
    backend = FakeBackend(pcm=_silence(100))
    original_open = backend.open

    def _tracking_open(**kwargs):
        order.append("open")
        return original_open(**kwargs)

    backend.open = _tracking_open  # type: ignore[assignment]
    capture_recording(
        backend=backend,
        wait_to_start=lambda: order.append("ack"),
        wait_to_stop=lambda deadline: None,
    )
    assert order[0] == "ack"
    assert "open" in order and order.index("ack") < order.index("open")


def test_pcm_to_wav_round_trips_mono_int16():
    pcm = _silence(1000)
    wav = pcm_to_wav(pcm, sample_rate=16_000)
    with wave.open(io.BytesIO(wav), "rb") as reader:
        assert reader.getnchannels() == 1
        assert reader.getsampwidth() == 2
        assert reader.getframerate() == 16_000
        assert reader.getnframes() == 1000


def test_capture_recording_returns_valid_wav():
    pcm = _silence(8_000)  # 0.5s at 16kHz
    backend = FakeBackend(pcm=pcm)
    recording = capture_recording(
        backend=backend,
        wait_to_start=lambda: None,
        wait_to_stop=lambda deadline: None,
    )
    assert recording.sample_rate == 16_000
    assert recording.truncated is False
    assert abs(recording.duration_seconds - 0.5) < 1e-6
    with wave.open(io.BytesIO(recording.wav_bytes), "rb") as reader:
        assert reader.getnframes() == 8_000
    assert backend.streams[0].started is True
    assert backend.streams[0].closed is True


def test_capture_negotiates_device_rate_when_16k_unsupported():
    backend = FakeBackend(
        pcm=_silence(4_000),
        devices=[InputDevice(0, "Studio", 2, 44_100.0, is_default=True)],
        unsupported_rates=(16_000,),
    )
    recording = capture_recording(
        backend=backend,
        wait_to_start=lambda: None,
        wait_to_stop=lambda deadline: None,
    )
    assert backend.opened_rates == [16_000, 44_100]
    assert recording.sample_rate == 44_100


def test_capture_raises_when_all_rates_unsupported():
    backend = FakeBackend(
        pcm=_silence(10),
        devices=[InputDevice(0, "Broken", 1, 44_100.0, is_default=True)],
        unsupported_rates=(16_000, 44_100, 48_000),
    )
    with pytest.raises(NativeCaptureFailed):
        capture_recording(
            backend=backend,
            wait_to_start=lambda: None,
            wait_to_stop=lambda deadline: None,
        )


def test_effective_capacity_shortens_above_48k():
    # 120s is fine up to 48kHz; a higher device rate must shorten the cap so the
    # finished WAV stays under the byte limit.
    assert effective_capacity_seconds(48_000) == pytest.approx(120.0)
    high = effective_capacity_seconds(96_000)
    assert high < 120.0
    # ...and the shortened window keeps the WAV under the byte cap.
    assert high * 96_000 * 2 <= voice_native.MAX_WAV_BYTES


def test_capture_truncates_overlong_pcm_to_byte_cap():
    # Force a tiny byte cap so any real audio overflows and must be trimmed.
    backend = FakeBackend(pcm=_silence(50_000))
    recording = capture_recording(
        backend=backend,
        wait_to_start=lambda: None,
        wait_to_stop=lambda deadline: None,
        max_bytes=voice_native.WAV_HEADER_BYTES + 2 * 10_000,  # room for 10k frames
        max_duration_seconds=120,
    )
    assert recording.truncated is True
    with wave.open(io.BytesIO(recording.wav_bytes), "rb") as reader:
        assert reader.getnframes() == 10_000


def test_capture_empty_recording_is_a_failure():
    backend = FakeBackend(pcm=b"")
    with pytest.raises(NativeCaptureFailed):
        capture_recording(
            backend=backend,
            wait_to_start=lambda: None,
            wait_to_stop=lambda deadline: None,
        )


def test_capture_unavailable_when_extra_missing():
    backend = FakeBackend(available=False)
    with pytest.raises(NativeCaptureUnavailable):
        capture_recording(
            backend=backend,
            wait_to_start=lambda: None,
            wait_to_stop=lambda deadline: None,
        )


def test_capture_unavailable_when_no_input_device():
    backend = FakeBackend(devices=[])
    with pytest.raises(NativeCaptureUnavailable):
        capture_recording(
            backend=backend,
            wait_to_start=lambda: None,
            wait_to_stop=lambda deadline: None,
        )


def test_stream_closed_even_on_keyboard_interrupt():
    backend = FakeBackend(pcm=_silence(100))

    def boom(_deadline):
        raise KeyboardInterrupt

    with pytest.raises(KeyboardInterrupt):
        capture_recording(
            backend=backend,
            wait_to_start=lambda: None,
            wait_to_stop=boom,
        )
    assert backend.streams[0].closed is True


def test_list_input_devices_requires_extra():
    with pytest.raises(NativeCaptureUnavailable):
        voice_native.list_input_devices(FakeBackend(available=False))
