from __future__ import annotations

import json
from contextlib import redirect_stdout
from io import StringIO
from unittest.mock import MagicMock
from urllib.parse import parse_qs, urlparse

import pytest
import httpx
from rich.console import Console
from typer.testing import CliRunner

from heyfood_cli import auth_application, main
from heyfood_cli.auth import LOGIN_SCOPES, build_authorize_url
from heyfood_cli.commands import auth as auth_command


def _capabilities(**overrides):
    document = {
        "schema_version": 1,
        "self_registration": {
            "status": "available",
            "regions": ["US"],
            "identity_methods": ["sms", "email"],
        },
        "authorization": {
            "loopback_pkce": True,
            "device_code": True,
            "identity_methods": ["sms", "email"],
        },
        "profile_readiness": True,
    }
    document.update(overrides)
    return document


def test_registration_intent_is_a_browser_ux_hint():
    url = build_authorize_url(
        auth_url="https://auth.hello.food/authorize",
        client_id="client-1",
        redirect_uri="http://127.0.0.1:8765/callback",
        state="state-1",
        code_challenge="x" * 43,
        intent="register",
    )

    assert parse_qs(urlparse(url).query)["intent"] == ["create_account"]


@pytest.mark.parametrize("device", [False, True])
def test_login_command_keeps_account_choice_visible_for_every_transport(
    monkeypatch, device
):
    seen = {}
    store = MagicMock()
    store.load.return_value = {}
    monkeypatch.setattr(auth_command, "ConfigStore", lambda: store)
    monkeypatch.setattr(
        auth_command,
        "_auth_urls",
        lambda *_args, **_kwargs: (
            "https://api.hello.food",
            "https://auth.hello.food/authorize",
        ),
    )

    def fake_authenticate(**kwargs):
        seen.update(kwargs)
        return auth_application.AuthApplicationResult(intent="auto", capabilities=None)

    monkeypatch.setattr(auth_command, "_authenticate", fake_authenticate)

    args = ["login"] + (["--device"] if device else ["--no-browser"])
    result = CliRunner().invoke(main.app, args)

    assert result.exit_code == 0, result.output
    assert seen["intent"] == "auto"
    assert seen["device"] is device


def test_bare_returning_login_uses_account_neutral_intent(monkeypatch):
    store = MagicMock()
    store.load.return_value = {"account_user_id": "returning-user"}
    seen = {}
    monkeypatch.setattr(auth_command, "ConfigStore", lambda: store)
    monkeypatch.setattr(auth_command.render, "intro", lambda _console: None)
    monkeypatch.setattr(auth_command.Prompt, "ask", lambda *_args, **_kwargs: "login")
    monkeypatch.setattr(
        auth_command,
        "_auth_urls",
        lambda *_args, **_kwargs: (
            "https://api.hello.food",
            "https://auth.hello.food/authorize",
        ),
    )

    def fake_authenticate(**kwargs):
        seen.update(kwargs)
        return auth_application.AuthApplicationResult(intent="auto", capabilities=None)

    monkeypatch.setattr(auth_command, "_authenticate", fake_authenticate)
    monkeypatch.setattr(
        auth_command,
        "_profile_readiness",
        lambda **_kwargs: {
            "profile_status": "ready",
            "has_profile_sync_consent": True,
            "profile_version": 1,
        },
    )
    monkeypatch.setattr("heyfood_cli.commands.agent.chat", lambda **_kwargs: None)

    auth_command.run_bare_first_run()

    assert seen["intent"] == "auto"


def test_auto_intent_is_explicit_and_identical_on_browser_and_device_urls():
    browser_url = build_authorize_url(
        auth_url="https://auth.hello.food/authorize",
        client_id="client-1",
        redirect_uri="http://127.0.0.1:8765/callback",
        state="state-1",
        code_challenge="x" * 43,
        intent="auto",
    )

    assert parse_qs(urlparse(browser_url).query)["intent"] == ["auto"]


