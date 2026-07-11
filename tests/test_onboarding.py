from heyfood_cli import onboarding


def test_build_profile_data_maps_fast_onboarding_fields():
    profile = onboarding.build_profile_data(
        replace=True,
        diets=["low-fodmap"],
        allergies=["peanuts"],
        conditions=["IBS"],
        avoid_ingredients=["onion"],
        activity_level="moderate",
        cuisines=["Thai"],
        notes="Prefers simple weeknight meals",
    )

    assert profile["preferences"] == ["low_fodmap"]
    assert profile["preference_strictness"] == {"low_fodmap": "strict"}
    assert profile["restrictions"] == ["peanutFree"]
    assert profile["restriction_handling"] == {"peanutFree": "strictAvoid"}
    assert profile["health_condition_ids"] == ["ibs"]
    assert profile["medical_condition_id"] == "ibs"
    assert profile["medical_constraints"] == ["high_fodmap", "carbonation"]
    assert profile["avoid_ingredients"] == ["onion"]
    assert profile["activity_level"] == "moderate"
    assert profile["cuisine_preferences"] == ["thai"]
    assert profile["notes"] == "Prefers simple weeknight meals"


def test_build_profile_data_preserves_existing_when_field_not_answered():
    existing = onboarding.build_profile_data(
        replace=True,
        diets=["vegan"],
        allergies=["sesame"],
        conditions=["GERD"],
    )

    profile = onboarding.build_profile_data(
        existing=existing,
        cuisines=["Mexican"],
    )

    assert profile["preferences"] == ["vegan"]
    assert profile["restrictions"] == ["sesameFree"]
    assert profile["health_condition_ids"] == ["gerd"]
    assert profile["cuisine_preferences"] == ["mexican"]


def test_build_profile_data_handles_celiac_as_gluten_restriction():
    profile = onboarding.build_profile_data(
        replace=True,
        conditions=["celiac"],
    )

    assert profile["health_condition_ids"] == ["celiac"]
    assert profile["medical_condition_id"] == "celiac"
    assert "gluten" in profile["medical_constraints"]
    assert "glutenFree" in profile["restrictions"]


def test_low_fodmap_diet_adds_evaluation_constraint():
    profile = onboarding.build_profile_data(
        replace=True,
        diets=["low fodmap"],
    )

    assert profile["preferences"] == ["low_fodmap"]
    assert profile["medical_constraints"] == ["high_fodmap"]


def test_none_clears_avoid_ingredients():
    profile = onboarding.build_profile_data(
        replace=True,
        avoid_ingredients=["none"],
    )

    assert profile["avoid_ingredients"] == []


def test_parse_profile_text_extracts_obvious_profile_fields():
    parsed = onboarding.parse_profile_text(
        "I'm keto, dairy-free, light activity, and mostly Thai food."
    )

    assert parsed["diets"] == ["Dairy-free", "Keto"]
    assert parsed["allergies"] == ["Milk / dairy"]
    assert parsed["conditions"] == []
    assert parsed["activity_level"] == "Light activity"
    assert parsed["cuisines"] == ["Thai"]


def test_parse_profile_text_extracts_conditions_and_avoid_ingredients():
    parsed = onboarding.parse_profile_text(
        "My wife has irritable bowel syndrome and we avoid onion and garlic."
    )

    assert parsed["conditions"] == ["IBS"]
    assert parsed["avoid_ingredients"] == ["onion", "garlic"]


def test_parse_profile_text_handles_no_or_avoid_list_without_swallowing_preferences():
    parsed = onboarding.parse_profile_text(
        "I have type 2 diabetes and high blood pressure, no onion or garlic, mostly Mediterranean food."
    )

    assert set(parsed["conditions"]) == {"Type 2 diabetes", "Hypertension / high blood pressure"}
    assert parsed["avoid_ingredients"] == ["onion", "garlic"]
    assert parsed["cuisines"] == ["Mediterranean"]


def test_parse_profile_text_respects_negated_conditions():
    parsed = onboarding.parse_profile_text(
        "I'm gluten intolerant and lactose intolerant, but not celiac."
    )

    assert "Celiac disease" not in parsed["conditions"]
    assert "Gluten (non-celiac sensitivity)" in parsed["allergies"]
    assert "Lactose intolerance" in parsed["allergies"]


