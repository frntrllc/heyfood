use heyfood_core::{
    AccountId, AgentEvent, BrowserUrl, CredentialVersion, NetworkPolicy, RefreshRequest,
    RefreshResult, SensitiveString, ServiceUrl, ServiceUrlError, SessionCredentials,
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
