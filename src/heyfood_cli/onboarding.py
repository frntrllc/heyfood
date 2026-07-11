from __future__ import annotations

from dataclasses import dataclass
from importlib.resources import files
import json
import re
from typing import Any


NONE_ID = "__none__"


@dataclass(frozen=True)
class DietaryOption:
    label: str
    id: str
    enum_key: str | None = None
    constraints: tuple[str, ...] = ()


def _load_catalog() -> dict[str, Any]:
    resource = files("heyfood_cli.data").joinpath("dietary_options.json")
    payload = json.loads(resource.read_text(encoding="utf-8"))
    if not isinstance(payload.get("version"), int):
        raise RuntimeError("packaged dietary option catalog has no schema version")
    return payload


_CATALOG = _load_catalog()
CANONICAL_SCHEMA_VERSION: int = _CATALOG["version"]


def _catalog_options(section_name: str) -> tuple[DietaryOption, ...]:
    section = _CATALOG["sections"][section_name]
    entries = [
        entry
        for group in ("tier1", "tier2", "options")
        for entry in section.get(group, [])
        if entry.get("id") != NONE_ID and not entry.get("deprecated", False)
    ]
    return tuple(
        DietaryOption(
            label=entry["label"],
            id=entry["id"],
            enum_key=entry.get("enum_key"),
            constraints=tuple(entry.get("constraints", ())),
        )
        for entry in entries
    )


HEALTH_CONDITIONS = _catalog_options("health_conditions")
DIET_STYLES = _catalog_options("diet_style")
ALLERGIES = _catalog_options("allergies")
ACTIVITY_LEVELS = _catalog_options("activity_level")
CUISINES = _catalog_options("cuisines")


PREFERENCE_VALUES = {
    "keto",
    "vegan",
    "vegetarian",
    "paleo",
    "mediterranean",
    "lowCarb",
    "whole30",
    "pescatarian",
    "low_fodmap",
    "high_protein",
    "none",
}
RESTRICTION_VALUES = {
    "glutenFree",
    "dairyFree",
    "nutFree",
    "peanutFree",
    "treeNutFree",
    "shellfishFree",
    "fishFree",
    "soyFree",
    "eggFree",
    "sesameFree",
    "lactoseIntolerant",
    "halal",
    "kosher",
}
DIET_TO_RESTRICTION = {
    "gluten_free": "glutenFree",
    "dairy_free": "dairyFree",
    "halal": "halal",
    "kosher": "kosher",
}
DIET_TO_CONSTRAINTS = {
    "low_fodmap": ("high_fodmap",),
    "low_sodium": ("high_sodium",),
    "dash": ("high_sodium",),
    "low_fat": ("high_fat",),
}
CONDITION_TO_RESTRICTION = {"celiac": "glutenFree"}

_DIET_BY_ID = {option.id: option for option in DIET_STYLES}
_ALLERGY_BY_ID = {option.id: option for option in ALLERGIES}
_CONDITION_BY_ID = {option.id: option for option in HEALTH_CONDITIONS}


def _diet_preference(option: DietaryOption) -> str | None:
    candidate = option.enum_key or option.id
    return candidate if candidate in PREFERENCE_VALUES else None


_PREFERENCE_TO_DIET = {
    preference: option.id
    for option in DIET_STYLES
    if (preference := _diet_preference(option)) is not None
}
_RESTRICTION_TO_ALLERGY = {
    option.enum_key: option.id
    for option in ALLERGIES
    if option.enum_key in RESTRICTION_VALUES
}

