from __future__ import annotations

import json

import pytest
from typer.testing import CliRunner

from heyfood_cli.client import LoginRequired
from heyfood_cli import main


AUTHENTICATED_JSON_CASES = (
    ("status", ("status", "--json")),
    ("channels-list", ("channels", "list", "--json")),
    (
        "channels-disconnect",
        ("channels", "disconnect", "link-1", "--yes", "--no-input", "--json"),
    ),
    ("profile", ("profile", "--json")),
    ("members-list", ("members", "list", "--json")),
    ("ask", ("ask", "hello", "--no-location", "--json")),
    ("reply", ("reply", "hello", "--no-location", "--json")),
    ("conversation-resume", ("conversation", "resume", "hello", "--no-location", "--json")),
    ("log", ("log", "lunch", "--json")),
    ("item", ("item", "soup", "--json")),
    ("search", ("search", "--lat", "1", "--lng", "2", "--json")),
    ("menu", ("menu", "restaurant-1", "--json")),
    ("get-menu", ("get-menu", "restaurant-1", "--json")),
    ("recommend", ("recommend", "restaurant-1", "--json")),
    ("recipes-search", ("recipes", "search", "dinner", "--json")),
    ("recipes-save", ("recipes", "save", "recipe:1", "--json")),
    ("recipes-saved", ("recipes", "saved", "--json")),
    ("daily", ("daily", "today", "--json")),
)


class _UnauthenticatedClient:
    api_url = "https://api.hello.food"

    def __init__(self, **_kwargs):
        pass

    def _login_required(self, *_args, **_kwargs):
        raise LoginRequired("Run `heyfood login` first.")

    me = _login_required
    channel_whoami = _login_required
    list_channel_links = _login_required
    disconnect_channel_link = _login_required
    profile_consent_status = _login_required
    list_profile_members = _login_required
    channel_tool = _login_required
    daily_summary = _login_required

    def saved_location(self):
        return None

    def last_conversation_id(self):
        return "conversation-1"

    def pending_confirmation(self):
        return None

    def stream_agent(self, _payload):
        raise LoginRequired("Run `heyfood login` first.")
        yield  # pragma: no cover - generator contract

    def restaurant_from_selector(self, _selector):
        return None

    def restaurant_id_from_selector(self, selector):
        return selector

    def recipe_save_payload(self, selector):
        return {"recipe_ref": selector}


class _SuccessfulClient(_UnauthenticatedClient):
    def me(self):
        return {"user_id": "user-1", "device_id": "device-1", "auth_mode": "cli"}

    def channel_whoami(self):
        return {"channel": "hellofood_cli", "scopes": ["profile:read"]}

    def list_channel_links(self):
        return {"links": [], "total_count": 0}

    def disconnect_channel_link(self, link_id):
        return {"revoked": True, "link_id": link_id}

    def profile_consent_status(self):
        return {"has_consent": True}

    def download_profile(self, *, member_id):
        return {
            "member_id": member_id,
            "version": 1,
            "updated_at": "2026-07-10T12:00:00Z",
            "profile_data": {},
        }

    def list_profile_members(self):
        return {"profiles": [], "total_count": 0}

    def stream_agent(self, _payload):
        yield "result", {"message": "done", "conversation_id": "conversation-1"}

    def remember_conversation(self, _result):
        pass

    def remember_restaurant_search(self, _result):
        pass

    def remember_recipe_search(self, _result):
        pass

    def channel_tool(self, name, _payload):
        responses = {
            "explain_item": {
                "item_name": "Soup",
                "status": "generally_safer",
                "summary": "No known conflict.",
                "confidence": 0.8,
            },
            "search_restaurant": {"restaurants": [], "total_count": 0},
            "get_menu": {
                "restaurant_id": "restaurant-1",
                "restaurant_name": "Cafe",
                "status": "ready",
                "sections": [],
            },
            "recommend_items": {
                "restaurant_id": "restaurant-1",
                "restaurant_name": "Cafe",
                "recommendations": [],
                "available": True,
                "message": "No matches.",
            },
            "search_recipes": {"recipes": [], "query_used": "dinner"},
            "save_recipe": {"ok": True, "recipe": {"title": "Dinner"}},
            "list_saved_recipes": {"recipes": [], "total_count": 0},
        }
        return responses[name]

    def daily_summary(self, date_value, *, member_id=None):
        return {"date": date_value, "member_id": member_id, "entries": []}


@pytest.mark.parametrize(("name", "args"), AUTHENTICATED_JSON_CASES)
def test_authenticated_command_unauthenticated_paths_are_structured(
    name, args, monkeypatch
):
    monkeypatch.setattr(main, "HelloFoodClient", _UnauthenticatedClient)

    result = CliRunner().invoke(main.app, list(args), prog_name="heyfood")

    assert result.exit_code == 1, f"{name}: {result.output}"
    document = json.loads(result.stdout)
    assert document["ok"] is False
    assert document["error"]["type"] == "login_required"
    assert "heyfood login" in document["error"]["message"]
    assert "\x1b" not in result.stdout


@pytest.mark.parametrize(("name", "args"), AUTHENTICATED_JSON_CASES)
def test_authenticated_command_happy_paths_emit_one_json_document(
    name, args, monkeypatch
):
    monkeypatch.setattr(main, "HelloFoodClient", _SuccessfulClient)

    result = CliRunner().invoke(main.app, list(args), prog_name="heyfood")

    assert result.exit_code == 0, f"{name}: {result.output}"
    assert isinstance(json.loads(result.stdout), dict)
    assert "\x1b" not in result.stdout


@pytest.mark.parametrize(
    "args",
    (
        ("profile", "--member-id", "", "--json"),
        ("channels", "disconnect", "link-1", "--no-input", "--json"),
        ("ask",),
        ("reply",),
        ("conversation", "resume"),
        ("log",),
        ("item",),
        ("search", "--lat", "1", "--json"),
        ("menu",),
        ("recommend", "restaurant-1", "--limit", "0", "--json"),
        ("recipes", "search", "--json"),
        ("recipes", "save", "--json"),
        ("recipes", "saved", "--limit", "0", "--json"),
        ("daily", "not-a-date", "--json"),
    ),
)
def test_invalid_command_matrix_exits_two_without_traceback(args):
    result = CliRunner().invoke(main.app, list(args), prog_name="heyfood")

    assert result.exit_code == 2
    assert "Traceback" not in result.output


def test_authenticated_command_matrix_has_no_duplicate_or_missing_labels():
    names = [name for name, _args in AUTHENTICATED_JSON_CASES]
    assert len(names) == len(set(names)) == 18
