from unittest.mock import MagicMock
from urllib.parse import parse_qs, urlparse

import httpx
import pytest

from heyfood_cli.auth import (
    DEVICE_LOGIN_DENIED_MESSAGE,
    DEVICE_LOGIN_EXPIRED_MESSAGE,
    DEVICE_LOGIN_INTERRUPTED_MESSAGE,
    DEVICE_LOGIN_INVALID_GRANT_MESSAGE,
    DEVICE_LOGIN_NETWORK_MESSAGE,
    DEVICE_LOGIN_UNAVAILABLE_MESSAGE,
    LOGIN_SCOPES,
    LoginCapabilities,
    LoginFlowError,
    LoginInterrupted,
    build_authorize_url,
    normalize_auth_url,
    perform_device_login,
    perform_login,
    pkce_pair,
    poll_device_authorization,
    resolve_oauth_client_id,
    start_device_authorization,
)
from heyfood_cli.config import OFFICIAL_CLI_OAUTH_CLIENT_ID


def _full_capabilities(*_args, **_kwargs) -> LoginCapabilities:
    """Discovery stub: behave as an unreachable-metadata fallback (full scopes,
    intent on) so login-flow tests never touch the network."""
    return LoginCapabilities(scopes=list(LOGIN_SCOPES), include_intent=True)


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
        "account:delete",
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
        "audio:transcribe",
    ]