def test_parse_profile_text_captures_sensory_food_needs():
    parsed = onboarding.parse_profile_text(
        "My son has ARFID and autism, avoids crunchy foods, and likes plain pasta."
    )

    assert parsed["conditions"] == ["Autism / sensory food needs", "ARFID"]
    assert parsed["avoid_ingredients"] == ["crunchy"]


def test_parse_profile_text_does_not_infer_fish_from_shellfish():
    parsed = onboarding.parse_profile_text("I have a shellfish allergy.")

    assert parsed["allergies"] == ["Shellfish"]


def test_parse_profile_text_marks_explicit_negative_sections_as_answered():
    parsed = onboarding.parse_profile_text(
        "I follow no special diet, have no food allergies and no health conditions, "
        "have no ingredients to avoid, and eat any cuisine."
    )

    assert parsed["diets"] == ["none"]
    assert parsed["allergies"] == ["none"]
    assert parsed["conditions"] == ["none"]
    assert parsed["avoid_ingredients"] == ["none"]
    assert parsed["cuisines"] == ["none"]
    assert set(parsed["answered_sections"]) >= {
        "diets",
        "allergies",
        "conditions",
        "avoid_ingredients",
        "cuisines",
    }


def test_unknown_labels_are_kept_as_custom_values():
    profile = onboarding.build_profile_data(
        replace=True,
        diets=["Blue Zone-ish"],
        allergies=["mango"],
        conditions=["Mystery condition"],
        cuisines=["Martian"],
    )

    assert profile["custom_diet_styles"] == ["Blue Zone-ish"]
    assert profile["custom_restrictions"] == ["mango"]
    assert profile["custom_health_conditions"] == ["Mystery condition"]
    assert profile["custom_cuisines"] == ["Martian"]


def test_profile_emits_authoritative_v5_selection_sources():
    profile = onboarding.build_profile_data(
        replace=True,
        diets=["gluten-free", "low-sodium"],
        allergies=["wheat", "lactose"],
        conditions=["celiac", "ibs"],
        severity_level=4,
    )

    assert profile["selection_provenance_version"] == 1
    assert profile["diet_style_ids"] == ["gluten_free", "low_sodium"]
    assert profile["allergy_ids"] == ["wheat", "lactose"]
    assert profile["health_condition_ids"] == ["celiac", "ibs"]
    assert profile["condition_severity_levels"] == {"celiac": 4, "ibs": 4}
    assert profile["severity_level"] == 4
    assert profile["restrictions"] == ["glutenFree", "lactoseIntolerant"]
    assert profile["medical_constraints"] == [
        "gluten",
        "high_fodmap",
        "carbonation",
        "high_sodium",
    ]


def test_legacy_flat_profile_migrates_to_sources_and_residuals():
    profile = onboarding.normalize_profile_data(
        {
            "preferences": ["vegan"],
            "restrictions": ["dairyFree", "nutFree", "halal"],
            "medical_condition_id": "ibs",
            "medical_constraints": ["carbonation", "doctor_protocol"],
            "custom_diet_styles": ["Low-sodium", "family tradition"],
            "severity_level": 4,
        }
    )

    assert profile["diet_style_ids"] == ["vegan", "low_sodium", "halal"]
    assert profile["allergy_ids"] == ["dairy"]
    assert profile["health_condition_ids"] == ["ibs"]
    assert profile["additional_restriction_ids"] == ["nutFree"]
    assert profile["additional_medical_constraints"] == ["doctor_protocol"]
    assert profile["custom_diet_styles"] == [
        "family tradition",
        "Low-sodium",
        "Halal",
    ]
    assert profile["restrictions"] == ["halal", "dairyFree", "nutFree"]
    assert profile["medical_constraints"] == [
        "high_fodmap",
        "carbonation",
        "high_sodium",
        "doctor_protocol",
    ]


