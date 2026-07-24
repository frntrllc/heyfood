use std::time::Duration;

use heyfood_agent_runtime::{CliAuthContext, HttpDeadlines, HttpService};
use heyfood_core::{
    AccountId, AddItemsRequestWire, ApplicationCapabilitiesWire, CredentialVersion,
    ExclusionMutationRequestWire, GroceryConfirmationToken, GroceryDecisionWire, GroceryEntityId,
    GroceryItemInputWire, GroceryItemStateWire, GroceryListVersion,
    GroceryMutationConfirmRequestWire, MenuWatchCreateRequestWire, NetworkPolicy, OperationId,
    RemoveItemsRequestWire, RestaurantId, SensitiveString, ServiceUrl, SessionCredentials,
    TranscriptionPurpose, UpdateItemStateRequestWire, WatchCadenceWire, WatchHour, WatchWeekday,
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
        transcription: Duration::from_secs(2),
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

fn fixture_wav() -> Vec<u8> {
    let mut wav = vec![0_u8; 46];
    wav[..4].copy_from_slice(b"RIFF");
    wav[4..8].copy_from_slice(&38_u32.to_le_bytes());
    wav[8..12].copy_from_slice(b"WAVE");
    wav[12..16].copy_from_slice(b"fmt ");
    wav[16..20].copy_from_slice(&16_u32.to_le_bytes());
    wav[20..22].copy_from_slice(&1_u16.to_le_bytes());
    wav[22..24].copy_from_slice(&1_u16.to_le_bytes());
    wav[24..28].copy_from_slice(&16_000_u32.to_le_bytes());
    wav[28..32].copy_from_slice(&32_000_u32.to_le_bytes());
    wav[32..34].copy_from_slice(&2_u16.to_le_bytes());
    wav[34..36].copy_from_slice(&16_u16.to_le_bytes());
    wav[36..40].copy_from_slice(b"data");
    wav[40..44].copy_from_slice(&2_u32.to_le_bytes());
    wav
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
    fixture_service_with_transcription_timeout(Duration::from_secs(2)).await
}

async fn fixture_service_with_transcription_timeout(
    transcription: Duration,
) -> (TcpListener, HttpService) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = ServiceUrl::parse(
        &format!("http://{}/", listener.local_addr().unwrap()),
        NetworkPolicy::DEVELOPMENT,
    )
    .unwrap();
    let mut deadlines = deadlines();
    deadlines.transcription = transcription;
    let service = HttpService::new(base, NetworkPolicy::DEVELOPMENT, deadlines)
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
    String::from_utf8_lossy(&bytes).into_owned()
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

        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("GET /v1/grocery/exclusions "));
        respond(
            &mut socket,
            200,
            json!({"exclusions": ["pork", "raw onion"]}),
        )
        .await;
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
    let exclusions = service
        .grocery_exclusions(
            &advertised,
            &credentials(),
            OperationId::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(exclusions.exclusions, ["pork", "raw onion"]);
    server.await.unwrap();
}

#[tokio::test]
async fn optional_scope_negotiation_uses_live_authorization_metadata() {
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("GET /.well-known/oauth-authorization-server "));
        assert!(
            !request
                .to_ascii_lowercase()
                .contains("authorization: bearer")
        );
        respond(
            &mut socket,
            200,
            json!({
                "issuer": "https://auth.hello.food",
                "scopes_supported": ["profile:read", "grocery:read", "grocery:write"]
            }),
        )
        .await;
    });
    let metadata = service
        .discover_authorization_metadata(CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(metadata.scopes_supported.last().unwrap(), "grocery:write");
    server.await.unwrap();
}

