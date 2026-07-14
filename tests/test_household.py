from __future__ import annotations

import json
from contextlib import redirect_stdout
from datetime import datetime, timedelta, timezone
from io import StringIO

import pytest
from rich.console import Console
from typer.testing import CliRunner

from heyfood_cli import household
from heyfood_cli import render
from heyfood_cli.client import HelloFoodClient
from heyfood_cli.config import ConfigStore, redacted_config


def _state() -> dict:
    state = household.default_state(owner_name="Justin")
    state["members"].append(
        {
            "id": "member-sarah",
            "name": "Sarah",
            "relationship": "spouse",
            "is_owner": False,
            "archived": False,
            "profile_synced": True,
        }
    )
    return state


def test_scope_resolution_supports_names_ids_and_everyone() -> None:
    state = _state()

    assert household.resolve_scope(state, "me") == household.OWNER_ID
    assert household.resolve_scope(state, "Sarah") == "member-sarah"
    assert household.resolve_scope(state, "member-sarah") == "member-sarah"
    assert household.resolve_scope(state, "everyone") == household.EVERYONE_ID


def test_everyone_requires_two_active_members() -> None:
    with pytest.raises(household.HouseholdError, match="another household member"):
        household.resolve_scope(household.default_state(), "everyone")


def test_reconcile_imports_synced_ids_without_inventing_identity() -> None:
    state = household.reconcile_profile_members(
        household.default_state(owner_name="Justin"),
        ["_self", "mobile-member-id"],
        owner_name="Justin",
    )

    imported = household.find_member(state, "mobile-member-id")
    assert imported == {
        "id": "mobile-member-id",
        "name": "mobile-member-id",
        "relationship": "other",
        "is_owner": False,
        "archived": False,
        "profile_synced": True,
        "created_at": imported["created_at"],
    }


def test_household_context_is_flat_per_member_like_ios() -> None:
    context = household.household_dietary_context(
        _state(),
        {
            "_self": {"restrictions": ["glutenFree"]},
            "member-sarah": {
                "preferences": ["vegetarian"],
                "medical_condition_id": "ibs",
            },
        },
    )

    assert context["mode"] == "household"
    sarah = next(member for member in context["members"] if member["member_id"] == "member-sarah")
    assert sarah["name"] == "Sarah"
    assert sarah["relationship"] == "spouse"
    assert sarah["preferences"] == ["vegetarian"]
    assert sarah["medical_condition"] == "ibs"
    assert "dietary_context" not in sarah


def test_add_mutation_is_local_and_duplicate_sideband_is_idempotent() -> None:
    mutation = {
        "operation": "add_member",
        "mutation_id": "local-1",
        "name": "Emma",
        "relationship": "child",
    }
    first_state, first = household.apply_mutation(_state(), mutation)
    second_state, second = household.apply_mutation(
        first_state,
        {**mutation, "mutation_id": "server-1"},
    )

    assert first["applied"] is True
    assert second["reason"] == "matching_member_exists"
    assert [member["name"] for member in household.active_members(second_state)].count("Emma") == 1


def test_add_mutation_allows_same_name_after_replay_window() -> None:
    state = _state()
    state["members"].append(
        {
            "id": "older-emma",
            "name": "Emma",
            "relationship": "child",
            "is_owner": False,
            "archived": False,
            "profile_synced": True,
            "created_at": (datetime.now(timezone.utc) - timedelta(minutes=3)).isoformat(),
        }
    )

    updated, effect = household.apply_mutation(
        state,
        {"operation": "add_member", "name": "Emma", "relationship": "child"},
    )

    assert effect["applied"] is True
    assert [member["name"] for member in household.active_members(updated)].count("Emma") == 2


def test_normalization_does_not_trust_a_second_owner_flag() -> None:
    state = _state()
    state["members"][1]["is_owner"] = True

    normalized = household.normalize_state(state)

    assert [member["id"] for member in normalized["members"] if member["is_owner"]] == ["_self"]


def test_synced_adult_cannot_be_relabeled_as_child() -> None:
    with pytest.raises(household.HouseholdError, match="server-synced adult"):
        household.label_member(
            _state(),
            "member-sarah",
            name="Sarah",
            relationship="child",
        )


