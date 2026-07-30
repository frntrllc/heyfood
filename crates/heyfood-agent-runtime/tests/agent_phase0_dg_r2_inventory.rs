use std::collections::BTreeSet;

use serde_json::Value;

const INVENTORY: &str = include_str!(
    "../../../docs/release-evidence/agent-native-phase0/dg-r2-dispatch-inventory.json"
);
const SERVICE_API: &str = include_str!("../src/service_api.rs");
const REGISTRATION: &str = include_str!("../src/registration.rs");
const RUNTIME: &str = include_str!("../src/lib.rs");

fn inventory() -> Value {
    serde_json::from_str(INVENTORY).unwrap()
}

fn source(path: &str) -> &'static str {
    match path {
        "crates/heyfood-agent-runtime/src/service_api.rs" => SERVICE_API,
        "crates/heyfood-agent-runtime/src/registration.rs" => REGISTRATION,
        "crates/heyfood-agent-runtime/src/lib.rs" => RUNTIME,
        other => panic!("unrecognized DG-R2 source file {other}"),
    }
}

#[test]
fn inventory_covers_the_exact_current_mutating_and_post_as_read_routes() {
    let actual = inventory()["dispatches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            format!(
                "{} {}",
                row["method"].as_str().unwrap(),
                row["path"].as_str().unwrap()
            )
        })
        .collect::<BTreeSet<_>>();
    let expected = BTreeSet::from([
        "DELETE /v1/channel/links/{link_id}".into(),
        "DELETE /v1/integrations/oura".into(),
        "DELETE /v1/menu/watch/{watch_id}".into(),
        "POST /v1/agent/converse".into(),
        "POST /v1/audio/transcriptions".into(),
        "POST /v1/auth/device/revoke".into(),
        "POST /v1/auth/session/refresh".into(),
        "POST /v1/auth/session/revoke".into(),
        "POST /v1/channel/oauth/cli/reauthorizations".into(),
        "POST /v1/channel/oauth/cli/reauthorizations/{stage_id}/abort".into(),
        "POST /v1/channel/oauth/cli/reauthorizations/{stage_id}/promote".into(),
        "POST /v1/channel/oauth/cli/session".into(),
        "POST /v1/channel/oauth/device/authorize".into(),
        "POST /v1/channel/oauth/device/token".into(),
        "POST /v1/channel/oauth/token".into(),
        "POST /v1/channel/tools/explain_item".into(),
        "POST /v1/grocery/confirm".into(),
        "POST /v1/grocery/exclusions".into(),
        "POST /v1/grocery/exclusions/remove".into(),
        "POST /v1/grocery/items".into(),
        "POST /v1/grocery/items/remove".into(),
        "POST /v1/grocery/items/state".into(),
        "POST /v1/integrations/authorize".into(),
        "POST /v1/integrations/oura/sync".into(),
        "POST /v1/menu/watch".into(),
        "POST /v1/profile/consent".into(),
        "PUT /v1/profile/sync".into(),
    ]);

    assert_eq!(actual, expected);
}

#[test]
fn every_dispatch_is_unique_complete_and_anchored_in_source() {
    let document = inventory();
    let rows = document["dispatches"].as_array().unwrap();
    let mut ids = BTreeSet::new();

    for row in rows {
        let id = row["id"].as_str().unwrap();
        assert!(ids.insert(id), "duplicate DG-R2 id {id}");
        let method = row["method"].as_str().unwrap();
        assert!(matches!(method, "POST" | "PUT" | "DELETE"));
        for field in [
            "path",
            "reachability",
            "operation",
            "client_rule",
            "server_replay_contract",
            "reconciliation",
            "source",
            "source_anchor",
        ] {
            assert!(
                row[field].as_str().is_some_and(|value| !value.is_empty()),
                "{id} is missing {field}"
            );
        }
        let source_text = source(row["source"].as_str().unwrap());
        let anchor = row["source_anchor"].as_str().unwrap();
        assert!(
            source_text.contains(anchor),
            "{id} source anchor is stale: {anchor}"
        );
        for additional in row
            .get("additional_source_anchors")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let additional = additional.as_str().unwrap();
            assert!(
                RUNTIME.contains(additional)
                    || SERVICE_API.contains(additional)
                    || REGISTRATION.contains(additional),
                "{id} additional source anchor is stale: {additional}"
            );
        }
        assert!(
            row["evidence"]
                .as_array()
                .is_some_and(|value| !value.is_empty())
        );
        assert!(row["blockers"].is_array());
    }
}

#[test]
fn summary_and_global_no_blind_retry_rules_are_self_consistent() {
    let document = inventory();
    let rows = document["dispatches"].as_array().unwrap();
    let deferred = rows
        .iter()
        .filter(|row| {
            row["reachability"]
                .as_str()
                .unwrap()
                .starts_with("compiled_deferred")
        })
        .count();
    let blockers = rows
        .iter()
        .filter(|row| !row["blockers"].as_array().unwrap().is_empty())
        .count();

    assert_eq!(
        document["summary"]["dispatches_total"].as_u64().unwrap() as usize,
        rows.len()
    );
    assert_eq!(
        document["summary"]["compiled_deferred"].as_u64().unwrap() as usize,
        deferred
    );
    assert_eq!(
        document["summary"]["public_or_feature_reachable"]
            .as_u64()
            .unwrap() as usize,
        rows.len() - deferred
    );
    assert_eq!(
        document["summary"]["rows_with_open_server_contract_blockers"]
            .as_u64()
            .unwrap() as usize,
        blockers
    );
    assert_eq!(
        document["rules"]["blind_retry_after_dispatch"],
        Value::Bool(false)
    );
    assert_eq!(
        document["rules"]["x_request_id_is_idempotency_authority"],
        Value::Bool(false)
    );
    assert_eq!(
        document["rules"]["agent_mutation_fallback"],
        Value::Bool(false)
    );
    assert_eq!(
        document["summary"]["phase_exit_authorized"],
        Value::Bool(false)
    );
}
