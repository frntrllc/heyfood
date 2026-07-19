from io import StringIO

import pytest
from rich.console import Console

from heyfood_cli import presentation, render
from heyfood_cli.theme import HEYFOOD_THEME


def _console() -> tuple[Console, StringIO]:
    output = StringIO()
    return Console(
        file=output,
        force_terminal=False,
        width=120,
        theme=HEYFOOD_THEME,
    ), output


def _terminal_console() -> tuple[Console, StringIO]:
    output = StringIO()
    return Console(
        file=output,
        force_terminal=True,
        color_system="truecolor",
        width=120,
        theme=HEYFOOD_THEME,
    ), output


def test_chat_chrome_matches_landing_page_hierarchy(monkeypatch):
    monkeypatch.delenv("NO_COLOR", raising=False)
    monkeypatch.setenv("TERM", "xterm-256color")
    console, output = _terminal_console()

    render.chat_header(console)
    render.chat_turn_gap(console)
    render.chat_user(console, "I ate a hamburger for lunch")
    render.agent_message(console, "I can help you log that.")

    rendered = output.getvalue()
    assert "hello.food chat" in rendered
    assert "you" in rendered
    assert "I ate a hamburger for lunch" in rendered
    assert "I can help you log that." in rendered
    assert "38;2;155;197;61m" in rendered
    assert "38;2;237;234;224m" in rendered
    assert "\n\n" in rendered


def test_explicit_agent_failure_uses_failure_contract_not_prose_matching(monkeypatch):
    monkeypatch.delenv("NO_COLOR", raising=False)
    monkeypatch.setenv("TERM", "xterm-256color")
    console, output = _terminal_console()
    failed = {
        "ok": False,
        "error": {
            "message": "Request could not be completed.",
            "hint": "Try again in a moment.",
        },
        "conversation_id": "conv-error",
    }

    assert render.agent_result_is_error(failed) is True
    assert render.agent_result_is_error({"message": "Sorry about the delay."}) is False
    render.agent_result(console, failed)

    rendered = output.getvalue()
    assert "Request could not be completed." in rendered
    assert "Try again in a moment." in rendered
    assert "38;2;241;124;117m" in rendered


def test_action_confirmation_renders_meal_card():
    console, output = _console()

    handled = render.agent_structured(
        console,
        {
            "type": "action_confirmation",
            "confirmation_id": "confirm-1",
            "idempotency_key": "idem-1",
            "action": "log_meal",
            "preview": "Log: two eggs and a cup of coffee for breakfast",
            "expires_at": "2026-06-29T16:14:19Z",
            "structured_preview": {
                "meal_name": "two eggs and a cup of coffee",
                "meal_type": "breakfast",
                "logged_at": "2026-06-29T16:09:19Z",
            },
        },
    )

    text = output.getvalue()
    assert handled is True
    assert "Log this meal" in text
    assert "two eggs and a cup of coffee" in text
    assert "Breakfast" in text
    assert "confirmed" in text


def test_general_response_renders_meal_nutrition_without_json():
    console, output = _console()

    handled = render.agent_structured(
        console,
        {
            "type": "general_response",
            "meal_nutrition": {
                "items": [
                    {
                        "name": "two eggs and a cup of coffee",
                        "portion": "1",
                        "calories": 155,
                        "protein_g": 13,
                        "carbs_g": 1,
                        "fat_g": 11,
                    }
                ],
                "totals": {
                    "calories": 155,
                    "protein_g": 13,
                    "carbs_g": 1,
                    "fat_g": 11,
                },
                "enrichment_status": "complete",
            },
        },
    )

    text = output.getvalue()
    assert handled is True
    assert "Nutrition estimate" in text
    assert "two eggs and a cup of coffee" in text
    assert "Total" in text
    assert "155" in text
    assert '"meal_nutrition"' not in text


def test_recipe_search_renders_refs_and_match():
    console, output = _console()

    render.recipe_search(
        console,
        {
            "query_used": "Mediterranean dinner",
            "personalized": True,
            "message": "Showing results for 'Mediterranean dinner'.",
            "recipes": [
                {
                    "title": "Greek Chicken Bowl",
                    "ready_in_minutes": 35,
                    "calories_per_serving": 420,
                    "dietary_match_hint": 0.95,
                    "dietary_tags": ["gluten_free", "dairy_free"],
                    "recipe_ref": {
                        "provider": "spoonacular",
                        "external_id": "12345",
                    },
                }
            ],
        },
    )

    text = output.getvalue()
    assert "Mediterranean dinner" in text
    assert "Greek Chicken Bowl" in text
    assert "35 min" in text
    assert "420 cal" in text
    assert "95%" in text
    assert "spoonacular:12345" in text
    assert "Showing results" in text


