from __future__ import annotations

import json

from typer.testing import CliRunner

from heyfood_cli import main


class _Client:
    def __init__(self, **_kwargs):
        pass

    def list_channel_links(self):
        return {
            "links": [
                {
                    "id": "link-1",
                    "channel": "chatgpt",
                    "scopes": ["menu:read"],
                    "status": "active",
                    "created_at": "2026-07-19T00:00:00Z",
                    "access_token": "must-not-render",
                    "refresh_token": "must-not-render",
                }
            ],
            "total_count": 1,
            "server_internal": "must-not-render",
        }

    def disconnect_channel_link(self, link_id):
        return {
            "revoked": True,
            "link_id": link_id,
            "access_token": "must-not-render",
        }


def test_channels_list_json_is_allowlisted_and_token_free(monkeypatch):
    monkeypatch.setattr(main, "HelloFoodClient", _Client)

    result = CliRunner().invoke(
        main.app,
        ["channels", "list", "--json"],
        prog_name="heyfood",
    )

    assert result.exit_code == 0
    document = json.loads(result.stdout)
    assert document == {
        "links": [
            {
                "channel": "chatgpt",
                "created_at": "2026-07-19T00:00:00Z",
                "id": "link-1",
                "scopes": ["menu:read"],
                "status": "active",
            }
        ],
        "ok": True,
        "total_count": 1,
    }
    assert "must-not-render" not in result.stdout


def test_channels_disconnect_json_is_allowlisted_and_token_free(monkeypatch):
    monkeypatch.setattr(main, "HelloFoodClient", _Client)

    result = CliRunner().invoke(
        main.app,
        ["channels", "disconnect", "link-1", "--yes", "--no-input", "--json"],
        prog_name="heyfood",
    )

    assert result.exit_code == 0
    assert json.loads(result.stdout) == {
        "link_id": "link-1",
        "ok": True,
        "revoked": True,
    }
    assert "must-not-render" not in result.stdout


def test_channels_disconnect_noninteractive_requires_explicit_yes(monkeypatch):
    monkeypatch.setattr(main, "HelloFoodClient", _Client)

    result = CliRunner().invoke(
        main.app,
        ["channels", "disconnect", "link-1", "--no-input", "--json"],
        prog_name="heyfood",
    )

    assert result.exit_code == 2
    assert "Pass --yes" in result.output
