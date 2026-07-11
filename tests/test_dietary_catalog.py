from __future__ import annotations

import json
from importlib.resources import files

from heyfood_cli import onboarding


def _packaged_catalog() -> dict:
    resource = files("heyfood_cli.data").joinpath("dietary_options.json")
    return json.loads(resource.read_text(encoding="utf-8"))


def _active_ids(section: dict) -> set[str]:
    return {
        entry["id"]
        for group in ("tier1", "tier2", "options")
        for entry in section.get(group, [])
        if entry["id"] != onboarding.NONE_ID and not entry.get("deprecated", False)
    }


def test_packaged_catalog_has_versioned_canonical_categories() -> None:
    catalog = _packaged_catalog()

    assert catalog["version"] == onboarding.CANONICAL_SCHEMA_VERSION == 2
    assert set(catalog["sections"]) == {
        "health_conditions",
        "diet_style",
        "allergies",
        "ingredients_to_avoid",
        "activity_level",
        "cuisines",
    }


def test_runtime_catalogs_are_loaded_from_packaged_snapshot() -> None:
    catalog = _packaged_catalog()["sections"]
    runtime = {
        "health_conditions": onboarding.HEALTH_CONDITIONS,
        "diet_style": onboarding.DIET_STYLES,
        "allergies": onboarding.ALLERGIES,
        "activity_level": onboarding.ACTIVITY_LEVELS,
        "cuisines": onboarding.CUISINES,
    }

    for section_name, options in runtime.items():
        assert {option.id for option in options} == _active_ids(catalog[section_name])


def test_previously_cli_only_ids_are_canonical_and_keep_their_mappings() -> None:
    conditions = {option.id: option for option in onboarding.HEALTH_CONDITIONS}
    allergies = {option.id: option for option in onboarding.ALLERGIES}

    assert conditions["hypertension"].constraints == ("high_sodium",)
    assert conditions["arfid"].constraints == (
        "texture_sensitivity",
        "limited_variety",
    )
    assert conditions["autism_sensory"].constraints == ("texture_sensitivity",)
    assert allergies["lactose"].enum_key == "lactoseIntolerant"


def test_cli_supported_canonical_diet_ids_remain_typed_preferences() -> None:
    profile = onboarding.build_profile_data(
        replace=True,
        diets=["low_fodmap", "high_protein"],
    )

    assert profile["preferences"] == ["low_fodmap", "high_protein"]
    assert profile["custom_diet_styles"] == ["Low-FODMAP", "High-protein"]


def _json_round_trip(profile: dict) -> dict:
    return onboarding.normalize_profile_data(json.loads(json.dumps(profile)))


def test_every_canonical_diet_style_round_trips_by_source_id() -> None:
    for option in onboarding.DIET_STYLES:
        profile = onboarding.build_profile_data(
            replace=True,
            diets=[option.id],
        )

        assert profile["diet_style_ids"] == [option.id]
        assert _json_round_trip(profile) == profile


def test_every_canonical_allergy_round_trips_by_source_id() -> None:
    for option in onboarding.ALLERGIES:
        profile = onboarding.build_profile_data(
            replace=True,
            allergies=[option.id],
        )

        assert profile["allergy_ids"] == [option.id]
        assert _json_round_trip(profile) == profile


def test_every_canonical_condition_round_trips_with_derived_constraints() -> None:
    for option in onboarding.HEALTH_CONDITIONS:
        profile = onboarding.build_profile_data(
            replace=True,
            conditions=[option.id],
        )

        assert profile["health_condition_ids"] == [option.id]
        assert set(option.constraints) <= set(profile["medical_constraints"])
        assert profile["condition_severity_levels"] == {option.id: 3}
        assert _json_round_trip(profile) == profile


def test_every_activity_and_cuisine_id_round_trips() -> None:
    for option in onboarding.ACTIVITY_LEVELS:
        profile = onboarding.build_profile_data(
            replace=True,
            activity_level=option.id,
        )
        assert profile["activity_level"] == option.id
        assert _json_round_trip(profile) == profile

    for option in onboarding.CUISINES:
        profile = onboarding.build_profile_data(
            replace=True,
            cuisines=[option.id],
        )
        assert profile["cuisine_preferences"] == [option.id]
        assert _json_round_trip(profile) == profile


def test_source_aware_client_filters_server_compatibility_labels_from_custom_input() -> None:
    profile = onboarding.normalize_profile_data(
        {
            "selection_provenance_version": 1,
            "diet_style_ids": ["gluten_free", "low_sodium"],
            "allergy_ids": ["mustard"],
            "custom_diet_styles": ["Gluten-free", "Low-sodium", "Family diet"],
            "custom_restrictions": ["Mustard", "Family restriction"],
        }
    )

    assert profile["custom_diet_styles"] == [
        "Family diet",
        "Gluten-free",
        "Low-sodium",
    ]
    assert profile["custom_restrictions"] == [
        "Family restriction",
        "Mustard",
    ]
    assert _json_round_trip(profile) == profile
