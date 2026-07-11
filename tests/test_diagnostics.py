from __future__ import annotations

from io import StringIO
import re

import httpx
from rich.console import Console

from heyfood_cli import diagnostics
from heyfood_cli.client import HelloFoodClient
from heyfood_cli.config import ConfigStore


class _FakeHTTPClient:
    captured_headers: dict[str, str] = {}

    def __init__(self, **_kwargs):
        pass

    def __enter__(self):
        return self

    def __exit__(self, *_args):
        return False

    def request(self, method, url, *, headers, json, params):
        self.captured_headers = dict(headers)
        request = httpx.Request(method, url)
        return httpx.Response(
            200,
            request=request,
            headers={"X-Request-ID": "server-request-1"},
            json={"ok": True},
        )


def test_http_diagnostics_include_correlation_and_timing_without_payload(
    tmp_path, monkeypatch
):
    stream = StringIO()
    console = Console(file=stream, force_terminal=False, highlight=False)
    diagnostics.reporter.configure(enabled=True, console=console)
    fake = _FakeHTTPClient()
    monkeypatch.setattr(httpx, "Client", lambda **kwargs: fake)
    client = HelloFoodClient(store=ConfigStore(tmp_path / "config.json"))

    try:
        result = client._request(
            "POST",
            "/v1/test",
            json_body={"query": "private diet", "phone": "+1 555 555 1212"},
            params={"token": "hf_at_do-not-print"},
        )
    finally:
        diagnostics.reporter.configure(enabled=False, console=console)

    assert result == {"ok": True}
    request_id = fake.captured_headers["X-Request-ID"]
    assert re.fullmatch(r"[0-9a-f-]{36}", request_id)
    rendered = stream.getvalue()
    assert f"request_id={request_id}" in rendered
    assert "server_request_id=server-request-1" in rendered
    assert "endpoint=/v1/test" in rendered
    assert "status=200" in rendered
    assert "duration_ms=" in rendered
    assert "private diet" not in rendered
    assert "555 555 1212" not in rendered
    assert "hf_at_do-not-print" not in rendered


def test_diagnostic_reporter_drops_sensitive_fields_and_redacts_token_patterns():
    stream = StringIO()
    console = Console(file=stream, force_terminal=False, highlight=False)
    diagnostics.reporter.configure(enabled=True, console=console)
    try:
        diagnostics.reporter.emit(
            "test.event",
            context="local",
            query="do not print",
            access_token="do not print",
            error="Bearer abc.def and hf_ct_private from +1 (555) 555-1212",
        )
    finally:
        diagnostics.reporter.configure(enabled=False, console=console)

    rendered = stream.getvalue()
    assert "context=local" in rendered
    assert "do not print" not in rendered
    assert "abc.def" not in rendered
    assert "hf_ct_private" not in rendered
    assert "555-1212" not in rendered
    assert rendered.count("[redacted]") == 2
    assert "[redacted-phone]" in rendered
