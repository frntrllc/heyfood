import pytest
from typer.testing import CliRunner

from heyfood_cli import onboarding
from heyfood_cli.main import _is_recipe_provider_unavailable, _parse_option_selection


def test_logout_json_reports_teardown_without_human_output(monkeypatch):
    from heyfood_cli import main as main_mod

    document = {
        "ok": True,
        "remote_complete": False,
        "teardown": {
            "link": {"attempted": True, "ok": False, "error": "request_failed"},
            "device": {"attempted": True, "ok": True},
            "session": {"attempted": True, "ok": True},
        },
        "local_credentials_cleared": True,
    }

    class FakeClient:
        def __init__(self, **_kwargs):
            pass

        def revoke_local_session(self):
            return document

    monkeypatch.setattr(main_mod, "HelloFoodClient", FakeClient)
    result = CliRunner().invoke(main_mod.app, ["logout", "--json"])

    assert result.exit_code == 0
    assert result.stdout.strip().startswith("{")
    assert "Logged out" not in result.stdout
    assert '"remote_complete": false' in result.stdout


def test_recipe_provider_unavailable_detection():
    assert _is_recipe_provider_unavailable(
        "503: Recipe service temporarily unavailable. Please try again later."
    )
    assert _is_recipe_provider_unavailable("Recipe provider unavailable: spoonacular")
    assert not _is_recipe_provider_unavailable("401: Run `heyfood login` first.")


def test_onboarding_option_selection_accepts_numbers_and_names():
    selected = _parse_option_selection(
        "1, Keto, 6",
        onboarding.DIET_STYLES[:8],
        multi=True,
    )

    assert selected == ["Gluten-free", "Keto", "Pescatarian"]


def test_onboarding_option_selection_accepts_ranges_and_none():
    selected = _parse_option_selection(
        "2-4, 0",
        onboarding.ALLERGIES[:10],
        multi=True,
    )

    assert selected == ["none"]


def test_menu_acquiring_polls_until_ready(monkeypatch):
    from heyfood_cli import main as main_mod

    class FakeClient:
        def __init__(self):
            self.calls = []
            self.responses = [
                {"status": "acquiring", "job_id": "job-1"},
                {
                    "status": "ready",
                    "job_id": "job-1",
                    "restaurant_name": "Ready Cafe",
                    "sections": [],
                },
            ]

        def get_menu_status(self, *, restaurant_id, job_id):
            self.calls.append((restaurant_id, job_id))
            return self.responses.pop(0)

    client = FakeClient()
    monkeypatch.setattr(main_mod.time, "sleep", lambda _: None)
    monkeypatch.setattr(main_mod.time, "monotonic", lambda: 0.0)

    result = main_mod._poll_menu_until_terminal(
        client,
        "rest-1",
        {"status": "acquiring", "job_id": "job-1"},
    )

    assert result["status"] == "ready"
    assert client.calls == [("rest-1", "job-1"), ("rest-1", "job-1")]


def test_menu_poll_local_timeout_preserves_resumable_context(monkeypatch):
    from heyfood_cli import main as main_mod

    class FakeClient:
        def get_menu_status(self, **kwargs):
            raise AssertionError("poll should not run after the local ceiling")

    times = iter([0.0, main_mod.MENU_POLL_TIMEOUT_SECONDS + 1])
    monkeypatch.setattr(main_mod.time, "monotonic", lambda: next(times))

    result = main_mod._poll_menu_until_terminal(
        FakeClient(),
        "rest-1",
        {"status": "acquiring", "job_id": "job-1"},
    )

    assert result["status"] == "timed_out"
    assert result["message"] == "Menu is still being fetched after 30 seconds."
    assert result["job_id"] == "job-1"
    assert result["poll_timeout_seconds"] == 30.0


