from __future__ import annotations

import json
from unittest.mock import MagicMock

import pytest
from typer.testing import CliRunner

from heyfood_cli import account_deletion, main
from heyfood_cli.client import HelloFoodError


DELETION_HANDLE = "hf_atx_" + "A" * 43
STATUS_TOKEN = "hf_dtx_" + "B" * 43


def _begin(**overrides):
    value = {
        "schema_version": 1,
        "deletion_handle": DELETION_HANDLE,
        "status_token": STATUS_TOKEN,
        "browser_url": f"https://auth.hello.food/account/delete?handle={DELETION_HANDLE}&csrf=public",
        "expires_in": 300,
    }
    value.update(overrides)
    return value


def _completed():
    return {
        "schema_version": 1,
        "state": "completed",
        "result": {
            "status": "scheduled",
            "deleted_at": "2026-07-16T12:00:00Z",
            "grace_period_ends_at": "2026-08-15T12:00:00Z",
            "confirmation_id": "hf-confirmation-1",
        },
    }


class Clock:
    def __init__(self):
        self.now = 0.0

    def monotonic(self):
        return self.now

    def sleep(self, seconds):
        self.now += seconds


def test_application_returns_only_strict_post_commit_receipt():
    client = MagicMock()
    client.begin_account_deletion.return_value = _begin()
    client.account_deletion_status.side_effect = [
        {"schema_version": 1, "state": "pending", "result": None},
        _completed(),
    ]
    clock = Clock()

    receipt = account_deletion.run_account_deletion(
        client,
        request_nonce="n" * 43,
        timeout_seconds=30,
        browser_callback=lambda _url: None,
        sleep=clock.sleep,
        monotonic=clock.monotonic,
    )

    assert receipt.confirmation_id == "hf-confirmation-1"
    client.cancel_account_deletion.assert_not_called()


@pytest.mark.parametrize(
    "overrides",
    [
        {"deletion_handle": "hf_atx_too-short"},
        {"deletion_handle": "hf_atx_" + "!" * 43},
        {"status_token": "hf_dtx_too-short"},
        {"status_token": "hf_dtx_" + "!" * 43},
    ],
)
def test_begin_rejects_noncanonical_capabilities(overrides):
    with pytest.raises(account_deletion.AccountDeletionContractError):
        account_deletion.validate_begin(_begin(**overrides))


def test_receipt_rejects_confirmation_id_over_backend_limit():
    completed = _completed()
    completed["result"]["confirmation_id"] = "x" * 65

    with pytest.raises(account_deletion.AccountDeletionContractError):
        account_deletion.validate_status(completed)


def test_timeout_cancels_exactly_once_and_retains_local_state():
    client = MagicMock()
    client.begin_account_deletion.return_value = _begin(expires_in=2)
    client.account_deletion_status.return_value = {
        "schema_version": 1,
        "state": "pending",
        "result": None,
    }
    client.cancel_account_deletion.return_value = {"schema_version": 1, "state": "denied"}
    clock = Clock()

    with pytest.raises(account_deletion.AccountDeletionTimeout):
        account_deletion.run_account_deletion(
            client,
            request_nonce="n" * 43,
            timeout_seconds=30,
            browser_callback=lambda _url: None,
            sleep=clock.sleep,
            monotonic=clock.monotonic,
        )

    client.cancel_account_deletion.assert_called_once_with(STATUS_TOKEN)


def test_malformed_completed_receipt_is_fail_closed_and_canceled():
    client = MagicMock()
    client.begin_account_deletion.return_value = _begin()
    malformed = _completed()
    malformed["result"]["status"] = "deleted"
    client.account_deletion_status.return_value = malformed
    client.cancel_account_deletion.return_value = {"schema_version": 1, "state": "denied"}

    with pytest.raises(account_deletion.AccountDeletionContractError):
        account_deletion.run_account_deletion(
            client,
            request_nonce="n" * 43,
            timeout_seconds=30,
            browser_callback=lambda _url: None,
        )

    client.cancel_account_deletion.assert_called_once_with(STATUS_TOKEN)


