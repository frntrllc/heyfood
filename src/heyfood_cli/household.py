from __future__ import annotations

from copy import deepcopy
from datetime import datetime, timedelta, timezone
from typing import Any, Iterable
from uuid import uuid4


EVERYONE_ID = "__everyone__"
OWNER_ID = "_self"
HOUSEHOLD_STATE_VERSION = 1
RELATIONSHIPS = {
    "self",
    "spouse",
    "partner",
    "parent",
    "child",
    "sibling",
    "grandparent",
    "friend",
    "other",
}

_PROFILE_CONTEXT_KEYS = {
    "preferences",
    "preference_strictness",
    "restrictions",
    "restriction_handling",
    "avoid_ingredients",
    "medical_constraints",
    "severity_level",
    "notes",
    "activity_level",
    "cuisine_preferences",
}
_MUTABLE_PROFILE_KEYS = {
    "restrictions",
    "preferences",
    "avoid_ingredients",
    "medical_condition_id",
}


class HouseholdError(ValueError):
    pass


def utcnow_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def clean_display_name(value: object) -> str:
    name = str(value or "").strip()
    if not name:
        raise HouseholdError("Member name cannot be empty.")
    if len(name) > 80:
        raise HouseholdError("Member name must be 80 characters or fewer.")
    if any(ord(character) < 32 or ord(character) == 127 for character in name):
        raise HouseholdError("Member name cannot contain control characters.")
    return name


def default_state(*, owner_name: str | None = None) -> dict[str, Any]:
    return {
        "version": HOUSEHOLD_STATE_VERSION,
        "owner_id": OWNER_ID,
        "active_scope": OWNER_ID,
        "members": [
            {
                "id": OWNER_ID,
                "name": (owner_name or "Me").strip() or "Me",
                "relationship": "self",
                "is_owner": True,
                "archived": False,
                "profile_synced": True,
            }
        ],
        "applied_mutation_ids": [],
        "updated_at": utcnow_iso(),
    }


def normalize_state(
    value: object,
    *,
    owner_name: str | None = None,
) -> dict[str, Any]:
    state = deepcopy(value) if isinstance(value, dict) else default_state(owner_name=owner_name)
    members_value = state.get("members")
    members = members_value if isinstance(members_value, list) else []
    normalized_members: list[dict[str, Any]] = []
    seen: set[str] = set()
    for raw in members:
        if not isinstance(raw, dict):
            continue
        member_id = str(raw.get("id") or "").strip()
        if not member_id or member_id in seen or member_id == EVERYONE_ID:
            continue
        seen.add(member_id)
        # Ownership is an invariant of the reserved owner id. Do not trust a
        # hand-edited local config to promote a second member into that role.
        is_owner = member_id == OWNER_ID
        relationship = str(raw.get("relationship") or ("self" if is_owner else "other"))
        if relationship not in RELATIONSHIPS:
            relationship = "self" if is_owner else "other"
        name = str(raw.get("name") or (owner_name if is_owner else member_id) or "Me").strip()
        normalized_members.append(
            {
                "id": member_id,
                "name": name or ("Me" if is_owner else member_id),
                "relationship": "self" if is_owner else relationship,
                "is_owner": is_owner,
                "archived": bool(raw.get("archived", False)) and not is_owner,
                "profile_synced": bool(raw.get("profile_synced", False)) or is_owner,
                **(
                    {"date_of_birth": str(raw["date_of_birth"])}
                    if raw.get("date_of_birth")
                    else {}
                ),
                **(
                    {"created_at": str(raw["created_at"])}
                    if raw.get("created_at")
                    else {}
                ),
            }
        )

    owner = next((m for m in normalized_members if m["id"] == OWNER_ID), None)
    if owner is None:
        owner = default_state(owner_name=owner_name)["members"][0]
        normalized_members.insert(0, owner)
    elif owner_name and owner["name"] in {"", "Me", OWNER_ID}:
        owner["name"] = owner_name

    active_scope = str(state.get("active_scope") or OWNER_ID)
    active_ids = {m["id"] for m in normalized_members if not m["archived"]}
    if active_scope not in active_ids | {EVERYONE_ID}:
        active_scope = OWNER_ID
    if active_scope == EVERYONE_ID and len(active_ids) < 2:
        active_scope = OWNER_ID

    mutation_ids = state.get("applied_mutation_ids")
    if not isinstance(mutation_ids, list):
        mutation_ids = []
    mutation_ids = [str(item) for item in mutation_ids if str(item).strip()][-100:]

    return {
        "version": HOUSEHOLD_STATE_VERSION,
        "owner_id": OWNER_ID,
        "active_scope": active_scope,
        "members": normalized_members,
        "applied_mutation_ids": mutation_ids,
        "updated_at": str(state.get("updated_at") or utcnow_iso()),
    }


