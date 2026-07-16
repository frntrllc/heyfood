"""Multipart transcription transport + error mapping, against a mock transport."""
from __future__ import annotations

import json
from pathlib import Path

import httpx
import pytest

from heyfood_cli import client as client_mod
from heyfood_cli.client import (
    HelloFoodClient,
    HelloFoodError,
    LoginRequired,
    TranscriptionRateLimited,
    TranscriptionRejected,
    TranscriptionScopeRequired,
    TranscriptionUnavailable,
)
from heyfood_cli.config import ConfigStore


def _client_with_channel_token(tmp_path, monkeypatch):
    from heyfood_cli import config as config_mod

    monkeypatch.setattr(config_mod, "DEFAULT_API_KEY", "", raising=False)
    client = HelloFoodClient(store=ConfigStore(tmp_path / "config.json"))
    client.config["oauth"] = {
        "access_token": "hf_ct_channel",
        "refresh_token": "hf_ct_refresh",
        "client_id": "client-1",
        "access_expires_at": "2999-01-01T00:00:00+00:00",
    }
    return client


def _install_transport(monkeypatch, handler):
    transport = httpx.MockTransport(handler)
    real_client = httpx.Client  # capture before patching to avoid self-recursion

    monkeypatch.setattr(
        client_mod.httpx,
        "Client",
        lambda **kwargs: real_client(transport=transport),
    )


def test_transcribe_audio_sends_multipart_with_channel_auth(tmp_path, monkeypatch):
    client = _client_with_channel_token(tmp_path, monkeypatch)
    captured = {}

    def handler(request: httpx.Request) -> httpx.Response:
        captured["content_type"] = request.headers.get("content-type", "")
        captured["auth"] = request.headers.get("authorization", "")
        captured["body"] = request.content
        return httpx.Response(
            200,
            json={
                "transcript": "I'm low-FODMAP.",
                "duration_seconds": 3.1,
                "language": "en",
                "model_version": "hf-transcribe-1",
            },
        )

    _install_transport(monkeypatch, handler)

    result = client.transcribe_audio(b"RIFFfakewav", purpose="ask", language="en")

    assert result["transcript"] == "I'm low-FODMAP."
    assert result["model_version"] == "hf-transcribe-1"
    # The multipart boundary must come from httpx, never a hardcoded JSON type.
    assert captured["content_type"].startswith("multipart/form-data; boundary=")
    assert captured["auth"] == "Bearer hf_ct_channel"
    body = captured["body"]
    assert b'name="file"' in body
    assert b'name="purpose"' in body
    assert b"ask" in body
    assert b"en" in body


def test_transcribe_audio_omits_language_when_absent(tmp_path, monkeypatch):
    client = _client_with_channel_token(tmp_path, monkeypatch)
    seen = {}

    def handler(request: httpx.Request) -> httpx.Response:
        seen["body"] = request.content
        return httpx.Response(200, json={"transcript": "hi"})

    _install_transport(monkeypatch, handler)
    client.transcribe_audio(b"RIFF", purpose="onboarding")
    assert b'name="language"' not in seen["body"]


@pytest.mark.parametrize(
    ("status", "body", "headers", "expected"),
    (
        (429, {"error": "rate_limited", "message": "slow down"}, {"Retry-After": "30"}, TranscriptionRateLimited),
        (413, {"error": "audio_too_large", "message": "too big"}, {}, TranscriptionRejected),
        (400, {"error": "audio_too_long", "message": "too long"}, {}, TranscriptionRejected),
        (403, {"error": "insufficient_scope", "message": "missing scope"}, {}, TranscriptionScopeRequired),
        (401, {"error": "invalid_token", "message": "no token"}, {}, LoginRequired),
        (503, {"error": "transcription_unavailable", "message": "off"}, {}, TranscriptionUnavailable),
        (404, {"error": "not_found", "message": "dark"}, {}, TranscriptionUnavailable),
    ),
)
def test_error_status_maps_to_typed_exception(
    tmp_path, monkeypatch, status, body, headers, expected
):
    client = _client_with_channel_token(tmp_path, monkeypatch)

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(status, json=body, headers=headers)

    _install_transport(monkeypatch, handler)
    with pytest.raises(expected):
        client.transcribe_audio(b"RIFF", purpose="ask")


def test_rate_limited_carries_retry_after(tmp_path, monkeypatch):
    client = _client_with_channel_token(tmp_path, monkeypatch)

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            429,
            json={"error": "rate_limited", "message": "slow"},
            headers={"Retry-After": "45"},
        )

    _install_transport(monkeypatch, handler)
    with pytest.raises(TranscriptionRateLimited) as excinfo:
        client.transcribe_audio(b"RIFF", purpose="ask")
    assert excinfo.value.retry_after == 45


def test_forbidden_without_scope_code_is_generic_error(tmp_path, monkeypatch):
    client = _client_with_channel_token(tmp_path, monkeypatch)

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(403, json={"error": "forbidden", "message": "nope"})

    _install_transport(monkeypatch, handler)
    with pytest.raises(HelloFoodError) as excinfo:
        client.transcribe_audio(b"RIFF", purpose="ask")
    assert not isinstance(excinfo.value, TranscriptionScopeRequired)