_ALIASES = {
    "crohns": "crohns",
    "crohn": "crohns",
    "crohnsdisease": "crohns",
    "irritablebowelsyndrome": "ibs",
    "irritablebowel": "ibs",
    "coeliac": "celiac",
    "coeliacdisease": "celiac",
    "acidreflux": "gerd",
    "reflux": "gerd",
    "gerdacidreflux": "gerd",
    "type1": "diabetes_type_1",
    "type1diabetes": "diabetes_type_1",
    "t1d": "diabetes_type_1",
    "type2": "diabetes_type_2",
    "type2diabetes": "diabetes_type_2",
    "t2d": "diabetes_type_2",
    "kidneydisease": "ckd",
    "kidneydiseaseckd": "ckd",
    "highbloodpressure": "hypertension",
    "bloodpressure": "hypertension",
    "hypertension": "hypertension",
    "eosinophilicesophagitiseoe": "eoe",
    "avoidantrestrictivefoodintakedisorder": "arfid",
    "autism": "autism_sensory",
    "autistic": "autism_sensory",
    "sensoryfoodneeds": "autism_sensory",
    "lowfodmap": "low_fodmap",
    "fodmap": "low_fodmap",
    "lowcarb": "low_carb",
    "highprotein": "high_protein",
    "glutenfree": "gluten_free",
    "dairyfree": "dairy_free",
    "milk": "dairy",
    "dairy": "dairy",
    "treenut": "tree_nuts",
    "treenuts": "tree_nuts",
    "peanut": "peanuts",
    "gluten": "wheat",
    "glutenintolerant": "gluten_sensitivity",
    "glutenintolerance": "gluten_sensitivity",
    "lactoseintolerant": "lactose",
    "lactoseintolerance": "lactose",
    "lactose": "lactose",
    "msg": "msg",
    "middleeastern": "middle_eastern",
    "spanish": "spanish",
    "tapas": "spanish",
    "caribbean": "caribbean",
    "southern": "southern",
    "soulfood": "southern",
    "cajun": "cajun_creole",
    "creole": "cajun_creole",
    "hawaiian": "hawaiian",
    "polynesian": "hawaiian",
    "veryactive": "very_active",
    "highlyactive": "very_active",
    "moderatelyactive": "moderate",
    "moderateactivity": "moderate",
    "lightlyactive": "light",
    "lightactivity": "light",
    "inactive": "sedentary",
    "prefernot": "prefer_not_to_say",
}


def empty_profile_data() -> dict[str, Any]:
    return {
        "preferences": [],
        "preference_strictness": {},
        "restrictions": [],
        "restriction_handling": {},
        "avoid_ingredients": [],
        "notes": None,
        "medical_condition_id": None,
        "medical_constraints": [],
        "severity_level": None,
        "activity_level": None,
        "cuisine_preferences": [],
        "health_condition_ids": [],
        "custom_health_conditions": [],
        "custom_diet_styles": [],
        "custom_restrictions": [],
        "custom_cuisines": [],
        "selection_provenance_version": 1,
        "diet_style_ids": [],
        "allergy_ids": [],
        "additional_restriction_ids": [],
        "additional_medical_constraints": [],
        "condition_severity_levels": {},
    }