def test_capabilities_contract_is_strict_and_us_only():
    parsed = auth_application.validate_auth_capabilities(_capabilities())

    assert parsed.registration_status == "available"
    assert parsed.registration_regions == ("US",)
    assert parsed.profile_readiness is True

    malformed = _capabilities(extra="future-field")
    with pytest.raises(auth_application.AuthContractError):
        auth_application.validate_auth_capabilities(malformed)

    non_us = _capabilities()
    non_us["self_registration"]["regions"] = ["US", "CA"]
    with pytest.raises(auth_application.AuthContractError):
        auth_application.validate_auth_capabilities(non_us)


def test_registration_preflight_fails_before_oauth_when_disabled():
    disabled = _capabilities()
    disabled["self_registration"] = {
        "status": "disabled",
        "regions": [],
        "identity_methods": [],
    }
    login_runner = MagicMock()

    with pytest.raises(auth_application.RegistrationUnavailable):
        auth_application.authenticate(
            intent="register",
            store=MagicMock(),
            api_url="https://api.hello.food",
            auth_url="https://auth.hello.food/authorize",
            api_key=None,
            device=False,
            open_browser=False,
            timeout_seconds=180,
            authorize_url_callback=lambda _url: None,
            device_authorization_callback=lambda _url, _code: None,
            capability_loader=lambda _url: auth_application.validate_auth_capabilities(disabled),
            login_runner=login_runner,
        )

    login_runner.assert_not_called()


@pytest.mark.parametrize(
    "payload",
    (
        {
            "schema_version": 1,
            "status": "ready",
            "has_profile_sync_consent": True,
            "member_id": "_self",
            "profile_version": None,
        },
        {
            "schema_version": 1,
            "status": "missing",
            "has_profile_sync_consent": None,
            "member_id": "_self",
            "profile_version": None,
        },
        {
            "schema_version": 1,
            "status": "unknown",
            "has_profile_sync_consent": False,
            "member_id": "_self",
            "profile_version": None,
        },
        {
            "schema_version": 2,
            "status": "ready",
            "has_profile_sync_consent": True,
            "member_id": "_self",
            "profile_version": 1,
        },
    ),
)
def test_profile_readiness_rejects_inconsistent_or_unknown_contracts(payload):
    with pytest.raises(auth_application.AuthContractError):
        auth_application.validate_profile_readiness(payload)


def test_client_profile_readiness_validates_before_returning(monkeypatch):
    client = object.__new__(main.HelloFoodClient)
    monkeypatch.setattr(
        client,
        "_request",
        lambda *_args, **_kwargs: {
            "schema_version": 1,
            "status": "ready",
            "has_profile_sync_consent": True,
            "member_id": "_self",
            "profile_version": 7,
        },
    )

    assert client.profile_readiness() == {
        "profile_status": "ready",
        "has_profile_sync_consent": True,
        "profile_version": 7,
    }


def test_register_json_is_one_document_and_never_offers_onboarding(monkeypatch):
    capabilities = auth_application.validate_auth_capabilities(_capabilities())
    seen = {}
    store = MagicMock()
    store.load.return_value = {}
    monkeypatch.setattr("heyfood_cli.commands.auth.ConfigStore", lambda: store)
    monkeypatch.setattr(
        "heyfood_cli.commands.auth._auth_urls",
        lambda *_args, **_kwargs: (
            "https://api.hello.food",
            "https://auth.hello.food/authorize",
        ),
    )

    def fake_authenticate(**kwargs):
        seen.update(kwargs)
        return auth_application.AuthApplicationResult(
            intent="register",
            capabilities=capabilities,
        )

    monkeypatch.setattr("heyfood_cli.commands.auth._authenticate", fake_authenticate)
    monkeypatch.setattr(
        "heyfood_cli.commands.auth._profile_readiness",
        lambda **_kwargs: {
            "profile_status": "missing",
            "has_profile_sync_consent": False,
            "profile_version": None,
        },
    )
    monkeypatch.setattr(
        "heyfood_cli.commands.auth._run_onboarding_handoff",
        lambda: pytest.fail("JSON registration must not launch onboarding"),
    )

    result = CliRunner().invoke(main.app, ["register", "--json"])

    assert result.exit_code == 0, result.output
    assert seen["json_mode"] is True
    document = json.loads(result.stdout)
    assert document == {
        "schema_version": 1,
        "authenticated": True,
        "account_outcome": None,
        "profile_status": "missing",
        "next_command": "heyfood onboard",
    }
    assert "Checking hello.food" not in result.stdout
    assert "\x1b" not in result.stdout


