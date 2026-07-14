from __future__ import annotations

import json
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path

import pytest
import typer
from click.utils import strip_ansi
from rich.console import Console
from typer.testing import CliRunner

from heyfood_cli import main, output
from heyfood_cli.client import LoginRequired


class _TTYBuffer(StringIO):
    def isatty(self) -> bool:
        return True


def test_json_writer_is_plain_and_parseable_even_for_tty_stream() -> None:
    stream = _TTYBuffer()

    output.write_json({"message": "generally safer", "value": 1}, stream=stream)

    value = stream.getvalue()
    assert json.loads(value) == {"message": "generally safer", "value": 1}
    assert "\x1b" not in value
    assert value.endswith("\n")


def test_json_writer_normalizes_only_safety_status_vocabulary() -> None:
    stream = StringIO()

    output.write_json(
        {
            "restaurants": [{"fit_status": "caution"}],
            "items": [
                {"status": "safe"},
                {"status": "unsafe"},
                {"status": "unable_to_evaluate"},
            ],
            "menu_job": {"status": "ready"},
        },
        stream=stream,
    )

    document = json.loads(stream.getvalue())
    assert document["restaurants"][0]["fit_status"] == "risky"
    assert [item["status"] for item in document["items"]] == [
        "generally_safer",
        "avoid",
        "unable_to_evaluate",
    ]
    assert document["menu_job"]["status"] == "ready"


class _StatusClient:
    def __init__(self, **_kwargs):
        pass

    def me(self) -> dict:
        return {"user_id": "user-1", "email": "developer@example.test"}

    def channel_whoami(self) -> dict:
        return {"channel": "hellofood_cli", "scopes": ["profile:read"]}


def test_status_json_emits_one_document(monkeypatch: pytest.MonkeyPatch) -> None:
    stream = _TTYBuffer()
    monkeypatch.setattr(main, "HelloFoodClient", _StatusClient)

    with redirect_stdout(stream):
        main.status(json_output=True, raw=False)

    assert json.loads(stream.getvalue()) == {
        "ok": True,
        "account": {"user_id": "user-1", "email": "developer@example.test"},
        "channel": {"channel": "hellofood_cli", "scopes": ["profile:read"]},
    }
    assert "\x1b" not in stream.getvalue()


