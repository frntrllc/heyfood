"""0.3.1 login compatibility: server capability detection (RFC 8414 metadata).

v0.3.0's login sent an ``intent`` field and requested the ``account:delete``
scope unconditionally. Against a production backend that has not yet deployed
those capabilities, the device-authorize request 422s on the unknown ``intent``
field and the browser/authorize request 400s on the unsupported scope, so every
fresh login was instantly broken.

These tests pin the fix: before login we read the authorization server's
published metadata (``scopes_supported``) and request only what the live server
accepts, sending ``intent`` only when the registration-capable backend is
detected. Discovery is fail-soft — an unreachable or malformed document falls
back to the full pre-hotfix request, so we are never worse than before.

Transport is injected (a scripted fake ``get()``); nothing here touches the
network, so the suite is deterministic.
"""
from __future__ import annotations

from urllib.parse import parse_qs, urlparse

import httpx
import pytest

from heyfood_cli import auth
from heyfood_cli.auth import (
    LOGIN_SCOPES,
    LoginCapabilities,
    build_authorize_url,
    fetch_supported_scopes,
    resolve_login_capabilities,
    start_device_authorization,
)


# The scope ceilings the two backends advertise. "Old" is the set live on
# production today: the 12 login scopes WITHOUT account:delete. "New" is the
# registration-capable backend that adds account:delete (all 13).
OLD_BACKEND_SCOPES = [scope for scope in LOGIN_SCOPES if scope != "account:delete"]
NEW_BACKEND_SCOPES = list(LOGIN_SCOPES)


class ScriptedGetClient:
    """Minimal fake httpx.Client exposing get(); returns or raises one entry."""

    def __init__(self, outcome):
        self._outcome = outcome

    def __enter__(self):
        return self

    def __exit__(self, *_):
        return False

    def get(self, *_args, **_kwargs):
        if isinstance(self._outcome, BaseException):
            raise self._outcome
        return self._outcome


def _metadata(scopes) -> httpx.Response:
    return httpx.Response(
        200,
        json={
            "issuer": "https://api.hello.food",
            "authorization_endpoint": "https://auth.hello.food/authorize",
            "token_endpoint": "https://api.hello.food/v1/channel/oauth/token",
            "scopes_supported": scopes,
        },
    )


# --- fetch_supported_scopes: fail-soft discovery ---------------------------


def test_fetch_supported_scopes_reads_advertised_list():
    scopes = fetch_supported_scopes(
        "https://api.hello.food", client=ScriptedGetClient(_metadata(OLD_BACKEND_SCOPES))
    )
    assert scopes == OLD_BACKEND_SCOPES


@pytest.mark.parametrize(
    "outcome",
    [
        httpx.ConnectError("offline"),
        httpx.ReadTimeout("slow"),
        httpx.Response(404, text="not found"),
        httpx.Response(500, text="boom"),
        httpx.Response(200, text="<html>not json</html>"),
        httpx.Response(200, json=["not", "a", "dict"]),
        httpx.Response(200, json={"issuer": "x"}),  # no scopes_supported
        httpx.Response(200, json={"scopes_supported": "not-a-list"}),
        httpx.Response(200, json={"scopes_supported": []}),  # empty
    ],
)
def test_fetch_supported_scopes_is_fail_soft(outcome):
    assert (
        fetch_supported_scopes("https://api.hello.food", client=ScriptedGetClient(outcome))
        is None
    )


# --- resolve_login_capabilities: the negotiated request --------------------


def test_old_backend_drops_account_delete_and_intent():
    capabilities = resolve_login_capabilities(
        "https://api.hello.food", client=ScriptedGetClient(_metadata(OLD_BACKEND_SCOPES))
    )
    assert capabilities.scopes == OLD_BACKEND_SCOPES
    assert "account:delete" not in capabilities.scopes
    assert capabilities.include_intent is False
    # Order is preserved exactly as LOGIN_SCOPES pins it (account:delete removed).
    assert capabilities.scopes == [s for s in LOGIN_SCOPES if s in OLD_BACKEND_SCOPES]


def test_new_backend_keeps_full_scopes_and_intent():
    capabilities = resolve_login_capabilities(
        "https://api.hello.food", client=ScriptedGetClient(_metadata(NEW_BACKEND_SCOPES))
    )
    assert capabilities.scopes == NEW_BACKEND_SCOPES
    assert "account:delete" in capabilities.scopes
    assert capabilities.include_intent is True


@pytest.mark.parametrize(
    "outcome",
    [
        httpx.ConnectError("offline"),
        httpx.Response(404, text="not found"),
        httpx.Response(200, text="not json"),
        httpx.Response(200, json={"issuer": "x"}),
    ],
)
def test_discovery_failure_falls_back_to_full_request(outcome):
    # Documented fallback == pre-hotfix behavior: full scopes + intent, so a
    # discovery outage never makes login worse than 0.3.0 was on a new backend.
    capabilities = resolve_login_capabilities(
        "https://api.hello.food", client=ScriptedGetClient(outcome)
    )
    assert capabilities.scopes == list(LOGIN_SCOPES)
    assert capabilities.include_intent is True


# --- The negotiated request reaches the wire correctly ---------------------


