from unittest.mock import MagicMock
from urllib.parse import parse_qs, urlparse

import httpx
import pytest

from heyfood_cli.auth import (
    LOGIN_SCOPES,
    LoginFlowError,
    build_authorize_url,
    normalize_auth_url,
    perform_device_login,
    perform_login,
    pkce_pair,
    poll_device_authorization,
)


def test_normalize_auth_url_appends_authorize():
    assert normalize_auth_url("http://localhost:3002") == "http://localhost:3002/authorize"


def test_build_authorize_url_contains_cli_scope():
    _, challenge = pkce_pair()
    url = build_authorize_url(
        auth_url="http://localhost:3002/authorize",
        client_id="client-1",
        redirect_uri="http://127.0.0.1:8765/callback",
        state="state-1",
        code_challenge=challenge,
    )
    assert "account%3Alink" in url
    assert "client_id=client-1" in url
    assert "code_challenge_method=S256" in url
    assert parse_qs(urlparse(url).query)["scope"] == [" ".join(LOGIN_SCOPES)]
    assert LOGIN_SCOPES == [
        "account:link",
        "knowledge:read",
        "menu:read",
        "recommend:read",
        "recipes:read",
        "recipes:write",
        "claims:read_derived",
        "profile:read",
        "profile:write",
        "meals:read",
        "meals:write",
    ]


def test_device_login_persists_both_token_bundles(monkeypatch):
    store = MagicMock()
    store.get_device_id.return_value = "heyfood-cli-device-1"
    store.load.return_value = {}
    callback = MagicMock()
    monkeypatch.setattr(
        "heyfood_cli.auth.register_client",
        lambda *_: {"client_id": "hf_cid_device"},
    )
    monkeypatch.setattr(
        "heyfood_cli.auth.start_device_authorization",
        lambda *_: {
            "device_code": "hf_dc_test",
            "user_code": "ABCD-EFGH",
            "verification_uri": "https://auth.hello.food/authorize?flow=device",
            "verification_uri_complete": (
                "https://auth.hello.food/authorize?flow=device&user_code=ABCD-EFGH"
            ),
            "expires_in": 600,
            "interval": 5,
        },
    )
    monkeypatch.setattr(
        "heyfood_cli.auth.poll_device_authorization",
        lambda **_: {
            "access_token": "hf_ct_test",
            "refresh_token": "hf_cr_test",
            "expires_in": 3600,
            "scope": " ".join(LOGIN_SCOPES),
            "link_id": "link-1",
        },
    )
    monkeypatch.setattr(
        "heyfood_cli.auth.exchange_cli_session",
        lambda **_: {"access_token": "hf_at_test", "refresh_token": "hf_rt_test"},
    )

    result = perform_device_login(
        store=store,
        api_url="https://api.hello.food",
        auth_url="https://auth.hello.food",
        api_key=None,
        open_browser=False,
        timeout_seconds=600,
        authorization_callback=callback,
    )

    callback.assert_called_once_with(
        "https://auth.hello.food/authorize?flow=device&user_code=ABCD-EFGH",
        "ABCD-EFGH",
    )
    assert result["oauth"]["access_token"] == "hf_ct_test"
    assert result["session"]["access_token"] == "hf_at_test"
    store.save.assert_called_once_with(result)


def test_device_poll_handles_pending_then_success(monkeypatch):
    responses = [
        httpx.Response(
            400,
            json={"error": "authorization_pending", "error_description": "pending"},
        ),
        httpx.Response(
            200,
            json={"access_token": "hf_ct_test", "refresh_token": "hf_cr_test"},
        ),
    ]

    class FakeClient:
        def __init__(self, **_):
            pass

        def __enter__(self):
            return self

        def __exit__(self, *_):
            return False

        def post(self, *_args, **_kwargs):
            return responses.pop(0)

    monkeypatch.setattr("heyfood_cli.auth.httpx.Client", FakeClient)
    monkeypatch.setattr("heyfood_cli.auth.time.sleep", lambda _: None)
    result = poll_device_authorization(
        api_url="https://api.hello.food",
        client_id="hf_cid_device",
        device_code="hf_dc_test",
        interval_seconds=5,
        timeout_seconds=60,
    )
    assert result["access_token"] == "hf_ct_test"