def normalize_profile_data(data: dict[str, Any] | None) -> dict[str, Any]:
    if not isinstance(data, dict):
        return empty_profile_data()

    source = dict(data)
    alias_map = {
        "healthConditionIds": "health_condition_ids",
        "customHealthConditions": "custom_health_conditions",
        "customDietStyles": "custom_diet_styles",
        "customRestrictions": "custom_restrictions",
        "customCuisines": "custom_cuisines",
        "selectionProvenanceVersion": "selection_provenance_version",
        "dietStyleIds": "diet_style_ids",
        "allergyIds": "allergy_ids",
        "additionalRestrictionIds": "additional_restriction_ids",
        "additionalMedicalConstraints": "additional_medical_constraints",
        "conditionSeverityLevels": "condition_severity_levels",
    }
    for alias, key in alias_map.items():
        if alias in source and key not in source:
            source[key] = source[alias]

    provenance_version = source.get("selection_provenance_version")
    if provenance_version not in (None, 1):
        raise ValueError(
            f"Unsupported selection provenance version: {provenance_version}"
        )
    legacy = provenance_version is None

    profile = empty_profile_data()
    for key in profile:
        if key in source:
            profile[key] = source[key]

    for key in (
        "preferences",
        "restrictions",
        "avoid_ingredients",
        "medical_constraints",
        "cuisine_preferences",
        "health_condition_ids",
        "custom_health_conditions",
        "custom_diet_styles",
        "custom_restrictions",
        "custom_cuisines",
        "diet_style_ids",
        "allergy_ids",
        "additional_restriction_ids",
        "additional_medical_constraints",
    ):
        profile[key] = _clean_list(profile.get(key))

    profile["preference_strictness"] = _clean_map(profile.get("preference_strictness"))
    profile["restriction_handling"] = _clean_map(profile.get("restriction_handling"))
    severity = profile.get("severity_level")
    profile["severity_level"] = (
        None
        if severity is None
        else _clamp_int(severity, default=3, minimum=1, maximum=5)
    )
    raw_severity_map = profile.get("condition_severity_levels")
    profile["condition_severity_levels"] = {
        str(key): _clamp_int(value, default=3, minimum=1, maximum=5)
        for key, value in (
            raw_severity_map.items()
            if isinstance(raw_severity_map, dict)
            else ()
        )
    }
    profile["notes"] = _clean_optional_text(profile.get("notes"), max_length=280)
    profile["medical_condition_id"] = _clean_optional_text(profile.get("medical_condition_id"), max_length=100)
    profile["activity_level"] = _clean_optional_text(profile.get("activity_level"), max_length=30)
    if legacy:
        _migrate_legacy_provenance(profile, source)
    return _rebuild_derived_profile(profile)


def split_values(values: list[str] | tuple[str, ...] | None) -> list[str]:
    result: list[str] = []
    for value in values or []:
        for part in re.split(r"[,;]", str(value)):
            cleaned = part.strip()
            if cleaned:
                result.append(cleaned)
    return result


def option_catalog() -> dict[str, tuple[DietaryOption, ...]]:
    return {
        "conditions": HEALTH_CONDITIONS,
        "diets": DIET_STYLES,
        "allergies": ALLERGIES,
        "activity": ACTIVITY_LEVELS,
        "cuisines": CUISINES,
    }


def parse_profile_text(text: str) -> dict[str, Any]:
    """Extract onboarding fields from a short natural-language profile.

    This is intentionally deterministic and vocabulary-backed. It is not trying
    to be a full medical parser; it turns obvious diet/allergy/condition/cuisine
    mentions into the same labels the interactive picker emits.
    """
    source = str(text or "").strip()
    parsed = {
        "diets": _find_options(source, DIET_STYLES),
        "allergies": _find_options(source, ALLERGIES),
        "conditions": _find_options(source, HEALTH_CONDITIONS),
        "avoid_ingredients": _extract_avoid_ingredients(source),
        "activity_level": _first_or_none(_find_options(source, ACTIVITY_LEVELS)),
        "cuisines": _find_options(source, CUISINES),
    }
    answered = {
        key
        for key in (
            "diets",
            "allergies",
            "conditions",
            "avoid_ingredients",
            "activity_level",
            "cuisines",
        )
        if parsed.get(key)
    }
    lowered = source.lower().replace("’", "'")
    negative_sections = {
        "diets": r"\b(?:no|without) (?:special |specific )?diet(?:ary style)?s?\b",
        "allergies": r"\b(?:no|without) (?:food )?(?:allergies|dietary restrictions)\b",
        "conditions": r"\b(?:no|without) (?:health|medical) conditions?\b",
        "avoid_ingredients": r"\b(?:nothing|no ingredients?) to avoid\b",
        "cuisines": r"\b(?:no cuisine preference|any cuisine)\b",
    }
    for key, pattern in negative_sections.items():
        if re.search(pattern, lowered):
            answered.add(key)
            parsed[key] = ["none"]
    parsed["answered_sections"] = sorted(answered)
    return parsed


