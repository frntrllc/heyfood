"""Authentication, session, and diagnostics commands."""
from __future__ import annotations

from .. import auth_application
from .. import main
from ..main import (
    Any,
    ConfigStore,
    DEFAULT_API_KEY,
    HelloFoodError,
    LoginInterrupted,
    LoginRequired,
    Optional,
    Prompt,
    _error_kind,
    _fail,
    _json_mode,
    _raise_command_error,
    _write_result,
    app,
    banner,
    configured_config_path,
    local_urls,
    output,
    personality,
    perform_device_login,
    perform_login,
    render,
    resolve_service_urls,
    typer,
)


def _auth_urls(
    store: ConfigStore,
    *,
    api_url: str | None,
    auth_url: str | None,
    local: bool,
) -> tuple[str, str]:
    configured_api, configured_auth, _ = resolve_service_urls(store.load())
    if local:
        return local_urls()
    return api_url or configured_api, auth_url or configured_auth


def _authenticate(
    *,
    intent: auth_application.AuthIntent,
    store: ConfigStore,
    api_url: str,
    auth_url: str,
    api_key: str,
    device: bool,
    no_browser: bool,
    timeout: int,
    json_mode: bool = False,
) -> auth_application.AuthApplicationResult:
    return auth_application.authenticate(
        intent=intent,
        store=store,
        api_url=api_url,
        auth_url=auth_url,
        api_key=api_key or None,
        device=device,
        # Machine output is never permission to launch a browser. The exact URL
        # remains on stderr so a controller or person can open it deliberately.
        open_browser=not no_browser and not json_mode,
        timeout_seconds=timeout,
        authorize_url_callback=lambda url: main.stderr_console.print(
            f"Open this URL:\n{url}"
        ),
        device_authorization_callback=lambda url, code: main.stderr_console.print(
            f"Open this URL:\n{url}\n\nEnter code: [bold]{code}[/bold]"
        ),
        capabilities_callback=(
            None
            if intent != "register" or json_mode
            else lambda _capabilities: main.stderr_console.print(
                "[dim]Create an account with email or a US mobile number.[/dim]"
            )
        ),
        login_runner=perform_login,
        device_login_runner=perform_device_login,
    )


def _profile_readiness(
    *,
    capability_available: bool,
) -> dict[str, Any]:
    if not capability_available:
        return {
            "profile_status": "unknown",
            "has_profile_sync_consent": None,
            "profile_version": None,
        }
    try:
        return main.HelloFoodClient(create_device=False).profile_readiness()
    except (LoginRequired, HelloFoodError):
        # Authentication remains committed. A readiness outage must never be
        # guessed as a missing profile or roll credentials back.
        return {
            "profile_status": "unknown",
            "has_profile_sync_consent": None,
            "profile_version": None,
        }


def _run_onboarding_handoff(*, voice: bool = False) -> None:
    """Invoke the existing onboarding application path with explicit values."""
    from .profiles import run_onboarding

    run_onboarding(
        profile_text=None,
        diet=None,
        allergy=None,
        condition=None,
        avoid=None,
        cuisine=None,
        activity=None,
        notes=None,
        severity=None,
        member_id="_self",
        replace=False,
        yes=False,
        voice=voice,
        voice_capture_mode="auto",
        audio_device=None,
        voice_timeout=300,
        no_browser=False,
        interactive=True,
        no_input=False,
        list_options=False,
        dry_run=False,
        json_output=False,
        raw=False,
    )


def _complete_first_profile(
    readiness: dict[str, Any],
    *,
    offer_onboarding: bool,
) -> tuple[dict[str, Any], str]:
    status_value = readiness.get("profile_status")
    if status_value != "missing":
        return readiness, "not_needed" if status_value == "ready" else "unavailable"
    if not offer_onboarding:
        return readiness, "deferred"
    try:
        method = Prompt.ask(
            "Build your dietary profile now",
            choices=["type", "voice", "later"],
            default="type",
        )
        if method == "later":
            return readiness, "deferred"
        _run_onboarding_handoff(voice=method == "voice")
    except KeyboardInterrupt:
        main.stderr_console.print(
            "[yellow]Onboarding canceled. Your account remains connected. "
            "Resume with `heyfood onboard`.[/yellow]"
        )
        return readiness, "deferred"
    except typer.Exit:
        main.stderr_console.print(
            "[yellow]Your account remains connected. Resume with `heyfood onboard`.[/yellow]"
        )
        return readiness, "deferred"
    refreshed = _profile_readiness(capability_available=True)
    return refreshed, "completed" if refreshed.get("profile_status") == "ready" else "unknown"


