use std::time::Duration;

use heyfood_agent_runtime::{CliAuthContext, HttpDeadlines, HttpService};
use heyfood_application::{
    CapabilitySnapshot, ReadDietCatalog, ReadDietDetail, RegistrationAvailability,
};
use heyfood_core::{
    AccountId, CredentialVersion, DietCapability, DietDetailStatus, GroceryCapability,
    NetworkPolicy, OperationId, SensitiveString, ServiceUrl, SessionCredentials,
};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

fn capabilities(diet: DietCapability) -> CapabilitySnapshot {
    CapabilitySnapshot {
        schema_version: 1,
        registration: RegistrationAvailability::Disabled,
        profile_readiness: true,
        loopback_pkce: true,
        device_code: true,
        grocery: GroceryCapability::Unavailable,
        diet,
    }
}

fn credentials() -> SessionCredentials {
    SessionCredentials::from_unix_expiry(
        AccountId::parse("diet-runtime-test").unwrap(),
        SensitiveString::new("session-access"),
        SensitiveString::new("session-refresh"),
        CredentialVersion::new(1),
        4_102_444_800,
    )
    .unwrap()
}

async fn fixture_service() -> (TcpListener, HttpService) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = ServiceUrl::parse(
        &format!("http://{}/", listener.local_addr().unwrap()),
        NetworkPolicy::DEVELOPMENT,
    )
    .unwrap();
    let service = HttpService::new(
        base,
        NetworkPolicy::DEVELOPMENT,
        HttpDeadlines {
            connect: Duration::from_secs(1),
            request: Duration::from_secs(2),
            transcription: Duration::from_secs(2),
            pool_idle: Duration::from_secs(1),
            sse_inactivity: Duration::from_secs(2),
        },
    )
    .unwrap()
    .with_cli_auth(
        CliAuthContext::new(
            "diet-device",
            SensitiveString::new("channel-access"),
            Some(SensitiveString::new("app-key")),
        )
        .unwrap(),
    );
    (listener, service)
}

async fn read_request(socket: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    loop {
        let mut chunk = [0_u8; 1024];
        let count = socket.read(&mut chunk).await.unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.windows(4).any(|part| part == b"\r\n\r\n") {
            return String::from_utf8(bytes).unwrap();
        }
    }
}

async fn respond(socket: &mut TcpStream, status: u16, body: &Value) {
    let bytes = serde_json::to_vec(body).unwrap();
    socket
        .write_all(
            format!(
                "HTTP/1.1 {status} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    socket.write_all(&bytes).await.unwrap();
}

fn fixture(source: &str) -> Value {
    serde_json::from_str(source).unwrap()
}

#[tokio::test]
async fn frozen_catalog_fixture_round_trips_through_the_authenticated_port() {
    let document = fixture(include_str!(
        "../../../fixtures/contracts/diet-backend/v1/fixtures/diet/catalog.json"
    ));
    let response = document["response"].clone();
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("GET /v1/diets HTTP/1.1"));
        let lower = request.to_ascii_lowercase();
        assert!(lower.contains("authorization: bearer session-access"));
        assert!(lower.contains("x-api-key: app-key"));
        respond(&mut socket, 200, &response).await;
    });

    let catalog = ReadDietCatalog::new(&service)
        .execute(
            capabilities(DietCapability::V1),
            credentials(),
            OperationId::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(catalog.count, 22);
    assert_eq!(catalog.diets[2].id, "keto");
    server.await.unwrap();
}

