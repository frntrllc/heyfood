"""The canonical client-side transcription contract, in code.

``schemas/v1/transcription.schema.json`` is the human- and cross-repo-facing
source of truth for the ``/v1/audio/transcriptions`` wire contract. This module
is its runtime equivalent: the numbers here are asserted byte-for-byte against
that schema by ``tests/test_transcription_contract.py`` so the two can never
drift, and the CLI reads *this* module at runtime (the schema file is not
packaged into the wheel). Nothing here is manually kept in sync by hand — the
parity test is the link.

Two responsibilities:

* expose the request-side limits (sample-rate window, per-file and per-request
  byte ceilings, duration/format) so capture negotiation can never build a WAV
  or a multipart envelope the server rejects; and
* validate a success response at runtime, so a malformed body becomes a typed
  contract failure instead of silently feeding an empty/garbage transcript into
  onboarding, meal history, or the agent.
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from .client import HelloFoodError


# --- Purpose enum ----------------------------------------------------------
PURPOSE_ONBOARDING = "onboarding"
PURPOSE_ASK = "ask"
PURPOSE_LOG = "log"
PURPOSES: tuple[str, ...] = (PURPOSE_ONBOARDING, PURPOSE_ASK, PURPOSE_LOG)

# --- Audio / WAV format ----------------------------------------------------
CHANNELS = 1
SAMPLE_WIDTH_BYTES = 2
WAV_HEADER_BYTES = 44
SAMPLE_RATE_MIN_HZ = 8_000
SAMPLE_RATE_MAX_HZ = 48_000
PREFERRED_SAMPLE_RATE_HZ = 16_000
MAX_DURATION_SECONDS = 120

# --- Byte ceilings (audio file vs whole multipart request) -----------------
# max_audio_bytes bounds the WAV file; max_request_bytes bounds the multipart
# envelope so framing overhead cannot reject an otherwise-valid maximum WAV.
MAX_AUDIO_BYTES = 12_500_000
MAX_REQUEST_BYTES = 13_107_200

# --- Response bounds -------------------------------------------------------
MAX_TRANSCRIPT_CHARS = 20_000
MAX_RESPONSE_DURATION_SECONDS = 3_600
MAX_LANGUAGE_CHARS = 35
MAX_MODEL_VERSION_CHARS = 128


class TranscriptionContractError(HelloFoodError):
    """A success (2xx) transcription body violated the versioned contract.

    Raised for empty/whitespace-only transcripts, wrong-typed or oversized
    fields, and other malformed-but-successful payloads. Callers treat this as a
    terminal typed service error and offer a typed recovery path — never another
    implicit capture loop.
    """


@dataclass(frozen=True)
class Transcription:
    """A validated transcription response."""

    transcript: str
    duration_seconds: float | None
    language: str | None
    model_version: str | None


def sample_rate_supported(sample_rate: int) -> bool:
    """True when a device rate falls inside the backend's accepted window."""
    return SAMPLE_RATE_MIN_HZ <= int(sample_rate) <= SAMPLE_RATE_MAX_HZ


def clamp_sample_rate(sample_rate: int) -> int:
    """Clamp a device rate into the accepted window (used when negotiating)."""
    return max(SAMPLE_RATE_MIN_HZ, min(SAMPLE_RATE_MAX_HZ, int(sample_rate)))


def max_capture_bytes() -> int:
    """The WAV ceiling a recording must stay under."""
    return MAX_AUDIO_BYTES


def validate_response(payload: Any) -> Transcription:
    """Validate a 2xx transcription body against the versioned contract.

    Returns a :class:`Transcription` on success; raises
    :class:`TranscriptionContractError` on any violation. Never returns an empty
    or whitespace-only transcript.
    """
    if not isinstance(payload, dict):
        raise TranscriptionContractError(
            "The transcription service returned a non-object response."
        )

    raw_transcript = payload.get("transcript")
    if not isinstance(raw_transcript, str):
        raise TranscriptionContractError(
            "The transcription response was missing a text transcript."
        )
    transcript = raw_transcript.strip()
    if not transcript:
        raise TranscriptionContractError(
            "The transcription response contained an empty transcript."
        )
    if len(raw_transcript) > MAX_TRANSCRIPT_CHARS:
        raise TranscriptionContractError(
            "The transcription response transcript exceeded the allowed length."
        )

    duration = _optional_number(
        payload.get("duration_seconds"),
        field="duration_seconds",
        maximum=MAX_RESPONSE_DURATION_SECONDS,
    )

    language = payload.get("language")
    if language is not None:
        if not isinstance(language, str):
            raise TranscriptionContractError(
                "The transcription response language was not a string."
            )
        if len(language) > MAX_LANGUAGE_CHARS:
            raise TranscriptionContractError(
                "The transcription response language tag was too long."
            )

    model_version = payload.get("model_version")
    if model_version is not None:
        if not isinstance(model_version, str):
            raise TranscriptionContractError(
                "The transcription response model version was not a string."
            )
        if len(model_version) > MAX_MODEL_VERSION_CHARS:
            raise TranscriptionContractError(
                "The transcription response model version was too long."
            )

    return Transcription(
        transcript=transcript,
        duration_seconds=duration,
        language=language,
        model_version=model_version,
    )


def _optional_number(value: Any, *, field: str, maximum: float) -> float | None:
    if value is None:
        return None
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise TranscriptionContractError(
            f"The transcription response {field} was not a number."
        )
    numeric = float(value)
    if numeric < 0 or numeric > maximum:
        raise TranscriptionContractError(
            f"The transcription response {field} was out of range."
        )
    return numeric