def test_delete_command_clears_credentials_only_after_completed_receipt(monkeypatch):
    client = MagicMock()
    client.channel_scopes.return_value = {"account:delete"}
    receipt = account_deletion.DeletionReceipt(
        "2026-07-16T12:00:00Z",
        "2026-08-15T12:00:00Z",
        "hf-confirmation-1",
    )
    monkeypatch.setattr(main, "HelloFoodClient", lambda **_kwargs: client)
    monkeypatch.setattr(
        account_deletion,
        "run_account_deletion",
        lambda *_args, **_kwargs: receipt,
    )

    result = CliRunner().invoke(main.app, ["account", "delete", "--yes", "--json"])

    assert result.exit_code == 0, result.output
    assert json.loads(result.stdout) == {
        "schema_version": 1,
        "state": "completed",
        "result": receipt.document(),
        "local_credentials_cleared": True,
    }
    client.store.delete.assert_called_once_with()


def test_delete_command_never_clears_on_denial_and_never_leaks_status_token(monkeypatch):
    client = MagicMock()
    client.channel_scopes.return_value = {"account:delete"}
    monkeypatch.setattr(main, "HelloFoodClient", lambda **_kwargs: client)
    monkeypatch.setattr(
        account_deletion,
        "run_account_deletion",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(
            account_deletion.AccountDeletionDenied(
                "Account deletion was canceled. Local credentials remain active."
            )
        ),
    )

    result = CliRunner().invoke(main.app, ["account", "delete", "--yes", "--json"])

    assert result.exit_code == 1
    client.store.delete.assert_not_called()
    assert json.loads(result.stdout)["error"]["type"] == "account_deletion_denied"
    assert "hf_dtx_" not in result.stdout


def test_completed_backend_with_local_cleanup_failure_is_truthful(monkeypatch):
    client = MagicMock()
    client.channel_scopes.return_value = {"account:delete"}
    client.store.delete.side_effect = OSError("read only")
    receipt = account_deletion.DeletionReceipt(
        "2026-07-16T12:00:00Z",
        "2026-08-15T12:00:00Z",
        "hf-confirmation-1",
    )
    monkeypatch.setattr(main, "HelloFoodClient", lambda **_kwargs: client)
    monkeypatch.setattr(
        account_deletion,
        "run_account_deletion",
        lambda *_args, **_kwargs: receipt,
    )

    result = CliRunner().invoke(main.app, ["account", "delete", "--yes", "--json"])

    assert result.exit_code == 1
    document = json.loads(result.stdout)
    assert document["error"]["type"] == "local_credential_cleanup_failed"
    assert "scheduled" in document["error"]["message"]


def test_missing_scope_fails_before_begin(monkeypatch):
    client = MagicMock()
    client.channel_scopes.return_value = {"account:link"}
    monkeypatch.setattr(main, "HelloFoodClient", lambda **_kwargs: client)

    result = CliRunner().invoke(main.app, ["account", "delete", "--yes", "--json"])

    assert result.exit_code == 1
    assert json.loads(result.stdout)["error"]["type"] == "missing_account_delete_scope"
    client.begin_account_deletion.assert_not_called()


def test_machine_deletion_requires_explicit_yes():
    result = CliRunner().invoke(main.app, ["account", "delete", "--json"])

    assert result.exit_code == 2
    assert json.loads(result.stdout)["error"]["type"] == "confirmation_required"


@pytest.mark.parametrize(
    ("status", "kind"),
    [(409, "fresh_grant_required"), (503, "account_deletion_unavailable")],
)
def test_retrust_or_readiness_failure_retains_local_credentials(
    monkeypatch, status, kind
):
    client = MagicMock()
    client.channel_scopes.return_value = {"account:delete"}
    monkeypatch.setattr(main, "HelloFoodClient", lambda **_kwargs: client)
    monkeypatch.setattr(
        account_deletion,
        "run_account_deletion",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(
            HelloFoodError(
                f"{status}: "
                + ("fresh_grant_required" if status == 409 else "service_unavailable")
            )
        ),
    )

    result = CliRunner().invoke(main.app, ["account", "delete", "--yes", "--json"])

    assert result.exit_code == 1
    assert json.loads(result.stdout)["error"]["type"] == kind
    client.store.delete.assert_not_called()
