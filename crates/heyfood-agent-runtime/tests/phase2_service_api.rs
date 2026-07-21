use std::time::Duration;

use heyfood_agent_runtime::{CliAuthContext, HttpDeadlines, HttpService};
use heyfood_core::{
    AccountId, AddItemsRequestWire, ApplicationCapabilitiesWire, CredentialVersion,
    GroceryConfirmationToken, GroceryDecisionWire, GroceryEntityId, GroceryItemInputWire,
    GroceryItemStateWire, GroceryListVersion, GroceryMutationConfirmRequestWire, NetworkPolicy,
    OperationId, RemoveItemsRequestWire, SensitiveString, ServiceUrl, SessionCredentials,
    UpdateItemStateRequestWire,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

fn deadlines() -> HttpDeadlines {
    HttpDeadlines {
        connect: Duration::from_secs(1),
        request: Duration::from_secs(2),
        pool_idle: Duration::from_secs(1),
        sse_inactivity: Duration::from_secs(2),
    }
}

fn credentials() -> SessionCredentials {
    SessionCredentials::from_unix_expiry(
        AccountId::parse("phase2-account").unwrap(),
        SensitiveString::new("session-access"),
        SensitiveString::new("session-refresh"),
        CredentialVersion::new(1),
        4_102_444_800,
    )
    .unwrap()
}

fn capabilities(version: Option<&str>) -> ApplicationCapabilitiesWire {
    let mut applications = serde_json::Map::new();
    if let Some(version) = version {
        applications.insert("grocery".into(), Value::String(version.into()));
    }
    serde_json::from_value(json!({
        "schema_version": 1,
        "self_registration": {"status": "disabled", "regions": [], "identity_methods": []},
        "authorization": {"loopback_pkce": true, "device_code": true, "identity_methods": []},
        "profile_readiness": true,
        "application_capabilities": applications,
    }))
    .unwrap()
}

async fn fixture_service() -> (TcpListener, HttpService) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = ServiceUrl::parse(
        &format!("http://{}/", listener.local_addr().unwrap()),
        NetworkPolicy::DEVELOPMENT,
    )
    .unwrap();
    let service = HttpService::new(base, NetworkPolicy::DEVELOPMENT, deadlines())
        .unwrap()
        .with_cli_auth(
            CliAuthContext::new(
                "phase2-device",
                SensitiveString::new("channel-access"),
                Some(SensitiveString::new("app-key")),
            )
            .unwrap(),
        );
    (listener, service)
}

async fn read_request(socket: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 1024];
        let count = socket.read(&mut chunk).await.unwrap();
        assert!(count > 0, "peer closed before request completed");
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
        })
        .unwrap_or(0);
    while bytes.len() - header_end < content_length {
        let mut chunk = vec![0; content_length - (bytes.len() - header_end)];
        let count = socket.read(&mut chunk).await.unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&chunk[..count]);
    }
    String::from_utf8(bytes).unwrap()
}

fn request_body(request: &str) -> Value {
    serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap_or_default()).unwrap()
}

async fn respond(socket: &mut TcpStream, status: u16, body: Value) {
    let reason = if status == 200 { "OK" } else { "Failure" };
    let body = serde_json::to_vec(&body).unwrap();
    socket
        .write_all(
            format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    socket.write_all(&body).await.unwrap();
}

fn list_fixture() -> Value {
    json!({
        "id": "00000000-0000-4000-8000-000000000123",
        "title": "Grocery List",
        "state": "active",
        "version": 4,
        "items": [],
        "created_at": "2026-07-21T12:00:00Z",
        "updated_at": "2026-07-21T12:00:00Z"
    })
}

fn proposal_fixture(operation: &str) -> Value {
    json!({
        "confirmation_id": "00000000-0000-4000-8000-000000000001",
        "idempotency_key": "00000000-0000-4000-8000-000000000002",
        "operation": operation,
        "expires_at": "2026-07-21T12:05:00Z",
        "structured_preview": {"items": []},
        "preconditions": [{"type": "list_version", "expected_version": 4}],
        "confirmation_token": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    })
}

fn list_id() -> GroceryEntityId {
    GroceryEntityId::parse("00000000-0000-4000-8000-000000000123").unwrap()
}

#[tokio::test]
async fn capability_discovery_gates_typed_grocery_reads() {
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("GET /v1/auth/capabilities "));
        assert!(request.to_ascii_lowercase().contains("x-api-key: app-key"));
        respond(
            &mut socket,
            200,
            json!({
                "schema_version": 1,
                "self_registration": {"status": "disabled", "regions": [], "identity_methods": []},
                "authorization": {"loopback_pkce": true, "device_code": true, "identity_methods": []},
                "profile_readiness": true,
                "application_capabilities": {"grocery": "v1"}
            }),
        )
        .await;

        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("GET /v1/grocery/list "));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer session-access")
        );
        respond(&mut socket, 200, list_fixture()).await;
    });

    let advertised = service
        .discover_capabilities(CancellationToken::new())
        .await
        .unwrap();
    let list = service
        .grocery_list(
            &advertised,
            &credentials(),
            OperationId::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(list.version, 4);
    server.await.unwrap();
}

