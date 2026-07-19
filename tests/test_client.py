from unittest.mock import MagicMock

import pytest

from heyfood_cli import diagnostics
from heyfood_cli.client import (
    ChannelToolUnavailable,
    HelloFoodClient,
    HelloFoodError,
    LoginRequired,
)
from heyfood_cli.config import ConfigStore


def test_recipe_save_payload_resolves_last_search_index(tmp_path):
    client = HelloFoodClient(store=ConfigStore(tmp_path / "config.json"))
    client.remember_recipe_search(
        {
            "query_used": "low FODMAP dinner",
            "recipes": [
                {
                    "title": "Grilled Lemon Garlic Chicken",
                    "spoonacular_id": 645753,
                    "recipe_ref": {
                        "provider": "spoonacular",
                        "external_id": "645753",
                    },
                    "ready_in_minutes": 45,
                    "dietary_tags": ["gluten_free", "low_fodmap"],
                    "can_save": True,
                }
            ],
        }
    )

    payload = client.recipe_save_payload("1")

    assert payload["spoonacular_id"] == 645753
    assert payload["recipe_ref"]["provider"] == "spoonacular"
    assert payload["title"] == "Grilled Lemon Garlic Chicken"
    assert payload["recipe_data"]["can_save"] is True


def test_recipe_save_payload_parses_provider_ref(tmp_path):
    client = HelloFoodClient(store=ConfigStore(tmp_path / "config.json"))

    payload = client.recipe_save_payload("spoonacular:645753")

    assert payload == {
        "recipe_ref": {
            "provider": "spoonacular",
            "external_id": "645753",
        }
    }


def test_recipe_save_payload_rejects_unknown_selector(tmp_path):
    client = HelloFoodClient(store=ConfigStore(tmp_path / "config.json"))

    try:
        client.recipe_save_payload("not-a-ref")
    except HelloFoodError as exc:
        assert "Use a recipe ref" in str(exc)
    else:
        raise AssertionError("expected HelloFoodError")


def test_remembers_restaurant_search_and_resolves_index(tmp_path):
    client = HelloFoodClient(store=ConfigStore(tmp_path / "config.json"))
    client.remember_restaurant_search(
        {
            "restaurants": [
                {"id": "rest-1", "name": "Thai Phuket", "has_menu": False},
                {"id": "rest-2", "name": "Pad Thai Restaurant", "has_menu": True},
            ]
        }
    )

    assert client.restaurant_id_from_selector("2") == "rest-2"
    assert client.restaurant_from_selector("1")["name"] == "Thai Phuket"
    assert client.restaurant_id_from_selector("ChIJabc") == "ChIJabc"


def test_restaurant_index_requires_previous_search(tmp_path):
    client = HelloFoodClient(store=ConfigStore(tmp_path / "config.json"))

    try:
        client.restaurant_id_from_selector("1")
    except HelloFoodError as exc:
        assert "No previous restaurant search" in str(exc)
    else:
        raise AssertionError("expected HelloFoodError")


def test_channel_tool_unavailable_wraps_plain_route_404(tmp_path, monkeypatch):
    client = HelloFoodClient(store=ConfigStore(tmp_path / "config.json"))

    def fake_request(method, path, **kwargs):
        raise HelloFoodError("404: Not Found")

    monkeypatch.setattr(client, "_request", fake_request)

    try:
        client.channel_tool("search_recipes", {})
    except ChannelToolUnavailable as exc:
        assert "search_recipes" in str(exc)
    else:
        raise AssertionError("expected ChannelToolUnavailable")


def test_channel_tool_refreshes_once_on_invalid_channel_token(tmp_path, monkeypatch):
    client = _client_with_tokens(tmp_path, monkeypatch)
    calls = []
    events = []

    def fake_request(method, path, **kwargs):
        calls.append(path)
        if len(calls) == 1:
            raise HelloFoodError("401: Invalid or expired channel token")
        return {"ok": True}

    refreshed = []
    monkeypatch.setattr(client, "_request", fake_request)
    monkeypatch.setattr(client, "refresh_channel", lambda: refreshed.append(True))
    monkeypatch.setattr(
        diagnostics.reporter,
        "emit",
        lambda event, **fields: events.append((event, fields)),
    )

    assert client.channel_tool("search_restaurant", {}) == {"ok": True}
    assert refreshed == [True]
    assert calls == ["/v1/channel/tools/search_restaurant", "/v1/channel/tools/search_restaurant"]
    assert events == [
        (
            "http.retry_after_channel_refresh",
            {
                "context": "production",
                "endpoint": "/v1/channel/tools/search_restaurant",
                "attempt": 2,
            },
        )
    ]


