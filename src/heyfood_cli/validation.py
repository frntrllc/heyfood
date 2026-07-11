from __future__ import annotations

import math
from datetime import date
from typing import TypeVar


class ValidationError(ValueError):
    pass


T = TypeVar("T")


def required_text(value: str, *, label: str, max_length: int) -> str:
    normalized = str(value or "").strip()
    if not normalized:
        raise ValidationError(f"{label} must not be empty.")
    if len(normalized) > max_length:
        raise ValidationError(f"{label} must be at most {max_length} characters.")
    return normalized


def optional_text(value: str | None, *, label: str, max_length: int) -> str | None:
    if value is None:
        return None
    normalized = str(value).strip()
    if not normalized:
        return None
    if len(normalized) > max_length:
        raise ValidationError(f"{label} must be at most {max_length} characters.")
    return normalized


def coordinates(
    latitude: float | None,
    longitude: float | None,
    *,
    required: bool = False,
) -> tuple[float | None, float | None]:
    if latitude is None and longitude is None:
        if required:
            raise ValidationError("Provide both --lat and --lng together.")
        return None, None
    if latitude is None or longitude is None:
        raise ValidationError("Provide both --lat and --lng together.")
    lat = finite_number(latitude, label="Latitude")
    lng = finite_number(longitude, label="Longitude")
    if not -90 <= lat <= 90:
        raise ValidationError("Latitude must be between -90 and 90.")
    if not -180 <= lng <= 180:
        raise ValidationError("Longitude must be between -180 and 180.")
    return lat, lng


def finite_number(value: float, *, label: str) -> float:
    if isinstance(value, bool):
        raise ValidationError(f"{label} must be a finite number.")
    number = float(value)
    if not math.isfinite(number):
        raise ValidationError(f"{label} must be a finite number.")
    return number


def bounded_number(
    value: float,
    *,
    label: str,
    minimum: float,
    maximum: float | None = None,
) -> float:
    number = finite_number(value, label=label)
    if number < minimum:
        raise ValidationError(f"{label} must be at least {minimum:g}.")
    if maximum is not None and number > maximum:
        raise ValidationError(f"{label} must be at most {maximum:g}.")
    return number


def bounded_integer(
    value: int,
    *,
    label: str,
    minimum: int,
    maximum: int | None = None,
) -> int:
    if isinstance(value, bool):
        raise ValidationError(f"{label} must be an integer.")
    number = int(value)
    if number < minimum:
        raise ValidationError(f"{label} must be at least {minimum}.")
    if maximum is not None and number > maximum:
        raise ValidationError(f"{label} must be at most {maximum}.")
    return number


def iso_date(value: str, *, label: str = "Date") -> str:
    normalized = required_text(value, label=label, max_length=10)
    try:
        parsed = date.fromisoformat(normalized)
    except ValueError as exc:
        raise ValidationError(f"{label} must use YYYY-MM-DD format.") from exc
    return parsed.isoformat()


def choice(value: str | None, *, label: str, choices: set[str]) -> str | None:
    if value is None:
        return None
    normalized = str(value).strip().lower()
    if normalized not in choices:
        allowed = ", ".join(sorted(choices))
        raise ValidationError(f"{label} must be one of: {allowed}.")
    return normalized