def test_remove_mutation_archives_and_resets_active_scope() -> None:
    state = _state()
    state["active_scope"] = "member-sarah"

    updated, effect = household.apply_mutation(
        state,
        {
            "operation": "remove_member",
            "mutation_id": "remove-1",
            "member_id": "member-sarah",
        },
    )

    assert effect["applied"] is True
    assert household.find_member(updated, "member-sarah")["archived"] is True
    assert updated["active_scope"] == household.OWNER_ID


def test_client_builds_scoped_agent_payload_without_caching_profiles(tmp_path, monkeypatch) -> None:
    store = ConfigStore(tmp_path / "config.json", credential_store=None)
    store.save({"first_name": "Justin", "household": _state()})
    client = HelloFoodClient(store=store)
    monkeypatch.setattr(client, "profile_consent_status", lambda: {"has_consent": True})

    profiles = {
        "_self": {"restrictions": ["glutenFree"]},
        "member-sarah": {"preferences": ["vegetarian"]},
    }
    monkeypatch.setattr(
        client,
        "download_profile",
        lambda *, member_id: {"member_id": member_id, "profile_data": profiles[member_id]},
    )

    context = client.agent_household_context("everyone")

    assert context["scope"] == {
        "id": household.EVERYONE_ID,
        "label": "Everyone",
        "mode": "household",
    }
    assert context["meal_context"] == {"is_cook_mode": True}
    assert context["device_context"]["household"]["owner_id"] == "_self"
    assert len(context["dietary_context"]["members"]) == 2
    assert "restrictions" not in json.dumps(store.load().get("household"))


def test_client_applies_confirmation_locally_then_syncs_new_profile(tmp_path, monkeypatch) -> None:
    store = ConfigStore(tmp_path / "config.json", credential_store=None)
    store.save({"first_name": "Justin", "household": _state()})
    client = HelloFoodClient(store=store)
    client.remember_conversation(
        {
            "conversation_id": "11111111-1111-1111-1111-111111111111",
            "structured": {
                "type": "action_confirmation",
                "confirmation_id": "22222222-2222-2222-2222-222222222222",
                "idempotency_key": "33333333-3333-3333-3333-333333333333",
                "action": "add_household_member",
                "structured_preview": {
                    "operation": "add_member",
                    "name": "Sarah",
                    "relationship": "spouse",
                    "restrictions": ["dairyFree"],
                },
            },
        }
    )
    uploads = []
    monkeypatch.setattr(
        client,
        "upload_profile",
        lambda profile_data, **kwargs: uploads.append((profile_data, kwargs)) or {"version": 1},
    )

    effect = client.apply_pending_household_confirmation()

    assert effect["applied"] is True
    assert effect["profile_sync"] == {"ok": True}
    assert uploads[0][0]["restrictions"] == ["dairyFree"]
    new_member = household.find_member(client.household_state(), effect["member_id"])
    assert new_member["name"] == "Sarah"
    assert new_member["profile_synced"] is True


def test_child_profile_stays_local_and_never_uses_profile_sync(tmp_path, monkeypatch) -> None:
    store = ConfigStore(tmp_path / "config.json", credential_store=None)
    store.save({"first_name": "Justin", "household": _state()})
    client = HelloFoodClient(store=store)
    client.remember_conversation(
        {
            "conversation_id": "11111111-1111-1111-1111-111111111111",
            "structured": {
                "type": "action_confirmation",
                "confirmation_id": "22222222-2222-2222-2222-222222222222",
                "idempotency_key": "33333333-3333-3333-3333-333333333333",
                "action": "add_household_member",
                "structured_preview": {
                    "operation": "add_member",
                    "name": "Emma",
                    "relationship": "child",
                    "restrictions": ["dairyFree"],
                },
            },
        }
    )
    monkeypatch.setattr(
        client,
        "upload_profile",
        lambda *_args, **_kwargs: pytest.fail("child profile must not be uploaded"),
    )

    effect = client.apply_pending_household_confirmation()

    member_id = effect["member_id"]
    assert effect["profile_sync"] == {
        "ok": True,
        "source": "local_only",
        "server": False,
    }
    assert client.local_household_profiles()[member_id]["restrictions"] == ["dairyFree"]
    assert household.find_member(client.household_state(), member_id)["profile_synced"] is False

    monkeypatch.setattr(client, "profile_consent_status", lambda: {"has_consent": False})
    context = client.agent_household_context(member_id)
    assert context["dietary_context"]["restrictions"] == ["dairyFree"]
    assert "device_context" not in context