#[tokio::test]
async fn menu_watch_conflicts_preserve_distinct_safe_recovery_guidance() {
    for (body, expected_code) in [
        (
            json!({
                "error_code": "forbidden",
                "message": "Explicit confirmation is required.",
                "trace_id": "trace-confirm",
                "details": {
                    "identity_verdict": "unverified",
                    "identity_confidence": 0.665,
                    "requires_confirmation": true,
                    "auto_threshold": 0.85
                }
            }),
            "menu_watch_confirmation_required",
        ),
        (
            json!({
                "error_code": "daily_limit_exceeded",
                "message": "Watch cap reached.",
                "trace_id": "trace-limit",
                "details": {"cap": 10, "current": 10}
            }),
            "menu_watch_limit_reached",
        ),
        (
            json!({
                "error_code": "invalid_request",
                "message": "A watch with this cadence already exists.",
                "trace_id": "trace-duplicate",
                "details": null
            }),
            "menu_watch_already_exists",
        ),
    ] {
        let (listener, service) = fixture_service().await;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            assert!(request.starts_with("POST /v1/menu/watch "));
            assert_eq!(
                request_body(&request),
                json!({
                    "restaurant_id": "00000000-0000-4000-8000-000000000456",
                    "cadence": {"weekday": 3, "hour": 9},
                    "notify": true,
                    "confirm_menu_url": false
                })
            );
            respond(&mut socket, 409, body).await;
            assert!(
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err(),
                "Menu Watch create was retried"
            );
        });
        let error = service
            .menu_watch_create(
                &credentials(),
                OperationId::new(),
                &MenuWatchCreateRequestWire {
                    restaurant_id: RestaurantId::parse("00000000-0000-4000-8000-000000000456")
                        .unwrap(),
                    cadence: WatchCadenceWire {
                        weekday: WatchWeekday::new(3).unwrap(),
                        hour: WatchHour::new(9).unwrap(),
                    },
                    notify: true,
                    menu_url: None,
                    confirm_menu_url: false,
                    tz: None,
                },
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, expected_code);
        assert!(!error.outcome_uncertain);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn unknown_or_unreadable_menu_watch_conflicts_remain_truthful() {
    for response in [
        "HTTP/1.1 409 Conflict\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"
            .to_owned(),
        format!(
            "HTTP/1.1 409 Conflict\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            16 * 1024 + 1
        ),
    ] {
        let (listener, service) = fixture_service().await;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = read_request(&mut socket).await;
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let error = service
            .menu_watch_create(
                &credentials(),
                OperationId::new(),
                &MenuWatchCreateRequestWire {
                    restaurant_id: RestaurantId::parse(
                        "00000000-0000-4000-8000-000000000456",
                    )
                    .unwrap(),
                    cadence: WatchCadenceWire {
                        weekday: WatchWeekday::new(3).unwrap(),
                        hour: WatchHour::new(9).unwrap(),
                    },
                    notify: false,
                    menu_url: None,
                    confirm_menu_url: false,
                    tz: None,
                },
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "menu_watch_conflict");
        assert!(!error.message.contains("confirm"));
        assert!(!error.outcome_uncertain);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn transcription_uses_bounded_multipart_channel_authority_and_validates_response() {
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("POST /v1/audio/transcriptions "));
        let lowercase = request.to_ascii_lowercase();
        assert!(lowercase.contains("authorization: bearer channel-access"));
        assert!(!lowercase.contains("authorization: bearer session-access"));
        assert!(lowercase.contains("content-type: multipart/form-data; boundary=heyfood-"));
        assert!(request.contains("name=\"purpose\"\r\n\r\nlog\r\n"));
        assert!(request.contains("name=\"language\"\r\n\r\nen-US\r\n"));
        assert!(request.contains("name=\"file\"; filename=\"audio.wav\""));
        assert!(request.contains("Content-Type: audio/wav\r\n\r\nRIFF"));
        respond(
            &mut socket,
            200,
            json!({
                "transcript": "Log oatmeal and berries",
                "duration_seconds": 1.25,
                "language": "en-US",
                "model_version": "hf-transcribe-1"
            }),
        )
        .await;
    });
    let wav = fixture_wav();
    let transcription = service
        .transcribe_audio(
            &wav,
            TranscriptionPurpose::Log,
            Some("en-US"),
            OperationId::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(transcription.transcript(), "Log oatmeal and berries");
    assert!(!format!("{transcription:?}").contains("oatmeal"));
    server.await.unwrap();
}

#[tokio::test]
async fn transcription_maps_oversize_status_to_audio_rejected() {
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("POST /v1/audio/transcriptions "));
        respond(&mut socket, 413, json!({"detail": "too large"})).await;
    });
    let wav = fixture_wav();
    let error = service
        .transcribe_audio(
            &wav,
            TranscriptionPurpose::Ask,
            None,
            OperationId::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "audio_rejected");
    assert!(!error.outcome_uncertain);
    server.await.unwrap();
}

#[tokio::test]
async fn transcription_preserves_its_frozen_error_kinds() {
    let mut observed = std::collections::BTreeSet::new();
    for (status, body_error, expected) in [
        (400, "audio_too_long", "audio_rejected"),
        (401, "invalid_token", "login_required"),
        (403, "insufficient_scope", "insufficient_scope"),
        (429, "rate_limited", "rate_limited"),
        (404, "not_found", "transcription_unavailable"),
        (
            503,
            "transcription_unavailable",
            "transcription_unavailable",
        ),
    ] {
        let (listener, service) = fixture_service().await;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            assert!(request.starts_with("POST /v1/audio/transcriptions "));
            respond(&mut socket, status, json!({"error": body_error})).await;
        });
        let error = service
            .transcribe_audio(
                &fixture_wav(),
                TranscriptionPurpose::Ask,
                None,
                OperationId::new(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, expected);
        assert!(!error.outcome_uncertain);
        observed.insert(error.code);
        server.await.unwrap();
    }
    observed.insert("transcription_contract_error");
    assert_eq!(
        observed,
        std::collections::BTreeSet::from([
            "audio_rejected",
            "insufficient_scope",
            "login_required",
            "rate_limited",
            "transcription_contract_error",
            "transcription_unavailable",
        ])
    );
}