def option_labels(options: tuple[DietaryOption, ...], *, primary_count: int | None = None) -> list[str]:
    selected = options if primary_count is None else options[:primary_count]
    return [option.label for option in selected]


def build_profile_data(
    *,
    existing: dict[str, Any] | None = None,
    replace: bool = False,
    diets: list[str] | None = None,
    allergies: list[str] | None = None,
    conditions: list[str] | None = None,
    avoid_ingredients: list[str] | None = None,
    activity_level: str | None = None,
    cuisines: list[str] | None = None,
    notes: str | None = None,
    severity_level: int | None = None,
) -> dict[str, Any]:
    profile = empty_profile_data() if replace else normalize_profile_data(existing)

    if diets is not None:
        resolved_diets = _resolve_many(diets, DIET_STYLES)
        diet_style_ids: list[str] = []
        custom_diet_styles: list[str] = []
        for raw, option in resolved_diets:
            if option is None:
                _append_unique(custom_diet_styles, raw)
                continue
            if option.id == NONE_ID:
                continue
            _append_unique(diet_style_ids, option.id)
        profile["diet_style_ids"] = diet_style_ids
        profile["custom_diet_styles"] = custom_diet_styles

    if allergies is not None:
        resolved_allergies = _resolve_many(allergies, ALLERGIES)
        allergy_ids: list[str] = []
        custom_restrictions: list[str] = []
        for raw, option in resolved_allergies:
            if option is None:
                _append_unique(custom_restrictions, raw)
                continue
            if option.id == NONE_ID:
                continue
            _append_unique(allergy_ids, option.id)
        profile["allergy_ids"] = allergy_ids
        profile["custom_restrictions"] = custom_restrictions

    if conditions is not None:
        resolved_conditions = _resolve_many(conditions, HEALTH_CONDITIONS)
        condition_ids: list[str] = []
        custom_conditions: list[str] = []
        for raw, option in resolved_conditions:
            if option is None:
                _append_unique(custom_conditions, raw)
                continue
            if option.id == NONE_ID:
                continue
            _append_unique(condition_ids, option.id)
        profile["health_condition_ids"] = condition_ids
        profile["custom_health_conditions"] = custom_conditions

    if avoid_ingredients is not None:
        profile["avoid_ingredients"] = (
            []
            if any(_is_none(value) for value in avoid_ingredients)
            else _clean_list(avoid_ingredients, limit=20, max_length=40)
        )

    if activity_level is not None:
        activity = _resolve_one(activity_level, ACTIVITY_LEVELS)
        profile["activity_level"] = None if activity is None or activity.id == NONE_ID else activity.id

    if cuisines is not None:
        resolved_cuisines = _resolve_many(cuisines, CUISINES)
        cuisine_ids: list[str] = []
        custom_cuisines: list[str] = []
        for raw, option in resolved_cuisines:
            if option is None:
                _append_unique(custom_cuisines, raw)
                continue
            _append_unique(cuisine_ids, option.id)
        profile["cuisine_preferences"] = cuisine_ids
        profile["custom_cuisines"] = custom_cuisines

    if notes is not None:
        profile["notes"] = _clean_optional_text(notes, max_length=280)

    if severity_level is not None:
        selected_conditions = list(profile.get("health_condition_ids") or [])
        if not selected_conditions:
            raise ValueError("Severity requires at least one health condition.")
        severity = _clamp_int(severity_level, default=3, minimum=1, maximum=5)
        profile["condition_severity_levels"] = {
            condition_id: severity for condition_id in selected_conditions
        }

    return _rebuild_derived_profile(profile)


def _derived_preferences(diet_style_ids: list[str]) -> list[str]:
    preferences: list[str] = []
    for diet_id in diet_style_ids:
        option = _DIET_BY_ID.get(diet_id)
        if option is not None:
            _append_unique(preferences, _diet_preference(option) or "")
    return preferences


