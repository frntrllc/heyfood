"""Authentication, session, and diagnostics commands."""
from __future__ import annotations

from .. import main
from ..main import (
    Any,
    ConfigStore,
    DEFAULT_API_KEY,
    HelloFoodError,
    LoginRequired,
    Optional,
    _error_kind,
    _json_mode,
    _raise_command_error,
    _write_result,
    app,
    banner,
    configured_config_path,
    local_urls,
    output,
    perform_device_login,
    perform_login,
    personality,
    render,
    resolve_service_urls,
    typer,
)

@app.command()
def login(
    api_url: Optional[str] = typer.Option(None, "--api-url", help="Override the active context API URL."),
    auth_url: Optional[str] = typer.Option(None, "--auth-url", help="Override the active context auth URL."),
    api_key: str = typer.Option(DEFAULT_API_KEY, "--api-key", help="API key for first-party app session refresh."),
    local: bool = typer.Option(False, "--local", help="Use the local Docker dev URLs."),
    device: bool = typer.Option(False, "--device", help="Use short-code login for SSH/headless systems."),
    no_browser: bool = typer.Option(False, "--no-browser", help="Print the login URL instead of opening a browser."),
    timeout: int = typer.Option(180, "--timeout", help="Seconds to wait for browser callback."),
) -> None:
    """Authenticate this machine with HelloFood."""
    store = ConfigStore()
    configured_api, configured_auth, _ = resolve_service_urls(store.load())
    if local:
        api_url, auth_url = local_urls()
    else:
        api_url = api_url or configured_api
        auth_url = auth_url or configured_auth
    banner.controller.loading(main.stderr_console)
    main.stderr_console.print("[bold]Opening hello.food login...[/bold]")
    try:
        if device:
            perform_device_login(
                store=store,
                api_url=api_url,
                auth_url=auth_url,
                api_key=api_key or None,
                open_browser=not no_browser,
                timeout_seconds=timeout,
                authorization_callback=lambda url, code: main.stderr_console.print(
                    f"Open this URL:\n{url}\n\nEnter code: [bold]{code}[/bold]"
                ),
            )
        else:
            perform_login(
                store=store,
                api_url=api_url,
                auth_url=auth_url,
                api_key=api_key or None,
                open_browser=not no_browser,
                timeout_seconds=timeout,
                authorize_url_callback=lambda url: main.stderr_console.print(f"Open this URL:\n{url}"),
            )
    except Exception as exc:
        message = str(exc)
        main.stderr_console.print(f"[red]Login failed:[/red] {message}")
        if "Channel OAuth is disabled" in message:
            main.stderr_console.print(
                "For local development, set [bold]CHANNEL_OAUTH_ENABLED=true[/bold] "
                "in [bold]backend/.env[/bold], restart the API, then run "
                "[bold]heyfood login --local[/bold]."
            )
        raise typer.Exit(1) from exc
    first_name = _first_name_from_account(store)
    if first_name:
        main.console.print(f"[green]Connected. Welcome, {first_name}. The CLI is ready.[/green]")
    else:
        main.console.print("[green]Connected. The CLI is ready.[/green]")


@app.command()
def logout(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON teardown results."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Forget local credentials and revoke the current app session."""
    client = main.HelloFoodClient(create_device=False)
    result = client.revoke_local_session()
    if _write_result(result, json_mode=_json_mode(json_output, raw)):
        return
    if result["remote_complete"]:
        main.console.print("[green]Logged out.[/green]")
    else:
        main.console.print(
            "[yellow]Logged out locally. Some server cleanup could not be confirmed; "
            "remaining sessions will expire automatically.[/yellow]"
        )


@app.command()
def status(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Show the active HelloFood account and channel scopes."""
    json_mode = _json_mode(json_output, raw)
    client = main.HelloFoodClient(create_device=False)
    try:
        me = client.me()
        whoami = client.channel_whoami()
    except (LoginRequired, HelloFoodError) as exc:
        _raise_command_error(exc, json_mode=json_mode)
    if _write_result({"ok": True, "account": me, "channel": whoami}, json_mode=json_mode):
        return
    render.status(main.console, me, whoami)


@app.command()
def doctor(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON diagnostics."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Check local config, auth refresh, and API reachability."""
    json_mode = _json_mode(json_output, raw)
    client = main.HelloFoodClient(
        store=ConfigStore(configured_config_path()),
        create_device=False,
    )
    config_check = {
        "ok": True,
        "path": str(client.store.path),
        "api_url": client.api_url,
        "auth_url": client.auth_url,
        "context": client.context_name,
        "has_api_key": bool(client.config.get("api_key")),
        "has_session": isinstance(client.config.get("session"), dict),
        "has_channel": isinstance(client.config.get("oauth"), dict),
    }
    checks: dict[str, Any] = {"config": config_check}

    try:
        me = client.me()
    except (LoginRequired, HelloFoodError) as exc:
        checks["session"] = {
            "ok": False,
            "error": str(exc),
            "type": _error_kind(exc),
        }
    else:
        checks["session"] = {"ok": True, "user_id": me.get("user_id")}

    try:
        whoami = client.channel_whoami()
    except (LoginRequired, HelloFoodError) as exc:
        checks["channel"] = {
            "ok": False,
            "error": str(exc),
            "type": _error_kind(exc),
        }
    else:
        checks["channel"] = {
            "ok": True,
            "channel": whoami.get("channel"),
            "scopes": whoami.get("scopes") or [],
        }

    ok = all(bool(value.get("ok")) for value in checks.values())
    document = {"ok": ok, "checks": checks}
    if json_mode:
        output.write_json(document)
    else:
        render.doctor_config(main.stderr_console, client.config, config_path=client.store.path)
        session = checks["session"]
        channel = checks["channel"]
        if session["ok"]:
            main.stderr_console.print(
                f"[green]Session OK[/green] user={session.get('user_id') or 'unknown'}"
            )
        else:
            main.stderr_console.print(f"[red]Session:[/red] {session['error']}")
        if channel["ok"]:
            scopes = ", ".join(channel.get("scopes") or [])
            main.stderr_console.print(
                f"[green]Channel OK[/green] {channel.get('channel') or 'unknown'} [{scopes}]"
            )
        else:
            main.stderr_console.print(f"[red]Channel:[/red] {channel['error']}")
    if not ok:
        raise typer.Exit(1)


def _first_name_from_account(store: ConfigStore) -> str | None:
    try:
        me = main.HelloFoodClient(store=store).me()
    except HelloFoodError:
        return None
    first_name = personality.first_name_from_account(me)
    if first_name:
        personality.save_cli_first_name(store, first_name)
    return first_name