def active_members(state: dict[str, Any]) -> list[dict[str, Any]]:
    return [member for member in state["members"] if not member.get("archived")]


def find_member(state: dict[str, Any], member_id: str) -> dict[str, Any] | None:
    return next((m for m in state["members"] if m.get("id") == member_id), None)


def resolve_scope(state: dict[str, Any], selector: str | None) -> str:
    raw = str(selector or state.get("active_scope") or OWNER_ID).strip()
    normalized = raw.casefold()
    if normalized in {"me", "myself", "self", OWNER_ID}:
        return OWNER_ID
    if normalized in {"all", "everyone", "household", "family", EVERYONE_ID}:
        if len(active_members(state)) < 2:
            raise HouseholdError("Add or import another household member before selecting everyone.")
        return EVERYONE_ID

    exact_id = next(
        (m for m in active_members(state) if str(m.get("id")) == raw),
        None,
    )
    if exact_id:
        return str(exact_id["id"])

    name_matches = [
        m
        for m in active_members(state)
        if str(m.get("name") or "").casefold() == normalized
    ]
    if len(name_matches) == 1:
        return str(name_matches[0]["id"])
    if len(name_matches) > 1:
        ids = ", ".join(str(m["id"]) for m in name_matches)
        raise HouseholdError(f"More than one member is named '{raw}'. Use a member id: {ids}")
    raise HouseholdError(
        f"Unknown household scope '{raw}'. Run `heyfood household list` to see valid names and ids."
    )


def scope_label(state: dict[str, Any], scope_id: str | None = None) -> str:
    selected = scope_id or str(state.get("active_scope") or OWNER_ID)
    if selected == EVERYONE_ID:
        return "Everyone"
    member = find_member(state, selected)
    return str((member or {}).get("name") or selected)


def set_active_scope(state: dict[str, Any], selector: str) -> dict[str, Any]:
    result = normalize_state(state)
    result["active_scope"] = resolve_scope(result, selector)
    result["updated_at"] = utcnow_iso()
    return result


def label_member(
    state: dict[str, Any],
    selector: str,
    *,
    name: str,
    relationship: str | None = None,
) -> dict[str, Any]:
    result = normalize_state(state)
    member_id = resolve_scope(result, selector)
    if member_id == EVERYONE_ID:
        raise HouseholdError("Everyone is a scope, not a household member.")
    member = find_member(result, member_id)
    if member is None:
        raise HouseholdError(f"Unknown household member '{selector}'.")
    clean_name = clean_display_name(name)
    if relationship is not None:
        normalized_relationship = relationship.strip().lower()
        if normalized_relationship not in RELATIONSHIPS:
            raise HouseholdError(
                f"Relationship must be one of: {', '.join(sorted(RELATIONSHIPS - {'self'}))}."
            )
        if member_id == OWNER_ID and normalized_relationship != "self":
            raise HouseholdError("The owner relationship must remain self.")
        if member_id != OWNER_ID and normalized_relationship == "self":
            raise HouseholdError("Only the household owner can use the self relationship.")
        if (
            member.get("relationship") != "child"
            and normalized_relationship == "child"
            and member.get("profile_synced")
        ):
            raise HouseholdError(
                "A server-synced adult profile cannot be converted safely to a "
                "local-only child profile. Delete its synced dietary data in "
                "hello.food first, or keep the member as an adult."
            )
        member["relationship"] = normalized_relationship
    member["name"] = clean_name
    result["updated_at"] = utcnow_iso()
    return result


def reconcile_profile_members(
    state: dict[str, Any],
    profile_ids: Iterable[str],
    *,
    owner_name: str | None = None,
) -> dict[str, Any]:
    result = normalize_state(state, owner_name=owner_name)
    known = {str(m["id"]): m for m in result["members"]}
    for raw_id in profile_ids:
        member_id = str(raw_id).strip()
        if not member_id or member_id == EVERYONE_ID:
            continue
        member = known.get(member_id)
        if member is not None:
            if member.get("relationship") != "child":
                member["profile_synced"] = True
            continue
        member = {
            "id": member_id,
            "name": owner_name or "Me" if member_id == OWNER_ID else member_id,
            "relationship": "self" if member_id == OWNER_ID else "other",
            "is_owner": member_id == OWNER_ID,
            "archived": False,
            "profile_synced": True,
            "created_at": utcnow_iso(),
        }
        result["members"].append(member)
        known[member_id] = member
    result["updated_at"] = utcnow_iso()
    return normalize_state(result, owner_name=owner_name)