def _capture_device_body(monkeypatch) -> dict:
    captured: dict = {}

    def fake_post(_api_url, _path, *, json_body, **_kwargs):
        captured.update(json_body)
        return httpx.Response(
            200,
            json={
                "device_code": "dc",
                "user_code": "ABCD-EFGH",
                "verification_uri": "https://auth.hello.food/authorize",
                "expires_in": 600,
                "interval": 5,
            },
        )

    monkeypatch.setattr("heyfood_cli.auth._post_with_diagnostics", fake_post)
    return captured


def test_device_authorize_body_old_backend_has_no_intent_or_delete(monkeypatch):
    captured = _capture_device_body(monkeypatch)
    capabilities = resolve_login_capabilities(
        "https://api.hello.food", client=ScriptedGetClient(_metadata(OLD_BACKEND_SCOPES))
    )
    start_device_authorization(
        "https://api.hello.food",
        "client-1",
        "auto",
        scopes=capabilities.scopes,
        include_intent=capabilities.include_intent,
    )
    assert "intent" not in captured
    assert "account:delete" not in captured["scope"].split(" ")
    assert captured["scope"] == " ".join(OLD_BACKEND_SCOPES)


def test_device_authorize_body_new_backend_has_intent_and_full_scopes(monkeypatch):
    captured = _capture_device_body(monkeypatch)
    capabilities = resolve_login_capabilities(
        "https://api.hello.food", client=ScriptedGetClient(_metadata(NEW_BACKEND_SCOPES))
    )
    start_device_authorization(
        "https://api.hello.food",
        "client-1",
        "auto",
        scopes=capabilities.scopes,
        include_intent=capabilities.include_intent,
    )
    assert captured["intent"] == "auto"
    assert captured["scope"] == " ".join(LOGIN_SCOPES)


def test_authorize_url_old_backend_has_no_intent_or_delete():
    capabilities = LoginCapabilities(scopes=OLD_BACKEND_SCOPES, include_intent=False)
    url = build_authorize_url(
        auth_url="https://auth.hello.food/authorize",
        client_id="client-1",
        redirect_uri="http://127.0.0.1:8765/callback",
        state="state-1",
        code_challenge="challenge-1",
        intent="register",
        scopes=capabilities.scopes,
        include_intent=capabilities.include_intent,
    )
    query = parse_qs(urlparse(url).query)
    assert query["scope"] == [" ".join(OLD_BACKEND_SCOPES)]
    assert "account:delete" not in query["scope"][0].split(" ")
    assert "intent" not in query


def test_authorize_url_new_backend_carries_intent_and_full_scopes():
    capabilities = LoginCapabilities(scopes=NEW_BACKEND_SCOPES, include_intent=True)
    url = build_authorize_url(
        auth_url="https://auth.hello.food/authorize",
        client_id="client-1",
        redirect_uri="http://127.0.0.1:8765/callback",
        state="state-1",
        code_challenge="challenge-1",
        intent="register",
        scopes=capabilities.scopes,
        include_intent=capabilities.include_intent,
    )
    query = parse_qs(urlparse(url).query)
    assert query["scope"] == [" ".join(LOGIN_SCOPES)]
    assert query["intent"] == ["create_account"]


# --- End-to-end: perform_device_login against an old backend ---------------


def test_perform_device_login_old_backend_sends_compatible_request(monkeypatch):
    """A full device login on a backend without account:delete must send neither
    the account:delete scope nor the intent field — the exact 0.3.0 breakage."""
    from unittest.mock import MagicMock

    store = MagicMock()
    store.get_device_id.return_value = "device-1"
    store.load.return_value = {}

    monkeypatch.setattr(
        "heyfood_cli.auth.fetch_supported_scopes",
        lambda *_a, **_k: list(OLD_BACKEND_SCOPES),
    )
    monkeypatch.setattr(
        "heyfood_cli.auth.register_client", lambda *_: {"client_id": "cid"}
    )
    captured = _capture_device_body(monkeypatch)
    monkeypatch.setattr(
        "heyfood_cli.auth.poll_device_authorization",
        lambda **_: {"access_token": "hf_ct", "refresh_token": "hf_cr", "expires_in": 3600},
    )
    monkeypatch.setattr(
        "heyfood_cli.auth.exchange_cli_session",
        lambda **_: {"access_token": "hf_at", "user_id": "user-a"},
    )

    auth.perform_device_login(
        store=store,
        api_url="https://api.hello.food",
        auth_url="https://auth.hello.food",
        api_key=None,
        open_browser=False,
        timeout_seconds=20,
        authorization_callback=lambda *_: None,
        intent="auto",
    )

    assert "intent" not in captured
    assert "account:delete" not in captured["scope"].split(" ")


# --- account delete: clean message when the server can't yet do it ---------


def test_account_delete_reports_server_unsupported(monkeypatch):
    from unittest.mock import MagicMock

    from typer.testing import CliRunner

    from heyfood_cli import main
    import json

    client = MagicMock()
    # The stored grant lacks account:delete because the live server never
    # offered it — not because the session needs refreshing.
    client.channel_scopes.return_value = {"account:link", "profile:read"}
    monkeypatch.setattr(main, "HelloFoodClient", lambda **_kwargs: client)

    result = CliRunner().invoke(main.app, ["account", "delete", "--yes", "--json"])

    assert result.exit_code == 1
    document = json.loads(result.stdout)
    assert document["error"]["type"] == "missing_account_delete_scope"
    assert "isn't available on this hello.food server yet" in document["error"]["message"]
    # The deletion flow itself is never entered.
    client.begin_account_deletion.assert_not_called()