def test_network_failure_degrades_to_unavailable(tmp_path, monkeypatch):
    client = _client_with_channel_token(tmp_path, monkeypatch)

    def handler(request: httpx.Request) -> httpx.Response:
        raise httpx.ConnectError("no route to host")

    _install_transport(monkeypatch, handler)
    with pytest.raises(TranscriptionUnavailable):
        client.transcribe_audio(b"RIFF", purpose="ask")


def test_transcription_schema_is_versioned_and_opaque():
    schema_path = (
        Path(__file__).resolve().parents[1]
        / "schemas"
        / "v1"
        / "transcription.schema.json"
    )
    schema = json.loads(schema_path.read_text(encoding="utf-8"))
    assert schema["x-heyfood-schema-version"] == 1
    assert schema["required"] == ["transcript"]
    model_version = schema["properties"]["model_version"]["description"].lower()
    assert "opaque" in model_version
    # The public schema must not name a provider or model family.
    for banned in ("openai", "whisper", "gpt", "deepgram", "eleven"):
        assert banned not in schema_path.read_text().lower()


# --------------------------------------------------------------------------- #
# Byte-boundary and scope-preflight behavior (blockers 5 and 2).
# --------------------------------------------------------------------------- #

from heyfood_cli import transcription_contract as _contract  # noqa: E402


def test_audio_over_the_file_ceiling_is_rejected_without_a_request(tmp_path, monkeypatch):
    client = _client_with_channel_token(tmp_path, monkeypatch)

    def handler(request: httpx.Request) -> httpx.Response:  # must never run
        raise AssertionError("oversized audio must not be uploaded")

    _install_transport(monkeypatch, handler)
    monkeypatch.setattr(_contract, "MAX_AUDIO_BYTES", 100)
    with pytest.raises(TranscriptionRejected):
        client.transcribe_audio(b"\x00" * 101, purpose="ask")


def test_audio_at_the_file_ceiling_is_sent(tmp_path, monkeypatch):
    client = _client_with_channel_token(tmp_path, monkeypatch)

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"transcript": "ok"})

    _install_transport(monkeypatch, handler)
    monkeypatch.setattr(_contract, "MAX_AUDIO_BYTES", 100)
    result = client.transcribe_audio(b"\x00" * 100, purpose="ask")
    assert result["transcript"] == "ok"


def test_multipart_request_envelope_boundary_is_enforced(tmp_path, monkeypatch):
    # Exercise the ACTUAL httpx multipart envelope at max-1 / max / max+1, so
    # framing overhead is bounded separately from the audio-file ceiling.
    client = _client_with_channel_token(tmp_path, monkeypatch)
    seen = {}

    def handler(request: httpx.Request) -> httpx.Response:
        seen["envelope"] = len(request.content)
        return httpx.Response(200, json={"transcript": "ok"})

    _install_transport(monkeypatch, handler)
    wav = b"\x00" * 5000

    # Discover the true envelope size with a generous ceiling.
    monkeypatch.setattr(_contract, "MAX_REQUEST_BYTES", 10_000_000)
    client.transcribe_audio(wav, purpose="ask")
    envelope = seen["envelope"]

    # max: an envelope of exactly the ceiling is allowed.
    monkeypatch.setattr(_contract, "MAX_REQUEST_BYTES", envelope)
    assert client.transcribe_audio(wav, purpose="ask")["transcript"] == "ok"

    # max+1: headroom, allowed.
    monkeypatch.setattr(_contract, "MAX_REQUEST_BYTES", envelope + 1)
    assert client.transcribe_audio(wav, purpose="ask")["transcript"] == "ok"

    # max-1: rejected locally before the wire.
    monkeypatch.setattr(_contract, "MAX_REQUEST_BYTES", envelope - 1)
    with pytest.raises(TranscriptionRejected):
        client.transcribe_audio(wav, purpose="ask")


def test_empty_and_nonobject_success_bodies_return_empty_dict(tmp_path, monkeypatch):
    # A malformed *success* body is not mapped to "unavailable" (which would
    # silently degrade to a different processor); it is returned for the caller's
    # contract validation to reject.
    client = _client_with_channel_token(tmp_path, monkeypatch)

    def empty(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, content=b"")

    _install_transport(monkeypatch, empty)
    assert client.transcribe_audio(b"RIFF", purpose="ask") == {}

    def not_object(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json=["not", "an", "object"])

    _install_transport(monkeypatch, not_object)
    assert client.transcribe_audio(b"RIFF", purpose="ask") == {}


def test_has_transcribe_scope_reads_persisted_grant(tmp_path, monkeypatch):
    client = _client_with_channel_token(tmp_path, monkeypatch)
    client.config["oauth"]["scope"] = "profile:read audio:transcribe meals:read"
    assert client.has_transcribe_scope() is True
    client.config["oauth"]["scope"] = "profile:read meals:read"
    assert client.has_transcribe_scope() is False
    client.config["oauth"].pop("scope", None)
    assert client.has_transcribe_scope() is False


def test_retry_after_non_integer_is_ignored(tmp_path, monkeypatch):
    client = _client_with_channel_token(tmp_path, monkeypatch)

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            429,
            json={"error": "rate_limited", "message": "slow"},
            headers={"Retry-After": "Wed, 21 Oct 2026 07:28:00 GMT"},
        )

    _install_transport(monkeypatch, handler)
    with pytest.raises(TranscriptionRateLimited) as excinfo:
        client.transcribe_audio(b"RIFF", purpose="ask")
    assert excinfo.value.retry_after is None