def test_register_json_stays_pure_on_simulated_tty_with_device_url(monkeypatch):
    class TTYBuffer(StringIO):
        def isatty(self):
            return True

    capabilities = auth_application.validate_auth_capabilities(_capabilities())
    store = MagicMock()
    store.load.return_value = {}
    stdout = TTYBuffer()
    stderr = TTYBuffer()
    monkeypatch.setattr(auth_command, "ConfigStore", lambda: store)
    monkeypatch.setattr(
        auth_command,
        "_auth_urls",
        lambda *_args, **_kwargs: (
            "https://api.hello.food",
            "https://auth.hello.food/authorize",
        ),
    )
    monkeypatch.setattr(
        main,
        "stderr_console",
        Console(file=stderr, force_terminal=True, color_system="truecolor"),
    )

    def fake_application(**kwargs):
        assert kwargs["open_browser"] is False
        kwargs["device_authorization_callback"](
            "https://auth.hello.food/authorize?flow=device&user_code=ABCD-EFGH",
            "ABCD-EFGH",
        )
        return auth_application.AuthApplicationResult(
            intent="register",
            capabilities=capabilities,
        )

    monkeypatch.setattr(auth_command.auth_application, "authenticate", fake_application)
    monkeypatch.setattr(
        auth_command,
        "_profile_readiness",
        lambda **_kwargs: {
            "profile_status": "missing",
            "has_profile_sync_consent": False,
            "profile_version": None,
        },
    )

    with redirect_stdout(stdout):
        auth_command.register(
            api_url=None,
            auth_url=None,
            api_key="",
            local=False,
            device=True,
            no_browser=False,
            timeout=180,
            no_onboard=False,
            json_output=True,
        )

    assert json.loads(stdout.getvalue())["authenticated"] is True
    assert "\x1b" not in stdout.getvalue()
    assert "ABCD-EFGH" in stderr.getvalue()
    assert "Open this URL" in stderr.getvalue()


def test_register_json_failure_is_structured_and_preflight_safe(monkeypatch):
    store = MagicMock()
    store.load.return_value = {}
    monkeypatch.setattr("heyfood_cli.commands.auth.ConfigStore", lambda: store)
    monkeypatch.setattr(
        "heyfood_cli.commands.auth._auth_urls",
        lambda *_args, **_kwargs: (
            "https://api.hello.food",
            "https://auth.hello.food/authorize",
        ),
    )
    monkeypatch.setattr(
        "heyfood_cli.commands.auth._authenticate",
        lambda **_kwargs: (_ for _ in ()).throw(
            auth_application.RegistrationUnavailable("disabled")
        ),
    )

    result = CliRunner().invoke(main.app, ["register", "--json"])

    assert result.exit_code == 1
    document = json.loads(result.stdout)
    assert document["ok"] is False
    assert document["error"]["type"] == "registration_unavailable"
    assert "heyfood login" in document["error"]["hint"]
    assert "\x1b" not in result.stdout


def test_json_authentication_suppresses_browser_launch(monkeypatch):
    captured = {}

    def fake_authenticate(**kwargs):
        captured.update(kwargs)
        return auth_application.AuthApplicationResult(
            intent="register",
            capabilities=auth_application.validate_auth_capabilities(_capabilities()),
        )

    monkeypatch.setattr(auth_command.auth_application, "authenticate", fake_authenticate)

    auth_command._authenticate(
        intent="register",
        store=MagicMock(),
        api_url="https://api.hello.food",
        auth_url="https://auth.hello.food/authorize",
        api_key="",
        device=True,
        no_browser=False,
        timeout=180,
        json_mode=True,
    )

    assert captured["open_browser"] is False


