from __future__ import annotations

import re
from typing import Any

from .config import ConfigStore, utcnow


_NOT_NAMES = {
    "keto",
    "vegan",
    "vegetarian",
    "paleo",
    "pescatarian",
    "gluten",
    "dairy",
    "dairy-free",
    "dairyfree",
    "ibs",
    "celiac",
    "gluten-free",
    "glutenfree",
    "low",
    "low-fodmap",
    "lowfodmap",
    "mostly",
    "avoiding",
    "allergic",
    "sensitive",
    "thai",
    "italian",
    "mexican",
}


def first_name_from_account(me: dict[str, Any] | None) -> str | None:
    display_name = str((me or {}).get("display_name") or "").strip()
    if not display_name:
        return None
    return _clean_first_name(display_name.split()[0])


def first_name_from_text(text: str) -> str | None:
    source = str(text or "").strip()
    patterns = (
        r"\bmy name is\s+([A-Za-z][A-Za-z'-]{1,40})\b",
        r"\bi am\s+([A-Za-z][A-Za-z'-]{1,40})\b",
        r"\bi'm\s+([A-Za-z][A-Za-z'-]{1,40})\b",
        r"\bcall me\s+([A-Za-z][A-Za-z'-]{1,40})\b",
    )
    for pattern in patterns:
        match = re.search(pattern, source, flags=re.IGNORECASE)
        if not match:
            continue
        name = _clean_first_name(match.group(1))
        if name:
            return name
    return None


def load_cli_first_name(store: ConfigStore) -> str | None:
    value = store.load().get("first_name")
    return _clean_first_name(str(value)) if value else None


def save_cli_first_name(store: ConfigStore, first_name: str) -> None:
    name = _clean_first_name(first_name)
    if not name:
        return
    data = store.load()
    data["first_name"] = name
    data["first_name_updated_at"] = utcnow().isoformat()
    store.save(data)


def should_show_first_welcome(store: ConfigStore) -> bool:
    return not bool(store.load().get("welcomed_at"))


def mark_welcomed(store: ConfigStore) -> None:
    data = store.load()
    data["welcomed_at"] = utcnow().isoformat()
    store.save(data)


def welcome_message(first_name: str | None, *, first_time: bool = False) -> str:
    if first_name:
        if first_time:
            return f"Hey {first_name}. Let's build the food brain."
        return f"Welcome back, {first_name}. Let's tune the food brain."
    if first_time:
        return "Hey. Let's build the food brain."
    return "Let's tune the food brain."


def onboarding_quip(profile: dict[str, Any]) -> str | None:
    preferences = set(profile.get("preferences") or [])
    restrictions = set(profile.get("restrictions") or [])
    conditions = set(profile.get("health_condition_ids") or [])
    cuisines = set(profile.get("cuisine_preferences") or [])
    medical_constraints = set(profile.get("medical_constraints") or [])
    avoid = {str(item).lower() for item in profile.get("avoid_ingredients") or []}

    if "thai" in cuisines and "peanutFree" in restrictions:
        return (
            "Thai food with a peanut allergy: delicious puzzle mode. "
            "I'll keep an extra eye on satay, pad Thai, and sneaky sauces."
        )
    if "thai" in cuisines and ("ibs" in conditions or "high_fodmap" in medical_constraints):
        return (
            "Thai plus IBS is a worthy mission. Garlic, onion, and rich sauces "
            "are now officially under surveillance."
        )
    if "italian" in cuisines and ("glutenFree" in restrictions or "celiac" in conditions):
        return (
            "Gluten-free Italian is not impossible. Risotto, polenta, grilled "
            "proteins, and a little menu skepticism can carry the day."
        )
    if "keto" in preferences and "mexican" in cuisines:
        return (
            "Keto Mexican has range: fajita plates, guacamole, and hold-the-tortilla "
            "energy. I respect the assignment."
        )
    if "vegan" in preferences and "japanese" in cuisines:
        return (
            "Vegan Japanese is beautiful, but dashi likes to hide in plain sight. "
            "I will be politely suspicious."
        )
    if "dairyFree" in restrictions and "french" in cuisines:
        return (
            "Dairy-free French is playing on hard mode. Butter is everywhere; "
            "we will still find the good stuff."
        )
    if {"onion", "garlic"} & avoid and ("ibs" in conditions or "low_fodmap" in preferences):
        return (
            "Onion and garlic are tiny flavor tyrants. I'll watch for them in "
            "sauces, broths, marinades, and anything described as 'house-made'."
        )
    if "keto" in preferences:
        return "Keto noted. I will treat hidden sugar like it owes us money."
    if "low_fodmap" in preferences or "ibs" in conditions:
        return "Low-FODMAP mode engaged. Onion and garlic just lost their invisibility cloak."
    if restrictions:
        return "Restrictions noted. I will be the annoying friend who reads the fine print."
    if cuisines:
        return "Excellent taste. I will try not to let the menu ruin it."
    return None


def _clean_first_name(value: str) -> str | None:
    cleaned = re.sub(r"[^A-Za-z'-]", "", str(value or "").strip())
    if len(cleaned) < 2:
        return None
    normalized = cleaned.lower()
    compact = re.sub(r"[-']", "", normalized)
    if normalized in _NOT_NAMES or compact in _NOT_NAMES:
        return None
    return cleaned[:1].upper() + cleaned[1:40]
