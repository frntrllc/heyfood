from heyfood_cli.config import ConfigStore
from heyfood_cli import personality


def test_first_name_from_text_extracts_name_without_treating_diet_as_name():
    assert personality.first_name_from_text("My name is Justin and I'm keto") == "Justin"
    assert personality.first_name_from_text("I'm keto and dairy-free") is None
    assert personality.first_name_from_text("I'm dairy-free and mostly Thai") is None


def test_first_name_from_account_uses_display_name():
    assert personality.first_name_from_account({"display_name": "Justin Hambleton"}) == "Justin"
    assert personality.first_name_from_account({"display_name": "Keto Friend"}) is None


def test_cli_first_name_round_trips(tmp_path):
    store = ConfigStore(tmp_path / "config.json")

    assert personality.load_cli_first_name(store) is None
    personality.save_cli_first_name(store, "justin")

    assert personality.load_cli_first_name(store) == "Justin"


def test_first_welcome_marker(tmp_path):
    store = ConfigStore(tmp_path / "config.json")

    assert personality.should_show_first_welcome(store) is True
    personality.mark_welcomed(store)
    assert personality.should_show_first_welcome(store) is False


def test_onboarding_quip_for_thai_ibs():
    quip = personality.onboarding_quip(
        {
            "preferences": [],
            "restrictions": [],
            "health_condition_ids": ["ibs"],
            "medical_constraints": ["high_fodmap"],
            "cuisine_preferences": ["thai"],
            "avoid_ingredients": [],
        }
    )

    assert quip is not None
    assert "Thai" in quip
    assert "IBS" in quip


def test_onboarding_quip_for_keto_mexican():
    quip = personality.onboarding_quip(
        {
            "preferences": ["keto"],
            "restrictions": [],
            "health_condition_ids": [],
            "medical_constraints": [],
            "cuisine_preferences": ["mexican"],
            "avoid_ingredients": [],
        }
    )

    assert quip is not None
    assert "Keto" in quip
    assert "Mexican" in quip
