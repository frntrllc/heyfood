"""Application service for deliberate, receipt-backed account deletion.

The browser performs the destructive confirmation. The CLI holds the opaque
status capability in memory, polls once per interval, and clears local
credentials only after a strictly validated post-commit receipt.
"""
from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime
import re
import time
from typing import Any, Callable, Protocol
from urllib.parse import parse_qs, urlsplit


class AccountDeletionError(RuntimeError):
    kind = "account_deletion_failed"


class AccountDeletionContractError(AccountDeletionError):
    kind = "account_deletion_contract_error"


class AccountDeletionTimeout(AccountDeletionError):
    kind = "account_deletion_timeout"


class AccountDeletionInterrupted(AccountDeletionError):
    kind = "account_deletion_interrupted"


class AccountDeletionDenied(AccountDeletionError):
    kind = "account_deletion_denied"


class AccountDeletionExpired(AccountDeletionError):
    kind = "account_deletion_expired"


class DeletionClient(Protocol):
    def begin_account_deletion(self, request_nonce: str) -> dict[str, Any]: ...
    def account_deletion_status(self, status_token: str) -> dict[str, Any]: ...
    def cancel_account_deletion(self, status_token: str) -> dict[str, Any]: ...


@dataclass(frozen=True)
class DeletionBegin:
    deletion_handle: str
    status_token: str
    browser_url: str
    expires_in: int


@dataclass(frozen=True)
class DeletionReceipt:
    deleted_at: str
    grace_period_ends_at: str
    confirmation_id: str

    def document(self) -> dict[str, str]:
        return {
            "status": "scheduled",
            "deleted_at": self.deleted_at,
            "grace_period_ends_at": self.grace_period_ends_at,
            "confirmation_id": self.confirmation_id,
        }


def run_account_deletion(
    client: DeletionClient,
    *,
    request_nonce: str,
    timeout_seconds: int,
    interval_seconds: float = 2.0,
    browser_callback: Callable[[str], None],
    sleep: Callable[[float], None] = time.sleep,
    monotonic: Callable[[], float] = time.monotonic,
) -> DeletionReceipt:
    begin = validate_begin(client.begin_account_deletion(request_nonce))
    deadline = monotonic() + min(max(1, timeout_seconds), begin.expires_in)
    active = True
    try:
        browser_callback(begin.browser_url)
        while monotonic() < deadline:
            status = validate_status(client.account_deletion_status(begin.status_token))
            state = status["state"]
            if state == "pending":
                sleep(min(max(0.1, interval_seconds), max(0.0, deadline - monotonic())))
                continue
            active = False
            if state == "completed":
                return validate_receipt(status["result"])
            if state == "denied":
                raise AccountDeletionDenied(
                    "Account deletion was canceled in the browser. Your account and local credentials remain active."
                )
            raise AccountDeletionExpired(
                "The account-deletion confirmation expired. Your account and local credentials remain active."
            )
    except KeyboardInterrupt:
        if active:
            _cancel_once(client, begin.status_token)
        raise AccountDeletionInterrupted(
            "Account deletion was interrupted and canceled. Your local credentials were retained."
        ) from None
    except (AccountDeletionDenied, AccountDeletionExpired):
        raise
    except AccountDeletionError:
        if active:
            _cancel_once(client, begin.status_token)
        raise
    except Exception:
        if active:
            _cancel_once(client, begin.status_token)
        raise

    if active:
        _cancel_once(client, begin.status_token)
    raise AccountDeletionTimeout(
        "Account deletion timed out and was canceled. Your account and local credentials were retained."
    )