def test_category_clears_remove_only_last_source_and_preserve_unrelated_data():
    profile = onboarding.build_profile_data(
        existing={
            "restrictions": ["nutFree"],
            "medical_constraints": ["doctor_protocol"],
        },
        diets=["gluten-free"],
        allergies=["wheat"],
        conditions=["celiac", "ibs"],
        avoid_ingredients=["onion"],
        activity_level="moderate",
        cuisines=["thai"],
    )

    diet_cleared = onboarding.build_profile_data(existing=profile, diets=[])
    assert diet_cleared["diet_style_ids"] == []
    assert "glutenFree" in diet_cleared["restrictions"]

    allergy_cleared = onboarding.build_profile_data(
        existing=diet_cleared,
        allergies=[],
    )
    assert allergy_cleared["allergy_ids"] == []
    assert "glutenFree" in allergy_cleared["restrictions"]

    condition_cleared = onboarding.build_profile_data(
        existing=allergy_cleared,
        conditions=[],
    )
    assert condition_cleared["health_condition_ids"] == []
    assert condition_cleared["condition_severity_levels"] == {}
    assert condition_cleared["severity_level"] is None
    assert condition_cleared["restrictions"] == ["nutFree"]
    assert condition_cleared["medical_constraints"] == ["doctor_protocol"]
    assert condition_cleared["avoid_ingredients"] == ["onion"]
    assert condition_cleared["activity_level"] == "moderate"
    assert condition_cleared["cuisine_preferences"] == ["thai"]


def test_explicit_empty_v5_sources_do_not_reinfer_stale_flat_values():
    profile = onboarding.normalize_profile_data(
        {
            "selection_provenance_version": 1,
            "diet_style_ids": [],
            "allergy_ids": [],
            "health_condition_ids": [],
            "additional_restriction_ids": [],
            "additional_medical_constraints": [],
            "preferences": ["keto"],
            "restrictions": ["glutenFree"],
            "medical_constraints": ["gluten"],
            "medical_condition_id": "celiac",
            "severity_level": 5,
        }
    )

    assert profile["preferences"] == []
    assert profile["restrictions"] == []
    assert profile["medical_constraints"] == []
    assert profile["medical_condition_id"] is None
    assert profile["severity_level"] is None


def test_severity_requires_a_condition_and_applies_to_all_selected_conditions():
    try:
        onboarding.build_profile_data(replace=True, severity_level=4)
    except ValueError as exc:
        assert "requires at least one health condition" in str(exc)
    else:
        raise AssertionError("severity without a condition must fail")

    profile = onboarding.build_profile_data(
        replace=True,
        conditions=["celiac", "ibs"],
        severity_level=5,
    )
    assert profile["condition_severity_levels"] == {"celiac": 5, "ibs": 5}


def test_avoid_cuisine_and_activity_clear_independently():
    profile = onboarding.build_profile_data(
        replace=True,
        diets=["keto"],
        avoid_ingredients=["onion"],
        cuisines=["thai"],
        activity_level="moderate",
    )

    avoid_cleared = onboarding.build_profile_data(
        existing=profile,
        avoid_ingredients=[],
    )
    assert avoid_cleared["avoid_ingredients"] == []
    assert avoid_cleared["cuisine_preferences"] == ["thai"]
    assert avoid_cleared["activity_level"] == "moderate"

    cuisine_cleared = onboarding.build_profile_data(
        existing=avoid_cleared,
        cuisines=[],
    )
    assert cuisine_cleared["cuisine_preferences"] == []
    assert cuisine_cleared["activity_level"] == "moderate"

    activity_cleared = onboarding.build_profile_data(
        existing=cuisine_cleared,
        activity_level="none",
    )
    assert activity_cleared["activity_level"] is None
    assert activity_cleared["diet_style_ids"] == ["keto"]


def test_replace_discards_the_complete_existing_graph():
    existing = onboarding.build_profile_data(
        replace=True,
        diets=["vegan"],
        allergies=["peanuts"],
        conditions=["ibs"],
        avoid_ingredients=["onion"],
        cuisines=["thai"],
        activity_level="moderate",
        notes="old profile",
    )

    replaced = onboarding.build_profile_data(
        existing=existing,
        replace=True,
        diets=["keto"],
    )

    assert replaced["diet_style_ids"] == ["keto"]
    assert replaced["allergy_ids"] == []
    assert replaced["health_condition_ids"] == []
    assert replaced["avoid_ingredients"] == []
    assert replaced["cuisine_preferences"] == []
    assert replaced["activity_level"] is None
    assert replaced["notes"] is None
    assert replaced["severity_level"] is None
