"""The transcription contract module and its canonical schema stay in lockstep.

``schemas/v1/transcription.schema.json`` is the single source of truth; this test
asserts the runtime ``transcription_contract`` constants are generated-equivalent
to it (never hand-copied out of sync), and exercises the response validator's
accept/reject behavior for the required, empty, whitespace, wrong-type,
oversized, and forward-compatible cases.
"""
from __future__ import annotations

import json
from pathlib import Path

import pytest

from heyfood_cli import transcription_contract as contract
from heyfood_cli.transcription_contract import TranscriptionContractError, validate_response


ROOT = Path(__file__).resolve().parents[1]
SCHEMA = json.loads((ROOT / "schemas" / "v1" / "transcription.schema.json").read_text())


def test_module_constants_match_the_canonical_schema() -> None:
    audio = SCHEMA["x-heyfood-audio"]
    assert contract.CHANNELS == audio["channels"]
    assert contract.SAMPLE_WIDTH_BYTES == audio["sample_width_bytes"]
    assert contract.WAV_HEADER_BYTES == audio["wav_header_bytes"]
    assert contract.SAMPLE_RATE_MIN_HZ == audio["sample_rate_min_hz"]
    assert contract.SAMPLE_RATE_MAX_HZ == audio["sample_rate_max_hz"]
    assert contract.PREFERRED_SAMPLE_RATE_HZ == audio["preferred_sample_rate_hz"]
    assert contract.MAX_DURATION_SECONDS == audio["max_duration_seconds"]
    assert contract.MAX_AUDIO_BYTES == audio["max_audio_bytes"]
    assert contract.MAX_REQUEST_BYTES == audio["max_request_bytes"]

    limits = SCHEMA["x-heyfood-response-limits"]
    assert contract.MAX_TRANSCRIPT_CHARS == limits["max_transcript_chars"]
    assert contract.MAX_RESPONSE_DURATION_SECONDS == limits["max_duration_seconds"]
    assert contract.MAX_LANGUAGE_CHARS == limits["max_language_chars"]
    assert contract.MAX_MODEL_VERSION_CHARS == limits["max_model_version_chars"]

    assert list(contract.PURPOSES) == SCHEMA["x-heyfood-purposes"]


def test_byte_ceilings_are_internally_consistent() -> None:
    # A maximum-length WAV at the maximum rate must fit inside max_audio_bytes,
    # and the request ceiling must exceed the audio ceiling.
    worst_case = (
        contract.MAX_DURATION_SECONDS
        * contract.SAMPLE_RATE_MAX_HZ
        * contract.CHANNELS
        * contract.SAMPLE_WIDTH_BYTES
        + contract.WAV_HEADER_BYTES
    )
    assert contract.MAX_AUDIO_BYTES >= worst_case
    assert contract.MAX_REQUEST_BYTES > contract.MAX_AUDIO_BYTES


def test_sample_rate_window_helpers() -> None:
    assert contract.sample_rate_supported(8000)
    assert contract.sample_rate_supported(48000)
    assert not contract.sample_rate_supported(7999)
    assert not contract.sample_rate_supported(96000)
    assert contract.clamp_sample_rate(96000) == 48000
    assert contract.clamp_sample_rate(4000) == 8000
    assert contract.clamp_sample_rate(16000) == 16000


def test_validate_accepts_minimal_and_full_bodies() -> None:
    minimal = validate_response({"transcript": "hello there"})
    assert minimal.transcript == "hello there"
    assert minimal.duration_seconds is None
    assert minimal.language is None

    full = validate_response(
        {
            "transcript": "  keto and dairy free  ",
            "duration_seconds": 3.2,
            "language": "en",
            "model_version": "hf-transcribe-1",
            "unknown_forward_compatible_field": {"anything": True},
        }
    )
    assert full.transcript == "keto and dairy free"
    assert full.duration_seconds == 3.2
    assert full.language == "en"
    assert full.model_version == "hf-transcribe-1"


def test_validate_accepts_null_language() -> None:
    result = validate_response({"transcript": "ok", "language": None})
    assert result.language is None


@pytest.mark.parametrize(
    "payload",
    [
        {},
        {"transcript": ""},
        {"transcript": "   "},
        {"transcript": 123},
        {"transcript": None},
        [],
        "not-a-dict",
        {"transcript": "x", "duration_seconds": "3"},
        {"transcript": "x", "duration_seconds": -1},
        {"transcript": "x", "duration_seconds": True},
        {"transcript": "x", "language": 42},
        {"transcript": "x", "model_version": 7},
        {"transcript": "x" * (contract.MAX_TRANSCRIPT_CHARS + 1)},
        {"transcript": "x", "language": "e" * (contract.MAX_LANGUAGE_CHARS + 1)},
        {"transcript": "x", "duration_seconds": contract.MAX_RESPONSE_DURATION_SECONDS + 1},
    ],
)
def test_validate_rejects_malformed_bodies(payload) -> None:
    with pytest.raises(TranscriptionContractError):
        validate_response(payload)