def roster_wire_context(state: dict[str, Any]) -> dict[str, Any]:
    normalized = normalize_state(state)
    return {
        "owner_id": normalized["owner_id"],
        "members": [
            {
                "id": member["id"],
                "name": member["name"],
                "relationship": member["relationship"],
                "is_owner": bool(member["is_owner"]),
            }
            for member in active_members(normalized)
        ],
    }


def profile_to_dietary_context(profile_data: object) -> dict[str, Any]:
    profile = profile_data if isinstance(profile_data, dict) else {}
    context = {
        key: deepcopy(profile[key])
        for key in _PROFILE_CONTEXT_KEYS
        if key in profile and profile[key] is not None
    }
    if "medical_condition_id" in profile:
        context["medical_condition"] = profile.get("medical_condition_id")
    return context


def member_dietary_context(
    member: dict[str, Any],
    profile_data: object,
    *,
    owner_name: str,
) -> dict[str, Any]:
    context = profile_to_dietary_context(profile_data)
    context.update(
        {
            "name": member["name"],
            "relationship": member["relationship"],
        }
    )
    if member["id"] != OWNER_ID:
        context["owner_name"] = owner_name
    if member.get("date_of_birth"):
        context["date_of_birth"] = member["date_of_birth"]
    return context


def household_dietary_context(
    state: dict[str, Any],
    profiles_by_member_id: dict[str, object],
) -> dict[str, Any]:
    normalized = normalize_state(state)
    owner = find_member(normalized, OWNER_ID) or normalized["members"][0]
    members = []
    for member in active_members(normalized):
        context = member_dietary_context(
            member,
            profiles_by_member_id.get(str(member["id"]), {}),
            owner_name=str(owner["name"]),
        )
        context.update(
            {
                "member_id": member["id"],
                "label": member["name"],
            }
        )
        members.append(context)
    return {"mode": "household", "members": members}


def apply_mutation(
    state: dict[str, Any],
    mutation: object,
) -> tuple[dict[str, Any], dict[str, Any]]:
    result = normalize_state(state)
    if not isinstance(mutation, dict):
        return result, {"applied": False, "reason": "invalid_mutation"}

    payload = deepcopy(mutation)
    fields = payload.pop("fields", None)
    if isinstance(fields, dict):
        payload.update(fields)
    operation = str(payload.get("operation") or "")
    mutation_id = str(payload.get("mutation_id") or f"cli-local-{uuid4()}")
    if mutation_id in result["applied_mutation_ids"]:
        return result, {
            "applied": False,
            "reason": "already_applied",
            "mutation_id": mutation_id,
        }

    effect: dict[str, Any] = {
        "applied": False,
        "operation": operation,
        "mutation_id": mutation_id,
    }
    if operation == "add_member":
        try:
            name = clean_display_name(payload.get("name"))
        except HouseholdError:
            return result, {**effect, "reason": "invalid_name"}
        relationship = str(payload.get("relationship") or "other").strip().lower()
        if relationship not in RELATIONSHIPS - {"self"}:
            relationship = "other"
        replay_window_start = datetime.now(timezone.utc) - timedelta(minutes=2)
        recent_match = next(
            (
                member
                for member in active_members(result)
                if member["id"] != OWNER_ID
                and str(member["name"]).casefold() == name.casefold()
                and member["relationship"] == relationship
                and _created_at(member) >= replay_window_start
            ),
            None,
        )
        if recent_match is not None:
            member_id = str(recent_match["id"])
            effect.update(
                {
                    "applied": False,
                    "reason": "matching_member_exists",
                    "member_id": member_id,
                    "name": recent_match["name"],
                }
            )
        else:
            member_id = str(payload.get("member_id") or uuid4())
            member = {
                "id": member_id,
                "name": name,
                "relationship": relationship,
                "is_owner": False,
                "archived": False,
                "profile_synced": False,
                "created_at": utcnow_iso(),
            }
            if payload.get("date_of_birth"):
                member["date_of_birth"] = str(payload["date_of_birth"])
            result["members"].append(member)
            effect.update({"applied": True, "member_id": member_id, "name": name})
    elif operation in {"update_member", "remove_member"}:
        member_id = str(payload.get("member_id") or "").strip()
        member = find_member(result, member_id)
        if member is None:
            return result, {**effect, "reason": "unknown_member", "member_id": member_id}
        if member_id == OWNER_ID and (
            operation == "remove_member" or payload.get("archived") is True
        ):
            return result, {**effect, "reason": "cannot_archive_owner", "member_id": member_id}
        if payload.get("name") is not None:
            try:
                member["name"] = clean_display_name(payload["name"])
            except HouseholdError:
                return result, {**effect, "reason": "invalid_name", "member_id": member_id}
        if payload.get("relationship") is not None and member_id != OWNER_ID:
            relationship = str(payload["relationship"]).strip().lower()
            if relationship in RELATIONSHIPS - {"self"}:
                member["relationship"] = relationship
        if payload.get("date_of_birth") is not None:
            date_of_birth = str(payload["date_of_birth"]).strip()
            if date_of_birth:
                member["date_of_birth"] = date_of_birth
            else:
                member.pop("date_of_birth", None)
        if operation == "remove_member" or payload.get("archived") is True:
            member["archived"] = True
            if result["active_scope"] == member_id:
                result["active_scope"] = OWNER_ID
        effect.update(
            {
                "applied": True,
                "member_id": member_id,
                "name": member["name"],
            }
        )
    else:
        return result, {**effect, "reason": "unknown_operation"}

    result["applied_mutation_ids"] = [
        *result["applied_mutation_ids"],
        mutation_id,
    ][-100:]
    result["updated_at"] = utcnow_iso()
    return normalize_state(result), effect