def _derived_restrictions(
    diet_style_ids: list[str],
    allergy_ids: list[str],
    condition_ids: list[str],
) -> list[str]:
    restrictions: list[str] = []
    for diet_id in diet_style_ids:
        _append_unique(restrictions, DIET_TO_RESTRICTION.get(diet_id) or "")
    for allergy_id in allergy_ids:
        option = _ALLERGY_BY_ID.get(allergy_id)
        if option is not None and option.enum_key in RESTRICTION_VALUES:
            _append_unique(restrictions, option.enum_key or "")
    for condition_id in condition_ids:
        _append_unique(
            restrictions,
            CONDITION_TO_RESTRICTION.get(condition_id) or "",
        )
    return restrictions


def _derived_constraints(
    diet_style_ids: list[str],
    condition_ids: list[str],
) -> list[str]:
    constraints: list[str] = []
    for condition_id in condition_ids:
        option = _CONDITION_BY_ID.get(condition_id)
        if option is not None:
            for constraint in option.constraints:
                _append_unique(constraints, constraint)
    for diet_id in diet_style_ids:
        for constraint in DIET_TO_CONSTRAINTS.get(diet_id, ()):
            _append_unique(constraints, constraint)
    return constraints


def _migrate_legacy_provenance(
    profile: dict[str, Any],
    source: dict[str, Any],
) -> None:
    diet_style_ids: list[str] = []
    for preference in profile.get("preferences") or []:
        _append_unique(diet_style_ids, _PREFERENCE_TO_DIET.get(preference) or "")

    true_custom_diets: list[str] = []
    for value in profile.get("custom_diet_styles") or []:
        option = _resolve_one(value, DIET_STYLES)
        if option is None:
            _append_unique(true_custom_diets, value)
        else:
            _append_unique(diet_style_ids, option.id)
    for diet_id, restriction in DIET_TO_RESTRICTION.items():
        if restriction in {"halal", "kosher"} and restriction in (
            profile.get("restrictions") or []
        ):
            _append_unique(diet_style_ids, diet_id)

    allergy_ids: list[str] = []
    for restriction in profile.get("restrictions") or []:
        _append_unique(
            allergy_ids,
            _RESTRICTION_TO_ALLERGY.get(restriction) or "",
        )

    condition_ids = list(profile.get("health_condition_ids") or [])
    if "health_condition_ids" not in source and profile.get("medical_condition_id"):
        condition_ids = [str(profile["medical_condition_id"])]

    derived_restrictions = _derived_restrictions(
        diet_style_ids,
        allergy_ids,
        condition_ids,
    )
    derived_constraints = _derived_constraints(diet_style_ids, condition_ids)
    profile["diet_style_ids"] = diet_style_ids
    profile["allergy_ids"] = allergy_ids
    profile["health_condition_ids"] = condition_ids
    profile["custom_diet_styles"] = true_custom_diets
    profile["additional_restriction_ids"] = [
        value
        for value in profile.get("restrictions") or []
        if value not in derived_restrictions
    ]
    profile["additional_medical_constraints"] = [
        value
        for value in profile.get("medical_constraints") or []
        if value not in derived_constraints
    ]
    if condition_ids:
        severity = profile.get("severity_level") or 3
        profile["condition_severity_levels"] = {
            condition_id: severity for condition_id in condition_ids
        }