def test_stream_agent_emits_lifecycle_without_payload(tmp_path, monkeypatch):
    client = _client_with_tokens(tmp_path, monkeypatch)
    client.config["session"]["access_expires_at"] = "2999-01-01T00:00:00+00:00"
    events = []
    captured_headers = {}

    class FakeResponse:
        status_code = 200
        headers = {"X-Request-ID": "server-stream-1"}

        def __enter__(self):
            return self

        def __exit__(self, *_args):
            return False

        def iter_lines(self):
            return iter(
                (
                    "event: result",
                    'data: {"message":"done"}',
                    "",
                )
            )

    class FakeHTTPClient:
        def __init__(self, **_kwargs):
            pass

        def __enter__(self):
            return self

        def __exit__(self, *_args):
            return False

        def stream(self, _method, _url, *, headers, json):
            captured_headers.update(headers)
            assert json == {"query": "private dietary request"}
            return FakeResponse()

    monkeypatch.setattr("heyfood_cli.client.httpx.Client", FakeHTTPClient)
    monkeypatch.setattr(
        diagnostics.reporter,
        "emit",
        lambda event, **fields: events.append((event, fields)),
    )

    result = list(client.stream_agent({"query": "private dietary request"}))

    assert result == [("result", {"message": "done"})]
    assert captured_headers["X-Request-ID"] == events[0][1]["request_id"]
    assert [event for event, _fields in events] == [
        "http.stream_start",
        "http.stream_complete",
    ]
    assert events[-1][1]["server_request_id"] == "server-stream-1"
    assert "private dietary request" not in str(events)


def test_channel_tool_requires_login_after_invalid_channel_token_retry(tmp_path, monkeypatch):
    from heyfood_cli.client import LoginRequired

    client = _client_with_tokens(tmp_path, monkeypatch)

    def fake_request(method, path, **kwargs):
        raise HelloFoodError("401: Invalid or expired channel token")

    monkeypatch.setattr(client, "_request", fake_request)
    monkeypatch.setattr(client, "refresh_channel", lambda: None)

    try:
        client.channel_tool("search_restaurant", {})
    except LoginRequired as exc:
        assert "heyfood login" in str(exc)
    else:
        raise AssertionError("expected LoginRequired")


def test_get_menu_status_uses_channel_poll_tool(tmp_path, monkeypatch):
    client = HelloFoodClient(store=ConfigStore(tmp_path / "config.json"))
    calls = []
    monkeypatch.setattr(
        client,
        "channel_tool",
        lambda name, payload: calls.append((name, payload)) or {"status": "ready"},
    )

    result = client.get_menu_status(restaurant_id="rest-1", job_id="job-1")

    assert result == {"status": "ready"}
    assert calls == [
        (
            "get_menu_status",
            {"restaurant_id": "rest-1", "job_id": "job-1"},
        )
    ]


def test_list_profile_members_uses_session_endpoint(tmp_path, monkeypatch):
    client = HelloFoodClient(store=ConfigStore(tmp_path / "config.json"))
    calls = []
    monkeypatch.setattr(
        client,
        "_request",
        lambda method, path, **kwargs: calls.append((method, path, kwargs))
        or {"profiles": [], "total_count": 0},
    )

    result = client.list_profile_members()

    assert result == {"profiles": [], "total_count": 0}
    assert calls == [
        ("GET", "/v1/profile/sync/members", {"auth": "session"})
    ]


def test_list_channel_links_uses_account_session(tmp_path, monkeypatch):
    client = HelloFoodClient(store=ConfigStore(tmp_path / "config.json"))
    calls = []
    monkeypatch.setattr(
        client,
        "_request",
        lambda method, path, **kwargs: calls.append((method, path, kwargs))
        or {"links": [], "total_count": 0},
    )

    assert client.list_channel_links() == {"links": [], "total_count": 0}
    assert calls == [("GET", "/v1/channel/links", {"auth": "session"})]


