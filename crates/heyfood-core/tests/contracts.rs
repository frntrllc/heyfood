use heyfood_core::{
    AccountId, AgentEvent, BrowserUrl, ClientConfig, ClientError, ContextFingerprint,
    CredentialVersion, GroceryCapability, GroceryEntityId, GroceryListVersion,
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
    assert!(!format!("{document:?}").contains("clipboard"));
}

#[test]
fn serde_ingress_cannot_bypass_validated_domain_constructors() {
    assert!(serde_json::from_str::<ServiceUrl>(r#""http://api.hello.food""#).is_err());
    assert!(
        serde_json::from_str::<BrowserUrl>(r#""https://user:secret@auth.hello.food""#).is_err()
    );
    assert!(serde_json::from_str::<ProxyUrl>(r#""http://user:secret@proxy.example""#).is_err());
    assert!(serde_json::from_str::<AccountId>(r#"""#).is_err());
    assert!(serde_json::from_str::<GroceryEntityId>(r#""not-a-uuid""#).is_err());
    assert!(serde_json::from_str::<GroceryListVersion>("0").is_err());
    assert!(serde_json::from_str::<ContextFingerprint>(r#""unsafe/value""#).is_err());
    assert!(serde_json::from_str::<ClientError>(
        r#"{"code":"bad code","category":"usage","public_message":"safe","retryable":false,"outcome_uncertain":false}"#,
    )
    .is_err());
    assert!(serde_json::from_str::<ClientError>(
        "{\"code\":\"bad_request\",\"category\":\"usage\",\"public_message\":\"unsafe\\nmessage\",\"retryable\":false,\"outcome_uncertain\":false}",
    )
    .is_err());
    assert!(
        serde_json::from_str::<PresentationText>(
            &serde_json::to_string(
                &"x".repeat(heyfood_core::presentation::MAX_PRESENTATION_TEXT_BYTES + 1)
            )
            .unwrap()
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<PresentationDocument>(
            r#"{"schema_version":2,"title":null,"blocks":[]}"#,
        )
        .is_err()
    );
    assert!(serde_json::from_str::<ClientConfig>(
        r#"{"active_context":"","api_url":"https://api.hello.food","auth_url":"https://auth.hello.food","revision":1}"#,
    )
    .is_err());
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

    let validation = &fixture["validation"];
    for row in validation["required_text"].as_array().unwrap() {
        let result = required_text(
            row["input"].as_str().unwrap(),
            row["maximum"].as_u64().unwrap() as usize,
        );
        match row.get("result") {
            Some(expected) => assert_eq!(result.unwrap(), expected.as_str().unwrap()),
            None => assert!(result.is_err(), "fixture row must be rejected: {row}"),
        }
    }
    for row in validation["optional_text"].as_array().unwrap() {
        let result = optional_text(
            Some(row["input"].as_str().unwrap()),
            row["maximum"].as_u64().unwrap() as usize,
        )
        .unwrap();
        assert_eq!(result.as_deref(), row["result"].as_str());
    }
    for row in validation["coordinates"].as_array().unwrap() {
        let result = coordinates(
            row["latitude"].as_f64().unwrap(),
            row["longitude"].as_f64().unwrap(),
        );
        assert_eq!(result.is_ok(), row["valid"].as_bool().unwrap(), "{row}");
    }
    for row in validation["iso_dates"].as_array().unwrap() {
        let result = iso_date(row["input"].as_str().unwrap());
        assert_eq!(result.is_ok(), row["valid"].as_bool().unwrap(), "{row}");
    }
    for row in validation["choices"].as_array().unwrap() {
        let allowed = row["allowed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<Vec<_>>();
        let result = choice(row["input"].as_str().unwrap(), &allowed);
        match row.get("result") {
            Some(expected) => assert_eq!(result.unwrap(), expected.as_str().unwrap()),
            None => assert!(result.is_err(), "fixture row must be rejected: {row}"),
        }
    }
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
    let browser = BrowserUrl::parse(
        "https://auth.hello.food/authorize?state=private-state",
        NetworkPolicy::HTTPS_ONLY,
    )
    .unwrap();
    assert!(!format!("{browser:?}").contains("private-state"));
    assert!(!browser.to_string().contains("private-state"));
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

    let private = AgentEvent::Partial {
        text: "peanut allergy private".into(),
    };
    assert!(!format!("{private:?}").contains("peanut"));

    let failure = heyfood_core::AgentFailure {
        code: "private_diagnosis".into(),
        message: "private message".into(),
        retryable: false,
    };
    let debug = format!("{failure:?}");
    assert!(!debug.contains("diagnosis"));
    assert!(!debug.contains("private message"));
}