def test_loopback_bind_failure_recommends_device_flow(monkeypatch):
    store = MagicMock()
    store.get_device_id.return_value = "device-1"

    def fail_callback_server():
        raise OSError("address unavailable")

    monkeypatch.setattr("heyfood_cli.auth.OAuthCallbackServer", fail_callback_server)
    with pytest.raises(LoginFlowError, match="login --device"):
        perform_login(
            store=store,
            api_url="https://api.hello.food",
            auth_url="https://auth.hello.food",
            api_key=None,
            open_browser=False,
            timeout_seconds=10,
        )


class FakeCallbackServer:
    port = 8765

    def __init__(self, result=None, error=None):
        self.result = result or {}
        self.error = error

    def __enter__(self):
        return self

    def __exit__(self, *_):
        return False

    def wait(self, _timeout):
        if self.error:
            raise self.error
        return self.result


def test_loopback_login_reports_browser_denial(monkeypatch):
    store = MagicMock()
    store.get_device_id.return_value = "device-1"
    monkeypatch.setattr(
        "heyfood_cli.auth.OAuthCallbackServer",
        lambda: FakeCallbackServer({"error": "access_denied"}),
    )
    monkeypatch.setattr(
        "heyfood_cli.auth.register_client",
        lambda *_: {"client_id": "client-1"},
    )
    with pytest.raises(LoginFlowError, match="denied"):
        perform_login(
            store=store,
            api_url="https://api.hello.food",
            auth_url="https://auth.hello.food",
            api_key=None,
            open_browser=False,
            timeout_seconds=10,
        )


def test_loopback_login_rejects_state_mismatch(monkeypatch):
    store = MagicMock()
    store.get_device_id.return_value = "device-1"
    monkeypatch.setattr(
        "heyfood_cli.auth.OAuthCallbackServer",
        lambda: FakeCallbackServer({"state": "wrong", "code": "code-1"}),
    )
    monkeypatch.setattr(
        "heyfood_cli.auth.register_client",
        lambda *_: {"client_id": "client-1"},
    )
    with pytest.raises(LoginFlowError, match="state mismatch"):
        perform_login(
            store=store,
            api_url="https://api.hello.food",
            auth_url="https://auth.hello.food",
            api_key=None,
            open_browser=False,
            timeout_seconds=10,
        )


def test_loopback_login_preserves_timeout_guidance(monkeypatch):
    store = MagicMock()
    store.get_device_id.return_value = "device-1"
    monkeypatch.setattr(
        "heyfood_cli.auth.OAuthCallbackServer",
        lambda: FakeCallbackServer(error=LoginFlowError("Timed out waiting for browser login.")),
    )
    monkeypatch.setattr(
        "heyfood_cli.auth.register_client",
        lambda *_: {"client_id": "client-1"},
    )
    with pytest.raises(LoginFlowError, match="Timed out waiting"):
        perform_login(
            store=store,
            api_url="https://api.hello.food",
            auth_url="https://auth.hello.food",
            api_key=None,
            open_browser=False,
            timeout_seconds=1,
        )


def test_client_registration_wraps_offline_error(monkeypatch):
    class OfflineClient:
        def __init__(self, **_):
            pass

        def __enter__(self):
            return self

        def __exit__(self, *_):
            return False

        def post(self, *_args, **_kwargs):
            raise httpx.ConnectError("offline")

    monkeypatch.setattr("heyfood_cli.auth.httpx.Client", OfflineClient)
    from heyfood_cli.auth import register_client

    with pytest.raises(LoginFlowError, match="client registration"):
        register_client("https://api.hello.food", "http://127.0.0.1:8765/callback")