def test_disconnect_channel_link_revokes_owned_link(tmp_path, monkeypatch):
    client = HelloFoodClient(store=ConfigStore(tmp_path / "config.json"))
    calls = []
    monkeypatch.setattr(
        client,
        "_request",
        lambda method, path, **kwargs: calls.append((method, path, kwargs))
        or {"revoked": True, "link_id": "link-1"},
    )

    assert client.disconnect_channel_link("link-1")["revoked"] is True
    assert calls == [
        ("DELETE", "/v1/channel/links/link-1", {"auth": "session"})
    ]


def test_channel_whoami_requires_login_after_invalid_channel_token_retry(tmp_path, monkeypatch):
    from heyfood_cli.client import LoginRequired

    client = _client_with_tokens(tmp_path, monkeypatch)

    def fake_request(method, path, **kwargs):
        raise HelloFoodError("401: Invalid or expired channel token")

    monkeypatch.setattr(client, "_request", fake_request)
    monkeypatch.setattr(client, "refresh_channel", lambda: None)

    try:
        client.channel_whoami()
    except LoginRequired as exc:
        assert "heyfood login" in str(exc)
    else:
        raise AssertionError("expected LoginRequired")


def test_logout_revokes_link_and_device_before_session_then_wipes_store(tmp_path):
    config_path = tmp_path / "config.json"
    store = ConfigStore(config_path)
    store.save(
        {
            "api_key": "test-key",
            "device_id": "heyfood-cli-test-device",
            "session": {"access_token": "hf_at_x", "refresh_token": "hf_rt_x"},
            "oauth": {"link_id": "link-123", "access_token": "hf_ct_x"},
        }
    )
    client = HelloFoodClient(store=store)

    calls = []

    def fake_request(method, path, **kwargs):
        calls.append((method, path, kwargs.get("json_body")))
        return {}

    client._request = fake_request

    result = client.revoke_local_session()

    # The session token authenticates the link and device calls, so the
    # session itself must be revoked last.
    assert [(method, path) for method, path, _ in calls] == [
        ("DELETE", "/v1/channel/links/link-123"),
        ("POST", "/v1/auth/device/revoke"),
        ("POST", "/v1/auth/session/revoke"),
    ]
    assert calls[1][2] == {
        "device_id": "heyfood-cli-test-device",
        "reason": "cli_logout",
    }
    assert result == {
        "ok": True,
        "remote_complete": True,
        "teardown": {
            "link": {"attempted": True, "ok": True},
            "device": {"attempted": True, "ok": True},
            "session": {"attempted": True, "ok": True},
        },
        "local_credentials_cleared": True,
    }
    assert not config_path.exists()


def test_logout_reports_remote_failures_without_exposing_tokens(tmp_path):
    store = ConfigStore(tmp_path / "config.json", credential_store=None)
    store.save(
        {
            "device_id": "device-1",
            "session": {"access_token": "hf_at_secret"},
            "oauth": {"link_id": "link-1", "access_token": "hf_ct_secret"},
        }
    )
    client = HelloFoodClient(store=store)
    client._request = lambda *_args, **_kwargs: (_ for _ in ()).throw(
        HelloFoodError("offline hf_at_secret")
    )

    result = client.revoke_local_session()

    assert result["ok"] is True
    assert result["remote_complete"] is False
    assert result["local_credentials_cleared"] is True
    assert "secret" not in str(result)
    assert not store.path.exists()


def _client_with_tokens(tmp_path, monkeypatch):
    from heyfood_cli import config as config_mod

    monkeypatch.setattr(config_mod, "DEFAULT_API_KEY", "", raising=False)
    client = HelloFoodClient(store=ConfigStore(tmp_path / "config.json"))
    client.config["session"] = {
        "access_token": "stale-access",
        "refresh_token": "stale-refresh",
        "access_expires_at": "2020-01-01T00:00:00+00:00",
    }
    client.config["oauth"] = {
        "access_token": "channel-access",
        "refresh_token": "channel-refresh",
        "client_id": "client-1",
        "access_expires_at": "2999-01-01T00:00:00+00:00",
    }
    client.config["credential_api_url"] = client.api_url
    return client