def test_menu_poll_contract_warns_at_twelve_and_stops_at_thirty_seconds():
    from heyfood_cli import main as main_mod

    assert main_mod.MENU_POLL_WARNING_SECONDS == 12.0
    assert main_mod.MENU_POLL_TIMEOUT_SECONDS == 30.0


def test_menu_poll_updates_status_after_warning_threshold(monkeypatch):
    from heyfood_cli import main as main_mod

    class FakeClient:
        def get_menu_status(self, **kwargs):
            return {"status": "ready", "job_id": "job-1", "sections": []}

    class FakeStatus:
        def __init__(self):
            self.updates = []

        def __enter__(self):
            return self

        def __exit__(self, *args):
            return False

        def update(self, message):
            self.updates.append(message)

    status = FakeStatus()
    times = iter([0.0, 13.0])
    monkeypatch.setattr(main_mod.time, "monotonic", lambda: next(times))
    monkeypatch.setattr(main_mod.time, "sleep", lambda _: None)
    monkeypatch.setattr(main_mod.stderr_console, "status", lambda _: status)

    result = main_mod._poll_menu_until_terminal(
        FakeClient(),
        "rest-1",
        {"status": "acquiring", "job_id": "job-1"},
    )

    assert result["status"] == "ready"
    assert status.updates == [
        "[yellow]Still fetching the menu — returning control by 30 seconds…[/yellow]"
    ]


def test_menu_poll_restores_status_when_interrupted(monkeypatch):
    from heyfood_cli import main as main_mod

    class FakeClient:
        def get_menu_status(self, **kwargs):
            raise KeyboardInterrupt

    class FakeStatus:
        exited = False

        def __enter__(self):
            return self

        def __exit__(self, *args):
            self.exited = True
            return False

        def update(self, message):
            pass

    status = FakeStatus()
    monkeypatch.setattr(main_mod.time, "monotonic", lambda: 0.0)
    monkeypatch.setattr(main_mod.time, "sleep", lambda _: None)
    monkeypatch.setattr(main_mod.stderr_console, "status", lambda _: status)

    with pytest.raises(KeyboardInterrupt):
        main_mod._poll_menu_until_terminal(
            FakeClient(),
            "rest-1",
            {"status": "acquiring", "job_id": "job-1"},
        )

    assert status.exited is True


def test_menu_poll_feature_gates_missing_status_tool(monkeypatch):
    from heyfood_cli import main as main_mod
    from heyfood_cli.client import ChannelToolUnavailable

    class OlderApiClient:
        def get_menu_status(self, **_kwargs):
            raise ChannelToolUnavailable("get_menu_status is unavailable")

    monkeypatch.setattr(main_mod.time, "sleep", lambda _seconds: None)
    monkeypatch.setattr(main_mod.time, "monotonic", lambda: 0.0)

    result = main_mod._poll_menu_until_terminal(
        OlderApiClient(),
        "restaurant-1",
        {"status": "acquiring", "job_id": "job-1"},
        show_progress=False,
        json_mode=True,
    )

    assert result["status"] == "timed_out"
    assert result["job_id"] == "job-1"
    assert result["polling_supported"] is False
    assert result["poll_timeout_seconds"] == 0
    assert "Retry the menu command" in result["message"]


def test_menu_timeout_prints_resume_command_and_exits_nonzero(monkeypatch):
    from io import StringIO

    import typer
    from rich.console import Console

    from heyfood_cli import main as main_mod

    class FakeClient:
        def restaurant_id_from_selector(self, value):
            assert value == "1"
            return "rest-1"

        def channel_tool(self, name, payload):
            assert name == "get_menu"
            assert payload == {"restaurant_id": "rest-1"}
            return {"status": "acquiring", "job_id": "job-1"}

    output = StringIO()
    monkeypatch.setattr(main_mod, "HelloFoodClient", FakeClient)
    monkeypatch.setattr(
        main_mod,
        "_poll_menu_until_terminal",
        lambda *args, **kwargs: {"status": "timed_out", "job_id": "job-1"},
    )
    monkeypatch.setattr(
        main_mod,
        "stderr_console",
        Console(file=output, force_terminal=False, width=120),
    )

    with pytest.raises(typer.Exit) as raised:
        main_mod.menu("1")

    assert raised.value.exit_code == 1
    assert "Job job-1" in output.getvalue()
    assert "heyfood menu rest-1" in output.getvalue()


