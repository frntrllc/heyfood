use std::collections::BTreeSet;

use serde_json::Value;

fn contract() -> Value {
    serde_json::from_str(include_str!(
        "../../../tests/showcase/showcase-contract.v1.json"
    ))
    .expect("showcase contract is valid JSON")
}

#[test]
fn every_landing_showcase_stage_is_a_release_blocking_test_case() {
    let contract = contract();
    assert_eq!(contract["schema_version"], 1);
    let journeys = contract["journeys"].as_array().unwrap();
    assert_eq!(journeys.len(), 3);

    let expected = BTreeSet::from(["dinner-planner", "menu-watch", "voice-meal-log"]);
    let actual = journeys
        .iter()
        .map(|journey| journey["id"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, expected);

    let mut stage_ids = BTreeSet::new();
    for journey in journeys {
        let stages = journey["stages"].as_array().unwrap();
        assert_eq!(stages.len(), 4);
        for stage in stages {
            let qualified_id = format!(
                "{}:{}",
                journey["id"].as_str().unwrap(),
                stage["id"].as_str().unwrap()
            );
            assert!(stage_ids.insert(qualified_id), "duplicate showcase stage");
            assert!(!stage["requires"].as_array().unwrap().is_empty());
            assert!(!stage["asserts"].as_array().unwrap().is_empty());
        }
    }
    assert_eq!(stage_ids.len(), 12);
    assert_eq!(
        contract["installed_artifact_gate"]["journey_pass_rate"],
        1.0
    );
    assert_eq!(
        contract["installed_artifact_gate"]["placeholder_or_simulated_success_forbidden"],
        true
    );
}

#[test]
fn presentation_contract_covers_retained_terminal_behavior() {
    let presentation = &contract()["presentation"];
    assert_eq!(
        presentation["composer"],
        "bottom_anchored_multiline_editable_while_streaming"
    );
    assert_eq!(
        presentation["responsive_widths"],
        serde_json::json!([40, 80, 120])
    );
    assert!(
        presentation["footer_controls"]
            .as_array()
            .unwrap()
            .iter()
            .any(|key| key == "Ctrl+C")
    );
    assert!(
        presentation["structured_response"]
            .as_array()
            .unwrap()
            .iter()
            .any(|part| part == "evidence_note")
    );
}