def test_scoped_agent_turn_retries_a_failed_profile_outbox(tmp_path, monkeypatch) -> None:
    path = tmp_path / "config.json"
    store = ConfigStore(path, credential_store=None)
    store.save({"first_name": "Justin", "household": _state()})
    client = HelloFoodClient(store=store)
    preview = {
        "operation": "add_member",
        "name": "Jordan",
        "relationship": "partner",
        "preferences": ["vegetarian"],
    }
    client.remember_conversation(
        {
            "conversation_id": "11111111-1111-1111-1111-111111111111",
            "structured": {
                "type": "action_confirmation",
                "confirmation_id": "22222222-2222-2222-2222-222222222222",
                "idempotency_key": "33333333-3333-3333-3333-333333333333",
                "action": "add_household_member",
                "structured_preview": preview,
            },
        }
    )
    attempts = []

    def upload(profile_data, **kwargs):
        attempts.append((profile_data, kwargs))
        if len(attempts) == 1:
            raise RuntimeError("offline")
        return {"version": 1}

    # The client catches HelloFoodError, so use its public error type.
    from heyfood_cli.client import HelloFoodError

    def upload_with_typed_failure(profile_data, **kwargs):
        try:
            return upload(profile_data, **kwargs)
        except RuntimeError as exc:
            raise HelloFoodError(str(exc)) from exc

    monkeypatch.setattr(client, "upload_profile", upload_with_typed_failure)

    local_effect = client.apply_pending_household_confirmation()
    member_id = local_effect["member_id"]
    assert client.household_profile_outbox()[member_id]["fields"]["preferences"] == ["vegetarian"]
    restarted = HelloFoodClient(store=ConfigStore(path, credential_store=None))
    monkeypatch.setattr(restarted, "profile_consent_status", lambda: {"has_consent": False})
    unsynced_context = restarted.agent_household_context(member_id)
    assert unsynced_context["dietary_context"]["preferences"] == ["vegetarian"]
    monkeypatch.setattr(restarted, "profile_consent_status", lambda: {"has_consent": True})
    monkeypatch.setattr(
        restarted,
        "download_profile",
        lambda *, member_id: {"member_id": member_id, "profile_data": {}, "version": 0},
    )
    monkeypatch.setattr(restarted, "upload_profile", upload_with_typed_failure)
    synced_context = restarted.agent_household_context(member_id)

    assert local_effect["profile_sync"]["ok"] is False
    assert synced_context["dietary_context"]["preferences"] == ["vegetarian"]
    assert len(attempts) == 2
    assert member_id not in restarted.household_profile_outbox()
    member = household.find_member(restarted.household_state(), member_id)
    assert member["profile_synced"] is True


def test_failed_profile_writes_merge_losslessly_across_later_mutations(tmp_path, monkeypatch) -> None:
    from heyfood_cli.client import HelloFoodError

    store = ConfigStore(tmp_path / "config.json", credential_store=None)
    store.save({"first_name": "Justin", "household": _state()})
    client = HelloFoodClient(store=store)
    monkeypatch.setattr(
        client,
        "download_profile",
        lambda *, member_id: {
            "member_id": member_id,
            "version": 7,
            "profile_data": {"preferences": ["keto"], "restrictions": []},
        },
    )
    uploads = []

    def upload(profile_data, **kwargs):
        uploads.append((profile_data, kwargs))
        if len(uploads) == 1:
            raise HelloFoodError("offline")
        return {"version": 8}

    monkeypatch.setattr(client, "upload_profile", upload)
    first = client._apply_and_sync_household_mutation(
        {
            "operation": "update_member",
            "mutation_id": "restriction-update",
            "member_id": "member-sarah",
            "restrictions": ["peanuts"],
        }
    )
    second = client._apply_and_sync_household_mutation(
        {
            "operation": "update_member",
            "mutation_id": "preference-update",
            "member_id": "member-sarah",
            "preferences": ["vegetarian"],
        }
    )

    assert first["profile_sync"]["ok"] is False
    assert client.household_profile_outbox() == {}
    assert second["profile_sync"] == {"ok": True}
    assert uploads[1][0]["restrictions"] == ["peanuts"]
    assert uploads[1][0]["preferences"] == ["vegetarian"]
    assert uploads[1][1]["expected_version"] == 7