def test_restaurants_renders_selection_indices():
    console, output = _console()

    render.restaurants(
        console,
        {
            "total_count": 1,
            "restaurants": [
                {
                    "id": "rest-1",
                    "name": "Thai Phuket",
                    "distance_miles": 2.2,
                    "has_menu": False,
                    "fit_status": "caution",
                }
            ],
        },
    )

    text = output.getvalue()
    assert "Restaurants" in text
    assert "Thai Phuket" in text
    assert "rest-1" not in text
    assert "1" in text


def test_recommendations_label_match_as_ranking_and_offer_safety_check():
    console, output = _console()

    render.recommendations(
        console,
        {
            "restaurant_id": "rest-1",
            "restaurant_name": "Thai Place",
            "recommendations": [
                {
                    "item_name": "Pad Thai",
                    "score": 0.82,
                    "confidence": 0.74,
                    "rationale": "Strong preference match.",
                    "alternatives": ["Pad See Ew"],
                }
            ],
        },
    )

    text = output.getvalue()
    block_data = str(
        presentation.to_data(
            render.recommendation_blocks(
                {
                    "restaurant_id": "rest-1",
                    "restaurant_name": "Thai Place",
                    "recommendations": [
                        {
                            "item_name": "Pad Thai",
                            "score": 0.82,
                            "confidence": 0.74,
                            "rationale": "Strong preference match.",
                            "alternatives": ["Pad See Ew"],
                        }
                    ],
                }
            )
        )
    )
    assert "Ranked matches" in text
    assert "not a safety verdict" in text
    assert "82%" in text
    assert "74%" in text
    assert "heyfood item 'Pad Thai' --restaurant rest-1" in block_data
    assert "Alternatives: Pad See Ew" in block_data


@pytest.mark.parametrize(
    ("value", "expected"),
    (
        ("safe", "Generally safer"),
        ("generally_safer", "Generally safer"),
        ("caution", "Risky"),
        ("unsafe", "Avoid"),
        ("unknown", "Unable to evaluate"),
    ),
)
def test_safety_statuses_use_canonical_human_vocabulary(value, expected):
    assert render._status_label(value) == expected


def test_safety_verdict_surfaces_available_guidance_context():
    console, output = _console()

    render.safety_verdict(
        console,
        {
            "items": [
                {
                    "food_item": "Soup",
                    "status": "caution",
                    "confidence": 0.6,
                    "reason": "Broth ingredients are incomplete.",
                    "member_id": "member-2",
                    "uncertainties": ["Shared fryer is unknown"],
                    "modifications": ["Request a clean pot"],
                    "questions_to_ask": ["Is the broth gluten-free?"],
                    "alternatives": ["Steamed rice"],
                    "menu_freshness": "updated today",
                    "provenance": "restaurant menu",
                }
            ]
        },
    )

    text = output.getvalue()
    detail = render._safety_detail(
        {
            "reason": "Broth ingredients are incomplete.",
            "member_id": "member-2",
            "uncertainties": ["Shared fryer is unknown"],
            "modifications": ["Request a clean pot"],
            "questions_to_ask": ["Is the broth gluten-free?"],
            "alternatives": ["Steamed rice"],
            "menu_freshness": "updated today",
            "provenance": "restaurant menu",
        }
    )
    for expected in (
        "Risky",
    ):
        assert expected in text
    for expected in (
        "Applies to: member-2",
        "Shared fryer is unknown",
        "Request a clean pot",
        "Is the broth gluten-free?",
        "Steamed rice",
        "updated today",
        "restaurant menu",
    ):
        assert expected in detail


def test_api_text_is_rendered_literally_not_as_rich_markup():
    console, output = _console()
    injected = "[bold red]not markup[/bold red]"

    render.menu(
        console,
        {
            "restaurant_name": injected,
            "item_count": 1,
            "sections": [
                {
                    "name": injected,
                    "items": [
                        {
                            "name": injected,
                            "price_display": "$5",
                            "description": injected,
                        }
                    ],
                }
            ],
        },
    )

    text = output.getvalue()
    assert text.count(injected) >= 4


@pytest.mark.parametrize("width", [40, 44, 80])
def test_critical_guidance_renderer_is_plain_in_narrow_no_color_terminal(width):
    output = StringIO()
    console = Console(
        file=output,
        force_terminal=True,
        color_system=None,
        width=width,
    )

    render.recommendations(
        console,
        {
            "restaurant_id": "rest-1",
            "restaurant_name": "Cafe",
            "recommendations": [
                {
                    "item_name": "Rice",
                    "score": 0.8,
                    "confidence": 0.7,
                    "rationale": "Simple preparation.",
                }
            ],
        },
    )

    assert "\x1b" not in output.getvalue()
    assert "not a safety verdict" in " ".join(output.getvalue().split())