def test_ask_agent_only_persists_completed_progress_events(monkeypatch):
    from io import StringIO

    from rich.console import Console

    from heyfood_cli import main as main_mod

    class FakeClient:
        def saved_location(self):
            return None

        def stream_agent(self, payload):
            yield "thinking", {"message": "resolving restaurant"}
            yield "thinking", {}
            yield "progress", {
                "stage": "resolving_restaurant",
                "message": "restaurant resolved: Thai Taste Restaurant",
            }
            yield "result", {"message": "All done.", "conversation_id": "conv-1"}

        def remember_conversation(self, result):
            pass

    output = StringIO()
    progress_output = StringIO()
    monkeypatch.setattr(main_mod, "HelloFoodClient", lambda: FakeClient())
    monkeypatch.setattr(
        main_mod, "console", Console(file=output, force_terminal=False, width=120)
    )
    monkeypatch.setattr(
        main_mod,
        "stderr_console",
        Console(file=progress_output, force_terminal=False, width=120),
    )

    main_mod._ask_agent("what can I eat", show_continue_hint=False)

    text = output.getvalue()
    assert "All done." in text
    assert "thinking" not in text
    assert "resolving restaurant" not in text
    assert "restaurant resolved: Thai Taste Restaurant" not in text
    assert "restaurant resolved: Thai Taste Restaurant" in progress_output.getvalue()


def test_ask_agent_raw_output_excludes_progress_and_continue_hint(monkeypatch):
    import json
    from contextlib import redirect_stdout
    from io import StringIO

    from rich.console import Console

    from heyfood_cli import main as main_mod

    class FakeClient:
        def saved_location(self):
            return None

        def stream_agent(self, payload):
            yield "progress", {"message": "menu loaded: 48 items"}
            yield "result", {
                "structured": {"type": "general_response"},
                "conversation_id": "conv-1",
            }

        def remember_conversation(self, result):
            pass

    output = StringIO()
    monkeypatch.setattr(main_mod, "HelloFoodClient", lambda: FakeClient())
    monkeypatch.setattr(main_mod, "console", Console(file=output, force_terminal=False, width=120))

    with redirect_stdout(output):
        main_mod._ask_agent("hello", raw=True)

    parsed = json.loads(output.getvalue())
    assert parsed["conversation_id"] == "conv-1"


def test_ask_agent_injects_saved_location_and_can_disable_it(monkeypatch):
    from io import StringIO

    from rich.console import Console

    from heyfood_cli import main as main_mod

    payloads = []

    class FakeClient:
        def saved_location(self):
            return {"latitude": 36.7, "longitude": -119.8}

        def stream_agent(self, payload):
            payloads.append(payload)
            yield "result", {"message": "done"}

        def remember_conversation(self, _result):
            pass

    monkeypatch.setattr(main_mod, "HelloFoodClient", lambda: FakeClient())
    monkeypatch.setattr(
        main_mod,
        "console",
        Console(file=StringIO(), force_terminal=False, width=120),
    )
    main_mod._ask_agent("near me", show_continue_hint=False)
    main_mod._ask_agent("private", no_location=True, show_continue_hint=False)

    assert payloads[0]["lat"] == 36.7
    assert payloads[0]["lng"] == -119.8
    assert "lat" not in payloads[1]
    assert "lng" not in payloads[1]