def validate_begin(value: Any) -> DeletionBegin:
    data = _strict_object(
        value,
        {"schema_version", "deletion_handle", "status_token", "browser_url", "expires_in"},
        "account-deletion begin",
    )
    _schema_one(data)
    handle = _nonempty(data["deletion_handle"], "deletion_handle")
    token = _nonempty(data["status_token"], "status_token")
    if not re.fullmatch(r"hf_atx_[A-Za-z0-9_-]{43}", handle) or not re.fullmatch(
        r"hf_dtx_[A-Za-z0-9_-]{43}", token
    ):
        raise AccountDeletionContractError("Account-deletion begin returned malformed capabilities.")
    expires_in = data["expires_in"]
    if isinstance(expires_in, bool) or not isinstance(expires_in, int) or not 1 <= expires_in <= 600:
        raise AccountDeletionContractError("Account-deletion begin returned an invalid expiry.")
    browser_url = _validated_browser_url(data["browser_url"], handle=handle)
    return DeletionBegin(handle, token, browser_url, expires_in)


def validate_status(value: Any) -> dict[str, Any]:
    data = _strict_object(value, {"schema_version", "state", "result"}, "account-deletion status")
    _schema_one(data)
    state = data["state"]
    if state not in {"pending", "completed", "denied", "expired"}:
        raise AccountDeletionContractError("Account-deletion status returned an unknown state.")
    if state == "completed":
        validate_receipt(data["result"])
    elif data["result"] is not None:
        raise AccountDeletionContractError("A non-completed deletion state included a receipt.")
    return data


def validate_receipt(value: Any) -> DeletionReceipt:
    data = _strict_object(
        value,
        {"status", "deleted_at", "grace_period_ends_at", "confirmation_id"},
        "account-deletion receipt",
    )
    if data["status"] != "scheduled":
        raise AccountDeletionContractError("Account-deletion receipt has an invalid status.")
    deleted_at = _iso_datetime(data["deleted_at"], "deleted_at")
    grace = _iso_datetime(data["grace_period_ends_at"], "grace_period_ends_at")
    confirmation = _nonempty(data["confirmation_id"], "confirmation_id")
    if len(confirmation) > 64:
        raise AccountDeletionContractError(
            "Account-deletion response has an invalid confirmation_id."
        )
    return DeletionReceipt(deleted_at, grace, confirmation)


def _cancel_once(client: DeletionClient, status_token: str) -> None:
    try:
        response = client.cancel_account_deletion(status_token)
        data = _strict_object(response, {"schema_version", "state"}, "account-deletion cancel")
        _schema_one(data)
        if data["state"] != "denied":
            raise AccountDeletionContractError("Account-deletion cancel was not acknowledged.")
    except Exception:
        # Cancellation is one-attempt and best-effort. Never retry a destructive
        # protocol request or mask the original interruption/timeout.
        return


def _strict_object(value: Any, keys: set[str], label: str) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != keys:
        raise AccountDeletionContractError(f"{label} returned an unexpected response.")
    return value


def _schema_one(data: dict[str, Any]) -> None:
    if data.get("schema_version") != 1:
        raise AccountDeletionContractError("Account-deletion response uses an unsupported schema version.")


def _nonempty(value: Any, field: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise AccountDeletionContractError(f"Account-deletion response has an invalid {field}.")
    return value


def _iso_datetime(value: Any, field: str) -> str:
    text = _nonempty(value, field)
    try:
        datetime.fromisoformat(text.replace("Z", "+00:00"))
    except ValueError as exc:
        raise AccountDeletionContractError(
            f"Account-deletion response has an invalid {field}."
        ) from exc
    return text


def _validated_browser_url(value: Any, *, handle: str) -> str:
    url = _nonempty(value, "browser_url")
    parsed = urlsplit(url)
    production = (
        parsed.scheme == "https"
        and parsed.hostname == "auth.hello.food"
        and parsed.port in {None, 443}
    )
    local = parsed.scheme == "http" and parsed.hostname in {"localhost", "127.0.0.1", "::1"}
    query = parse_qs(parsed.query, keep_blank_values=True)
    trusted_query = (
        set(query) == {"handle", "csrf"}
        and query.get("handle") == [handle]
        and len(query.get("csrf") or []) == 1
        and bool(query["csrf"][0])
    )
    if (
        not (production or local)
        or parsed.path != "/account/delete"
        or parsed.username
        or parsed.password
        or parsed.fragment
        or not trusted_query
    ):
        raise AccountDeletionContractError("Account-deletion begin returned an untrusted browser URL.")
    return url