def test_child_onboarding_never_calls_profile_sync(tmp_path, monkeypatch) -> None:
    from heyfood_cli import main

    state = _state()
    state["members"].append(
        {
            "id": "child-1",
            "name": "Ava",
            "relationship": "child",
            "is_owner": False,
            "archived": False,
            "profile_synced": False,
        }
    )
    ConfigStore(tmp_path / "heyfood" / "config.json", credential_store=None).save(
        {"first_name": "Justin", "household": state}
    )
    monkeypatch.setattr(
        HelloFoodClient,
        "profile_consent_status",
        lambda *_args, **_kwargs: pytest.fail("child onboarding must not query consent"),
    )
    monkeypatch.setattr(
        HelloFoodClient,
        "download_profile",
        lambda *_args, **_kwargs: pytest.fail("child onboarding must not download a profile"),
    )
    monkeypatch.setattr(
        HelloFoodClient,
        "upload_profile",
        lambda *_args, **_kwargs: pytest.fail("child onboarding must not upload a profile"),
    )

    result = CliRunner(env={"XDG_CONFIG_HOME": str(tmp_path), "NO_COLOR": "1"}).invoke(
        main.app,
        [
            "onboard",
            "--member-id",
            "child-1",
            "--allergy",
            "peanuts",
            "--yes",
            "--no-input",
            "--json",
        ],
    )

    assert result.exit_code == 0, result.output
    assert json.loads(result.stdout)["storage"] == "local_only"
    saved = ConfigStore(tmp_path / "heyfood" / "config.json").load()
    assert saved["household_local_profiles"]["child-1"]["restrictions"] == ["peanutFree"]


def test_household_names_are_rendered_as_literal_text() -> None:
    output = StringIO()
    console = Console(file=output, force_terminal=False, width=200)
    hostile_name = "[/][bold red on white]ALLERGY CLEARED[/]"

    render.household_scope(console, {"id": "member-1", "label": hostile_name})
    render.household_mutation_effect(
        console,
        {"applied": True, "operation": "add_member", "member_id": "member-1", "name": hostile_name},
    )

    rendered = output.getvalue()
    assert hostile_name in rendered
    assert "Checking for" in rendered


def test_config_show_redacts_local_household_names() -> None:
    redacted = redacted_config(
        {
            "household": _state(),
            "household_local_profiles": {"child-1": {"restrictions": ["dairyFree"]}},
        }
    )

    assert redacted["household"]["member_count"] == 2
    assert redacted["household"]["local_roster"] == "<redacted>"
    assert redacted["household_local_profiles"]["dietary_data"] == "<redacted>"
    assert "Sarah" not in json.dumps(redacted)
    assert "dairyFree" not in json.dumps(redacted)


def test_household_local_commands_are_machine_readable(tmp_path) -> None:
    from heyfood_cli import main

    store = ConfigStore(tmp_path / "heyfood" / "config.json", credential_store=None)
    store.save({"first_name": "Justin", "household": _state()})
    runner = CliRunner(env={"XDG_CONFIG_HOME": str(tmp_path), "NO_COLOR": "1"})

    listed = runner.invoke(main.app, ["household", "list", "--local-only", "--json"])
    selected = runner.invoke(main.app, ["household", "use", "Sarah", "--json"])

    assert listed.exit_code == 0, listed.output
    assert json.loads(listed.stdout)["members"][1]["name"] == "Sarah"
    assert selected.exit_code == 0, selected.output
    assert json.loads(selected.stdout)["active_scope"]["id"] == "member-sarah"