def test_missing_profile_acceptance_reuses_onboarding_and_rechecks(monkeypatch):
    onboarding_calls = []
    monkeypatch.setattr(auth_command.Prompt, "ask", lambda *_args, **_kwargs: "type")
    monkeypatch.setattr(
        auth_command,
        "_run_onboarding_handoff",
        lambda **kwargs: onboarding_calls.append(kwargs),
    )
    monkeypatch.setattr(
        auth_command,
        "_profile_readiness",
        lambda **_kwargs: {
            "profile_status": "ready",
            "has_profile_sync_consent": True,
            "profile_version": 1,
        },
    )

    readiness, onboarding = auth_command._complete_first_profile(
        {
            "profile_status": "missing",
            "has_profile_sync_consent": False,
            "profile_version": None,
        },
        offer_onboarding=True,
    )

    assert onboarding_calls == [{"voice": False}]
    assert onboarding == "completed"
    assert readiness["profile_status"] == "ready"


def test_fresh_bare_command_defaults_to_register_then_enters_chat(monkeypatch):
    store = MagicMock()
    store.load.return_value = {}
    choices = {}
    capabilities = auth_application.validate_auth_capabilities(_capabilities())
    authenticated = []
    chat_calls = []
    monkeypatch.setattr(auth_command, "ConfigStore", lambda: store)
    monkeypatch.setattr(auth_command.render, "intro", lambda _console: None)

    def choose(*_args, **kwargs):
        choices.update(kwargs)
        return kwargs["default"]

    monkeypatch.setattr(auth_command.Prompt, "ask", choose)
    monkeypatch.setattr(
        auth_command,
        "_auth_urls",
        lambda *_args, **_kwargs: (
            "https://api.hello.food",
            "https://auth.hello.food/authorize",
        ),
    )

    def fake_authenticate(**kwargs):
        authenticated.append(kwargs["intent"])
        return auth_application.AuthApplicationResult(
            intent="register",
            capabilities=capabilities,
        )

    monkeypatch.setattr(auth_command, "_authenticate", fake_authenticate)
    monkeypatch.setattr(
        auth_command,
        "_profile_readiness",
        lambda **_kwargs: {
            "profile_status": "ready",
            "has_profile_sync_consent": True,
            "profile_version": 1,
        },
    )
    monkeypatch.setattr(
        "heyfood_cli.commands.agent.chat",
        lambda **kwargs: chat_calls.append(kwargs),
    )

    auth_command.run_bare_first_run()

    assert choices["default"] == "register"
    assert authenticated == ["register"]
    assert len(chat_calls) == 1
    assert chat_calls[0]["new"] is False


def test_bare_non_tty_is_side_effect_free(monkeypatch):
    monkeypatch.setattr(main, "_interactive_terminal", lambda: False)
    monkeypatch.setattr(
        "heyfood_cli.commands.auth.run_bare_first_run",
        lambda: pytest.fail("non-TTY bare command must not enter first run"),
    )

    result = CliRunner().invoke(main.app, [])

    assert result.exit_code == 0
    assert "heyfood register" in result.stdout
    assert "heyfood login" in result.stdout
    assert "╭" not in result.stdout


