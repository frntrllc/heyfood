from __future__ import annotations

import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SCHEMA_PATH = ROOT / "schemas" / "v1" / "heyfood-output.schema.json"
CANONICAL_STATUSES = [
    "generally_safer",
    "risky",
    "avoid",
    "unable_to_evaluate",
]


def test_machine_output_schema_is_versioned_and_covers_public_result_families():
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))

    assert schema["$schema"] == "https://json-schema.org/draft/2020-12/schema"
    assert schema["x-heyfood-schema-version"] == 1
    assert set(schema["$defs"]) >= {
        "safetyStatus",
        "safetyVerdict",
        "restaurantFit",
        "menuEvaluation",
        "recommendationRanking",
        "recipeCompatibility",
    }
    assert schema["$defs"]["safetyStatus"]["enum"] == CANONICAL_STATUSES


def test_ranking_schema_cannot_be_described_as_a_safety_verdict():
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    ranking = schema["$defs"]["recommendationRanking"]
    score = ranking["properties"]["recommendations"]["items"]["properties"]["score"]

    assert "not a safety verdict" in ranking["description"].lower()
    assert "not a probability" in score["description"].lower()
    assert "status" not in ranking["properties"]["recommendations"]["items"]["required"]


def test_schema_documentation_maps_all_five_result_families():
    documentation = (ROOT / "docs" / "JSON_SCHEMAS.md").read_text(encoding="utf-8")

    for definition in (
        "safetyVerdict",
        "restaurantFit",
        "menuEvaluation",
        "recommendationRanking",
        "recipeCompatibility",
    ):
        assert definition in documentation
    assert "neither a probability nor a safety status" in documentation
