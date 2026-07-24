use std::collections::BTreeSet;

use heyfood_core::{
    ActionConfirmationEnvelopeWire, AgentConfirmationCommandWire, ApplicationCapabilitiesWire,
    ConfirmationDecisionWire, GROCERY_WIRE_SCHEMA_SHA256, GroceryConfirmationId,
    GroceryConfirmationToken, GroceryEditPatch, GroceryIdempotencyKey, GroceryListWire,
    GroceryMutationProposalWire, HealthContextWire,
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
fn c3_action_confirmation_is_parsed_only_from_the_structured_result_member() {
    let document = json!({
        "text": "review",
        "structured": {
            "type": "action_confirmation",
            "envelope_version": 1,
            "confirmation_id": "00000000-0000-4000-8000-000000000001",
            "idempotency_key": "00000000-0000-4000-8000-000000000002",
            "action": "grocery_list_add_items",
            "preview": "Add onion",
            "card_form": "item_list",
            "structured_preview": {"items": [{"name": "onion"}]},
            "additive_future_field": true
        }
    });
    let envelope = ActionConfirmationEnvelopeWire::from_result_document(&document)
        .unwrap()
        .unwrap();
    let command = envelope.command(ConfirmationDecisionWire::Cancel);
    assert_eq!(
        serde_json::to_value(command).unwrap(),
        json!({
            "confirmation_id": "00000000-0000-4000-8000-000000000001",
            "idempotency_key": "00000000-0000-4000-8000-000000000002",
            "decision": "cancel"
        })
    );
    assert!(
        ActionConfirmationEnvelopeWire::from_result_document(
            &json!({"structured": {"type": "general_response"}})
        )
        .unwrap()
        .is_none()
    );
    assert!(
        ActionConfirmationEnvelopeWire::from_result_document(
            &json!({"structured": {"type": "action_confirmation"}})
        )
        .is_err()
    );
}

#[test]
fn c3_confirmation_edits_are_optional_bounded_and_lossless_on_the_wire() {
    let base = AgentConfirmationCommandWire {
        confirmation_id: GroceryConfirmationId::parse("00000000-0000-4000-8000-000000000001")
            .unwrap(),
        idempotency_key: GroceryIdempotencyKey::parse("00000000-0000-4000-8000-000000000002")
            .unwrap(),
        decision: ConfirmationDecisionWire::Accept,
        edits: None,
    };
    let base_json = serde_json::to_value(&base).unwrap();
    assert!(base_json.get("edits").is_none());

    let edits = GroceryEditPatch::new(
        serde_json::from_value(json!({
            "items": [
                {"name": "scallion greens", "quantity": 1, "unit": "bunch", "source_type": "manual"}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let edited = AgentConfirmationCommandWire {
        edits: Some(edits),
        ..base
    };
    assert_eq!(
        serde_json::to_value(edited).unwrap(),
        json!({
            "confirmation_id": "00000000-0000-4000-8000-000000000001",
            "idempotency_key": "00000000-0000-4000-8000-000000000002",
            "decision": "accept",
            "edits": {
                "items": [
                    {"name": "scallion greens", "quantity": 1, "unit": "bunch", "source_type": "manual"}
                ]
            }
        })
    );
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
