use heyfood_core::{
    AccountId, AgentEvent, BrowserUrl, ClientConfig, ClientError, ContextFingerprint,
    CredentialVersion, FrozenGroceryPreconditions, GroceryCapability, GroceryConfirmation,
    GroceryConfirmationDecision, GroceryConfirmationId, GroceryConfirmationState, GroceryEntityId,
    GroceryErrorCode, GroceryIdempotencyKey, GroceryListVersion, GroceryValidatedEdits,
    HealthConnectionStatus, HealthFreshness, HealthFreshnessStatus, HealthMetric, HealthProvider,
    HouseholdContextHashVersion, NetworkPolicy, NoticeLevel, PresentationBlock,
    PresentationDocument, PresentationText, ProxyUrl, RefreshRequest, RefreshResult,
    SensitiveString, ServiceUrl, ServiceUrlError, SessionCredentials, choice, coordinates,
    iso_date, optional_text, required_text,
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
    assert!(
        serde_json::from_str::<GroceryConfirmationId>(r#""00000000-0000-4000-8000-00000000000A""#)
            .is_err()
    );
    assert!(
        serde_json::from_str::<GroceryIdempotencyKey>(r#""00000000-0000-4000-8000-00000000000B""#)
            .is_err()
    );
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
        status: HealthFreshnessStatus::Stale,
        provider: Some(HealthProvider::Oura),
        data_freshness_hours: Some(48),
        stale_since: Some("2026-07-18T00:00:00Z".into()),
    };
    assert_eq!(freshness.status, HealthFreshnessStatus::Stale);
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
fn frozen_c3_drives_lossless_grocery_confirmation_semantics() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/contracts/grocery-backend/c3-confirmation-contract.json"
    ))
    .unwrap();
    let request = &fixture["confirm_request"];
    assert_eq!(
        request["required"],
        serde_json::json!(["confirmation_id", "idempotency_key"])
    );
    let decision_field = &request["fields"]["decision"];
    assert_eq!(decision_field["default_when_absent"], "accept");
    assert!(
        decision_field["enum"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("cancel"))
    );
    assert!(
        request["fields"]["edits"]["type"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("object"))
    );
    assert!(fixture["precondition_descriptors"]["types"]["household_context_hash"]
        ["operands"]["hash_version"]
        .is_object());

    let confirmation_id =
        GroceryConfirmationId::parse("00000000-0000-4000-8000-000000000010").unwrap();
    let idempotency_key =
        GroceryIdempotencyKey::parse("00000000-0000-4000-8000-000000000011").unwrap();
    let preconditions = FrozenGroceryPreconditions {
        list_id: GroceryEntityId::parse("00000000-0000-4000-8000-000000000012").unwrap(),
        list_version: GroceryListVersion::new(7).unwrap(),
        context_fingerprint: ContextFingerprint::parse("abcdef0123456789").unwrap(),
        household_context_hash_version: Some(HouseholdContextHashVersion::new(2)),
    };
    let proposed = GroceryConfirmation {
        confirmation_id,
        idempotency_key,
        preconditions: preconditions.clone(),
        state: GroceryConfirmationState::Proposed,
    };

    // Released legacy clients omit `decision`; frozen C3 normalizes that to an
    // explicit accept without minting or rewriting either server identity.
    let legacy = GroceryConfirmationDecision::from_contract_fields(None, None).unwrap();
    assert_eq!(legacy.as_contract_value(), "accept");
    let first_accept = proposed.command(legacy.clone()).unwrap();
    let duplicate_accept = proposed.command(legacy).unwrap();
    assert_eq!(first_accept, duplicate_accept);
    assert_eq!(first_accept.confirmation_id, confirmation_id);
    assert_eq!(first_accept.idempotency_key, idempotency_key);

    let cancel = GroceryConfirmationDecision::from_contract_fields(Some("cancel"), None).unwrap();
    assert_eq!(cancel, GroceryConfirmationDecision::Cancel);
    let cancel_command = proposed.command(cancel).unwrap();
    assert_eq!(cancel_command.decision, GroceryConfirmationDecision::Cancel);

    let edits = GroceryValidatedEdits::new(
        serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
            serde_json::json!({"items": [{"name": "oats", "quantity": 2}]}),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(!format!("{edits:?}").contains("oats"));
    let accept_with_edits =
        GroceryConfirmationDecision::from_contract_fields(Some("accept"), Some(edits.clone()))
            .unwrap();
    let edit_command = proposed.command(accept_with_edits).unwrap();
    assert_eq!(
        edit_command.decision,
        GroceryConfirmationDecision::Accept { edits: Some(edits) }
    );
    assert_eq!(
        GroceryConfirmationDecision::from_contract_fields(
            Some("cancel"),
            Some(
                GroceryValidatedEdits::new(
                    serde_json::from_value(serde_json::json!({"items": []})).unwrap(),
                )
                .unwrap()
            )
        ),
        Err(GroceryErrorCode::EditInvalid)
    );
    assert_eq!(
        GroceryValidatedEdits::new(
            serde_json::from_value(serde_json::json!({"name": "unsafe\nedit"})).unwrap()
        ),
        Err(GroceryErrorCode::EditInvalid)
    );
    assert!(
        proposed
            .command(GroceryConfirmationDecision::from_contract_fields(None, None).unwrap())
            .is_ok()
    );

    let mut live = preconditions.clone();
    live.list_id = GroceryEntityId::parse("00000000-0000-4000-8000-000000000013").unwrap();
    assert_eq!(
        preconditions.validate_live(&live),
        Err(GroceryErrorCode::ListReplaced)
    );
    live = preconditions.clone();
    live.list_version = GroceryListVersion::new(8).unwrap();
    assert_eq!(
        preconditions.validate_live(&live),
        Err(GroceryErrorCode::ListVersionConflict)
    );
    live = preconditions.clone();
    live.context_fingerprint = ContextFingerprint::parse("0123456789abcdef").unwrap();
    assert_eq!(
        preconditions.validate_live(&live),
        Err(GroceryErrorCode::ContextChanged)
    );
    live = preconditions.clone();
    live.household_context_hash_version = Some(HouseholdContextHashVersion::new(3));
    assert_eq!(
        preconditions.validate_live(&live),
        Err(GroceryErrorCode::ContextChanged)
    );
    live = preconditions.clone();
    live.household_context_hash_version = None;
    assert_eq!(
        preconditions.validate_live(&live),
        Err(GroceryErrorCode::ContextChanged)
    );

    let cancelled = GroceryConfirmation {
        state: GroceryConfirmationState::Cancelled,
        ..proposed
    };
    assert_eq!(
        cancelled.command(GroceryConfirmationDecision::from_contract_fields(None, None).unwrap()),
        Err(GroceryErrorCode::AlreadyCancelled)
    );
}

#[test]
fn frozen_health_fixture_keeps_status_domains_disjoint() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/contracts/health-h1h2.v1.json"
    ))
    .unwrap();
    let freshness = fixture["health_context"]["status_enum"].as_array().unwrap();
    let connections = fixture["integration"]["status_enum"].as_array().unwrap();

    for value in freshness {
        serde_json::from_value::<HealthFreshnessStatus>(value.clone()).unwrap();
        if value != "connected" {
            assert!(serde_json::from_value::<HealthConnectionStatus>(value.clone()).is_err());
        }
    }
    for value in connections {
        serde_json::from_value::<HealthConnectionStatus>(value.clone()).unwrap();
        if value != "connected" {
            assert!(serde_json::from_value::<HealthFreshnessStatus>(value.clone()).is_err());
        }
    }
    assert!(serde_json::from_str::<HealthFreshnessStatus>(r#""expired""#).is_err());
    assert!(serde_json::from_str::<HealthConnectionStatus>(r#""stale""#).is_err());
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