#[tokio::test]
async fn covered_and_uncovered_detail_fixtures_preserve_their_distinct_success_states() {
    for (source, requested, expected_status) in [
        (
            include_str!(
                "../../../fixtures/contracts/diet-backend/v1/fixtures/diet/detail_covered.json"
            ),
            "mediterranean",
            DietDetailStatus::Covered,
        ),
        (
            include_str!(
                "../../../fixtures/contracts/diet-backend/v1/fixtures/diet/detail_not_covered.json"
            ),
            "keto",
            DietDetailStatus::DietNotCovered,
        ),
    ] {
        let response = fixture(source)["response"].clone();
        let (listener, service) = fixture_service().await;
        let expected_path = format!("GET /v1/diets/{requested} HTTP/1.1");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            assert!(request.starts_with(&expected_path));
            respond(&mut socket, 200, &response).await;
        });
        let detail = ReadDietDetail::new(&service)
            .execute(
                capabilities(DietCapability::V1),
                credentials(),
                OperationId::new(),
                requested.into(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(detail.detail_status, expected_status);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn detail_response_must_match_the_exact_requested_id() {
    let response = fixture(include_str!(
        "../../../fixtures/contracts/diet-backend/v1/fixtures/diet/detail_covered.json"
    ))["response"]
        .clone();
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("GET /v1/diets/keto HTTP/1.1"));
        respond(&mut socket, 200, &response).await;
    });
    let error = ReadDietDetail::new(&service)
        .execute(
            capabilities(DietCapability::V1),
            credentials(),
            OperationId::new(),
            "keto".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "diet_contract_error");
    assert!(error.details.is_none());
    server.await.unwrap();
}

#[tokio::test]
async fn detail_ids_are_exactly_path_encoded_and_typed_errors_are_bounded() {
    let mut unknown = fixture(include_str!(
        "../../../fixtures/contracts/diet-backend/v1/fixtures/diet/unknown_diet_error.json"
    ))["rest"]["expected_error"]
        .clone();
    unknown["details"]["diet_id"] = Value::from("Keto / DASH");
    unknown["details"]["accepted"][0] = Value::from("plant-based");
    let expected_details = unknown["details"].clone();
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("GET /v1/diets/Keto%20%2F%20DASH HTTP/1.1"));
        respond(&mut socket, 404, &unknown).await;
    });
    let error = ReadDietDetail::new(&service)
        .execute(
            capabilities(DietCapability::V1),
            credentials(),
            OperationId::new(),
            "Keto / DASH".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "diet_unknown_diet");
    assert_eq!(error.details, Some(Box::new(expected_details)));
    server.await.unwrap();

    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut socket).await;
        socket
            .write_all(
                b"HTTP/1.1 404 Result\r\nContent-Type: application/json\r\nContent-Length: 65537\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
    });
    let error = service
        .diet_detail(
            &capabilities(DietCapability::V1),
            &credentials(),
            OperationId::new(),
            "keto",
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "response_too_large");
    server.await.unwrap();
}

#[tokio::test]
async fn cancellation_after_safe_get_dispatch_stops_waiting_without_retry() {
    let (listener, service) = fixture_service().await;
    let (seen_tx, seen_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut socket).await;
        seen_tx.send(()).unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err(),
            "diet GET was retried"
        );
    });
    let cancellation = CancellationToken::new();
    let child = cancellation.clone();
    let advertised = capabilities(DietCapability::V1);
    let session = credentials();
    let read = service.diet_catalog(&advertised, &session, OperationId::new(), child);
    tokio::pin!(read);
    tokio::select! {
        () = async { seen_rx.await.unwrap() } => cancellation.cancel(),
        _ = &mut read => panic!("server was expected to hold the response"),
    }
    let error = read.await.unwrap_err();
    assert_eq!(error.code, "request_cancelled_after_dispatch");
    server.await.unwrap();
}

#[tokio::test]
async fn feature_disabled_fixture_maps_to_the_diet_capability_error() {
    let body = fixture(include_str!(
        "../../../fixtures/contracts/diet-backend/v1/fixtures/diet/feature_disabled_error.json"
    ))["catalog"]["expected_error"]
        .clone();
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut socket).await;
        respond(&mut socket, 404, &body).await;
    });
    let error = service
        .diet_catalog(
            &capabilities(DietCapability::V1),
            &credentials(),
            OperationId::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "diet_feature_disabled");
    server.await.unwrap();
}

#[test]
fn capability_versions_fail_closed_exactly_at_v1() {
    assert!(DietCapability::from_advertised(Some("v1")).is_usable());
    assert!(!DietCapability::from_advertised(None).is_usable());
    assert!(!DietCapability::from_advertised(Some("v2")).is_usable());
}