def test_device_login_persists_both_token_bundles(monkeypatch):
    store = MagicMock()
    store.get_device_id.return_value = "heyfood-cli-device-1"
    store.load.return_value = {}
    callback = MagicMock()
    monkeypatch.setattr("heyfood_cli.auth.resolve_login_capabilities", _full_capabilities)
    monkeypatch.setattr(
        "heyfood_cli.auth.register_client",
        lambda *_: {"client_id": "hf_cid_device"},
    )
    monkeypatch.setattr(
        "heyfood_cli.auth.start_device_authorization",
        lambda *a, **k: {
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
        lambda **_: {
            "access_token": "hf_at_test",
            "refresh_token": "hf_rt_test",
            "user_id": "user-a",
        },
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
    assert result["oauth"]["client_id"] == OFFICIAL_CLI_OAUTH_CLIENT_ID
    assert result["session"]["access_token"] == "hf_at_test"
    assert result["account_user_id"] == "user-a"
    store.save.assert_called_once_with(result)


def test_official_service_uses_server_owned_public_client(monkeypatch):
    register = MagicMock()
    monkeypatch.setattr("heyfood_cli.auth.register_client", register)

    assert resolve_oauth_client_id(
        "https://api.hello.food/",
        "http://127.0.0.1:54321/callback",
    ) == OFFICIAL_CLI_OAUTH_CLIENT_ID
    register.assert_not_called()


def test_custom_service_retains_dynamic_client_registration(monkeypatch):
    register = MagicMock(return_value={"client_id": "hf_cid_custom"})
    monkeypatch.setattr("heyfood_cli.auth.register_client", register)

    assert resolve_oauth_client_id(
        "https://compatible.example",
        "http://127.0.0.1:54321/callback",
    ) == "hf_cid_custom"
    register.assert_called_once_with(
        "https://compatible.example",
        "http://127.0.0.1:54321/callback",
    )


@pytest.mark.parametrize(
    ("intent", "wire_intent"),
    [
        ("register", "create_account"),
        ("login", "sign_in"),
        ("auto", "auto"),
    ],
)
def test_device_authorization_sends_backend_intent(monkeypatch, intent, wire_intent):
    request = {}
    response = httpx.Response(
        200,
        json={
            "device_code": "hf_dc_test",
            "user_code": "ABCD-EFGH",
            "verification_uri": "https://auth.hello.food/authorize?flow=device",
            "expires_in": 600,
            "interval": 5,
        },
    )

    def fake_post(_api_url, _path, *, json_body, **_kwargs):
        request.update(json_body)
        return response

    monkeypatch.setattr("heyfood_cli.auth._post_with_diagnostics", fake_post)

    start_device_authorization("https://api.hello.food", "client-1", intent)

    assert request == {
        "client_id": "client-1",
        "scope": " ".join(LOGIN_SCOPES),
        "intent": wire_intent,
    }


def test_authenticated_account_change_clears_account_scoped_local_state():
    from heyfood_cli.auth import _save_authenticated_config

    store = MagicMock()
    store.load.return_value = {
        "account_user_id": "user-a",
        "household": {"members": [{"id": "child-1", "name": "Emma"}]},
        "household_local_profiles": {"child-1": {"restrictions": ["peanuts"]}},
        "household_profile_outbox": {
            "adult-1": {
                "fields": {"restrictions": ["dairyFree"]},
                "local_context": {"restrictions": ["dairyFree"]},
            }
        },
        "last_conversation": {"conversation_id": "account-a-conversation"},
        "location": {"label": "Account A home"},
    }

    saved = _save_authenticated_config(
        store=store,
        api_url="https://api.hello.food",
        auth_url="https://auth.hello.food/authorize",
        api_key="",
        device_id="device-1",
        client_id="client-1",
        oauth_bundle={
            "access_token": "channel-access",
            "refresh_token": "channel-refresh",
        },
        session_bundle={
            "access_token": "session-access",
            "refresh_token": "session-refresh",
            "user_id": "user-b",
        },
    )

    assert saved["account_user_id"] == "user-b"
    for key in (
        "household",
        "household_local_profiles",
        "household_profile_outbox",
        "last_conversation",
        "location",
    ):
        assert key not in saved
    store.save.assert_called_once_with(saved)


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


def test_loopback_login_uses_fixed_client_and_explicit_port(monkeypatch):
    store = MagicMock()
    store.get_device_id.return_value = "device-1"
    store.load.return_value = {}
    authorize_url_callback = MagicMock()
    exchange_code = MagicMock(
        return_value={
            "access_token": "hf_ct_test",
            "refresh_token": "hf_cr_test",
            "expires_in": 3600,
        }
    )
    monkeypatch.setattr("heyfood_cli.auth.resolve_login_capabilities", _full_capabilities)
    monkeypatch.setattr(
        "heyfood_cli.auth.OAuthCallbackServer",
        lambda: FakeCallbackServer({"state": "state-1", "code": "code-1"}),
    )
    monkeypatch.setattr("heyfood_cli.auth.pkce_pair", lambda: ("verifier-1", "challenge-1"))
    monkeypatch.setattr("heyfood_cli.auth.secrets.token_urlsafe", lambda _: "state-1")
    monkeypatch.setattr("heyfood_cli.auth.exchange_code", exchange_code)
    monkeypatch.setattr(
        "heyfood_cli.auth.exchange_cli_session",
        lambda **_: {
            "access_token": "hf_at_test",
            "refresh_token": "hf_rt_test",
            "user_id": "user-a",
        },
    )

    result = perform_login(
        store=store,
        api_url="https://api.hello.food",
        auth_url="https://auth.hello.food",
        api_key=None,
        open_browser=False,
        timeout_seconds=10,
        authorize_url_callback=authorize_url_callback,
    )

    authorize_params = parse_qs(urlparse(authorize_url_callback.call_args.args[0]).query)
    assert authorize_params["client_id"] == [OFFICIAL_CLI_OAUTH_CLIENT_ID]
    assert authorize_params["redirect_uri"] == ["http://127.0.0.1:8765/callback"]
    exchange_code.assert_called_once_with(
        api_url="https://api.hello.food",
        client_id=OFFICIAL_CLI_OAUTH_CLIENT_ID,
        code="code-1",
        verifier="verifier-1",
        redirect_uri="http://127.0.0.1:8765/callback",
    )
    assert result["oauth"]["client_id"] == OFFICIAL_CLI_OAUTH_CLIENT_ID


def test_loopback_login_reports_browser_denial(monkeypatch):
    store = MagicMock()
    store.get_device_id.return_value = "device-1"
    monkeypatch.setattr("heyfood_cli.auth.resolve_login_capabilities", _full_capabilities)
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
    monkeypatch.setattr("heyfood_cli.auth.resolve_login_capabilities", _full_capabilities)
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
    monkeypatch.setattr("heyfood_cli.auth.resolve_login_capabilities", _full_capabilities)
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


# --- Device-flow polling contract ------------------------------------------
#
# The device flow polls a token endpoint until the user approves, denies, or the
# advertised window closes. These tests drive that loop on a fake clock (no real
# sleeping) with a scripted transport so we can assert the reliability contract:
# jittered cadence, an exact deadline, transient-error tolerance, classified
# terminal outcomes, and a clean Ctrl-C.


class FakeClock:
    """Deterministic monotonic clock; sleeping advances virtual time."""

    def __init__(self, start: float = 0.0):
        self.now = start
        self.sleeps: list[float] = []

    def monotonic(self) -> float:
        return self.now

    def sleep(self, seconds: float) -> None:
        self.sleeps.append(seconds)
        self.now += seconds


class ScriptedClient:
    """Fake httpx.Client whose post() replays a scripted sequence of ticks.

    Each script entry is either an ``httpx.Response`` (returned) or an exception
    instance (raised). A single trailing entry repeats forever, which lets a test
    model an endpoint that never resolves.
    """

    def __init__(self, script):
        self._script = list(script)

    def __enter__(self):
        return self

    def __exit__(self, *_):
        return False

    def post(self, *_args, **_kwargs):
        entry = self._script[0] if len(self._script) == 1 else self._script.pop(0)
        if isinstance(entry, BaseException):
            raise entry
        return entry


def _pending(interval: int = 5) -> httpx.Response:
    return httpx.Response(400, json={"error": "authorization_pending"})


def _slow_down() -> httpx.Response:
    return httpx.Response(400, json={"error": "slow_down"})


def _approved() -> httpx.Response:
    return httpx.Response(200, json={"access_token": "hf_ct", "refresh_token": "hf_cr"})


def _install_client(monkeypatch, script):
    monkeypatch.setattr(
        "heyfood_cli.auth.httpx.Client", lambda **_: ScriptedClient(script)
    )


def test_device_poll_jitter_slow_down_and_transient_retry(monkeypatch):
    clock = FakeClock()
    _install_client(
        monkeypatch,
        [
            _pending(),
            _slow_down(),
            httpx.ConnectError("blip"),
            _approved(),
        ],
    )
    result = poll_device_authorization(
        api_url="https://api.hello.food",
        client_id="hf_cid",
        device_code="hf_dc",
        interval_seconds=5,
        timeout_seconds=600,
        expires_in=600,
        authorized_at=0.0,
        sleep=clock.sleep,
        monotonic=clock.monotonic,
        jitter=lambda: 0.5,
    )
    assert result["access_token"] == "hf_ct"
    # pending waits interval+jitter; slow_down raises interval by 5 (RFC 8628);
    # the transport blip retries on the same, now-raised cadence.
    assert clock.sleeps == [5.5, 10.5, 10.5]


def test_device_poll_jitter_stays_within_one_second_of_interval(monkeypatch):
    clock = FakeClock()
    _install_client(monkeypatch, [_pending(), _approved()])
    poll_device_authorization(
        api_url="https://api.hello.food",
        client_id="hf_cid",
        device_code="hf_dc",
        interval_seconds=5,
        timeout_seconds=600,
        expires_in=600,
        authorized_at=0.0,
        sleep=clock.sleep,
        monotonic=clock.monotonic,
        # Real jitter distribution; assert only the bounds.
    )
    assert len(clock.sleeps) == 1
    assert 5.0 <= clock.sleeps[0] <= 6.0


def test_device_poll_never_polls_past_advertised_deadline(monkeypatch):
    clock = FakeClock()
    _install_client(monkeypatch, [_pending()])  # never resolves
    with pytest.raises(LoginFlowError) as excinfo:
        poll_device_authorization(
            api_url="https://api.hello.food",
            client_id="hf_cid",
            device_code="hf_dc",
            interval_seconds=5,
            timeout_seconds=600,  # user timeout is generous
            expires_in=30,  # advertised window is the binding limit
            authorized_at=0.0,
            sleep=clock.sleep,
            monotonic=clock.monotonic,
            jitter=lambda: 0.0,
        )
    # Stopped exactly at the advertised deadline, not one tick past it.
    assert clock.now == 30
    assert str(excinfo.value) == DEVICE_LOGIN_EXPIRED_MESSAGE


def test_device_poll_clamps_final_sleep_to_deadline(monkeypatch):
    clock = FakeClock()
    _install_client(monkeypatch, [_pending()])
    with pytest.raises(LoginFlowError):
        poll_device_authorization(
            api_url="https://api.hello.food",
            client_id="hf_cid",
            device_code="hf_dc",
            interval_seconds=60,  # much larger than what's left
            timeout_seconds=10,
            expires_in=10,
            authorized_at=0.0,
            sleep=clock.sleep,
            monotonic=clock.monotonic,
            jitter=lambda: 0.0,
        )
    # A 60s interval must not carry us past a 10s window.
    assert clock.now == 10
    assert clock.sleeps == [10]


def test_device_poll_user_timeout_shorter_than_window(monkeypatch):
    clock = FakeClock()
    _install_client(monkeypatch, [_pending()])
    with pytest.raises(LoginFlowError) as excinfo:
        poll_device_authorization(
            api_url="https://api.hello.food",
            client_id="hf_cid",
            device_code="hf_dc",
            interval_seconds=5,
            timeout_seconds=10,  # user's --timeout is the binding limit
            expires_in=600,
            authorized_at=0.0,
            sleep=clock.sleep,
            monotonic=clock.monotonic,
            jitter=lambda: 0.0,
        )
    assert clock.now == 10
    # Not the server window closing — the user's timeout elapsed first.
    assert "Timed out waiting" in str(excinfo.value)


def test_device_poll_tolerates_transient_then_succeeds(monkeypatch):
    clock = FakeClock()
    _install_client(
        monkeypatch,
        [
            httpx.ConnectError("blip"),
            httpx.ReadError("blip"),
            httpx.Response(503, text="upstream"),
            _approved(),
        ],
    )
    result = poll_device_authorization(
        api_url="https://api.hello.food",
        client_id="hf_cid",
        device_code="hf_dc",
        interval_seconds=2,
        timeout_seconds=600,
        expires_in=600,
        authorized_at=0.0,
        sleep=clock.sleep,
        monotonic=clock.monotonic,
        jitter=lambda: 0.0,
    )
    assert result["access_token"] == "hf_ct"
    assert clock.sleeps == [2, 2, 2]  # three retries, one per transient tick


def test_device_poll_aborts_after_consecutive_transport_failures(monkeypatch):
    clock = FakeClock()
    _install_client(monkeypatch, [httpx.ConnectError("down")])  # always fails
    with pytest.raises(LoginFlowError) as excinfo:
        poll_device_authorization(
            api_url="https://api.hello.food",
            client_id="hf_cid",
            device_code="hf_dc",
            interval_seconds=1,
            timeout_seconds=6000,
            expires_in=6000,
            authorized_at=0.0,
            sleep=clock.sleep,
            monotonic=clock.monotonic,
            jitter=lambda: 0.0,
        )
    assert str(excinfo.value) == DEVICE_LOGIN_NETWORK_MESSAGE
    # Gave up on the 10th consecutive failure, well before the deadline.
    assert len(clock.sleeps) == 9


def test_device_poll_server_5xx_exhaustion_maps_to_unavailable(monkeypatch):
    clock = FakeClock()
    _install_client(monkeypatch, [httpx.Response(503, text="upstream")])
    with pytest.raises(LoginFlowError) as excinfo:
        poll_device_authorization(
            api_url="https://api.hello.food",
            client_id="hf_cid",
            device_code="hf_dc",
            interval_seconds=1,
            timeout_seconds=6000,
            expires_in=6000,
            authorized_at=0.0,
            sleep=clock.sleep,
            monotonic=clock.monotonic,
            jitter=lambda: 0.0,
        )
    assert str(excinfo.value) == DEVICE_LOGIN_UNAVAILABLE_MESSAGE


def test_device_poll_temporarily_unavailable_exhaustion(monkeypatch):
    clock = FakeClock()
    _install_client(
        monkeypatch, [httpx.Response(400, json={"error": "temporarily_unavailable"})]
    )
    with pytest.raises(LoginFlowError) as excinfo:
        poll_device_authorization(
            api_url="https://api.hello.food",
            client_id="hf_cid",
            device_code="hf_dc",
            interval_seconds=1,
            timeout_seconds=6000,
            expires_in=6000,
            authorized_at=0.0,
            sleep=clock.sleep,
            monotonic=clock.monotonic,
            jitter=lambda: 0.0,
        )
    assert str(excinfo.value) == DEVICE_LOGIN_UNAVAILABLE_MESSAGE


def test_device_poll_transient_counter_resets_between_wobbles(monkeypatch):
    clock = FakeClock()
    # Nine failures, one healthy pending (resets), then nine more, then success.
    script = (
        [httpx.ConnectError("blip")] * 9
        + [_pending()]
        + [httpx.ConnectError("blip")] * 9
        + [_approved()]
    )
    _install_client(monkeypatch, script)
    result = poll_device_authorization(
        api_url="https://api.hello.food",
        client_id="hf_cid",
        device_code="hf_dc",
        interval_seconds=1,
        timeout_seconds=6000,
        expires_in=6000,
        authorized_at=0.0,
        sleep=clock.sleep,
        monotonic=clock.monotonic,
        jitter=lambda: 0.0,
    )
    # A healthy tick in the middle keeps the 18 total failures from ever hitting
    # the consecutive bound of 10.
    assert result["access_token"] == "hf_ct"


@pytest.mark.parametrize(
    "error_code,expected",
    [
        ("access_denied", DEVICE_LOGIN_DENIED_MESSAGE),
        ("expired_token", DEVICE_LOGIN_EXPIRED_MESSAGE),
        ("invalid_grant", DEVICE_LOGIN_INVALID_GRANT_MESSAGE),
    ],
)
def test_device_poll_classified_terminal_states(monkeypatch, error_code, expected):
    clock = FakeClock()
    _install_client(monkeypatch, [httpx.Response(400, json={"error": error_code})])
    with pytest.raises(LoginFlowError) as excinfo:
        poll_device_authorization(
            api_url="https://api.hello.food",
            client_id="hf_cid",
            device_code="hf_dc",
            interval_seconds=5,
            timeout_seconds=600,
            expires_in=600,
            authorized_at=0.0,
            sleep=clock.sleep,
            monotonic=clock.monotonic,
            jitter=lambda: 0.0,
        )
    assert str(excinfo.value) == expected
    # Terminal codes are decided immediately, never after a wait.
    assert clock.sleeps == []


def test_device_poll_keyboard_interrupt_is_clean(monkeypatch):
    clock = FakeClock()
    _install_client(monkeypatch, [_pending()])

    def interrupt(_seconds):
        raise KeyboardInterrupt

    with pytest.raises(LoginInterrupted) as excinfo:
        poll_device_authorization(
            api_url="https://api.hello.food",
            client_id="hf_cid",
            device_code="hf_dc",
            interval_seconds=5,
            timeout_seconds=600,
            expires_in=600,
            authorized_at=0.0,
            sleep=interrupt,
            monotonic=clock.monotonic,
            jitter=lambda: 0.0,
        )
    assert str(excinfo.value) == DEVICE_LOGIN_INTERRUPTED_MESSAGE
    # Clean surface: it is a LoginFlowError subclass and carries no chained cause.
    assert isinstance(excinfo.value, LoginFlowError)
    assert excinfo.value.__cause__ is None
    assert "restart" in str(excinfo.value).lower()


def test_device_login_anchors_deadline_to_authorize_time(monkeypatch):
    """perform_device_login must hand poll the authoritative expires_in + anchor."""
    store = MagicMock()
    store.get_device_id.return_value = "device-1"
    store.load.return_value = {}
    monkeypatch.setattr("heyfood_cli.auth.resolve_login_capabilities", _full_capabilities)
    monkeypatch.setattr(
        "heyfood_cli.auth.register_client", lambda *_: {"client_id": "cid"}
    )
    monkeypatch.setattr(
        "heyfood_cli.auth.start_device_authorization",
        lambda *a, **k: {
            "device_code": "dc",
            "user_code": "ABCD-EFGH",
            "verification_uri": "https://auth.hello.food/authorize",
            "expires_in": 600,
            "interval": 5,
        },
    )
    captured = {}

    def fake_poll(**kwargs):
        captured.update(kwargs)
        return {"access_token": "hf_ct", "refresh_token": "hf_cr", "expires_in": 3600}

    monkeypatch.setattr("heyfood_cli.auth.poll_device_authorization", fake_poll)
    monkeypatch.setattr(
        "heyfood_cli.auth.exchange_cli_session",
        lambda **_: {"access_token": "hf_at", "user_id": "user-a"},
    )

    perform_device_login(
        store=store,
        api_url="https://api.hello.food",
        auth_url="https://auth.hello.food",
        api_key=None,
        open_browser=False,
        timeout_seconds=180,
        authorization_callback=lambda *_: None,
    )
    assert captured["expires_in"] == 600
    assert captured["timeout_seconds"] == 180
    assert isinstance(captured["authorized_at"], float)


def test_login_command_handles_keyboard_interrupt_cleanly(monkeypatch, tmp_path):
    """Ctrl-C at the command boundary exits 130 with a calm notice, no traceback."""
    from typer.testing import CliRunner

    from heyfood_cli import auth as auth_mod
    from heyfood_cli import main as main_mod

    def interrupt(**_kwargs):
        raise auth_mod.LoginInterrupted(auth_mod.DEVICE_LOGIN_INTERRUPTED_MESSAGE)

    # The command binds perform_device_login into its own namespace at import,
    # so patch it there (and the module it was re-exported from) to be safe.
    from heyfood_cli.commands import auth as auth_cmd

    monkeypatch.setattr(auth_cmd, "perform_device_login", interrupt)
    monkeypatch.setattr(main_mod, "perform_device_login", interrupt)

    runner = CliRunner(env={"XDG_CONFIG_HOME": str(tmp_path), "NO_COLOR": "1"})
    result = runner.invoke(main_mod.app, ["login", "--device", "--no-browser"])

    assert result.exit_code == 130
    assert result.exception is None or isinstance(result.exception, SystemExit)
    assert "Login canceled" in result.output
    assert "Traceback" not in result.output
