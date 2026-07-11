from __future__ import annotations

import math

import pytest

from heyfood_cli import validation


@pytest.mark.parametrize(
    ("latitude", "longitude"),
    ((-90, -180), (0, 0), (90, 180)),
)
def test_coordinate_boundaries(latitude: float, longitude: float) -> None:
    assert validation.coordinates(latitude, longitude) == (
        float(latitude),
        float(longitude),
    )


@pytest.mark.parametrize(
    ("latitude", "longitude", "message"),
    (
        (91, 0, "Latitude"),
        (-91, 0, "Latitude"),
        (0, 181, "Longitude"),
        (0, -181, "Longitude"),
        (math.nan, 0, "finite"),
        (0, math.inf, "finite"),
        (1, None, "both --lat and --lng"),
    ),
)
def test_invalid_coordinates(latitude, longitude, message: str) -> None:
    with pytest.raises(validation.ValidationError, match=message):
        validation.coordinates(latitude, longitude)


def test_radius_and_limit_bounds() -> None:
    assert validation.bounded_number(
        0.1,
        label="Radius",
        minimum=0.1,
        maximum=50,
    ) == 0.1
    assert validation.bounded_integer(20, label="Limit", minimum=1, maximum=20) == 20

    with pytest.raises(validation.ValidationError, match="at least 0.1"):
        validation.bounded_number(0, label="Radius", minimum=0.1, maximum=50)
    with pytest.raises(validation.ValidationError, match="at most 50"):
        validation.bounded_number(51, label="Radius", minimum=0.1, maximum=50)
    with pytest.raises(validation.ValidationError, match="at most 20"):
        validation.bounded_integer(21, label="Limit", minimum=1, maximum=20)


def test_text_date_and_choice_contracts() -> None:
    assert validation.required_text("  thai  ", label="Query", max_length=20) == "thai"
    assert validation.optional_text("   ", label="Query", max_length=20) is None
    assert validation.iso_date("2026-07-10") == "2026-07-10"
    assert validation.choice(
        "Dinner",
        label="Meal type",
        choices={"breakfast", "dinner"},
    ) == "dinner"

    with pytest.raises(validation.ValidationError, match="must not be empty"):
        validation.required_text(" ", label="Query", max_length=20)
    with pytest.raises(validation.ValidationError, match="YYYY-MM-DD"):
        validation.iso_date("07/10/2026")
    with pytest.raises(validation.ValidationError, match="must be one of"):
        validation.choice("brunch", label="Meal type", choices={"breakfast", "dinner"})
