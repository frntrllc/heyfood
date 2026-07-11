from __future__ import annotations

import json
import sys
from typing import Any, TextIO


_SAFETY_FIELDS = {
    "status",
    "fit_status",
    "safety_status",
    "verdict",
    "composite_level",
    "level",
}
_SAFETY_ALIASES = {
    "safe": "generally_safer",
    "safer": "generally_safer",
    "generally_safe": "generally_safer",
    "generally_safer": "generally_safer",
    "caution": "risky",
    "risk": "risky",
    "risky": "risky",
    "unsafe": "avoid",
    "avoid": "avoid",
    "unable": "unable_to_evaluate",
    "unknown": "unable_to_evaluate",
    "not_evaluated": "unable_to_evaluate",
    "unable_to_evaluate": "unable_to_evaluate",
}


def normalize_safety_vocabulary(value: Any, *, field: str | None = None) -> Any:
    """Normalize legacy safety enums without changing operational status values."""
    if isinstance(value, dict):
        return {
            key: normalize_safety_vocabulary(item, field=str(key))
            for key, item in value.items()
        }
    if isinstance(value, list):
        return [normalize_safety_vocabulary(item, field=field) for item in value]
    if isinstance(value, tuple):
        return [normalize_safety_vocabulary(item, field=field) for item in value]
    if field in _SAFETY_FIELDS and isinstance(value, str):
        normalized = value.strip().lower().replace("-", "_").replace(" ", "_")
        return _SAFETY_ALIASES.get(normalized, value)
    return value


def write_json(data: Any, *, stream: TextIO | None = None) -> None:
    """Write exactly one ANSI-free JSON value and a trailing newline."""
    target = stream if stream is not None else sys.stdout
    target.write(
        json.dumps(
            normalize_safety_vocabulary(data),
            ensure_ascii=False,
            indent=2,
            sort_keys=True,
            default=str,
        )
    )
    target.write("\n")
    target.flush()


def error_document(kind: str, message: str, *, hint: str | None = None) -> dict[str, Any]:
    error: dict[str, Any] = {"type": kind, "message": message}
    if hint:
        error["hint"] = hint
    return {"ok": False, "error": error}