def _created_at(member: dict[str, Any]) -> datetime:
    raw = member.get("created_at")
    if not isinstance(raw, str) or not raw:
        return datetime.min.replace(tzinfo=timezone.utc)
    try:
        parsed = datetime.fromisoformat(raw.replace("Z", "+00:00"))
    except ValueError:
        return datetime.min.replace(tzinfo=timezone.utc)
    if parsed.tzinfo is None:
        return parsed.replace(tzinfo=timezone.utc)
    return parsed.astimezone(timezone.utc)


def profile_patch_from_mutation(
    profile_data: object,
    mutation: object,
) -> dict[str, Any]:
    profile = deepcopy(profile_data) if isinstance(profile_data, dict) else {}
    if not isinstance(mutation, dict):
        return profile
    return apply_profile_fields(profile, profile_fields_from_mutation(mutation))


def profile_fields_from_mutation(mutation: object) -> dict[str, Any]:
    """Return only dietary fields explicitly changed by a mutation.

    These patch semantics power the durable sync outbox. Keeping the changed
    fields separate from a full profile prevents a later replay from replacing
    unrelated server-side dietary data with a stale snapshot.
    """
    if not isinstance(mutation, dict):
        return {}
    payload = deepcopy(mutation)
    fields = payload.pop("fields", None)
    if isinstance(fields, dict):
        payload.update(fields)
    patch: dict[str, Any] = {}
    for key in _MUTABLE_PROFILE_KEYS:
        if key not in payload:
            continue
        if key == "medical_condition_id":
            patch[key] = payload.get(key) or None
        else:
            value = payload.get(key)
            patch[key] = list(value) if isinstance(value, list) else []
    return patch


def apply_profile_fields(
    profile_data: object,
    fields: object,
) -> dict[str, Any]:
    profile = deepcopy(profile_data) if isinstance(profile_data, dict) else {}
    if not isinstance(fields, dict):
        return profile
    for key, value in fields.items():
        if key not in _MUTABLE_PROFILE_KEYS:
            continue
        if key == "medical_condition_id":
            profile[key] = value or None
        else:
            profile[key] = list(value) if isinstance(value, list) else []
    return profile


def profile_fields_from_profile(profile_data: object) -> dict[str, Any]:
    profile = profile_data if isinstance(profile_data, dict) else {}
    return {
        key: deepcopy(value)
        for key, value in profile.items()
        if key in _MUTABLE_PROFILE_KEYS
    }


def public_document(state: dict[str, Any]) -> dict[str, Any]:
    normalized = normalize_state(state)
    selected = normalized["active_scope"]
    return {
        "ok": True,
        "active_scope": {
            "id": selected,
            "label": scope_label(normalized, selected),
            "mode": "household" if selected == EVERYONE_ID else "member",
        },
        "members": [
            {
                "id": member["id"],
                "name": member["name"],
                "relationship": member["relationship"],
                "is_owner": member["is_owner"],
                "archived": member["archived"],
                "profile_synced": member["profile_synced"],
                "active": selected == member["id"],
            }
            for member in normalized["members"]
        ],
        "everyone_available": len(active_members(normalized)) >= 2,
        "storage": "local_roster_server_profiles",
    }