def test_household_list_degrades_to_local_roster_without_sync_consent(
    tmp_path,
    monkeypatch,
) -> None:
    from heyfood_cli import main
    from heyfood_cli.client import HelloFoodError

    ConfigStore(tmp_path / "heyfood" / "config.json", credential_store=None).save(
        {"first_name": "Justin", "household": _state()}
    )

    def consent_required(_client):
        raise HelloFoodError("403: Sync consent required")

    monkeypatch.setattr(
        HelloFoodClient,
        "refresh_household_state",
        consent_required,
    )

    result = CliRunner(
        env={"XDG_CONFIG_HOME": str(tmp_path), "NO_COLOR": "1"}
    ).invoke(main.app, ["household", "list", "--json"])

    assert result.exit_code == 0, result.output
    document = json.loads(result.stdout)
    assert document["members"][1]["name"] == "Sarah"
    assert document["reconciliation"] == {
        "status": "skipped",
        "reason": "profile_sync_consent_required",
        "source": "local_roster",
    }


def test_household_list_does_not_hide_other_profile_discovery_errors(
    tmp_path,
    monkeypatch,
) -> None:
    from heyfood_cli import main
    from heyfood_cli.client import HelloFoodError

    ConfigStore(tmp_path / "heyfood" / "config.json", credential_store=None).save(
        {"first_name": "Justin", "household": _state()}
    )

    def forbidden(_client):
        raise HelloFoodError("403: Forbidden")

    monkeypatch.setattr(
        HelloFoodClient,
        "refresh_household_state",
        forbidden,
    )

    result = CliRunner(
        env={"XDG_CONFIG_HOME": str(tmp_path), "NO_COLOR": "1"}
    ).invoke(main.app, ["household", "list", "--json"])

    assert result.exit_code == 1
    assert json.loads(result.stdout)["error"]["message"] == "403: Forbidden"


def test_agent_turn_sends_scope_context_and_preserves_choices(monkeypatch) -> None:
    from heyfood_cli import main

    payloads = []

    class FakeClient:
        def saved_location(self):
            return None

        def last_conversation_id(self):
            return None

        def pending_confirmation(self):
            return None

        def agent_household_context(self, selector):
            assert selector == "Sarah"
            return {
                "scope": {"id": "member-sarah", "label": "Sarah", "mode": "member"},
                "dietary_context": {"name": "Sarah", "preferences": ["vegetarian"]},
                "device_context": {"household": {"owner_id": "_self", "members": []}},
                "meal_context": {"active_member_id": "member-sarah"},
            }

        def stream_agent(self, payload):
            payloads.append(payload)
            yield "choices", {"choices": ["Spouse", "Partner"], "allow_multiple": False}
            yield "result", {"text": "How are they related?", "conversation_id": "conv-1"}

        def remember_conversation(self, _result):
            pass

    output = StringIO()
    monkeypatch.setattr(main, "console", Console(file=output, force_terminal=False, width=120))
    with redirect_stdout(output):
        result = main._ask_agent(
            "Add Sarah",
            checking_for="Sarah",
            json_output=True,
            show_continue_hint=False,
            client=FakeClient(),
        )

    assert payloads[0]["dietary_context"]["name"] == "Sarah"
    assert payloads[0]["meal_context"]["active_member_id"] == "member-sarah"
    assert result["choices"] == {
        "choices": ["Spouse", "Partner"],
        "allow_multiple": False,
    }
    assert json.loads(output.getvalue())["choices"]["choices"] == ["Spouse", "Partner"]


def test_chat_numeric_choice_resolution() -> None:
    from heyfood_cli.main import _resolve_chat_choice

    single = {"choices": ["Spouse", "Partner"], "allow_multiple": False}
    multiple = {"choices": ["Vegan", "Vegetarian", "None"], "allow_multiple": True}

    assert _resolve_chat_choice("2", single) == "Partner"
    assert _resolve_chat_choice("1, 2", multiple) == "Vegan, Vegetarian"
    assert _resolve_chat_choice("Partner", single) == "Partner"
    with pytest.raises(household.HouseholdError, match="Choose one number"):
        _resolve_chat_choice("1,2", single)