def test_registration_device_runner_end_to_end_at_http_boundary(monkeypatch):
    store = MagicMock()
    store.load.return_value = {}
    store.get_device_id.return_value = "heyfood-device-e2e"
    requests = []
    stderr = StringIO()
    monkeypatch.setattr(
        main,
        "stderr_console",
        Console(file=stderr, force_terminal=False, highlight=False),
    )
    monkeypatch.setattr(auth_command, "ConfigStore", lambda: store)
    monkeypatch.setattr(
        auth_command,
        "_auth_urls",
        lambda *_args, **_kwargs: (
            "https://api.hello.food",
            "https://auth.hello.food/authorize",
        ),
    )
    monkeypatch.setattr(
        auth_application,
        "fetch_auth_capabilities",
        lambda _url: auth_application.validate_auth_capabilities(_capabilities()),
    )

    def fake_post(_api_url, path, *, json_body, **_kwargs):
        requests.append((path, json_body))
        payloads = {
            "/v1/channel/oauth/register": {"client_id": "hf_cid_e2e"},
            "/v1/channel/oauth/device/authorize": {
                "device_code": "hf_dc_e2e",
                "user_code": "ABCD-EFGH",
                "verification_uri": "https://auth.hello.food/authorize?flow=device",
                "expires_in": 600,
                "interval": 1,
            },
            "/v1/channel/oauth/device/token": {
                "access_token": "hf_ct_e2e",
                "refresh_token": "hf_cr_e2e",
                "expires_in": 3600,
                "scope": "account:link account:delete profile:read profile:write",
            },
            "/v1/channel/oauth/cli/session": {
                "access_token": "hf_at_e2e",
                "refresh_token": "hf_rt_e2e",
                "user_id": "user-e2e",
            },
        }
        return httpx.Response(200, json=payloads[path])

    monkeypatch.setattr("heyfood_cli.auth._post_with_diagnostics", fake_post)
    # This boundary test models a registration-capable backend: server metadata
    # advertises the full scope set (including account:delete), so the device
    # request legitimately carries the create_account intent. Stubbed here so the
    # capability discovery GET stays off the network and the test is hermetic.
    monkeypatch.setattr(
        "heyfood_cli.auth.fetch_supported_scopes",
        lambda *_a, **_k: list(LOGIN_SCOPES),
    )
    monkeypatch.setattr(
        auth_command,
        "_profile_readiness",
        lambda **_kwargs: {
            "profile_status": "ready",
            "has_profile_sync_consent": True,
            "profile_version": 1,
        },
    )

    result = CliRunner().invoke(main.app, ["register", "--device", "--json"])

    assert result.exit_code == 0, result.output
    assert json.loads(result.stdout) == {
        "schema_version": 1,
        "authenticated": True,
        "account_outcome": None,
        "profile_status": "ready",
        "next_command": "heyfood chat",
    }
    device_request = dict(requests)["/v1/channel/oauth/device/authorize"]
    assert device_request["intent"] == "create_account"
    saved = store.save.call_args.args[0]
    assert saved["account_user_id"] == "user-e2e"
    assert saved["oauth"]["access_token"] == "hf_ct_e2e"
    assert saved["session"]["access_token"] == "hf_at_e2e"


def test_registration_errors_always_name_register(monkeypatch):
    store = MagicMock()
    store.load.return_value = {}
    monkeypatch.setattr(auth_command, "ConfigStore", lambda: store)
    monkeypatch.setattr(
        auth_command,
        "_auth_urls",
        lambda *_args, **_kwargs: (
            "https://api.hello.food",
            "https://auth.hello.food/authorize",
        ),
    )
    monkeypatch.setattr(
        auth_command,
        "_authenticate",
        lambda **_kwargs: (_ for _ in ()).throw(
            RuntimeError("The approval window ended. Run heyfood login again.")
        ),
    )

    result = CliRunner().invoke(main.app, ["register", "--json"])

    assert result.exit_code == 1
    message = json.loads(result.stdout)["error"]["message"]
    assert "heyfood register" in message
    assert "heyfood login" not in message


def test_initial_onboarding_prompt_ctrl_c_retains_authenticated_account(monkeypatch):
    monkeypatch.setattr(
        auth_command.Prompt,
        "ask",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(KeyboardInterrupt()),
    )
    messages = []
    monkeypatch.setattr(
        auth_command.main.stderr_console,
        "print",
        lambda message: messages.append(str(message)),
    )

    readiness, state = auth_command._complete_first_profile(
        {
            "profile_status": "missing",
            "has_profile_sync_consent": False,
            "profile_version": None,
        },
        offer_onboarding=True,
    )

    assert readiness["profile_status"] == "missing"
    assert state == "deferred"
    assert "account remains connected" in " ".join(messages)


def test_voice_is_contextual_and_optional_during_first_profile(monkeypatch):
    calls = []
    monkeypatch.setattr(auth_command.Prompt, "ask", lambda *_args, **_kwargs: "voice")
    monkeypatch.setattr(
        auth_command,
        "_run_onboarding_handoff",
        lambda **kwargs: calls.append(kwargs),
    )
    monkeypatch.setattr(
        auth_command,
        "_profile_readiness",
        lambda **_kwargs: {"profile_status": "ready"},
    )

    readiness, state = auth_command._complete_first_profile(
        {"profile_status": "missing"},
        offer_onboarding=True,
    )

    assert calls == [{"voice": True}]
    assert readiness["profile_status"] == "ready"
    assert state == "completed"


def test_non_tty_intro_is_ascii_safe():
    buffer = StringIO()
    auth_command.render.noninteractive_intro(Console(file=buffer, color_system=None))

    assert buffer.getvalue().encode("ascii")
