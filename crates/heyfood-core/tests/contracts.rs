use heyfood_core::{
    AccountId, AgentEvent, BrowserUrl, CredentialVersion, GroceryCapability,
    HealthConnectionStatus, HealthFreshness, HealthMetric, HealthProvider, NetworkPolicy,
    NoticeLevel, PresentationBlock, PresentationDocument, PresentationText, ProxyUrl,
    RefreshRequest, RefreshResult, SensitiveString, ServiceUrl, ServiceUrlError,
    SessionCredentials, choice, coordinates, iso_date, optional_text, required_text,
};
use time::OffsetDateTime;

fn credentials(account: &str, version: u64) -> SessionCredentials {
    SessionCredentials {
        account_id: AccountId::parse(account).unwrap(),
        access_token: SensitiveString::new(format!("access-{version}")),
        refresh_token: SensitiveString::new(format!("refresh-{version}")),
        version: CredentialVersion::new(version),
        expires_at: OffsetDateTime::UNIX_EPOCH,
    }
}

#[test]
fn presentation_documents_strip_terminal_controls_and_are_bounded() {
    let text = PresentationText::from_untrusted("hello\u{1b}]52;clipboard\u{7} world").unwrap();
    assert_eq!(text.as_str(), "hello]52;clipboard world");
    let document = PresentationDocument::new(
        Some(PresentationText::from_untrusted("Result").unwrap()),
        vec![PresentationBlock::Notice {
            level: NoticeLevel::Information,
            text,
        }],
    )
    .unwrap();
    let encoded = serde_json::to_string(&document).unwrap();
    assert!(!encoded.contains("\\u001b"));
    assert_eq!(document.schema_version, 1);
}

#[test]
fn generic_grocery_and_health_contracts_fail_closed_and_redact_values() {
    assert_eq!(
        GroceryCapability::from_advertised(None),
        GroceryCapability::Unavailable
    );
    assert_eq!(
        GroceryCapability::from_advertised(Some("v1")),
        GroceryCapability::V1
    );
    assert!(!GroceryCapability::from_advertised(Some("v2")).is_usable());

    let freshness = HealthFreshness {
        status: HealthConnectionStatus::Stale,
        provider: Some(HealthProvider::Oura),
        data_freshness_hours: Some(48),
        stale_since: Some("2026-07-18T00:00:00Z".into()),
    };
    assert_eq!(freshness.status, HealthConnectionStatus::Stale);
    let metric = HealthMetric {
        key: "sleep_avg".into(),
        value: SensitiveString::new("private-value"),
        label: Some(SensitiveString::new("private-label")),
    };
    let debug = format!("{metric:?}");
    assert!(!debug.contains("private-value"));
    assert!(!debug.contains("private-label"));
}

#[test]
fn standard_forward_proxies_are_distinct_from_service_transport_policy() {
    assert!(ProxyUrl::parse("http://proxy.example:8080").is_ok());
    assert!(ProxyUrl::parse("https://proxy.example").is_ok());
    assert!(ProxyUrl::parse("http://user:password@proxy.example").is_err());
    assert!(ServiceUrl::parse("http://proxy.example", NetworkPolicy::DEVELOPMENT).is_err());
}

#[test]
fn phase1_core_validation_matches_the_frozen_python_oracle() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/compat/phase1-core-python-0.4.0.json"
    ))
    .unwrap();
    assert_eq!(
        fixture["oracle"]["commit_sha"],
        "73494a57468dac83b4904ce6c390e36926f5c6fe"
    );

    assert_eq!(required_text("  thai  ", 20).unwrap(), "thai");
    assert!(required_text(" ", 20).is_err());
    assert_eq!(optional_text(Some("   "), 20).unwrap(), None);
    assert_eq!(
        optional_text(Some(" dinner "), 20).unwrap().as_deref(),
        Some("dinner")
    );
    assert_eq!(coordinates(-90.0, -180.0).unwrap(), (-90.0, -180.0));
    assert_eq!(coordinates(90.0, 180.0).unwrap(), (90.0, 180.0));
    assert!(coordinates(91.0, 0.0).is_err());
    assert!(coordinates(0.0, 181.0).is_err());
    assert_eq!(iso_date("2026-07-10").unwrap(), "2026-07-10");
    assert!(iso_date("2026-02-30").is_err());
    assert!(iso_date("07/10/2026").is_err());
    assert_eq!(
        choice("Dinner", &["breakfast", "dinner"]).unwrap(),
        "dinner"
    );
    assert!(choice("brunch", &["breakfast", "dinner"]).is_err());
}