def test_context_switch_cannot_send_production_session_token(tmp_path, monkeypatch):
    store = ConfigStore(tmp_path / "config.json", credential_store=None)
    store.save(
        {
            "active_context": "custom",
            "contexts": {
                "custom": {
                    "api_url": "https://custom.example",
                    "auth_url": "https://auth.custom.example/authorize",
                }
            },
            "credential_api_url": "https://api.hello.food",
            "session": {
                "access_token": "production-session-secret",
                "access_expires_at": "2999-01-01T00:00:00+00:00",
            },
        }
    )
    request = MagicMock()
    monkeypatch.setattr("heyfood_cli.client.httpx.Client.request", request)
    client = HelloFoodClient(store=store, create_device=False)

    with pytest.raises(LoginRequired, match="different API context"):
        client.list_channel_links()

    request.assert_not_called()


def test_environment_override_cannot_send_bound_channel_or_api_key(
    tmp_path, monkeypatch
):
    store = ConfigStore(tmp_path / "config.json", credential_store=None)
    store.save(
        {
            "api_url": "https://api.hello.food",
            "auth_url": "https://auth.hello.food/authorize",
            "credential_api_url": "https://api.hello.food",
            "api_key": "production-api-key",
            "oauth": {
                "access_token": "production-channel-secret",
                "access_expires_at": "2999-01-01T00:00:00+00:00",
            },
        }
    )
    monkeypatch.setenv("HEYFOOD_API_URL", "https://custom.example")
    monkeypatch.setenv("HEYFOOD_AUTH_URL", "https://auth.custom.example/authorize")
    request = MagicMock()
    monkeypatch.setattr("heyfood_cli.client.httpx.Client.request", request)
    client = HelloFoodClient(store=store, create_device=False)

    with pytest.raises(LoginRequired, match="different API context"):
        client.channel_whoami()
    with pytest.raises(LoginRequired, match="different API context"):
        client._request("GET", "/public", auth=None)

    request.assert_not_called()


def test_legacy_credentials_adopt_only_the_exact_stored_api_origin(tmp_path):
    store = ConfigStore(tmp_path / "config.json", credential_store=None)
    store.save(
        {
            "api_url": "https://api.hello.food/",
            "auth_url": "https://auth.hello.food/authorize",
            "session": {
                "access_token": "legacy-session",
                "access_expires_at": "2999-01-01T00:00:00+00:00",
            },
        }
    )
    client = HelloFoodClient(store=store, create_device=False)

    assert client.session_access_token() == "legacy-session"
    assert store.load()["credential_api_url"] == "https://api.hello.food"


def test_refresh_session_falls_back_to_channel_reexchange(tmp_path, monkeypatch):
    client = _client_with_tokens(tmp_path, monkeypatch)
    calls = []

    def fake_request(method, path, **kwargs):
        calls.append(path)
        if path == "/v1/auth/session/refresh":
            raise HelloFoodError("401: Missing API key. Include X-API-Key header.")
        if path == "/v1/channel/oauth/cli/session":
            assert kwargs.get("auth") == "channel"
            return {
                "access_token": "fresh-access",
                "refresh_token": "fresh-refresh",
                "access_expires_at": "2999-01-01T00:00:00+00:00",
            }
        raise AssertionError(f"unexpected path {path}")

    monkeypatch.setattr(client, "_request", fake_request)

    client.refresh_session()

    assert calls == ["/v1/auth/session/refresh", "/v1/channel/oauth/cli/session"]
    assert client.config["session"]["access_token"] == "fresh-access"


def test_refresh_session_raises_login_required_when_reexchange_fails(tmp_path, monkeypatch):
    from heyfood_cli.client import LoginRequired

    client = _client_with_tokens(tmp_path, monkeypatch)

    def fake_request(method, path, **kwargs):
        raise HelloFoodError("401: nope")

    monkeypatch.setattr(client, "_request", fake_request)

    try:
        client.refresh_session()
    except LoginRequired as exc:
        assert "heyfood login" in str(exc)
    else:
        raise AssertionError("expected LoginRequired")


