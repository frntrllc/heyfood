"""Hardening of the localhost browser capture server + page (blocker 7)."""
from __future__ import annotations

import httpx
import pytest

from heyfood_cli.voice import (
    PURPOSE_ASK,
    PURPOSE_LOG,
    PURPOSE_ONBOARDING,
    VoiceCaptureError,
    VoiceCaptureServer,
    voice_capture_html,
)


def _post(server, path, json_body, *, headers=None):
    return httpx.post(
        f"{server.origin}{path}",
        json=json_body,
        headers={"Host": server.host, **(headers or {})},
    )


def test_get_serves_page_with_security_headers():
    with VoiceCaptureServer(purpose=PURPOSE_ASK) as server:
        response = httpx.get(server.url, headers={"Host": server.host})
    assert response.status_code == 200
    assert response.headers["Content-Security-Policy"].startswith("default-src 'none'")
    assert response.headers["Referrer-Policy"] == "no-referrer"
    assert response.headers["X-Content-Type-Options"] == "nosniff"
    assert response.headers["Cache-Control"] == "no-store"
    assert "voice request" in response.text  # purpose-aware (ask)


def test_get_rejects_wrong_host():
    with VoiceCaptureServer() as server:
        response = httpx.get(server.url, headers={"Host": "evil.example"})
    assert response.status_code == 400


def test_get_rejects_wrong_state():
    with VoiceCaptureServer() as server:
        response = httpx.get(
            f"{server.origin}/?state=wrong", headers={"Host": server.host}
        )
    assert response.status_code == 403


def test_submit_rejects_cross_origin():
    with VoiceCaptureServer() as server:
        response = _post(
            server,
            "/submit",
            {"state": server._state, "transcript": "hi"},
            headers={"Origin": "http://evil.example"},
        )
    assert response.status_code == 403


def test_submit_rejects_bad_state():
    with VoiceCaptureServer() as server:
        response = _post(server, "/submit", {"state": "nope", "transcript": "hi"})
    assert response.status_code == 403


def test_submit_is_one_shot():
    with VoiceCaptureServer() as server:
        first = _post(server, "/submit", {"state": server._state, "transcript": "keto please"})
        assert first.status_code == 200
        result = server.wait(2)
        assert result.transcript == "keto please"
        # A second submit after consumption is refused.
        second = _post(server, "/submit", {"state": server._state, "transcript": "again"})
        assert second.status_code == 409


def test_cancel_unblocks_wait_with_error():
    with VoiceCaptureServer() as server:
        response = _post(server, "/cancel", {"state": server._state})
        assert response.status_code == 200
        with pytest.raises(VoiceCaptureError):
            server.wait(2)


def test_page_is_purpose_aware_and_discloses_vendor():
    for purpose, marker in (
        (PURPOSE_ONBOARDING, "profile"),
        (PURPOSE_ASK, "Ask hello.food"),
        (PURPOSE_LOG, "Log a meal"),
    ):
        html = voice_capture_html("state-token", purpose)
        assert marker.lower() in html.lower()
        # The browser-vendor processing disclosure appears before any capture.
        assert "browser vendor" in html
        assert "Start Talking" in html
        assert 'id="cancel"' in html
        # Stop-before-unload teardown is wired.
        assert "beforeunload" in html