def _registration_document(
    readiness: dict[str, Any],
) -> dict[str, Any]:
    profile_status = str(readiness.get("profile_status") or "unknown")
    return {
        "schema_version": 1,
        "authenticated": True,
        "account_outcome": None,
        "profile_status": profile_status,
        "next_command": "heyfood chat" if profile_status == "ready" else "heyfood onboard",
    }


def _register_error_message(exc: BaseException) -> str:
    message = str(exc).replace("heyfood login", "heyfood register")
    if "heyfood register" not in message:
        message = f"heyfood register failed: {message}"
    return message


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
    api_url, auth_url = _auth_urls(
        store,
        api_url=api_url,
        auth_url=auth_url,
        local=local,
    )
    banner.controller.loading(main.stderr_console)
    main.stderr_console.print("[bold]Opening hello.food login...[/bold]")
    try:
        _authenticate(
            # Public login is deliberately account-neutral: a new user who
            # arrived here by mistake must still be offered Create account.
            # Both loopback and device transports serialize this as `auto`.
            intent="auto",
            store=store,
            api_url=api_url,
            auth_url=auth_url,
            api_key=api_key,
            device=device,
            no_browser=no_browser,
            timeout=timeout,
        )
    except (LoginInterrupted, KeyboardInterrupt) as exc:
        # Deliberate Ctrl-C: exit calmly, no traceback. The loopback flow raises a
        # bare KeyboardInterrupt; the device flow wraps it as LoginInterrupted.
        from ..auth import DEVICE_LOGIN_INTERRUPTED_MESSAGE

        message = str(exc) if isinstance(exc, LoginInterrupted) else DEVICE_LOGIN_INTERRUPTED_MESSAGE
        main.stderr_console.print(f"[yellow]{message}[/yellow]")
        raise typer.Exit(130) from None
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
def register(
    api_url: Optional[str] = typer.Option(None, "--api-url", help="Override the active context API URL."),
    auth_url: Optional[str] = typer.Option(None, "--auth-url", help="Override the active context auth URL."),
    api_key: str = typer.Option(DEFAULT_API_KEY, "--api-key", help="API key for local first-party session refresh."),
    local: bool = typer.Option(False, "--local", help="Use the local Docker dev URLs."),
    device: bool = typer.Option(False, "--device", help="Use a short code for SSH/headless systems."),
    no_browser: bool = typer.Option(False, "--no-browser", help="Print the registration URL instead of opening a browser."),
    timeout: int = typer.Option(180, "--timeout", min=1, max=900, help="Seconds to wait for browser authorization."),
    no_onboard: bool = typer.Option(False, "--no-onboard", help="Do not offer dietary profile onboarding after registration."),
    json_output: bool = typer.Option(False, "--json", help="Print one stable JSON result; never open a browser or prompt."),
) -> None:
    """Create or connect a hello.food account, then offer onboarding."""
    json_mode = json_output is True
    try:
        store = ConfigStore()
        resolved_api, resolved_auth = _auth_urls(
            store,
            api_url=api_url,
            auth_url=auth_url,
            local=local,
        )
    except Exception as exc:
        _fail(
            _register_error_message(exc),
            kind="invalid_configuration",
            json_mode=json_mode,
            exit_code=2,
        )

    banner.controller.loading(main.stderr_console, json_mode=json_mode)
    if not json_mode:
        main.stderr_console.print("[bold]Checking hello.food registration...[/bold]")
    try:
        auth_result = _authenticate(
            intent="register",
            store=store,
            api_url=resolved_api,
            auth_url=resolved_auth,
            api_key=api_key,
            device=device,
            no_browser=no_browser,
            timeout=timeout,
            json_mode=json_mode,
        )
    except (LoginInterrupted, KeyboardInterrupt) as exc:
        from ..auth import DEVICE_LOGIN_INTERRUPTED_MESSAGE

        message = str(exc) if isinstance(exc, LoginInterrupted) else DEVICE_LOGIN_INTERRUPTED_MESSAGE
        message = _register_error_message(RuntimeError(message))
        _fail(message, kind="registration_interrupted", json_mode=json_mode, exit_code=130)
    except auth_application.AuthApplicationError as exc:
        _fail(
            _register_error_message(exc),
            kind=exc.kind,
            json_mode=json_mode,
            hint=(
                exc.hint
                if exc.hint is None or "heyfood register" in exc.hint
                else f"Retry `heyfood register`. {exc.hint}"
            ),
        )
    except Exception as exc:
        _fail(_register_error_message(exc), kind="registration_failed", json_mode=json_mode)

    capabilities = auth_result.capabilities
    readiness = _profile_readiness(
        capability_available=bool(capabilities and capabilities.profile_readiness)
    )
    offer_onboarding = (
        not no_onboard
        and not json_mode
        and main._interactive_terminal()
    )
    readiness, _ = _complete_first_profile(
        readiness,
        offer_onboarding=offer_onboarding,
    )
    document = _registration_document(readiness)
    if _write_result(document, json_mode=json_mode):
        return

    first_name = _first_name_from_account(store)
    if first_name:
        main.console.print(f"[green]Connected. Welcome, {first_name}.[/green]")
    else:
        main.console.print("[green]Connected to hello.food.[/green]")
    profile_status = document["profile_status"]
    if profile_status == "ready":
        main.console.print("[green]Your dietary profile is ready.[/green]")
    elif profile_status == "missing":
        main.console.print("[yellow]Finish anytime with `heyfood onboard`.[/yellow]")
    else:
        main.console.print(
            "[yellow]Your login succeeded, but profile readiness could not be confirmed. "
            "Retry with `heyfood profile` or continue later with `heyfood onboard`.[/yellow]"
        )


