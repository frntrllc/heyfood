"""Restaurant search, menu, recommendation, and location commands."""
from __future__ import annotations

from .. import main
from ..main import (
    Any,
    ChannelToolUnavailable,
    HelloFoodError,
    LoginRequired,
    MENU_POLL_INTERVAL_SECONDS,
    MENU_POLL_TIMEOUT_SECONDS,
    MENU_POLL_WARNING_SECONDS,
    Optional,
    _fail,
    _geocode_place,
    _json_mode,
    _raise_command_error,
    _validated,
    _write_result,
    app,
    banner,
    location_app,
    nullcontext,
    output,
    render,
    time,
    typer,
    validation,
)

def resolve_location(
    client: main.HelloFoodClient,
    *,
    lat: Optional[float],
    lng: Optional[float],
    near: Optional[str],
    radius: Optional[float],
    json_mode: bool = False,
) -> tuple[float, float, float]:
    """Resolve search coordinates + radius by precedence: explicit > --near > saved."""
    lat, lng = _validated(lambda: validation.coordinates(lat, lng))
    validated_radius = (
        None
        if radius is None
        else _validated(
            lambda: validation.bounded_number(
                radius,
                label="Radius",
                minimum=0.1,
                maximum=50.0,
            )
        )
    )
    if lat is not None and lng is not None:
        return lat, lng, validated_radius if validated_radius is not None else 5.0
    if near:
        near = _validated(
            lambda: validation.required_text(near, label="Near", max_length=200)
        )
        resolved_lat, resolved_lng, resolved_label = _geocode_place(
            client,
            near,
            json_mode=json_mode,
        )
        if not json_mode:
            main.stderr_console.print(f"[dim]Searching near {resolved_label}[/dim]")
        return (
            resolved_lat,
            resolved_lng,
            validated_radius if validated_radius is not None else 5.0,
        )
    saved = client.saved_location()
    if saved:
        saved_radius = saved.get("radius_miles")
        saved_lat, saved_lng = _validated(
            lambda: validation.coordinates(
                saved.get("latitude"),
                saved.get("longitude"),
                required=True,
            )
        )
        effective = validated_radius
        if effective is None:
            effective = _validated(
                lambda: validation.bounded_number(
                    float(saved_radius) if isinstance(saved_radius, (int, float)) else 5.0,
                    label="Saved radius",
                    minimum=0.1,
                    maximum=50.0,
                )
            )
        return float(saved_lat), float(saved_lng), effective
    raise typer.BadParameter(
        'No location. Pass --lat/--lng, --near "City, ST", or run `heyfood location set`.'
    )