def test_saved_recipes_renders_cookbook_rows():
    console, output = _console()

    render.saved_recipes(
        console,
        {
            "total_count": 1,
            "recipes": [
                {
                    "title": "Grilled Lemon Garlic Chicken",
                    "ready_in_minutes": 45,
                    "calories_per_serving": 389,
                    "times_cooked": 2,
                    "dietary_tags": ["gluten_free", "low_fodmap"],
                    "recipe_ref": {
                        "provider": "spoonacular",
                        "external_id": "645753",
                    },
                }
            ],
        },
    )

    text = output.getvalue()
    assert "Saved Recipes" in text
    assert "Grilled Lemon Garlic Chicken" in text
    assert "45 min" in text
    assert "389 cal" in text
    assert "spoonacular:645753" in text


def test_profile_summary_renders_dietary_graph():
    console, output = _console()

    render.profile_summary(
        console,
        {
            "preferences": ["low_fodmap"],
            "restrictions": ["peanutFree"],
            "avoid_ingredients": ["onion"],
            "medical_condition_id": "ibs",
            "medical_constraints": ["high_fodmap"],
            "activity_level": "moderate",
            "cuisine_preferences": ["thai"],
            "health_condition_ids": ["ibs"],
        },
        member_id="_self",
        version=2,
    )

    text = output.getvalue()
    assert "Dietary Graph - _self v2" in text
    assert "IBS" in text
    assert "Low-FODMAP" in text
    assert "Peanut-free" in text
    assert "onion" in text
    assert "Thai" in text


def test_profile_summary_renders_sources_without_compatibility_label_duplicates():
    console, output = _console()

    render.profile_summary(
        console,
        {
            "selection_provenance_version": 1,
            "diet_style_ids": ["gluten_free", "low_sodium"],
            "allergy_ids": ["wheat"],
            "custom_diet_styles": ["Gluten-free", "Low-sodium", "Family diet"],
            "preferences": [],
            "restrictions": ["glutenFree"],
            "health_condition_ids": [],
            "severity_level": None,
        },
    )

    text = output.getvalue()
    assert "Diet styles" in text
    assert "Gluten-free, Low-sodium" in text
    assert "Allergies" in text
    assert "Wheat" in text
    assert "Family diet" in text
    assert "Severity" not in text


def test_format_datetime_treats_naive_backend_timestamp_as_utc():
    assert render._format_datetime("2026-07-01T00:49:00") == render._format_datetime(
        "2026-07-01T00:49:00+00:00"
    )


def test_menu_reason_prefers_member_safety_over_raw_description():
    item = {
        "description": "(Strawberry add .75) Refills $2",
        "safety": {
            "member-1": {"label": "Me", "reason": "High sugar, violates keto"},
        },
    }

    assert render._menu_reason(item) == "Me: High sugar, violates keto"


def test_menu_reason_explanation_still_wins_over_safety():
    item = {
        "explanation": "Contains wheat flour",
        "description": "House-made daily",
        "safety": {"member-1": {"label": "Me", "reason": "Verify carbs"}},
    }

    assert render._menu_reason(item) == "Contains wheat flour"


def test_menu_reason_falls_back_to_description_without_safety_reasons():
    item = {
        "description": "Lemon beurre blanc",
        "safety": {"member-1": {"label": "Me"}},
    }

    assert render._menu_reason(item) == "Lemon beurre blanc"


@pytest.mark.parametrize("width", [44, 80, 120])
def test_agent_recommendations_remain_unboxed_at_supported_widths(width):
    output = StringIO()
    console = Console(file=output, force_terminal=False, width=width)

    render.agent_recommendations(
        console,
        {
            "restaurant_name": "Thai Taste Restaurant - Fresno, CA",
            "safer": [{"name": "Steamed Jasmine Rice", "section": "Sides", "explanation": "Plain preparation."}],
            "risky": [{"name": "Tom Yum Soup", "section": "Soups", "explanation": "Verify the broth."}],
            "avoid": [{"name": "Pad Thai", "section": "Noodles", "explanation": "Contains peanuts."}],
        },
    )

    text = output.getvalue()
    assert "Generally safer" in text
    assert "Risky" in text
    assert "Avoid" in text
    assert "Thai Taste Restaurant" in text
    assert not any(character in text for character in "╭╮╰╯│─")


def test_agent_recommendations_use_heyfood_semantic_colors():
    output = StringIO()
    console = Console(
        file=output,
        force_terminal=True,
        color_system="truecolor",
        no_color=False,
        width=120,
    )

    render.agent_recommendations(
        console,
        {
            "restaurant_name": "Thai Taste Restaurant",
            "safer": [{"name": "Rice", "explanation": "Plain."}],
            "risky": [{"name": "Soup", "explanation": "Verify."}],
            "avoid": [{"name": "Noodles", "explanation": "Peanuts."}],
        },
    )

    ansi = output.getvalue()
    assert "38;2;155;197;61" in ansi
    assert "38;2;239;193;93" in ansi
    assert "38;2;241;124;117" in ansi