def _rebuild_derived_profile(profile: dict[str, Any]) -> dict[str, Any]:
    diet_style_ids = [
        value
        for value in _clean_list(profile.get("diet_style_ids"))
        if value in _DIET_BY_ID
    ]
    allergy_ids = [
        value
        for value in _clean_list(profile.get("allergy_ids"))
        if value in _ALLERGY_BY_ID
    ]
    condition_ids = _clean_list(profile.get("health_condition_ids"))

    derived_restrictions = _derived_restrictions(
        diet_style_ids,
        allergy_ids,
        condition_ids,
    )
    additional_restrictions = [
        value
        for value in _valid_restrictions(
            profile.get("additional_restriction_ids") or []
        )
        if value not in derived_restrictions
    ]
    restrictions = _valid_restrictions(
        derived_restrictions + additional_restrictions
    )

    derived_constraints = _derived_constraints(diet_style_ids, condition_ids)
    additional_constraints = [
        value
        for value in _clean_list(
            profile.get("additional_medical_constraints")
        )
        if value not in derived_constraints
    ]
    constraints = _clean_list(derived_constraints + additional_constraints)

    raw_severity_map = profile.get("condition_severity_levels")
    severity_map = {
        condition_id: _clamp_int(
            raw_severity_map.get(condition_id),
            default=int(profile.get("severity_level") or 3),
            minimum=1,
            maximum=5,
        )
        for condition_id in condition_ids
        if isinstance(raw_severity_map, dict)
    }
    if condition_ids and len(severity_map) != len(condition_ids):
        fallback = int(profile.get("severity_level") or 3)
        for condition_id in condition_ids:
            severity_map.setdefault(condition_id, fallback)

    custom_diets = [
        value
        for value in _clean_list(profile.get("custom_diet_styles"))
        if not (
            (option := _resolve_one(value, DIET_STYLES)) is not None
            and option.id in diet_style_ids
        )
    ]
    custom_restrictions = [
        value
        for value in _clean_list(profile.get("custom_restrictions"))
        if not (
            (option := _resolve_one(value, ALLERGIES)) is not None
            and option.id in allergy_ids
        )
    ]
    for diet_id in diet_style_ids:
        option = _DIET_BY_ID[diet_id]
        if option.enum_key is None:
            _append_unique(custom_diets, option.label)
    for allergy_id in allergy_ids:
        option = _ALLERGY_BY_ID[allergy_id]
        if option.enum_key is None:
            _append_unique(custom_restrictions, option.label)

    profile.update(
        selection_provenance_version=1,
        diet_style_ids=diet_style_ids,
        allergy_ids=allergy_ids,
        additional_restriction_ids=additional_restrictions,
        additional_medical_constraints=additional_constraints,
        condition_severity_levels=severity_map,
        health_condition_ids=condition_ids,
        preferences=_derived_preferences(diet_style_ids),
        restrictions=restrictions,
        medical_condition_id=condition_ids[0] if condition_ids else None,
        medical_constraints=constraints,
        severity_level=max(severity_map.values()) if severity_map else None,
        custom_diet_styles=custom_diets,
        custom_restrictions=custom_restrictions,
    )
    profile["preference_strictness"] = _default_preference_strictness(
        profile["preferences"]
    )
    profile["restriction_handling"] = _default_restriction_handling(
        restrictions
    )
    return profile


def profile_has_content(profile: dict[str, Any]) -> bool:
    data = normalize_profile_data(profile)
    return any(
        bool(data.get(key))
        for key in (
            "preferences",
            "restrictions",
            "avoid_ingredients",
            "notes",
            "medical_condition_id",
            "medical_constraints",
            "activity_level",
            "cuisine_preferences",
            "health_condition_ids",
            "custom_health_conditions",
            "custom_diet_styles",
            "custom_restrictions",
            "custom_cuisines",
        )
    )


def _resolve_many(
    values: list[str],
    options: tuple[DietaryOption, ...],
) -> list[tuple[str, DietaryOption | None]]:
    resolved: list[tuple[str, DietaryOption | None]] = []
    for raw in split_values(values):
        if _is_none(raw):
            resolved.append((raw, DietaryOption("None of these", NONE_ID)))
            continue
        option = _resolve_one(raw, options)
        resolved.append((raw, option))
    return resolved