@app.command()
def search(
    query: Optional[str] = typer.Option(None, "--query", "-q", help="Restaurant search text."),
    lat: Optional[float] = typer.Option(None, "--lat", help="Latitude (with --lng). Overrides saved location."),
    lng: Optional[float] = typer.Option(None, "--lng", help="Longitude (with --lat). Overrides saved location."),
    near: Optional[str] = typer.Option(None, "--near", help='Place name to search near, e.g. "Fresno, CA".'),
    radius: Optional[float] = typer.Option(None, "--radius", help="Search radius in miles (default 5, or the saved location's radius)."),
    limit: int = typer.Option(10, "--limit", help="Maximum restaurants."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Search restaurants near coordinates, a saved default location, or a named place."""
    json_mode = _json_mode(json_output, raw)
    query = _validated(
        lambda: validation.optional_text(query, label="Query", max_length=200)
    )
    limit = _validated(
        lambda: validation.bounded_integer(
            limit,
            label="Limit",
            minimum=1,
            maximum=50,
        )
    )
    lat, lng = _validated(lambda: validation.coordinates(lat, lng))
    if radius is not None:
        radius = _validated(
            lambda: validation.bounded_number(
                radius,
                label="Radius",
                minimum=0.1,
                maximum=50.0,
            )
        )
    client = main.HelloFoodClient()
    resolved_lat, resolved_lng, resolved_radius = resolve_location(
        client, lat=lat, lng=lng, near=near, radius=radius, json_mode=json_mode
    )
    try:
        data = client.channel_tool(
            "search_restaurant",
            {
                "latitude": resolved_lat,
                "longitude": resolved_lng,
                "radius_miles": resolved_radius,
                "limit": limit,
                "query": query,
            },
        )
    except (LoginRequired, HelloFoodError) as exc:
        _raise_command_error(exc, json_mode=json_mode)
    client.remember_restaurant_search(data)
    if _write_result(data, json_mode=json_mode):
        return
    render.restaurants(main.console, data)


def _show_location(*, json_mode: bool = False) -> None:
    client = main.HelloFoodClient()
    location = client.saved_location()
    if _write_result({"location": location}, json_mode=json_mode):
        return
    render.location(main.console, location)


@location_app.callback(invoke_without_command=True)
def location_callback(
    ctx: typer.Context,
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Show the saved location when run with no subcommand."""
    if ctx.invoked_subcommand is None:
        _show_location(json_mode=_json_mode(json_output, raw))


@location_app.command("show")
def location_show(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Show the saved default location."""
    _show_location(json_mode=_json_mode(json_output, raw))


@location_app.command("set")
def location_set(
    place: Optional[str] = typer.Argument(None, help='Place name to geocode, e.g. "San Luis Obispo, CA".'),
    lat: Optional[float] = typer.Option(None, "--lat", help="Latitude (with --lng; skips geocoding)."),
    lng: Optional[float] = typer.Option(None, "--lng", help="Longitude (with --lat; skips geocoding)."),
    label: Optional[str] = typer.Option(None, "--label", help="Label for a coordinate location."),
    radius: float = typer.Option(5.0, "--radius", help="Default search radius in miles."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Save a default location by place name, or directly with --lat/--lng."""
    json_mode = _json_mode(json_output, raw)
    place = _validated(
        lambda: validation.optional_text(place, label="Place", max_length=200)
    )
    label = _validated(
        lambda: validation.optional_text(label, label="Label", max_length=100)
    )
    lat, lng = _validated(lambda: validation.coordinates(lat, lng))
    radius = _validated(
        lambda: validation.bounded_number(
            radius,
            label="Radius",
            minimum=0.1,
            maximum=50.0,
        )
    )
    if lat is not None and lng is not None:
        if place:
            raise typer.BadParameter("Pass either a place name or --lat/--lng, not both.")
        client = main.HelloFoodClient()
        resolved_label = label or f"{lat:.4f}, {lng:.4f}"
        client.save_location(label=resolved_label, latitude=lat, longitude=lng, radius_miles=radius)
        saved = client.saved_location()
        if _write_result({"location": saved}, json_mode=json_mode):
            return
        render.location(main.console, saved)
        return
    if not place:
        raise typer.BadParameter("Provide a place name, or --lat and --lng.")
    client = main.HelloFoodClient()
    resolved_lat, resolved_lng, resolved_label = _geocode_place(
        client,
        place,
        json_mode=json_mode,
    )
    # Always echo Google's formatted_address so an ambiguous input (e.g.
    # "Springfield" landing in the wrong state) is visible before it sticks.
    if not json_mode:
        main.stderr_console.print(f"[green]Resolved to[/green] [bold]{resolved_label}[/bold]")
    client.save_location(
        label=label or resolved_label,
        latitude=resolved_lat,
        longitude=resolved_lng,
        radius_miles=radius,
    )
    saved = client.saved_location()
    if _write_result({"location": saved}, json_mode=json_mode):
        return
    render.location(main.console, saved)


@location_app.command("clear")
def location_clear(
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Forget the saved default location."""
    json_mode = _json_mode(json_output, raw)
    client = main.HelloFoodClient()
    cleared = client.clear_location()
    if _write_result({"cleared": cleared}, json_mode=json_mode):
        return
    if cleared:
        main.console.print("[green]Saved location cleared.[/green]")
    else:
        main.console.print("[dim]No saved location to clear.[/dim]")


@app.command()
def menu(
    restaurant_id: str = typer.Argument(..., help="HelloFood restaurant id or index from the last search."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Browse a full restaurant menu."""
    json_mode = _json_mode(json_output, raw)
    restaurant_id = _validated(
        lambda: validation.required_text(
            restaurant_id,
            label="Restaurant id",
            max_length=255,
        )
    )
    client = main.HelloFoodClient()
    try:
        resolved_restaurant_id = client.restaurant_id_from_selector(restaurant_id)
        data = client.channel_tool(
            "get_menu",
            {"restaurant_id": resolved_restaurant_id},
        )
        data = main._poll_menu_until_terminal(
            client,
            resolved_restaurant_id,
            data,
            show_progress=not json_mode,
            json_mode=json_mode,
        )
    except ChannelToolUnavailable as exc:
        _raise_command_error(exc, json_mode=json_mode)
    except (LoginRequired, HelloFoodError) as exc:
        _raise_command_error(exc, json_mode=json_mode)
    status = str(data.get("status") or "ready")
    if json_mode:
        output.write_json(data)
        if status != "ready":
            raise typer.Exit(1)
        return
    if status == "ready":
        render.menu(main.console, data)
    elif status == "unavailable":
        main.stderr_console.print(f"[yellow]{data.get('message') or 'No public menu is available for this restaurant.'}[/yellow]")
        raise typer.Exit(1)
    elif status == "rate_limited":
        retry_after = data.get("retry_after_seconds")
        hint = f" Try again in about {int(retry_after)} seconds." if retry_after else " Try again later."
        main.stderr_console.print(f"[yellow]Menu fetch limit reached.{hint}[/yellow]")
        raise typer.Exit(1)
    elif status == "timed_out":
        job_id = data.get("job_id")
        main.stderr_console.print(
            f"[yellow]{data.get('message') or 'Menu is still being fetched after 30 seconds.'}[/yellow]"
        )
        if job_id:
            main.stderr_console.print(
                f"[dim]Job {job_id}. Rerun: heyfood menu {resolved_restaurant_id}[/dim]"
            )
        raise typer.Exit(1)
    else:
        main.stderr_console.print(f"[red]{data.get('reason') or data.get('message') or 'Menu fetching could not be completed.'}[/red]")
        raise typer.Exit(1)


def _poll_menu_until_terminal(
    client: main.HelloFoodClient,
    restaurant_id: str,
    initial: dict[str, Any],
    *,
    show_progress: bool = True,
    json_mode: bool = False,
) -> dict[str, Any]:
    """Poll an acquire-on-miss job until the server reports a terminal state."""
    if str(initial.get("status") or "ready") != "acquiring":
        return initial
    job_id = initial.get("job_id")
    if not isinstance(job_id, str) or not job_id:
        return {
            **initial,
            "status": "failed",
            "message": "The server did not return a menu job id.",
        }

    started_at = time.monotonic()
    deadline = started_at + MENU_POLL_TIMEOUT_SECONDS
    latest = initial
    banner.controller.loading(main.stderr_console, json_mode=json_mode)
    warned = False
    status_context = (
        main.stderr_console.status("[dim]Fetching and checking the menu…[/dim]")
        if show_progress
        else nullcontext(None)
    )
    with status_context as status:
        while str(latest.get("status") or "") == "acquiring":
            now = time.monotonic()
            remaining = deadline - now
            if remaining <= 0:
                return {
                    **latest,
                    "status": "timed_out",
                    "message": "Menu is still being fetched after 30 seconds.",
                    "poll_timeout_seconds": MENU_POLL_TIMEOUT_SECONDS,
                }
            if status is not None and not warned and now - started_at >= MENU_POLL_WARNING_SECONDS:
                warned = True
                status.update(
                    "[yellow]Still fetching the menu — returning control by 30 seconds…[/yellow]"
                )
            time.sleep(min(MENU_POLL_INTERVAL_SECONDS, remaining))
            try:
                latest = client.get_menu_status(
                    restaurant_id=restaurant_id,
                    job_id=job_id,
                )
            except ChannelToolUnavailable:
                return {
                    **latest,
                    "status": "timed_out",
                    "message": (
                        "This API does not expose menu-status polling yet. "
                        "Retry the menu command with the same restaurant id."
                    ),
                    "polling_supported": False,
                    "poll_timeout_seconds": 0,
                }
    return latest


@app.command("get-menu")
def get_menu_command(
    restaurant_id: str = typer.Argument(
        ...,
        help="HelloFood restaurant id or index from the last search.",
    ),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Fetch or browse a full restaurant menu (alias for `heyfood menu`)."""
    menu(restaurant_id, json_output=json_output, raw=raw)


@app.command()
def recommend(
    restaurant_id: str = typer.Argument(..., help="HelloFood restaurant id or index from the last search."),
    query: Optional[str] = typer.Option(None, "--query", "-q", help="What you are in the mood for."),
    limit: int = typer.Option(5, "--limit", help="Maximum recommendations."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Recommend menu items for your profile."""
    json_mode = _json_mode(json_output, raw)
    restaurant_id = _validated(
        lambda: validation.required_text(
            restaurant_id,
            label="Restaurant id",
            max_length=255,
        )
    )
    query = _validated(
        lambda: validation.optional_text(query, label="Query", max_length=200)
    )
    limit = _validated(
        lambda: validation.bounded_integer(
            limit,
            label="Limit",
            minimum=1,
            maximum=20,
        )
    )
    client = main.HelloFoodClient()
    try:
        selected = client.restaurant_from_selector(restaurant_id)
        if selected and selected.get("has_menu") is False:
            name = selected.get("name") or "that restaurant"
            _fail(
                f"No menu is available for {name} yet.",
                kind="menu_unavailable",
                json_mode=json_mode,
                hint="Try another result with a menu or run `heyfood item` for a specific food.",
            )
        data = client.channel_tool(
            "recommend_items",
            {
                "restaurant_id": client.restaurant_id_from_selector(restaurant_id),
                "query": query,
                "limit": limit,
            },
        )
    except ChannelToolUnavailable as exc:
        _raise_command_error(exc, json_mode=json_mode)
    except (LoginRequired, HelloFoodError) as exc:
        _raise_command_error(exc, json_mode=json_mode)
    if _write_result(data, json_mode=json_mode):
        return
    render.recommendations(main.console, data)
