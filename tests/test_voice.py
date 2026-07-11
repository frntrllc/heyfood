from urllib.parse import parse_qs, urlparse

import httpx

from heyfood_cli.voice import VoiceCaptureServer, voice_capture_html


def test_voice_capture_html_contains_speech_and_manual_fallback():
    html = voice_capture_html("state-1")

    assert "SpeechRecognition" in html
    assert "getUserMedia" in html
    assert "describeVoiceError" in html
    assert "textarea" in html
    assert "system dictation" in html
    assert "state-1" in html
    assert "Use This Transcript" in html


def test_voice_capture_server_accepts_transcript():
    with VoiceCaptureServer() as server:
        parsed = urlparse(server.url)
        state = parse_qs(parsed.query)["state"][0]

        response = httpx.post(
            f"http://127.0.0.1:{server.port}/submit",
            json={"state": state, "transcript": "I'm keto and dairy-free"},
            timeout=5,
        )

        assert response.status_code == 200
        result = server.wait(timeout_seconds=1)
        assert result.transcript == "I'm keto and dairy-free"


def test_voice_capture_server_rejects_wrong_state():
    with VoiceCaptureServer() as server:
        response = httpx.post(
            f"http://127.0.0.1:{server.port}/submit",
            json={"state": "wrong", "transcript": "I'm keto"},
            timeout=5,
        )

        assert response.status_code == 403