def test_context_and_config_commands_do_not_create_device_state(tmp_path):
    from heyfood_cli import main as main_mod

    runner = CliRunner(env={"XDG_CONFIG_HOME": str(tmp_path), "NO_COLOR": "1"})
    path_result = runner.invoke(main_mod.app, ["config", "path", "--json"])
    show_result = runner.invoke(main_mod.app, ["config", "show", "--json"])
    list_result = runner.invoke(main_mod.app, ["context", "list", "--json"])

    assert path_result.exit_code == 0
    assert show_result.exit_code == 0
    assert list_result.exit_code == 0
    assert not (tmp_path / "heyfood" / "config.json").exists()


def test_doctor_without_login_does_not_mint_device_or_config(tmp_path):
    from heyfood_cli import main as main_mod

    runner = CliRunner(env={"XDG_CONFIG_HOME": str(tmp_path), "NO_COLOR": "1"})
    result = runner.invoke(main_mod.app, ["doctor", "--json"])

    assert result.exit_code == 1
    assert '"has_session": false' in result.stdout
    assert not (tmp_path / "heyfood" / "config.json").exists()


def test_members_list_exposes_synced_member_ids(monkeypatch):
    from heyfood_cli import main as main_mod

    class FakeClient:
        def __init__(self, **_kwargs):
            pass

        def list_profile_members(self):
            return {
                "profiles": [
                    {
                        "member_id": "_self",
                        "version": 3,
                        "schema_version": 5,
                        "updated_at": "2026-07-10T12:00:00Z",
                    },
                    {
                        "member_id": "member-2",
                        "version": 1,
                        "schema_version": 5,
                        "updated_at": "2026-07-10T12:01:00Z",
                    },
                ],
                "total_count": 2,
            }

    monkeypatch.setattr(main_mod, "HelloFoodClient", FakeClient)
    result = CliRunner().invoke(main_mod.app, ["members", "list", "--json"])

    assert result.exit_code == 0
    document = __import__("json").loads(result.stdout)
    assert [item["member_id"] for item in document["profiles"]] == ["_self", "member-2"]


def test_conversation_list_and_clear_are_local_and_automation_safe(tmp_path):
    from heyfood_cli import main as main_mod
    from heyfood_cli.config import ConfigStore

    config_path = tmp_path / "heyfood" / "config.json"
    ConfigStore(config_path, credential_store=None).save(
        {
            "last_conversation": {
                "conversation_id": "conversation-1",
                "updated_at": "2026-07-10T12:00:00Z",
                "pending_confirmation": {
                    "confirmation_id": "confirmation-secret",
                    "idempotency_key": "idempotency-secret",
                },
            }
        }
    )
    runner = CliRunner(env={"XDG_CONFIG_HOME": str(tmp_path), "NO_COLOR": "1"})

    listed = runner.invoke(main_mod.app, ["conversation", "list", "--json"])
    refused = runner.invoke(main_mod.app, ["conversation", "clear", "--json"])
    cleared = runner.invoke(
        main_mod.app,
        ["conversation", "clear", "--yes", "--no-input", "--json"],
    )

    assert listed.exit_code == 0
    assert __import__("json").loads(listed.stdout)["conversations"][0][
        "conversation_id"
    ] == "conversation-1"
    assert "confirmation-secret" not in listed.stdout
    assert "idempotency-secret" not in listed.stdout
    assert refused.exit_code == 2
    assert "--yes" in refused.output
    document = __import__("json").loads(cleared.stdout)
    assert cleared.exit_code == 0
    assert document == {
        "ok": True,
        "cleared": True,
        "conversation_id": "conversation-1",
        "scope": "local_pointer_only",
    }
    assert "last_conversation" not in ConfigStore(
        config_path, credential_store=None
    ).load()