def test_verbose_status_keeps_json_stdout_clean_and_uses_stderr(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from heyfood_cli import diagnostics

    class VerboseStatusClient(_StatusClient):
        def me(self) -> dict:
            diagnostics.reporter.emit(
                "http.complete",
                request_id="request-1",
                endpoint="/v1/auth/me",
                status=200,
                query="must-not-appear",
            )
            return super().me()

    stderr = StringIO()
    monkeypatch.setattr(main, "HelloFoodClient", VerboseStatusClient)
    monkeypatch.setattr(
        main,
        "stderr_console",
        Console(file=stderr, force_terminal=False, highlight=False),
    )

    result = CliRunner().invoke(main.app, ["--verbose", "status", "--json"])

    assert result.exit_code == 0
    assert json.loads(result.stdout)["ok"] is True
    assert "verbose http.complete" in stderr.getvalue()
    assert "request_id=request-1" in stderr.getvalue()
    assert "must-not-appear" not in stderr.getvalue()
    assert "verbose" not in result.stdout


class _UnauthenticatedStatusClient:
    def __init__(self, **_kwargs):
        pass

    def me(self) -> dict:
        raise LoginRequired("Run `heyfood login` first.")


def test_status_json_failure_is_structured_and_nonzero(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    stream = _TTYBuffer()
    monkeypatch.setattr(main, "HelloFoodClient", _UnauthenticatedStatusClient)

    with redirect_stdout(stream), pytest.raises(typer.Exit) as raised:
        main.status(json_output=True, raw=False)

    assert raised.value.exit_code == 1
    document = json.loads(stream.getvalue())
    assert document["ok"] is False
    assert document["error"]["type"] == "login_required"
    assert "\x1b" not in stream.getvalue()


def test_status_human_failure_uses_stderr_and_exits_one(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    stdout = StringIO()
    stderr = StringIO()
    monkeypatch.setattr(main, "HelloFoodClient", _UnauthenticatedStatusClient)
    monkeypatch.setattr(
        main,
        "stderr_console",
        Console(file=stderr, force_terminal=False, width=120),
    )

    with redirect_stdout(stdout), pytest.raises(typer.Exit) as raised:
        main.status(json_output=False, raw=False)

    assert raised.value.exit_code == 1
    assert stdout.getvalue() == ""
    assert "heyfood login" in stderr.getvalue()


def test_raw_alias_uses_same_plain_writer_and_warns_on_stderr(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    stdout = _TTYBuffer()
    stderr = StringIO()
    monkeypatch.setattr(main, "HelloFoodClient", _StatusClient)
    monkeypatch.setattr(
        main,
        "stderr_console",
        Console(file=stderr, force_terminal=True, color_system="truecolor", width=120),
    )

    with redirect_stdout(stdout):
        main.status(json_output=False, raw=True)

    assert json.loads(stdout.getvalue())["ok"] is True
    assert "\x1b" not in stdout.getvalue()
    assert "--raw is deprecated" in stderr.getvalue()


class _DoctorStore:
    path = Path("/tmp/heyfood-test-config.json")


class _UnhealthyDoctorClient:
    store = _DoctorStore()
    config: dict = {}
    api_url = "https://api.example.test"
    auth_url = "https://auth.example.test/authorize"
    context_name = "test"

    def __init__(self, **_kwargs):
        pass

    def me(self) -> dict:
        raise LoginRequired("Run `heyfood login` first.")

    def channel_whoami(self) -> dict:
        raise LoginRequired("Run `heyfood login` first.")


def test_doctor_json_reports_failed_checks_and_exits_one(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    stream = _TTYBuffer()
    monkeypatch.setattr(main, "HelloFoodClient", _UnhealthyDoctorClient)

    with redirect_stdout(stream), pytest.raises(typer.Exit) as raised:
        main.doctor(json_output=True, raw=False)

    assert raised.value.exit_code == 1
    document = json.loads(stream.getvalue())
    assert document["ok"] is False
    assert document["checks"]["session"]["ok"] is False
    assert document["checks"]["channel"]["ok"] is False
    assert "access_token" not in stream.getvalue()
    assert "\x1b" not in stream.getvalue()


@pytest.mark.parametrize(
    "args",
    (
        ("status",),
        ("doctor",),
        ("profile",),
        ("onboard",),
        ("ask",),
        ("reply",),
        ("log",),
        ("item",),
        ("search",),
        ("location",),
        ("location", "show"),
        ("location", "set"),
        ("location", "clear"),
        ("menu",),
        ("get-menu",),
        ("recommend",),
        ("recipes", "search"),
        ("recipes", "save"),
        ("recipes", "saved"),
        ("daily",),
        ("members", "list"),
        ("household", "list"),
        ("household", "current"),
        ("household", "use"),
        ("household", "label"),
        ("conversation", "list"),
        ("conversation", "resume"),
        ("conversation", "clear"),
    ),
)
def test_data_commands_advertise_json(args: tuple[str, ...]) -> None:
    result = CliRunner().invoke(main.app, [*args, "--help"], prog_name="heyfood")

    assert result.exit_code == 0
    assert "--json" in strip_ansi(result.stdout)


def test_interactive_chat_rejects_json() -> None:
    result = CliRunner().invoke(main.app, ["chat", "--json"], prog_name="heyfood")

    assert result.exit_code == 2
    assert "does not support --json" in strip_ansi(result.output)


def test_menu_json_pending_is_spinner_free_and_nonzero(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    class FakeClient:
        def restaurant_id_from_selector(self, value: str) -> str:
            return value

        def channel_tool(self, name: str, payload: dict) -> dict:
            return {"status": "acquiring", "job_id": "job-1"}

    stdout = _TTYBuffer()
    stderr = StringIO()
    monkeypatch.setattr(main, "HelloFoodClient", FakeClient)
    monkeypatch.setattr(
        main,
        "_poll_menu_until_terminal",
        lambda *args, **kwargs: {
            "status": "timed_out",
            "job_id": "job-1",
            "poll_timeout_seconds": 30.0,
        },
    )
    monkeypatch.setattr(
        main,
        "stderr_console",
        Console(file=stderr, force_terminal=True, color_system="truecolor", width=120),
    )

    with redirect_stdout(stdout), pytest.raises(typer.Exit) as raised:
        main.menu("restaurant-1", json_output=True, raw=False)

    assert raised.value.exit_code == 1
    assert json.loads(stdout.getvalue()) == {
        "status": "timed_out",
        "job_id": "job-1",
        "poll_timeout_seconds": 30.0,
    }
    assert "\x1b" not in stdout.getvalue()
    assert stderr.getvalue() == ""
