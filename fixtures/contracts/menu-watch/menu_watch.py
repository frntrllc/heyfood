"""Request/response schemas for the menu-watch subscription API (Menu Freshness D3).

Directive: ``docs/plans/backend/2026-07-18-menu-freshness-on-demand-scheduled-watch.md``
"""
from __future__ import annotations

from datetime import datetime
from typing import List, Optional

from pydantic import BaseModel, ConfigDict, Field


class WatchCadence(BaseModel):
    """When the scheduler fetches fresh, in the restaurant's local time.

    ``weekday`` follows Python's ``datetime.weekday()`` convention:
    Monday=0 .. Sunday=6 (so Thursday=3). ``hour`` is 0-23 local.
    """

    weekday: int = Field(
        ge=0, le=6, description="Monday=0 .. Sunday=6 (Thursday=3)."
    )
    hour: int = Field(ge=0, le=23, description="Local hour of day, 0-23.")


class MenuWatchCreateRequest(BaseModel):
    """Body for ``POST /v1/menu/watch``."""

    restaurant_id: str = Field(description="Internal restaurant UUID.")
    cadence: WatchCadence
    notify: bool = Field(
        default=False,
        description="Emit a change notification on scheduled runs whose diff is "
        "non-empty (the firing itself is D5/D7 — the flag is stored now).",
    )
    menu_url: Optional[str] = Field(
        default=None,
        description="The selected menu URL to verify + watch. Falls back to the "
        "restaurant's website / best known source when omitted.",
    )
    confirm_menu_url: bool = Field(
        default=False,
        description="Caller explicitly vouches that ``menu_url`` (or the resolved "
        "source) is the correct menu. Activates a watch whose identity confidence "
        "is below the auto-threshold. A MISMATCH verdict is NEVER activatable, "
        "confirmation or not.",
    )
    tz: Optional[str] = Field(
        default=None,
        description="Optional IANA timezone override (e.g. 'America/Chicago'). "
        "When omitted the server derives it from the restaurant's coordinates; "
        "supply it explicitly for restaurants with missing/coarse coordinates.",
    )

    model_config = ConfigDict(
        json_schema_extra={
            "example": {
                "restaurant_id": "0c1cb790-0000-0000-0000-000000000000",
                "cadence": {"weekday": 3, "hour": 9},
                "notify": True,
            }
        }
    )


class MenuWatchResponse(BaseModel):
    """A single menu-watch row as returned by create/list."""

    id: str
    restaurant_id: str
    cadence: WatchCadence
    tz: str = Field(description="Resolved IANA timezone frozen at creation.")
    active: bool
    notify: bool
    next_run_at: datetime
    last_run_at: Optional[datetime] = None
    last_snapshot_id: Optional[str] = None
    created_at: datetime
    # Identity-gate provenance (echoed on create so a CLI can show why a watch is
    # active or what confidence it activated at). Not persisted on the row.
    identity_verdict: Optional[str] = None
    identity_confidence: Optional[float] = None


class MenuWatchListResponse(BaseModel):
    """List of the authenticated subscriber's watches."""

    watches: List[MenuWatchResponse] = Field(default_factory=list)
    count: int = 0
