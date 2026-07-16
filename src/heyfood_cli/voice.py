from __future__ import annotations

from dataclasses import dataclass
import html
import json
import secrets
import threading
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Callable
from urllib.parse import parse_qs, urlparse


# Purpose enum for the loopback capture page. Keeps the page copy honest about
# what the transcript will be used for.
PURPOSE_ONBOARDING = "onboarding"
PURPOSE_ASK = "ask"
PURPOSE_LOG = "log"
_VALID_PURPOSES = (PURPOSE_ONBOARDING, PURPOSE_ASK, PURPOSE_LOG)

_MAX_BODY_BYTES = 32_768
_REQUEST_TIMEOUT_SECONDS = 30.0

# The page is fully self-contained (inline CSS/JS, no external hosts). A strict
# CSP plus nosniff/no-referrer keep the loopback origin from being used as a
# springboard or leaking anything by referrer.
_CSP = (
    "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; "
    "connect-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'"
)


class VoiceCaptureError(RuntimeError):
    pass


@dataclass(frozen=True)
class VoiceCaptureResult:
    transcript: str


def _purpose_copy(purpose: str) -> tuple[str, str, str]:
    """(title, heading, example) copy for a capture purpose."""
    mapping = {
        PURPOSE_ONBOARDING: (
            "hello.food voice onboarding",
            "Build your dietary profile by voice",
            "I'm keto, dairy-free, light activity, and mostly Thai food.",
        ),
        PURPOSE_ASK: (
            "hello.food voice request",
            "Ask hello.food by voice",
            "What can I eat for lunch near me that's gluten-free?",
        ),
        PURPOSE_LOG: (
            "hello.food voice meal log",
            "Log a meal by voice",
            "I had a chicken burrito bowl with black beans and guacamole.",
        ),
    }
    return mapping.get(purpose, mapping[PURPOSE_ONBOARDING])


def capture_voice_transcript(
    *,
    timeout_seconds: int = 300,
    open_browser: bool = True,
    purpose: str = PURPOSE_ONBOARDING,
    url_callback: Callable[[str], None] | None = None,
) -> VoiceCaptureResult:
    """Capture a transcript through a hardened localhost browser page."""
    if purpose not in _VALID_PURPOSES:
        purpose = PURPOSE_ONBOARDING
    with VoiceCaptureServer(purpose=purpose) as server:
        if open_browser:
            webbrowser.open(server.url)
        if url_callback:
            url_callback(server.url)
        return server.wait(timeout_seconds)