def run_bare_first_run() -> None:
    """Own the interactive bare-command journey; never used for a pipe."""
    store = ConfigStore()
    configured = store.load()
    render.intro(main.console)

    readiness: dict[str, Any] | None = None
    if isinstance(configured.get("oauth"), dict):
        try:
            readiness = main.HelloFoodClient(create_device=False).profile_readiness()
        except LoginRequired:
            readiness = None
        except HelloFoodError:
            readiness = {
                "profile_status": "unknown",
                "has_profile_sync_consent": None,
                "profile_version": None,
            }

    if readiness is None:
        has_prior_account = any(
            configured.get(key) for key in ("account_user_id", "session", "oauth")
        )
        default = "login" if has_prior_account else "register"
        main.console.print(
            "[bold]Get started[/bold]\n"
            "  [green]register[/green]  Create a hello.food account\n"
            "  [green]login[/green]     Connect an existing account"
        )
        intent = Prompt.ask(
            "Choose",
            choices=["register", "login"],
            default=default,
            show_choices=False,
        )
        api_url, auth_url = _auth_urls(
            store,
            api_url=None,
            auth_url=None,
            local=False,
        )
        try:
            auth_intent: auth_application.AuthIntent = (
                "auto" if intent == "login" else "register"
            )
            auth_result = _authenticate(
                intent=auth_intent,
                store=store,
                api_url=api_url,
                auth_url=auth_url,
                api_key=DEFAULT_API_KEY,
                device=False,
                no_browser=False,
                timeout=180,
            )
        except (LoginInterrupted, KeyboardInterrupt) as exc:
            from ..auth import DEVICE_LOGIN_INTERRUPTED_MESSAGE

            message = (
                str(exc)
                if isinstance(exc, LoginInterrupted)
                else DEVICE_LOGIN_INTERRUPTED_MESSAGE
            )
            main.stderr_console.print(f"[yellow]{message}[/yellow]")
            raise typer.Exit(130) from None
        except Exception as exc:
            main.stderr_console.print(f"[red]heyfood error:[/red] {exc}")
            main.stderr_console.print(
                f"Retry with [bold]heyfood {intent}[/bold]."
            )
            raise typer.Exit(1) from exc
        capability_available = (
            auth_result.capabilities.profile_readiness
            if auth_result.capabilities is not None
            else True
        )
        readiness = _profile_readiness(
            capability_available=capability_available
        )

    if readiness.get("profile_status") == "unknown":
        main.stderr_console.print(
            "[yellow]You are connected, but hello.food could not confirm your profile readiness. "
            "Nothing was changed. Retry bare `heyfood`, or run `heyfood status`.[/yellow]"
        )
        return

    readiness, _ = _complete_first_profile(readiness, offer_onboarding=True)
    if readiness.get("profile_status") == "unknown":
        main.stderr_console.print(
            "[yellow]You are connected. Resume profile setup with `heyfood onboard`.[/yellow]"
        )
        return

    from .agent import chat

    chat(
        initial=None,
        new=False,
        no_input=False,
        lat=None,
        lng=None,
        near=None,
        no_location=False,
        checking_for=None,
        json_output=False,
        raw=False,
    )


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
