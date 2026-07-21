use std::collections::BTreeSet;

use heyfood_core::{
    ApplicationCapabilitiesWire, GROCERY_WIRE_SCHEMA_SHA256, GroceryConfirmationToken,
    GroceryListWire, GroceryMutationProposalWire, HealthContextWire,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const GROCERY_SCHEMA: &str =
    include_str!("../../../fixtures/contracts/grocery-backend/phase-a/grocery-wire-schema.json");

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn grocery_wire_types_are_pinned_to_the_approved_schema_bytes() {
    assert_eq!(
        digest(GROCERY_SCHEMA.as_bytes()),
        GROCERY_WIRE_SCHEMA_SHA256
    );
}

#[test]
fn final_grocery_dto_inventory_matches_every_generated_model() {
    let schema: Value = serde_json::from_str(GROCERY_SCHEMA).unwrap();
    let actual = schema["models"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let expected = BTreeSet::from([
        "AddItemsRequest",
        "ExclusionMutationRequest",
        "GroceryItemView",
        "GroceryListView",
        "GroceryMutationConfirmRequest",
        "GroceryMutationProposal",
        "GroceryMutationResult",
        "ItemSourceView",
        "MemberFlag",
        "ProposedItemView",
        "RemoveItemsRequest",
        "SafetyAnnotationView",
        "UpdateItemStateRequest",
        "VersionConflictDetail",
    ]);
    assert_eq!(actual, expected);
}

#[test]
fn imported_grocery_list_fixture_deserializes_without_shape_loss() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../fixtures/contracts/grocery-backend/phase-a/fixtures/grocery/happy_path_render.json"
    ))
    .unwrap();
    let list: GroceryListWire = serde_json::from_value(fixture["list"].clone()).unwrap();
    assert_eq!(list.version, 1);
    assert_eq!(list.items.len(), 2);
    assert_eq!(serde_json::to_value(list).unwrap(), fixture["list"]);
}

#[test]
fn capability_contract_is_strict_and_grocery_version_is_explicit() {
    let value = json!({
        "schema_version": 1,
        "self_registration": {
            "status": "disabled",
            "regions": [],
            "identity_methods": ["sms", "email"]
        },
        "authorization": {
            "loopback_pkce": true,
            "device_code": true,
            "identity_methods": ["sms", "email"]
        },
        "profile_readiness": true,
        "application_capabilities": {"grocery": "v1"}
    });
    let capabilities: ApplicationCapabilitiesWire = serde_json::from_value(value).unwrap();
    assert_eq!(capabilities.application_version("grocery"), Some("v1"));

    let mut unknown = serde_json::to_value(json!({
        "schema_version": 1,
        "self_registration": {"status": "disabled", "regions": [], "identity_methods": []},
        "authorization": {"loopback_pkce": true, "device_code": true, "identity_methods": []},
        "profile_readiness": true,
        "unexpected": true
    }))
    .unwrap();
    assert!(serde_json::from_value::<ApplicationCapabilitiesWire>(unknown.take()).is_err());
}

#[test]
fn confirmation_authority_is_redacted_and_bounded() {
    let proposal: GroceryMutationProposalWire = serde_json::from_value(json!({
        "confirmation_id": "00000000-0000-4000-8000-000000000001",
        "idempotency_key": "00000000-0000-4000-8000-000000000002",
        "operation": "add_items",
        "expires_at": "2026-07-21T12:05:00Z",
        "structured_preview": {"items": [{"name": "onion"}]},
        "preconditions": [{"type": "list_version", "expected_version": 4}],
        "confirmation_token": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }))
    .unwrap();
    let debug = format!("{proposal:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains("onion"));
    assert!(!debug.contains("aaaaaaaa"));
    assert!(GroceryConfirmationToken::parse("short".into()).is_err());
}

#[test]
fn health_h1_context_requires_the_frozen_present_nullable_shape() {
    let context: HealthContextWire = serde_json::from_value(json!({
        "status": "not_connected",
        "provider": null,
        "stale_since": null,
        "data_freshness_hours": null,
        "sleep_avg": null,
        "readiness_avg": null,
        "activity_avg": null,
        "sleep_label": null,
        "readiness_label": null,
        "activity_label": null,
        "steps_avg": null,
        "active_calories_avg": null,
        "stress_label": null,
        "deep_sleep_label": null,
        "goals": []
    }))
    .unwrap();
    assert_eq!(context.goals.len(), 0);

    let missing_provider = json!({
        "status": "not_connected",
        "stale_since": null,
        "data_freshness_hours": null,
        "sleep_avg": null,
        "readiness_avg": null,
        "activity_avg": null,
        "sleep_label": null,
        "readiness_label": null,
        "activity_label": null,
        "steps_avg": null,
        "active_calories_avg": null,
        "stress_label": null,
        "deep_sleep_label": null,
        "goals": []
    });
    assert!(serde_json::from_value::<HealthContextWire>(missing_provider).is_err());
}