def _resolve_one(raw: str, options: tuple[DietaryOption, ...]) -> DietaryOption | None:
    normalized = _normalize_key(raw)
    aliases = {normalized, _ALIASES.get(normalized, normalized)}
    for option in options:
        keys = {
            _normalize_key(option.label),
            _normalize_key(option.id),
        }
        if option.enum_key:
            keys.add(_normalize_key(option.enum_key))
        if keys & aliases:
            return option
    return None


def _find_options(text: str, options: tuple[DietaryOption, ...]) -> list[str]:
    lowered = str(text or "").lower().replace("’", "'")
    matches: list[str] = []
    for option in sorted(options, key=lambda item: len(item.label), reverse=True):
        if option.id == NONE_ID:
            continue
        if any(_contains_term(lowered, term) for term in _option_terms(option)):
            _append_unique(matches, option.label)
    return matches


def _option_terms(option: DietaryOption) -> list[str]:
    terms = [option.label, option.id.replace("_", " ")]
    if option.enum_key:
        terms.append(_split_camel(option.enum_key))

    for part in re.split(r"[/()]", option.label):
        part = part.strip()
        if part:
            terms.append(part)

    for alias, target in _ALIASES.items():
        if target == option.id:
            terms.append(alias)
    return _dedupe_strings(terms)


def _contains_term(text: str, term: str) -> bool:
    normalized_term = _normalize_key(term)
    if not normalized_term:
        return False
    words = re.findall(r"[a-z0-9]+", _split_camel(term).lower())
    if not words:
        return False
    if len(words) == 1 and words[0] == normalized_term:
        pattern = rf"\b{re.escape(words[0])}\b"
    else:
        pattern = r"\b" + r"[-_\s/()]*".join(re.escape(word) for word in words) + r"\b"
    for match in re.finditer(pattern, text):
        if not _is_negated_match(text, match.start()):
            return True
    # Compact aliases such as "irritablebowelsyndrome" should match the
    # spaced phrase, while short terms such as "fish" must not match
    # "shellfish".
    if len(normalized_term) > 6 and normalized_term in _normalize_key(text):
        return not _has_negated_compact_match(text, words)
    return False


def _is_negated_match(text: str, start: int) -> bool:
    prefix = text[max(0, start - 28):start]
    return bool(
        re.search(
            r"(?:\b(?:no|not|without|never|denies|deny)\s+(?:a\s+|an\s+|any\s+)?|(?:^|\W)non[-\s]*)$",
            prefix,
        )
    )


def _has_negated_compact_match(text: str, words: list[str]) -> bool:
    if not words:
        return False
    loose_term = r"[-_\s/()]*".join(re.escape(word) for word in words)
    pattern = rf"\b(?:no|not|without|never|denies|deny)\s+(?:a\s+|an\s+|any\s+)?{loose_term}\b"
    return bool(re.search(pattern, text))


def _extract_avoid_ingredients(text: str) -> list[str]:
    lowered = str(text or "").lower().replace("’", "'")
    ingredients: list[str] = []
    patterns = (
        r"(?:avoid(?:s|ing)?|can't eat|cannot eat|stay away from|no)\s+([^.;]+)",
    )
    for pattern in patterns:
        for match in re.finditer(pattern, lowered):
            phrase = match.group(1)
            phrase = re.split(
                r"\b(?:because|but|mostly|mainly|usually|typically|likes?|prefers?|and i|and we|and likes?|and prefers?|with|while|for breakfast|for lunch|for dinner)\b",
                phrase,
                maxsplit=1,
            )[0]
            for part in re.split(r",|\band\b|\bor\b|&", phrase):
                item = _clean_ingredient_candidate(part)
                if item and not _is_known_profile_term(item):
                    _append_unique(ingredients, item)
    return ingredients[:20]