def test_conversation_resume_is_an_additive_reply_alias(monkeypatch):
    from heyfood_cli import main as main_mod

    calls = []
    monkeypatch.setattr(
        main_mod,
        "_ask_agent",
        lambda text, **kwargs: calls.append((text, kwargs)),
    )
    result = CliRunner().invoke(
        main_mod.app,
        ["conversation", "resume", "the", "second", "one", "--no-location"],
    )

    assert result.exit_code == 0
    assert calls[0][0] == "the second one"
    assert calls[0][1]["continue_last"] is True
    assert calls[0][1]["no_location"] is True


def test_completion_scripts_are_available_for_zsh_bash_and_fish():
    from heyfood_cli import main as main_mod

    runner = CliRunner()
    expected = {
        "zsh": "compdef",
        "bash": "complete",
        "fish": "complete --command",
    }
    for shell, marker in expected.items():
        result = runner.invoke(
            main_mod.app,
            [],
            prog_name="heyfood",
            env={"_HEYFOOD_COMPLETE": f"source_{shell}"},
        )
        assert result.exit_code == 0, result.output
        assert marker in result.stdout


def test_custom_context_can_be_saved_and_selected(tmp_path):
    from heyfood_cli import main as main_mod

    runner = CliRunner(env={"XDG_CONFIG_HOME": str(tmp_path), "NO_COLOR": "1"})
    result = runner.invoke(
        main_mod.app,
        [
            "context",
            "set",
            "staging",
            "--api-url",
            "https://api.staging.example",
            "--auth-url",
            "https://auth.staging.example/authorize",
            "--use",
        ],
    )
    shown = runner.invoke(main_mod.app, ["context", "show", "--json"])

    assert result.exit_code == 0
    assert shown.exit_code == 0
    document = __import__("json").loads(shown.stdout)
    assert document["name"] == "staging"
    assert document["api_url"] == "https://api.staging.example"


def test_config_validate_json_reports_repair_without_traceback(tmp_path):
    from heyfood_cli import main as main_mod

    config_dir = tmp_path / "heyfood"
    config_dir.mkdir()
    (config_dir / "config.json").write_text('{"broken":', encoding="utf-8")
    result = CliRunner(env={"XDG_CONFIG_HOME": str(tmp_path)}).invoke(
        main_mod.app,
        ["config", "validate", "--json"],
    )

    assert result.exit_code == 2
    document = __import__("json").loads(result.stdout)
    assert document["error"]["type"] == "invalid_config"
    assert "Move the invalid file aside" in document["repair"]
    assert "Traceback" not in result.stdout


class _LocationFakeClient:
    def __init__(self, saved=None, geocode_result=None, geocode_error=None):
        self._saved = saved
        self._geocode_result = geocode_result
        self._geocode_error = geocode_error
        self.geocode_calls = []

    def saved_location(self):
        return self._saved

    def geocode_location(self, query):
        self.geocode_calls.append(query)
        if self._geocode_error is not None:
            raise self._geocode_error
        return self._geocode_result


def test_resolve_location_explicit_coords_default_radius():
    from heyfood_cli import main as main_mod

    client = _LocationFakeClient()
    lat, lng, radius = main_mod.resolve_location(client, lat=1.0, lng=2.0, near=None, radius=None)
    assert (lat, lng, radius) == (1.0, 2.0, 5.0)


def test_resolve_location_explicit_radius_overrides():
    from heyfood_cli import main as main_mod

    client = _LocationFakeClient()
    _, _, radius = main_mod.resolve_location(client, lat=1.0, lng=2.0, near=None, radius=12.0)
    assert radius == 12.0


def test_resolve_location_half_supplied_coords_errors():
    import typer

    from heyfood_cli import main as main_mod

    client = _LocationFakeClient()
    try:
        main_mod.resolve_location(client, lat=1.0, lng=None, near=None, radius=None)
    except typer.BadParameter as exc:
        assert "both --lat and --lng" in str(exc)
    else:
        raise AssertionError("expected BadParameter")


