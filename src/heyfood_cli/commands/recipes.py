"""Recipe discovery commands."""
from __future__ import annotations

from .. import main
from ..main import (
    ChannelToolUnavailable,
    HelloFoodError,
    LoginRequired,
    Optional,
    _fail,
    _json_mode,
    _raise_command_error,
    _validated,
    _write_result,
    is_local_api_url,
    recipes_app,
    render,
    typer,
    validation,
)

@recipes_app.command("search")
def recipes_search(
    query: list[str] = typer.Argument(..., help="Recipe search text."),
    cuisine: Optional[str] = typer.Option(None, "--cuisine", "-c", help="Cuisine filter."),
    meal_type: Optional[str] = typer.Option(None, "--type", "-t", help="Override inferred meal type: breakfast, lunch, dinner, snack, or dessert."),
    max_ready_time: Optional[int] = typer.Option(None, "--max-ready-time", help="Maximum ready time in minutes."),
    limit: int = typer.Option(5, "--limit", "-n", help="Maximum recipes to show."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Search real recipes matched to your hello.food profile."""
    json_mode = _json_mode(json_output, raw)
    text = _validated(
        lambda: validation.required_text(
            " ".join(query),
            label="Recipe query",
            max_length=200,
        )
    )
    cuisine = _validated(
        lambda: validation.optional_text(cuisine, label="Cuisine", max_length=80)
    )
    meal_type = _validated(
        lambda: validation.choice(
            meal_type,
            label="Meal type",
            choices={"breakfast", "lunch", "dinner", "snack", "dessert"},
        )
    )
    if max_ready_time is not None:
        max_ready_time = _validated(
            lambda: validation.bounded_integer(
                max_ready_time,
                label="Maximum ready time",
                minimum=1,
            )
        )
    limit = _validated(
        lambda: validation.bounded_integer(
            limit,
            label="Limit",
            minimum=1,
            maximum=20,
        )
    )
    payload: dict = {
        "query": text,
        "limit": limit,
    }
    if cuisine:
        payload["cuisine"] = cuisine
    if meal_type:
        payload["meal_type"] = meal_type
    if max_ready_time is not None:
        payload["max_ready_time"] = max_ready_time

    client = main.HelloFoodClient()
    try:
        data = client.channel_tool("search_recipes", payload)
    except LoginRequired as exc:
        _raise_command_error(exc, json_mode=json_mode)
    except ChannelToolUnavailable as exc:
        _raise_command_error(exc, json_mode=json_mode)
    except HelloFoodError as exc:
        message = str(exc)
        if _is_recipe_provider_unavailable(message):
            hint = None
            if is_local_api_url(client.api_url):
                hint = "The configured local recipe provider is unavailable."
            _fail(
                "Recipe search is temporarily unavailable.",
                kind="recipe_provider_unavailable",
                json_mode=json_mode,
                hint=hint or message,
            )
        _raise_command_error(exc, json_mode=json_mode)

    client.remember_recipe_search(data)
    if _write_result(data, json_mode=json_mode):
        return
    render.recipe_search(main.console, data)


def _is_recipe_provider_unavailable(message: str) -> bool:
    normalized = message.lower()
    return (
        normalized.startswith("503:")
        or "recipe service temporarily unavailable" in normalized
        or "recipe provider unavailable" in normalized
    )


@recipes_app.command("save")
def recipes_save(
    selector: str = typer.Argument(..., help="Recipe ref, Spoonacular id, or index from the last recipe search."),
    notes: Optional[str] = typer.Option(None, "--notes", help="Optional cookbook note."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Save a recipe to your hello.food cookbook."""
    json_mode = _json_mode(json_output, raw)
    selector = _validated(
        lambda: validation.required_text(selector, label="Recipe selector", max_length=300)
    )
    notes = _validated(
        lambda: validation.optional_text(notes, label="Notes", max_length=1000)
    )
    client = main.HelloFoodClient()
    try:
        payload = client.recipe_save_payload(selector)
        if notes:
            payload["notes"] = notes
        data = client.channel_tool("save_recipe", payload)
    except ChannelToolUnavailable as exc:
        _raise_command_error(exc, json_mode=json_mode)
    except (LoginRequired, HelloFoodError) as exc:
        _raise_command_error(exc, json_mode=json_mode)

    if _write_result(data, json_mode=json_mode):
        return
    render.saved_recipe_saved(main.console, data)


@recipes_app.command("saved")
def recipes_saved(
    limit: int = typer.Option(20, "--limit", "-n", help="Maximum saved recipes to show."),
    json_output: bool = typer.Option(False, "--json", help="Print stable JSON."),
    raw: bool = typer.Option(False, "--raw", help="Deprecated alias for --json."),
) -> None:
    """Show your saved recipe cookbook."""
    json_mode = _json_mode(json_output, raw)
    limit = _validated(
        lambda: validation.bounded_integer(
            limit,
            label="Limit",
            minimum=1,
            maximum=100,
        )
    )
    client = main.HelloFoodClient()
    try:
        data = client.channel_tool("list_saved_recipes", {"limit": limit})
    except ChannelToolUnavailable as exc:
        _raise_command_error(exc, json_mode=json_mode)
    except (LoginRequired, HelloFoodError) as exc:
        _raise_command_error(exc, json_mode=json_mode)

    if _write_result(data, json_mode=json_mode):
        return
    render.saved_recipes(main.console, data)
