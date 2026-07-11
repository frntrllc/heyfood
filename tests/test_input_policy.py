from __future__ import annotations

import json
from pathlib import Path

import pytest
import typer
from click.utils import strip_ansi
from typer.testing import CliRunner

from heyfood_cli import main
from heyfood_cli.client import HelloFoodError


def test_onboard_dry_run_no_input_has_no_network_prompt_or_persistence(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    config_root = tmp_path / "config"

    def forbidden_client():
        raise AssertionError("dry-run must not construct an API client")

    monkeypatch.setattr(main, "HelloFoodClient", forbidden_client)
    result = CliRunner(env={"XDG_CONFIG_HOME": str(config_root)}).invoke(
        main.app,
        [
            "onboard",
            "--diet",
            "keto",
            "--dry-run",
            "--no-input",
            "--json",
        ],
        prog_name="heyfood",
    )

    assert result.exit_code == 0
    profile = json.loads(result.stdout)["profile_data"]
    assert profile["preferences"] == ["keto"]
    assert profile["selection_provenance_version"] == 1
    assert profile["diet_style_ids"] == ["keto"]
    assert "What should I call you" not in result.output
    assert not config_root.exists()


def test_onboard_rejects_severity_without_a_condition_as_structured_input_error(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        main,
        "HelloFoodClient",
        lambda: (_ for _ in ()).throw(AssertionError("client should not be created")),
    )

    result = CliRunner().invoke(
        main.app,
        [
            "onboard",
            "--severity",
            "4",
            "--dry-run",
            "--no-input",
            "--json",
        ],
        prog_name="heyfood",
    )

    assert result.exit_code == 2
    assert json.loads(result.stdout) == {
        "error": {
            "message": "Severity requires at least one health condition.",
            "type": "invalid_input",
        },
        "ok": False,
    }


def test_natural_language_onboarding_reviews_extraction_and_prompts_only_missing_sections(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls: list[str] = []
    captured: list[dict] = []

    monkeypatch.setattr(main, "_stdin_is_tty", lambda: True)
    monkeypatch.setattr(main.Confirm, "ask", lambda *args, **kwargs: True)
    monkeypatch.setattr(
        main,
        "_prompt_multi_options",
        lambda label, options: calls.append(label) or None,
    )
    monkeypatch.setattr(
        main,
        "_prompt_one_option",
        lambda label, options: calls.append(label) or None,
    )
    monkeypatch.setattr(
        main,
        "_prompt_free_text_list",
        lambda label, examples: calls.append(label) or None,
    )
    monkeypatch.setattr(
        main.Prompt,
        "ask",
        lambda label, **kwargs: calls.append(str(label)) or "",
    )
    monkeypatch.setattr(main.output, "write_json", captured.append)

    result = CliRunner().invoke(
        main.app,
        [
            "onboard",
            "I'm keto and avoid onion",
            "--dry-run",
        ],
        prog_name="heyfood",
    )

    assert result.exit_code == 0
    assert "Extracted dietary profile" in result.output
    assert "Diet style" not in calls
    assert "Specific ingredients to avoid" not in calls
    assert calls == [
        "Allergies or restrictions",
        "Health conditions",
        "Activity level",
        "Cuisines you love",
        "Notes [dim](Enter to skip, '-' to clear)[/dim]",
    ]
    assert captured[0]["profile_data"]["diet_style_ids"] == ["keto"]
    assert captured[0]["profile_data"]["avoid_ingredients"] == ["onion"]


def test_rejecting_extraction_runs_full_guided_replacement(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    captured: list[dict] = []

    monkeypatch.setattr(main, "_stdin_is_tty", lambda: True)
    monkeypatch.setattr(main.Confirm, "ask", lambda *args, **kwargs: False)
    monkeypatch.setattr(
        main,
        "_prompt_onboarding_values",
        lambda: {
            "diets": ["vegan"],
            "allergies": None,
            "conditions": None,
            "avoid_ingredients": None,
            "activity_level": None,
            "cuisines": None,
            "severity_level": None,
            "notes": None,
        },
    )
    monkeypatch.setattr(main.output, "write_json", captured.append)

    result = CliRunner().invoke(
        main.app,
        ["onboard", "I'm keto", "--dry-run"],
        prog_name="heyfood",
    )

    assert result.exit_code == 0
    profile = captured[0]["profile_data"]
    assert profile["diet_style_ids"] == ["vegan"]
    assert "keto" not in profile["preferences"]


def test_onboard_no_input_mutation_requires_yes_before_client_creation(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        main,
        "HelloFoodClient",
        lambda: (_ for _ in ()).throw(AssertionError("client should not be created")),
    )

    result = CliRunner().invoke(
        main.app,
        ["onboard", "--diet", "keto", "--no-input", "--json"],
        prog_name="heyfood",
    )

    assert result.exit_code == 2
    assert "require --yes" in strip_ansi(result.output)


def test_onboard_yes_no_input_mutates_without_prompt(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    class FakeClient:
        def profile_consent_status(self):
            return {"has_consent": False}

        def grant_profile_consent(self, *, consent_version):
            return {"consent_version": consent_version}

        def download_profile(self, *, member_id):
            raise HelloFoodError("404: profile_not_found")

        def upload_profile(self, profile_data, *, member_id, expected_version):
            return {
                "member_id": member_id,
                "version": 1,
                "profile_data": profile_data,
            }

    monkeypatch.setattr(main, "HelloFoodClient", FakeClient)
    result = CliRunner(env={"XDG_CONFIG_HOME": str(tmp_path / "config")}).invoke(
        main.app,
        [
            "onboard",
            "--diet",
            "keto",
            "--yes",
            "--no-input",
            "--json",
        ],
        prog_name="heyfood",
    )

    assert result.exit_code == 0
    assert json.loads(result.stdout)["version"] == 1
    assert "Allow profile sync" not in result.output
    assert "What should I call you" not in result.output


def test_yes_does_not_disable_unrelated_guided_name_prompt(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    monkeypatch.setattr(main, "_stdin_is_tty", lambda: True)
    result = CliRunner(env={"XDG_CONFIG_HOME": str(tmp_path / "config")}).invoke(
        main.app,
        ["onboard", "--diet", "keto", "--yes", "--dry-run"],
        input="Alex\n",
        prog_name="heyfood",
    )

    assert result.exit_code == 0
    assert "First name (for your CLI greeting" in result.output


def test_chat_rejects_no_input_and_non_tty_without_constructing_client(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(main, "_stdin_is_tty", lambda: False)
    monkeypatch.setattr(
        main,
        "HelloFoodClient",
        lambda: (_ for _ in ()).throw(AssertionError("client should not be created")),
    )

    result = CliRunner().invoke(
        main.app,
        ["chat", "--no-input"],
        prog_name="heyfood",
    )

    assert result.exit_code == 2
    assert "requires TTY stdin" in result.output


@pytest.mark.parametrize(
    ("callback", "message"),
    (
        (
            lambda: main.search(
                query="thai",
                lat=91,
                lng=0,
                near=None,
                radius=5,
                limit=10,
                json_output=True,
                raw=False,
            ),
            "Latitude",
        ),
        (
            lambda: main.recipes_search(
                query=["   "],
                cuisine=None,
                meal_type=None,
                max_ready_time=None,
                limit=5,
                json_output=True,
                raw=False,
            ),
            "must not be empty",
        ),
        (
            lambda: main.daily_summary(
                day="07/10/2026",
                member_id=None,
                json_output=True,
                raw=False,
            ),
            "YYYY-MM-DD",
        ),
    ),
)
def test_invalid_command_input_fails_before_client_creation(
    monkeypatch: pytest.MonkeyPatch,
    callback,
    message: str,
) -> None:
    monkeypatch.setattr(
        main,
        "HelloFoodClient",
        lambda: (_ for _ in ()).throw(AssertionError("client should not be created")),
    )

    with pytest.raises(typer.BadParameter, match=message):
        callback()