class VoiceCaptureServer:
    def __init__(self, *, purpose: str = PURPOSE_ONBOARDING):
        self.purpose = purpose if purpose in _VALID_PURPOSES else PURPOSE_ONBOARDING
        self._event = threading.Event()
        self._result: VoiceCaptureResult | None = None
        self._error: str | None = None
        self._consumed = False
        self._lock = threading.Lock()
        self._state = secrets.token_urlsafe(24)
        self._server = ThreadingHTTPServer(("127.0.0.1", 0), self._handler())
        # Request-handling threads must not keep the process alive.
        self._server.daemon_threads = True
        self.port = int(self._server.server_address[1])
        self.host = f"127.0.0.1:{self.port}"
        self.origin = f"http://{self.host}"
        self.url = f"{self.origin}/?state={self._state}"
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)

    def __enter__(self) -> "VoiceCaptureServer":
        self._thread.start()
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self.close()

    def close(self) -> None:
        self._server.shutdown()
        self._server.server_close()
        self._thread.join(timeout=1)

    def wait(self, timeout_seconds: int) -> VoiceCaptureResult:
        if not self._event.wait(timeout=max(1, timeout_seconds)):
            raise VoiceCaptureError("Timed out waiting for voice capture.")
        if self._error:
            raise VoiceCaptureError(self._error)
        if self._result is None or not self._result.transcript.strip():
            raise VoiceCaptureError("No transcript was returned.")
        return self._result

    def _consume(self, *, result: VoiceCaptureResult | None, error: str | None) -> bool:
        """One-shot state transition. Returns False if already consumed."""
        with self._lock:
            if self._consumed:
                return False
            self._consumed = True
            self._result = result
            self._error = error
        self._event.set()
        return True

    def _handler(self):
        parent = self

        class Handler(BaseHTTPRequestHandler):
            timeout = _REQUEST_TIMEOUT_SECONDS
            protocol_version = "HTTP/1.1"

            def _host_ok(self) -> bool:
                return (self.headers.get("Host") or "").strip().lower() == parent.host

            def do_GET(self) -> None:
                if not self._host_ok():
                    self._send_text(400, "Bad host.")
                    return
                parsed = urlparse(self.path)
                if parsed.path != "/":
                    self._send_text(404, "Not found")
                    return
                query = parse_qs(parsed.query)
                if query.get("state", [""])[0] != parent._state:
                    self._send_text(403, "Invalid voice session.")
                    return
                self._send_html(voice_capture_html(parent._state, parent.purpose))

            def do_POST(self) -> None:
                if not self._host_ok():
                    self._send_json(400, {"ok": False, "error": "bad_host"})
                    return
                # Exact same-origin check: block cross-origin/DNS-rebinding POSTs.
                origin = (self.headers.get("Origin") or "").strip()
                if origin and origin != parent.origin:
                    self._send_json(403, {"ok": False, "error": "bad_origin"})
                    return
                parsed = urlparse(self.path)
                if parsed.path not in ("/submit", "/cancel"):
                    self._send_json(404, {"ok": False, "error": "not_found"})
                    return
                try:
                    length = int(self.headers.get("Content-Length", "0"))
                except ValueError:
                    length = 0
                raw_body = self.rfile.read(min(max(length, 0), _MAX_BODY_BYTES))
                try:
                    data = json.loads(raw_body.decode("utf-8")) if raw_body else {}
                except Exception:
                    self._send_json(400, {"ok": False, "error": "invalid_json"})
                    return
                if not isinstance(data, dict) or data.get("state") != parent._state:
                    self._send_json(403, {"ok": False, "error": "invalid_state"})
                    return

                if parsed.path == "/cancel":
                    if parent._consume(result=None, error="Voice capture cancelled."):
                        self._send_json(200, {"ok": True})
                    else:
                        self._send_json(409, {"ok": False, "error": "already_done"})
                    return

                transcript = str(data.get("transcript") or "").strip()
                if not transcript:
                    self._send_json(422, {"ok": False, "error": "empty_transcript"})
                    return
                if parent._consume(
                    result=VoiceCaptureResult(transcript=transcript[:4000]),
                    error=None,
                ):
                    self._send_json(200, {"ok": True})
                else:
                    self._send_json(409, {"ok": False, "error": "already_done"})

            def log_message(self, format: str, *args: object) -> None:
                return

            def _security_headers(self) -> None:
                self.send_header("Content-Security-Policy", _CSP)
                self.send_header("Referrer-Policy", "no-referrer")
                self.send_header("X-Content-Type-Options", "nosniff")
                self.send_header("Cache-Control", "no-store")

            def _send_html(self, body: str) -> None:
                encoded = body.encode("utf-8")
                self.send_response(200)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Content-Length", str(len(encoded)))
                self._security_headers()
                self.end_headers()
                self.wfile.write(encoded)

            def _send_text(self, status: int, text: str) -> None:
                encoded = text.encode("utf-8")
                self.send_response(status)
                self.send_header("Content-Type", "text/plain; charset=utf-8")
                self.send_header("Content-Length", str(len(encoded)))
                self._security_headers()
                self.end_headers()
                self.wfile.write(encoded)

            def _send_json(self, status: int, payload: dict) -> None:
                encoded = json.dumps(payload).encode("utf-8")
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(encoded)))
                self._security_headers()
                self.end_headers()
                self.wfile.write(encoded)

        return Handler