def test_refresh_session_without_refresh_token_reexchanges(tmp_path, monkeypatch):
    client = _client_with_tokens(tmp_path, monkeypatch)
    client.config["session"].pop("refresh_token")

    def fake_request(method, path, **kwargs):
        assert path == "/v1/channel/oauth/cli/session"
        return {
            "access_token": "fresh-access",
            "access_expires_at": "2999-01-01T00:00:00+00:00",
        }

    monkeypatch.setattr(client, "_request", fake_request)

    client.refresh_session()

    assert client.config["session"]["access_token"] == "fresh-access"


def test_refresh_channel_failure_requires_login(tmp_path, monkeypatch):
    from heyfood_cli.client import LoginRequired

    client = _client_with_tokens(tmp_path, monkeypatch)
    client.config["oauth"]["access_expires_at"] = "2020-01-01T00:00:00+00:00"

    def fake_request(method, path, **kwargs):
        raise HelloFoodError("401: Invalid or expired refresh token")

    monkeypatch.setattr(client, "_request", fake_request)

    try:
        client.channel_access_token()
    except LoginRequired as exc:
        assert "heyfood login" in str(exc)
    else:
        raise AssertionError("expected LoginRequired")


def test_save_and_read_location(tmp_path):
    client = HelloFoodClient(store=ConfigStore(tmp_path / "config.json"))
    client.save_location(label="Home", latitude=35.2828, longitude=-120.6596, radius_miles=8.0)

    saved = client.saved_location()
    assert saved is not None
    assert saved["label"] == "Home"
    assert saved["latitude"] == 35.2828
    assert saved["longitude"] == -120.6596
    assert saved["radius_miles"] == 8.0
    assert "updated_at" in saved

    # Survives a reload from disk.
    reloaded = HelloFoodClient(store=ConfigStore(tmp_path / "config.json"))
    assert reloaded.saved_location()["latitude"] == 35.2828


def test_saved_location_defaults_to_none_when_unset(tmp_path):
    client = HelloFoodClient(store=ConfigStore(tmp_path / "config.json"))
    assert client.saved_location() is None


def test_saved_location_rejects_non_numeric_coordinates(tmp_path):
    client = HelloFoodClient(store=ConfigStore(tmp_path / "config.json"))
    client.config["location"] = {"label": "bad", "latitude": "nope", "longitude": None}
    assert client.saved_location() is None


def test_saved_location_rejects_bool_coordinates(tmp_path):
    client = HelloFoodClient(store=ConfigStore(tmp_path / "config.json"))
    client.config["location"] = {"label": "bool", "latitude": True, "longitude": False}
    assert client.saved_location() is None


def test_clear_location(tmp_path):
    client = HelloFoodClient(store=ConfigStore(tmp_path / "config.json"))
    client.save_location(label="Home", latitude=1.0, longitude=2.0)
    assert client.clear_location() is True
    assert client.saved_location() is None
    # Second clear is a no-op.
    assert client.clear_location() is False


def test_geocode_location_calls_channel_tool(tmp_path, monkeypatch):
    client = HelloFoodClient(store=ConfigStore(tmp_path / "config.json"))
    calls = []

    def fake_channel_tool(name, payload):
        calls.append((name, payload))
        return {"label": "San Luis Obispo, CA", "latitude": 35.28, "longitude": -120.66}

    monkeypatch.setattr(client, "channel_tool", fake_channel_tool)
    data = client.geocode_location("San Luis Obispo")
    assert calls == [("geocode_location", {"query": "San Luis Obispo"})]
    assert data["latitude"] == 35.28


def test_geocode_location_not_found_is_not_tool_unavailable(tmp_path, monkeypatch):
    """A place-not-found 404 must stay a plain HelloFoodError so the CLI can show
    the friendly 'couldn't find location' message. Only the exact route-level
    '404: Not Found' means 'tool not deployed' (ChannelToolUnavailable). This is
    the explicit contrast to test_channel_tool_unavailable_wraps_plain_route_404.
    """
    client = HelloFoodClient(store=ConfigStore(tmp_path / "config.json"))

    def fake_request(method, path, **kwargs):
        raise HelloFoodError("404: location_not_found")

    monkeypatch.setattr(client, "_request", fake_request)

    try:
        client.geocode_location("Nowheresville")
    except ChannelToolUnavailable:  # noqa
        raise AssertionError("location_not_found must NOT be treated as tool-unavailable")
    except HelloFoodError as exc:
        assert "location_not_found" in str(exc)
    else:
        raise AssertionError("expected HelloFoodError")
