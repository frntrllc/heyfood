"""Destructive account-management commands."""
from __future__ import annotations

import secrets
import webbrowser

from .. import account_deletion, main
from ..main import Confirm, HelloFoodError, LoginRequired, _fail, _write_result, account_app, typer


@account_app.command("delete")
def account_delete(
    yes: bool = typer.Option(False, "--yes", "-y", help="Acknowledge the destructive browser confirmation step."),
    no_browser: bool = typer.Option(False, "--no-browser", help="Print the confirmation URL instead of opening it."),
    timeout: int = typer.Option(300, "--timeout", min=1, max=600, help="Seconds to wait for browser confirmation."),
    json_output: bool = typer.Option(False, "--json", help="Print one stable JSON receipt and never prompt or open a browser."),
) -> None:
    """Permanently delete your hello.food account after browser confirmation."""
    json_mode = json_output is True
    if not yes:
        if json_mode or not main._interactive_terminal():
            _fail(
                "Account deletion requires --yes before starting browser confirmation.",
                kind="confirmation_required",
                json_mode=json_mode,
                exit_code=2,
            )
        try:
            accepted = Confirm.ask(
                "Permanently delete your hello.food account and dietary data?",
                default=False,
            )
        except KeyboardInterrupt:
            _fail(
                "Account deletion canceled. Your account and local credentials were retained.",
                kind="account_deletion_interrupted",
                json_mode=False,
                exit_code=130,
            )
        if not accepted:
            main.console.print("Account deletion canceled. Nothing was changed.")
            return

    client = main.HelloFoodClient(create_device=False)
    if "account:delete" not in client.channel_scopes():
        # A stored grant without account:delete now most often means the live
        # hello.food server does not yet offer account deletion, so login never
        # requested the scope (see resolve_login_capabilities). Re-authenticating
        # would not help, so we say so plainly instead of raising a raw
        # authorization error or telling the user to log in again.
        _fail(
            "Account deletion isn't available on this hello.food server yet — "
            "it arrives with an upcoming update.",
            kind="missing_account_delete_scope",
            json_mode=json_mode,
        )

    def show_browser(url: str) -> None:
        main.stderr_console.print(f"Open this URL to confirm account deletion:\n{url}")
        if not no_browser and not json_mode:
            try:
                webbrowser.open(url)
            except webbrowser.Error:
                pass

    try:
        receipt = account_deletion.run_account_deletion(
            client,
            request_nonce=secrets.token_urlsafe(32),
            timeout_seconds=timeout,
            browser_callback=show_browser,
        )
    except account_deletion.AccountDeletionError as exc:
        _fail(
            str(exc),
            kind=exc.kind,
            json_mode=json_mode,
            exit_code=130 if isinstance(exc, account_deletion.AccountDeletionInterrupted) else 1,
        )
    except (LoginRequired, HelloFoodError) as exc:
        message = str(exc)
        if message.startswith("409:") or "fresh_grant_required" in message:
            _fail(
                "Account deletion requires a fresh authorization grant.",
                kind="fresh_grant_required",
                json_mode=json_mode,
                hint=(
                    "Your account and local credentials were retained. "
                    "Run `heyfood login` again, then retry `heyfood account delete`."
                ),
            )
        if message.startswith("503:"):
            _fail(
                "Account deletion is temporarily unavailable.",
                kind="account_deletion_unavailable",
                json_mode=json_mode,
                hint="Your account and local credentials were retained. Try again later.",
            )
        _fail(
            message,
            kind="account_deletion_failed",
            json_mode=json_mode,
            hint="Your account and local credentials were retained.",
        )

    # The completed receipt is emitted only after the backend transaction has
    # committed. This is the sole point where local credentials may be cleared.
    try:
        client.store.delete()
    except Exception:
        _fail(
            "Account deletion was scheduled, but local credential cleanup failed.",
            kind="local_credential_cleanup_failed",
            json_mode=json_mode,
            hint=(
                f"Confirmation {receipt.confirmation_id}. Remove the heyfood config "
                "and credential-store entry before reusing this machine."
            ),
        )
    document = {
        "schema_version": 1,
        "state": "completed",
        "result": receipt.document(),
        "local_credentials_cleared": True,
    }
    if _write_result(document, json_mode=json_mode):
        return
    main.console.print(
        "[green]Account deletion scheduled and local credentials cleared.[/green]"
    )
    main.console.print(
        f"Confirmation: {receipt.confirmation_id}\nGrace period ends: {receipt.grace_period_ends_at}"
    )