#[tokio::test]
async fn transcription_generic_forbidden_is_not_misreported_as_missing_scope() {
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut socket).await;
        respond(
            &mut socket,
            403,
            json!({"error": "forbidden", "message": "policy denied"}),
        )
        .await;
    });
    let error = service
        .transcribe_audio(
            &fixture_wav(),
            TranscriptionPurpose::Ask,
            None,
            OperationId::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "transcription_unavailable");
    server.await.unwrap();
}

#[tokio::test]
async fn malformed_and_oversized_successes_are_contract_errors() {
    for response in [
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
            .to_owned(),
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            128 * 1024 + 1
        ),
    ] {
        let (listener, service) = fixture_service().await;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = read_request(&mut socket).await;
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let error = service
            .transcribe_audio(
                &fixture_wav(),
                TranscriptionPurpose::Ask,
                None,
                OperationId::new(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "transcription_contract_error");
        server.await.unwrap();
    }
}

#[tokio::test]
async fn disconnected_success_body_and_timeout_are_unavailable() {
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut socket).await;
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{",
            )
            .await
            .unwrap();
    });
    let disconnected = service
        .transcribe_audio(
            &fixture_wav(),
            TranscriptionPurpose::Ask,
            None,
            OperationId::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(disconnected.code, "transcription_unavailable");
    server.await.unwrap();

    let (listener, service) =
        fixture_service_with_transcription_timeout(Duration::from_millis(30)).await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut socket).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    });
    let timed_out = service
        .transcribe_audio(
            &fixture_wav(),
            TranscriptionPurpose::Ask,
            None,
            OperationId::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(timed_out.code, "transcription_unavailable");
    server.await.unwrap();
}

#[tokio::test]
async fn malformed_wav_is_rejected_before_network_dispatch() {
    let (listener, service) = fixture_service().await;
    let error = service
        .transcribe_audio(
            b"RIFF malformed WAVE",
            TranscriptionPurpose::Ask,
            None,
            OperationId::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "audio_rejected");
    assert!(
        tokio::time::timeout(Duration::from_millis(25), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn profile_consent_and_versioned_upload_preserve_the_frozen_contract() {
    let (listener, service) = fixture_service().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("POST /v1/profile/consent "));
        assert_eq!(request_body(&request), json!({"consent_version": 1}));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer session-access")
        );
        respond(
            &mut socket,
            200,
            json!({"has_consent": true, "consent_version": 1}),
        )
        .await;

        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with("PUT /v1/profile/sync "));
        assert_eq!(
            request_body(&request),
            json!({
                "member_id": "_self",
                "profile_data": {"diet_style_ids": ["vegan"]},
                "expected_version": 7,
            })
        );
        respond(
            &mut socket,
            200,
            json!({"member_id": "_self", "version": 8}),
        )
        .await;
    });

    service
        .grant_profile_consent(&credentials(), OperationId::new(), CancellationToken::new())
        .await
        .unwrap();
    let uploaded = service
        .upload_profile(
            &credentials(),
            "_self",
            &json!({"diet_style_ids": ["vegan"]}),
            Some(7),
            OperationId::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(uploaded["version"], 8);
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
            (
                "/v1/grocery/exclusions",
                json!({
                    "name": "pork",
                    "list_id": "00000000-0000-4000-8000-000000000123",
                    "expected_version": 4
                }),
                "add_exclusion",
            ),
            (
                "/v1/grocery/exclusions/remove",
                json!({
                    "name": "pork",
                    "list_id": "00000000-0000-4000-8000-000000000123",
                    "expected_version": 4
                }),
                "remove_exclusion",
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
    let exclusion = ExclusionMutationRequestWire {
        name: "pork".into(),
        list_id: list_id(),
        expected_version: version,
    };
    service
        .grocery_prepare_add_exclusion(
            &capabilities,
            &credentials,
            OperationId::new(),
            &exclusion,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    service
        .grocery_prepare_remove_exclusion(
            &capabilities,
            &credentials,
            OperationId::new(),
            &exclusion,
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
        // The cancelled client is required to drop this socket. Do not write a
        // synthetic response after cancellation: Linux may correctly report a
        // broken pipe before the no-retry assertion runs.
        drop(socket);
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