def voice_capture_html(state: str, purpose: str = PURPOSE_ONBOARDING) -> str:
    escaped_state = html.escape(state, quote=True)
    title, heading, example = _purpose_copy(
        purpose if purpose in _VALID_PURPOSES else PURPOSE_ONBOARDING
    )
    title_e = html.escape(title)
    heading_e = html.escape(heading)
    example_e = html.escape(example)
    return f"""<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{title_e}</title>
  <style>
    :root {{
      color-scheme: light dark;
      --bg: #101411;
      --fg: #f4f2ec;
      --muted: #a8b2a8;
      --line: #2b332d;
      --green: #7ee787;
      --green-dark: #1f6f3e;
      --yellow: #f2cc60;
      --red: #ff7b72;
    }}
    * {{ box-sizing: border-box; }}
    body {{
      margin: 0;
      min-height: 100vh;
      display: grid;
      place-items: center;
      background:
        radial-gradient(circle at 20% 20%, rgba(126, 231, 135, 0.14), transparent 28rem),
        var(--bg);
      color: var(--fg);
      font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      line-height: 1.5;
    }}
    main {{
      width: min(760px, calc(100vw - 32px));
      padding: 28px;
      border: 1px solid var(--line);
      border-radius: 8px;
      background: rgba(16, 20, 17, 0.9);
      box-shadow: 0 24px 80px rgba(0, 0, 0, 0.35);
    }}
    h1 {{ margin: 0 0 8px; font-size: clamp(24px, 4vw, 40px); letter-spacing: 0; }}
    p {{ margin: 0 0 18px; color: var(--muted); }}
    .disclosure {{
      margin: 0 0 18px;
      padding: 12px 14px;
      border: 1px solid var(--line);
      border-radius: 8px;
      background: rgba(242, 204, 96, 0.08);
      color: var(--fg);
      font-size: 14px;
    }}
    .controls {{ display: flex; flex-wrap: wrap; gap: 10px; margin: 18px 0; }}
    button {{
      appearance: none;
      border: 1px solid var(--line);
      border-radius: 8px;
      padding: 11px 14px;
      background: #172018;
      color: var(--fg);
      font: inherit;
      cursor: pointer;
    }}
    button.primary {{ background: var(--green-dark); border-color: #2ea043; color: #fff; }}
    button.cancel {{ border-color: #5a2b2b; }}
    button:disabled {{ opacity: 0.45; cursor: not-allowed; }}
    textarea {{
      width: 100%;
      min-height: 190px;
      resize: vertical;
      border-radius: 8px;
      border: 1px solid var(--line);
      background: #0d1110;
      color: var(--fg);
      padding: 14px;
      font: 16px/1.5 ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
    }}
    .status {{ min-height: 24px; color: var(--muted); }}
    .recording {{ color: var(--yellow); }}
    .error {{ color: var(--red); }}
    .done {{ color: var(--green); }}
    .hint {{ margin-top: 14px; font-size: 14px; }}
    .fallback {{
      display: none;
      margin-top: 12px;
      padding: 12px 14px;
      border: 1px solid var(--line);
      border-radius: 8px;
      color: var(--muted);
      background: rgba(255, 255, 255, 0.03);
      font-size: 14px;
    }}
    .fallback.visible {{ display: block; }}
    code {{ color: var(--green); }}
  </style>
</head>
<body>
  <main>
    <h1>{heading_e}</h1>
    <p>Speak naturally. Example: <code>{example_e}</code></p>
    <div class="disclosure" role="note">
      Before you start: browser speech recognition sends your audio to your
      browser vendor's speech service — a third party — not to hello.food.
      To keep audio within hello.food, cancel and use native voice capture in
      the terminal instead. Nothing is sent anywhere until you press
      <strong>Use This Transcript</strong>.
    </div>
    <div class="controls">
      <button id="start" class="primary" aria-label="Start talking">Start Talking</button>
      <button id="stop" disabled aria-label="Stop listening">Stop</button>
      <button id="clear" aria-label="Clear transcript">Clear</button>
      <button id="submit" class="primary" aria-label="Use this transcript">Use This Transcript</button>
      <button id="cancel" class="cancel" aria-label="Cancel voice capture">Cancel</button>
    </div>
    <div id="status" class="status" role="status" aria-live="polite">Ready when you are.</div>
    <label for="transcript" class="hint">Transcript (editable)</label>
    <textarea id="transcript" autofocus placeholder="Your transcript will appear here. You can also type or edit it manually."></textarea>
    <div id="fallback" class="fallback">
      Browser speech capture can be picky. You can still use voice by focusing
      the transcript box and using your system dictation, then click
      <strong>Use This Transcript</strong>. On macOS, enable Dictation in
      Keyboard settings and press the configured dictation shortcut.
    </div>
    <p class="hint">When you submit, this tab sends only the transcript to your local CLI at <code>127.0.0.1</code>. Return to the terminal to review it before it saves.</p>
  </main>
  <script>
    const state = "{escaped_state}";
    const SpeechRecognition = window.SpeechRecognition || window.webkitSpeechRecognition;
    const start = document.getElementById("start");
    const stop = document.getElementById("stop");
    const clear = document.getElementById("clear");
    const submit = document.getElementById("submit");
    const cancelBtn = document.getElementById("cancel");
    const transcript = document.getElementById("transcript");
    const statusEl = document.getElementById("status");
    const fallback = document.getElementById("fallback");
    let recognition = null;
    let finalText = "";
    let isListening = false;
    let done = false;

    function setStatus(text, className = "") {{
      statusEl.textContent = text;
      statusEl.className = "status " + className;
    }}

    function showFallback() {{
      fallback.classList.add("visible");
    }}

    function resetButtons() {{
      start.disabled = !recognition || done;
      stop.disabled = true;
    }}

    function stopListening() {{
      if (recognition && isListening) {{
        try {{ recognition.stop(); }} catch (e) {{}}
      }}
      isListening = false;
    }}

    function describeVoiceError(error) {{
      const code = String(error && (error.error || error.name || error.message) || "unknown");
      const messages = {{
        "not-allowed": "Microphone access was blocked. Allow microphone access for this tab, then try again.",
        "service-not-allowed": "This browser blocked its speech recognition service. Try Chrome, or use system dictation in the transcript box.",
        "audio-capture": "No microphone was found. Check your input device, then try again.",
        "network": "The browser speech service could not connect. You can use system dictation or type the transcript.",
        "aborted": "Listening was stopped before speech was captured. Try again, or use system dictation.",
        "no-speech": "I didn't catch speech. Try again a little closer to the mic.",
        "language-not-supported": "This browser does not support speech recognition for your current language."
      }};
      return messages[code] || ("Voice capture could not start (" + code + "). You can type or use system dictation instead.");
    }}

    async function requestMicrophone() {{
      if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) {{
        return;
      }}
      const stream = await navigator.mediaDevices.getUserMedia({{ audio: true }});
      stream.getTracks().forEach((track) => track.stop());
    }}

    if (!window.isSecureContext) {{
      showFallback();
      setStatus("Voice capture needs a secure browser context. Type or use system dictation instead.", "error");
    }}

    if (!SpeechRecognition) {{
      start.disabled = true;
      stop.disabled = true;
      showFallback();
      setStatus("Speech recognition is not available in this browser. Type below and submit.", "error");
    }} else {{
      recognition = new SpeechRecognition();
      recognition.continuous = true;
      recognition.interimResults = true;
      recognition.lang = navigator.language || "en-US";

      recognition.onstart = () => {{
        isListening = true;
        setStatus("Listening. Speak naturally.", "recording");
        start.disabled = true;
        stop.disabled = false;
      }};
      recognition.onerror = (event) => {{
        isListening = false;
        showFallback();
        setStatus(describeVoiceError(event), "error");
        resetButtons();
      }};
      recognition.onend = () => {{
        isListening = false;
        resetButtons();
        if (transcript.value.trim()) {{
          setStatus("Transcript ready. Edit it if needed, then submit.", "done");
        }} else {{
          setStatus("Stopped. You can try again or type manually.");
        }}
      }};
      recognition.onresult = (event) => {{
        let interim = "";
        for (let i = event.resultIndex; i < event.results.length; i++) {{
          const chunk = event.results[i][0].transcript;
          if (event.results[i].isFinal) {{
            finalText += " " + chunk;
          }} else {{
            interim += chunk;
          }}
        }}
        transcript.value = (finalText + " " + interim).trim();
      }};
    }}

    start.addEventListener("click", async () => {{
      if (!recognition || done) return;
      if (isListening) {{
        setStatus("Already listening.", "recording");
        return;
      }}
      finalText = transcript.value.trim();
      start.disabled = true;
      setStatus("Requesting microphone access...");
      try {{
        await requestMicrophone();
        recognition.start();
      }} catch (error) {{
        isListening = false;
        showFallback();
        resetButtons();
        setStatus(describeVoiceError(error), "error");
      }}
    }});
    stop.addEventListener("click", () => {{
      stopListening();
    }});
    clear.addEventListener("click", () => {{
      finalText = "";
      transcript.value = "";
      setStatus("Cleared.");
      transcript.focus();
    }});
    submit.addEventListener("click", async () => {{
      const text = transcript.value.trim();
      if (!text) {{
        setStatus("Add a transcript first.", "error");
        return;
      }}
      // Always stop recognition before sending, so no audio keeps streaming.
      stopListening();
      submit.disabled = true;
      cancelBtn.disabled = true;
      try {{
        const response = await fetch("/submit", {{
          method: "POST",
          headers: {{ "Content-Type": "application/json" }},
          body: JSON.stringify({{ state, transcript: text }})
        }});
        if (!response.ok) throw new Error(await response.text());
        done = true;
        resetButtons();
        setStatus("Sent. You can close this tab and return to your terminal.", "done");
      }} catch (error) {{
        submit.disabled = false;
        cancelBtn.disabled = false;
        setStatus("Could not send transcript: " + error.message, "error");
      }}
    }});
    cancelBtn.addEventListener("click", async () => {{
      stopListening();
      cancelBtn.disabled = true;
      submit.disabled = true;
      try {{
        await fetch("/cancel", {{
          method: "POST",
          headers: {{ "Content-Type": "application/json" }},
          body: JSON.stringify({{ state }})
        }});
      }} catch (error) {{}}
      done = true;
      resetButtons();
      setStatus("Cancelled. Return to your terminal — nothing was submitted.", "done");
    }});
    window.addEventListener("beforeunload", () => {{
      // Never leave the microphone streaming when the tab goes away.
      stopListening();
    }});
  </script>
</body>
</html>"""