def _clean_ingredient_candidate(value: str) -> str:
    item = re.sub(r"\b(?:a|an|the|any|all|foods?|ingredients?|stuff|things)\b", " ", value)
    item = re.sub(r"\s+", " ", item).strip(" -_.,")
    return item[:40].strip()


def _is_known_profile_term(value: str) -> bool:
    if not value or _is_none(value):
        return True
    generic = {"allergies", "restrictions", "conditions", "health conditions", "diet"}
    if value in generic:
        return True
    for options in (HEALTH_CONDITIONS, DIET_STYLES, ALLERGIES, ACTIVITY_LEVELS, CUISINES):
        if _resolve_one(value, options) is not None:
            return True
    return False


def _split_camel(value: str) -> str:
    return re.sub(r"(?<=[a-z])(?=[A-Z])", " ", str(value).replace("_", " "))


def _first_or_none(values: list[str]) -> str | None:
    return values[0] if values else None


def _normalize_key(value: str) -> str:
    text = str(value).strip().lower()
    text = text.replace("&", "and")
    text = text.replace("+", "plus")
    text = text.replace("’", "'")
    return re.sub(r"[^a-z0-9]+", "", text)


def _is_none(value: str) -> bool:
    return _normalize_key(value) in {"none", "noneofthese", "no", "nope", "skip"}


def _append_unique(items: list[str], value: str) -> None:
    cleaned = str(value).strip()
    if cleaned and cleaned not in items:
        items.append(cleaned)


def _dedupe_strings(values: list[str]) -> list[str]:
    result: list[str] = []
    for value in values:
        _append_unique(result, value)
    return result


def _clean_list(values: Any, *, limit: int | None = None, max_length: int | None = None) -> list[str]:
    if values is None:
        return []
    raw_values = values if isinstance(values, list) else [values]
    cleaned: list[str] = []
    for value in raw_values:
        text = str(value).strip()
        if max_length is not None:
            text = text[:max_length].strip()
        if text and text not in cleaned:
            cleaned.append(text)
        if limit is not None and len(cleaned) >= limit:
            break
    return cleaned


def _clean_map(value: Any) -> dict[str, str]:
    if not isinstance(value, dict):
        return {}
    return {
        str(key).strip(): str(item).strip()
        for key, item in value.items()
        if str(key).strip() and str(item).strip()
    }


def _clean_optional_text(value: Any, *, max_length: int) -> str | None:
    if value is None:
        return None
    text = str(value).strip()[:max_length].strip()
    return text or None


def _clamp_int(value: Any, *, default: int, minimum: int, maximum: int) -> int:
    try:
        parsed = int(value)
    except (TypeError, ValueError):
        return default
    return min(max(parsed, minimum), maximum)


def _valid_preferences(values: list[str]) -> list[str]:
    return [value for value in _clean_list(values) if value in PREFERENCE_VALUES]


def _valid_restrictions(values: list[str]) -> list[str]:
    return [value for value in _clean_list(values) if value in RESTRICTION_VALUES]


def _default_preference_strictness(preferences: list[str]) -> dict[str, str]:
    return {
        preference: "strict" if preference in {"keto", "low_fodmap"} else "moderate"
        for preference in preferences
    }


def _default_restriction_handling(restrictions: list[str]) -> dict[str, str]:
    strict_avoid = {
        "nutFree",
        "peanutFree",
        "treeNutFree",
        "shellfishFree",
        "fishFree",
        "eggFree",
        "sesameFree",
    }
    dose_dependent = {"lactoseIntolerant"}
    verification_required = {"halal", "kosher"}

    handling: dict[str, str] = {}
    for restriction in restrictions:
        if restriction in strict_avoid:
            handling[restriction] = "strictAvoid"
        elif restriction in dose_dependent:
            handling[restriction] = "doseDependent"
        elif restriction in verification_required:
            handling[restriction] = "verificationRequired"
        else:
            handling[restriction] = "ingredientsOnly"
    return handling