def test_resolve_location_near_geocodes():
    from heyfood_cli import main as main_mod

    client = _LocationFakeClient(
        geocode_result={"label": "Fresno, CA", "latitude": 36.7, "longitude": -119.8}
    )
    lat, lng, radius = main_mod.resolve_location(client, lat=None, lng=None, near="Fresno", radius=None)
    assert (lat, lng, radius) == (36.7, -119.8, 5.0)
    assert client.geocode_calls == ["Fresno"]


def test_resolve_location_falls_back_to_saved_with_saved_radius():
    from heyfood_cli import main as main_mod

    client = _LocationFakeClient(
        saved={"latitude": 35.28, "longitude": -120.66, "radius_miles": 8.0}
    )
    lat, lng, radius = main_mod.resolve_location(client, lat=None, lng=None, near=None, radius=None)
    assert (lat, lng, radius) == (35.28, -120.66, 8.0)


def test_resolve_location_explicit_radius_beats_saved_radius():
    from heyfood_cli import main as main_mod

    client = _LocationFakeClient(
        saved={"latitude": 35.28, "longitude": -120.66, "radius_miles": 8.0}
    )
    _, _, radius = main_mod.resolve_location(client, lat=None, lng=None, near=None, radius=3.0)
    assert radius == 3.0


def test_resolve_location_none_errors_with_guidance():
    import typer

    from heyfood_cli import main as main_mod

    client = _LocationFakeClient(saved=None)
    try:
        main_mod.resolve_location(client, lat=None, lng=None, near=None, radius=None)
    except typer.BadParameter as exc:
        assert "heyfood location set" in str(exc)
    else:
        raise AssertionError("expected BadParameter")


def test_geocode_place_maps_location_not_found_to_friendly_message(monkeypatch):
    from io import StringIO

    import typer
    from rich.console import Console

    from heyfood_cli import main as main_mod
    from heyfood_cli.client import HelloFoodError

    output = StringIO()
    monkeypatch.setattr(main_mod, "stderr_console", Console(file=output, force_terminal=False, width=120))
    client = _LocationFakeClient(geocode_error=HelloFoodError("404: location_not_found"))

    try:
        main_mod._geocode_place(client, "Nowheresville")
    except typer.Exit:
        pass
    else:
        raise AssertionError("expected typer.Exit")

    text = output.getvalue()
    assert "Couldn't find a location" in text
    assert "Nowheresville" in text


def test_geocode_place_maps_unavailable_and_upstream(monkeypatch):
    from io import StringIO

    import typer
    from rich.console import Console

    from heyfood_cli import main as main_mod
    from heyfood_cli.client import HelloFoodError

    for detail, needle in (
        ("503: geocoding_unavailable", "isn't available"),
        ("502: geocoding_upstream_error", "upstream"),
    ):
        output = StringIO()
        monkeypatch.setattr(
            main_mod, "stderr_console", Console(file=output, force_terminal=False, width=120)
        )
        client = _LocationFakeClient(geocode_error=HelloFoodError(detail))
        try:
            main_mod._geocode_place(client, "Fresno, CA")
        except typer.Exit:
            pass
        else:
            raise AssertionError("expected typer.Exit")
        assert needle in output.getvalue()


def test_geocode_place_tool_unavailable_uses_tool_unavailable_path(monkeypatch):
    from io import StringIO

    import typer
    from rich.console import Console

    from heyfood_cli import main as main_mod
    from heyfood_cli.client import ChannelToolUnavailable

    output = StringIO()
    monkeypatch.setattr(main_mod, "stderr_console", Console(file=output, force_terminal=False, width=120))
    client = _LocationFakeClient(
        geocode_error=ChannelToolUnavailable(
            "The connected API does not expose the `geocode_location` channel tool yet."
        )
    )
    try:
        main_mod._geocode_place(client, "Fresno, CA")
    except typer.Exit:
        pass
    else:
        raise AssertionError("expected typer.Exit")
    assert "different versions" in output.getvalue()
