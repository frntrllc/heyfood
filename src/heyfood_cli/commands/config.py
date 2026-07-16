"""Context and local-configuration commands."""
from __future__ import annotations

from .. import main
from ..main import (
    BUILTIN_CONTEXTS,
    ConfigError,
    ConfigStore,
    Optional,
    Table,
    _fail,
    _write_result,
    config_app,
    configured_config_path,
    configured_contexts,
    context_app,
    output,
    redacted_config,
    resolve_service_urls,
    typer,
)

def _validated_service_url(value: str, *, label: str) -> str:
    from ..config import ConfigError, validate_service_url

    try:
        validated = validate_service_url(value, field=label)
    except ConfigError as exc:
        raise typer.BadParameter(str(exc)) from exc
    return validated.rstrip("/")


@context_app.command("list")
def context_list(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
) -> None:
    store = ConfigStore(configured_config_path())
    data = store.load()
    contexts = configured_contexts(data)
    _, _, active = resolve_service_urls(data)
    document = {
        "ok": True,
        "active": active,
        "contexts": [
            {"name": name, "active": name == active, **values}
            for name, values in sorted(contexts.items())
        ],
    }
    if _write_result(document, json_mode=json_output):
        return
    table = Table(title="heyfood contexts")
    table.add_column("Active")
    table.add_column("Name")
    table.add_column("API URL")
    table.add_column("Auth URL")
    for item in document["contexts"]:
        table.add_row("*" if item["active"] else "", item["name"], item["api_url"], item["auth_url"])
    main.console.print(table)


@context_app.command("show")
def context_show(
    name: Optional[str] = typer.Argument(None, help="Context name; defaults to active."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
) -> None:
    data = ConfigStore(configured_config_path()).load()
    contexts = configured_contexts(data)
    _, _, active = resolve_service_urls(data)
    selected = name or active
    if selected not in contexts:
        raise typer.BadParameter(f"Unknown context '{selected}'. Run `heyfood context list`.")
    document = {"ok": True, "name": selected, "active": selected == active, **contexts[selected]}
    if _write_result(document, json_mode=json_output):
        return
    main.console.print(f"[bold]{selected}[/bold]{' [green](active)[/green]' if selected == active else ''}")
    main.console.print(f"API:  {document['api_url']}")
    main.console.print(f"Auth: {document['auth_url']}")


@context_app.command("use")
def context_use(name: str = typer.Argument(..., help="Context name.")) -> None:
    store = ConfigStore()
    data = store.load()
    contexts = configured_contexts(data)
    if name not in contexts:
        raise typer.BadParameter(f"Unknown context '{name}'. Run `heyfood context list`.")
    data["active_context"] = name
    # Remove the legacy single-environment override so the selected context
    # becomes authoritative. Environment variables still take precedence.
    data.pop("api_url", None)
    data.pop("auth_url", None)
    store.save(data)
    main.console.print(f"[green]Using context '{name}'.[/green]")


@context_app.command("set")
def context_set(
    name: str = typer.Argument(..., help="New custom context name."),
    api_url: str = typer.Option(..., "--api-url", help="API base URL."),
    auth_url: str = typer.Option(..., "--auth-url", help="Auth authorize URL."),
    use: bool = typer.Option(False, "--use", help="Make this context active."),
) -> None:
    normalized_name = name.strip().lower()
    if not normalized_name or normalized_name in BUILTIN_CONTEXTS:
        raise typer.BadParameter("Choose a non-empty custom name other than production or local.")
    store = ConfigStore()
    data = store.load()
    contexts = data.setdefault("contexts", {})
    if not isinstance(contexts, dict):
        contexts = {}
        data["contexts"] = contexts
    contexts[normalized_name] = {
        "api_url": _validated_service_url(api_url, label="API URL"),
        "auth_url": _validated_service_url(auth_url, label="Auth URL"),
    }
    if use:
        data["active_context"] = normalized_name
        data.pop("api_url", None)
        data.pop("auth_url", None)
    store.save(data)
    main.console.print(f"[green]Saved context '{normalized_name}'.[/green]")


@config_app.command("path")
def config_path(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
) -> None:
    path = configured_config_path()
    if _write_result({"ok": True, "path": str(path)}, json_mode=json_output):
        return
    main.console.print(str(path))


@config_app.command("show")
def config_show(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
) -> None:
    store = ConfigStore(configured_config_path())
    try:
        data = store.load()
    except ConfigError as exc:
        _fail(str(exc), kind="invalid_config", json_mode=json_output, exit_code=2)
    effective_api, effective_auth, active = resolve_service_urls(data)
    document = {
        "ok": True,
        "path": str(store.path),
        "active_context": active,
        "effective": {"api_url": effective_api, "auth_url": effective_auth},
        "config": redacted_config(data),
    }
    if _write_result(document, json_mode=json_output):
        return
    output.write_json(document)


@config_app.command("validate")
def config_validate(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
) -> None:
    store = ConfigStore(configured_config_path())
    try:
        store.load()
    except ConfigError as exc:
        document = {
            "ok": False,
            "path": str(store.path),
            "error": {"type": "invalid_config", "message": str(exc)},
            "repair": "Move the invalid file aside, then rerun `heyfood login`.",
        }
        if json_output:
            output.write_json(document)
        else:
            main.stderr_console.print(f"[red]{exc}[/red]")
            main.stderr_console.print(
                f"Move it aside with: mv '{store.path}' '{store.path}.invalid'"
            )
        raise typer.Exit(2)
    document = {"ok": True, "path": str(store.path), "valid": True}
    if _write_result(document, json_mode=json_output):
        return
    main.console.print("[green]Configuration is valid.[/green]")