#[test]
fn service_urls_fail_closed_except_for_exact_development_loopback() {
    assert!(ServiceUrl::parse("https://api.hello.food", NetworkPolicy::HTTPS_ONLY).is_ok());
    assert!(ServiceUrl::parse("http://localhost:8000", NetworkPolicy::DEVELOPMENT).is_ok());
    assert!(ServiceUrl::parse("http://127.0.0.1:8000", NetworkPolicy::DEVELOPMENT).is_ok());
    assert!(ServiceUrl::parse("http://[::1]:8000", NetworkPolicy::DEVELOPMENT).is_ok());

    for unsafe_url in [
        "http://api.hello.food",
        "http://localhost.evil.example",
        "http://127.0.0.1.evil.example",
        "http://127.0.0.2:8000",
    ] {
        assert_eq!(
            ServiceUrl::parse(unsafe_url, NetworkPolicy::DEVELOPMENT),
            Err(ServiceUrlError::InsecureTransport)
        );
    }
    assert_eq!(
        ServiceUrl::parse(
            "https://user:pass@api.hello.food",
            NetworkPolicy::HTTPS_ONLY
        ),
        Err(ServiceUrlError::EmbeddedCredentials)
    );
    assert_eq!(
        ServiceUrl::parse("https://api.hello.food?token=x", NetworkPolicy::HTTPS_ONLY),
        Err(ServiceUrlError::Query)
    );
    assert!(
        BrowserUrl::parse(
            "https://auth.hello.food/authorize?state=opaque",
            NetworkPolicy::HTTPS_ONLY
        )
        .is_ok()
    );
}

#[test]
fn secrets_are_redacted_and_refresh_must_rotate_version_for_same_account() {
    let old = credentials("account-1", 4);
    assert!(!format!("{old:?}").contains("access-4"));
    assert_eq!(old.access_token.to_string(), "[REDACTED]");

    let request = RefreshRequest::from(&old);
    assert!(RefreshResult::validated(&request, credentials("account-1", 5)).is_ok());
    assert_eq!(
        RefreshResult::validated(&request, credentials("account-1", 4)),
        Err("refresh response credential version must advance")
    );
    assert_eq!(
        RefreshResult::validated(&request, credentials("account-2", 5)),
        Err("refresh response account does not match the request")
    );
}

#[test]
fn agent_event_wire_names_cover_the_vertical_stream() {
    let events = [
        AgentEvent::Thinking {
            stage: Some("planning".into()),
            message: None,
        },
        AgentEvent::Progress {
            message: "menu loaded".into(),
            current: Some(1),
            total: Some(2),
        },
        AgentEvent::Partial { text: "hi".into() },
        AgentEvent::Choices {
            choices: vec![],
            allow_multiple: false,
        },
        AgentEvent::Result {
            document: serde_json::json!({"message": "done"}),
            conversation_id: Some("conversation-1".into()),
        },
        AgentEvent::Error {
            error: heyfood_core::AgentFailure {
                code: "failed".into(),
                message: "failed".into(),
                retryable: false,
            },
        },
    ];

    let names: Vec<_> = events
        .iter()
        .map(|event| {
            serde_json::to_value(event).unwrap()["event"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(
        names,
        [
            "thinking", "progress", "partial", "choices", "result", "error"
        ]
    );
}