#[tokio::test]
async fn every_grocery_post_preserves_the_contract_payload() {
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let expected = [
            (
                "/v1/grocery/items",
                json!({
                    "list_id": "00000000-0000-4000-8000-000000000123",
                    "expected_version": 4,
                    "items": [{
                        "name": "onion",
                        "quantity": null,
                        "unit": null,
                        "package_quantity": null,
                        "note": null,
                        "intended_for": null,
                        "source_type": "manual",
                        "source_ref": null,
                        "source_detail": null
                    }]
                }),
                "add_items",
            ),
            (
                "/v1/grocery/items/remove",
                json!({
                    "list_id": "00000000-0000-4000-8000-000000000123",
                    "expected_version": 4,
                    "item_ids": ["item-1"]
                }),
                "remove_items",
            ),
            (
                "/v1/grocery/items/state",
                json!({
                    "list_id": "00000000-0000-4000-8000-000000000123",
                    "expected_version": 4,
                    "item_id": "item-1",
                    "state": "purchased"
                }),
                "update_item_state",
            ),
        ];
        for (path, body, operation) in expected {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            assert!(request.starts_with(&format!("POST {path} ")));
            assert_eq!(request_body(&request), body);
            respond(&mut socket, 200, proposal_fixture(operation)).await;
        }

        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("POST /v1/grocery/confirm "));
        assert_eq!(
            request_body(&request),
            json!({
                "confirmation_token": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "decision": "cancel"
            })
        );
        respond(
            &mut socket,
            200,
            json!({
                "status": "cancelled",
                "operation": "add_items",
                "confirmation_id": "00000000-0000-4000-8000-000000000001",
                "list": null,
                "exclusions": null
            }),
        )
        .await;
    });

    let capabilities = capabilities(Some("v1"));
    let credentials = credentials();
    let version = GroceryListVersion::new(4).unwrap();
    service
        .grocery_prepare_add(
            &capabilities,
            &credentials,
            OperationId::new(),
            &AddItemsRequestWire {
                list_id: list_id(),
                expected_version: version,
                items: vec![GroceryItemInputWire {
                    name: "onion".into(),
                    quantity: None,
                    unit: None,
                    package_quantity: None,
                    note: None,
                    intended_for: None,
                    source_type: "manual".into(),
                    source_ref: None,
                    source_detail: None,
                }],
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    service
        .grocery_prepare_remove(
            &capabilities,
            &credentials,
            OperationId::new(),
            &RemoveItemsRequestWire {
                list_id: list_id(),
                expected_version: version,
                item_ids: vec!["item-1".into()],
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    service
        .grocery_prepare_state(
            &capabilities,
            &credentials,
            OperationId::new(),
            &UpdateItemStateRequestWire {
                list_id: list_id(),
                expected_version: version,
                item_id: "item-1".into(),
                state: GroceryItemStateWire::Purchased,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let result = service
        .grocery_confirm(
            &capabilities,
            &credentials,
            OperationId::new(),
            &GroceryMutationConfirmRequestWire {
                confirmation_token: GroceryConfirmationToken::parse(
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                )
                .unwrap(),
                decision: GroceryDecisionWire::Cancel,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(matches!(
        result.status,
        heyfood_core::GroceryMutationStatusWire::Cancelled
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn mutation_status_failure_is_observed_once_and_never_retried() {
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("POST /v1/grocery/items "));
        respond(&mut socket, 503, json!({"detail": "unavailable"})).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(250), listener.accept())
                .await
                .is_err(),
            "mutation was retried"
        );
    });
    let error = service
        .grocery_prepare_add(
            &capabilities(Some("v1")),
            &credentials(),
            OperationId::new(),
            &AddItemsRequestWire {
                list_id: list_id(),
                expected_version: GroceryListVersion::new(4).unwrap(),
                items: vec![GroceryItemInputWire {
                    name: "onion".into(),
                    quantity: None,
                    unit: None,
                    package_quantity: None,
                    note: None,
                    intended_for: None,
                    source_type: "manual".into(),
                    source_ref: None,
                    source_detail: None,
                }],
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "service_unavailable");
    assert!(!error.outcome_uncertain);
    server.await.unwrap();
}

#[tokio::test]
async fn cancellation_after_post_dispatch_is_uncertain_and_never_retried() {
    let (listener, service) = fixture_service().await;
    let (dispatched, dispatched_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("POST /v1/grocery/items "));
        dispatched.send(()).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let _ = respond(&mut socket, 200, proposal_fixture("add_items")).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(250), listener.accept())
                .await
                .is_err(),
            "cancelled mutation was retried"
        );
    });
    let cancellation = CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let task = tokio::spawn(async move {
        service
            .grocery_prepare_add(
                &capabilities(Some("v1")),
                &credentials(),
                OperationId::new(),
                &AddItemsRequestWire {
                    list_id: list_id(),
                    expected_version: GroceryListVersion::new(4).unwrap(),
                    items: vec![GroceryItemInputWire {
                        name: "onion".into(),
                        quantity: None,
                        unit: None,
                        package_quantity: None,
                        note: None,
                        intended_for: None,
                        source_type: "manual".into(),
                        source_ref: None,
                        source_detail: None,
                    }],
                },
                task_cancellation,
            )
            .await
    });
    dispatched_rx.await.unwrap();
    cancellation.cancel();
    let error = task.await.unwrap().unwrap_err();
    assert!(error.outcome_uncertain);
    assert_eq!(error.code, "request_cancelled_after_dispatch");
    server.await.unwrap();
}

#[tokio::test]
async fn absent_or_unknown_grocery_capability_performs_no_network_io() {
    for version in [None, Some("v2")] {
        let (listener, service) = fixture_service().await;
        let result = service
            .grocery_list(
                &capabilities(version),
                &credentials(),
                OperationId::new(),
                CancellationToken::new(),
            )
            .await;
        let Err(error) = result else {
            panic!("unavailable Grocery capability unexpectedly reached the service");
        };
        assert!(error.code.starts_with("grocery_capability_"));
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn h1_h2_management_posts_are_provider_neutral_and_token_free() {
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let authorize = read_request(&mut socket).await;
        assert!(authorize.starts_with("POST /v1/integrations/authorize "));
        assert_eq!(
            request_body(&authorize),
            json!({"device_id": "phase2-device", "provider": "oura", "redirect_target": "cli"})
        );
        assert!(!authorize.contains("provider_token"));
        respond(
            &mut socket,
            200,
            json!({"auth_url": "https://provider.invalid/authorize?state=opaque", "provider": "oura"}),
        )
        .await;

        let (mut socket, _) = listener.accept().await.unwrap();
        let sync = read_request(&mut socket).await;
        assert!(sync.starts_with("POST /v1/integrations/oura/sync "));
        assert_eq!(sync.split("\r\n\r\n").nth(1).unwrap_or_default(), "");
        respond(
            &mut socket,
            200,
            json!({
                "provider": "oura",
                "suggested_goals": [],
                "data_period_start": null,
                "data_period_end": null
            }),
        )
        .await;

        let (mut socket, _) = listener.accept().await.unwrap();
        let disconnect = read_request(&mut socket).await;
        assert!(disconnect.starts_with("DELETE /v1/integrations/oura "));
        respond(
            &mut socket,
            200,
            json!({"provider": "oura", "status": "disconnected", "message": "Integration disconnected"}),
        )
        .await;
    });

    service
        .health_authorize_oura(&credentials(), OperationId::new(), CancellationToken::new())
        .await
        .unwrap();
    service
        .health_sync_oura(&credentials(), OperationId::new(), CancellationToken::new())
        .await
        .unwrap();
    service
        .health_disconnect_oura(&credentials(), OperationId::new(), CancellationToken::new())
        .await
        .unwrap();
    server.await.unwrap();
}
