//! Ordered remote authority teardown followed by mandatory local logout.

use heyfood_core::{AuthCredentialBundle, OperationId};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::{BoxFuture, PortError};

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct LogoutStep {
    pub attempted: bool,
    pub ok: bool,
    pub outcome_uncertain: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<&'static str>,
}

impl LogoutStep {
    const fn skipped() -> Self {
        Self {
            attempted: false,
            ok: true,
            outcome_uncertain: false,
            error: None,
        }
    }

    const fn succeeded() -> Self {
        Self {
            attempted: true,
            ok: true,
            outcome_uncertain: false,
            error: None,
        }
    }

    fn failed(error: &PortError) -> Self {
        Self {
            attempted: true,
            ok: false,
            outcome_uncertain: error.outcome_uncertain,
            error: Some(if error.outcome_uncertain {
                "outcome_uncertain"
            } else {
                "request_failed"
            }),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LogoutTeardown {
    pub link: LogoutStep,
    pub device: LogoutStep,
    pub session: LogoutStep,
}

impl LogoutTeardown {
    #[must_use]
    pub const fn remote_complete(&self) -> bool {
        self.link.ok && self.device.ok && self.session.ok
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LogoutOutcome {
    pub ok: bool,
    pub remote_complete: bool,
    pub teardown: LogoutTeardown,
    pub local_credentials_cleared: bool,
}

impl LogoutOutcome {
    #[must_use]
    pub const fn already_logged_out() -> Self {
        Self {
            ok: true,
            remote_complete: true,
            teardown: LogoutTeardown {
                link: LogoutStep::skipped(),
                device: LogoutStep::skipped(),
                session: LogoutStep::skipped(),
            },
            local_credentials_cleared: true,
        }
    }

    #[must_use]
    pub const fn recovered_local_logout() -> Self {
        const UNKNOWN: LogoutStep = LogoutStep {
            attempted: false,
            ok: false,
            outcome_uncertain: true,
            error: Some("outcome_uncertain"),
        };
        Self {
            ok: true,
            remote_complete: false,
            teardown: LogoutTeardown {
                link: UNKNOWN,
                device: UNKNOWN,
                session: UNKNOWN,
            },
            local_credentials_cleared: true,
        }
    }
}

pub trait LogoutRemotePort: Send + Sync {
    fn current_link<'a>(
        &'a self,
        credentials: &'a AuthCredentialBundle,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<Option<String>, PortError>>;

    fn revoke_link<'a>(
        &'a self,
        credentials: &'a AuthCredentialBundle,
        link_id: String,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<(), PortError>>;

    fn revoke_device<'a>(
        &'a self,
        credentials: &'a AuthCredentialBundle,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<(), PortError>>;

    fn revoke_session<'a>(
        &'a self,
        credentials: &'a AuthCredentialBundle,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<(), PortError>>;
}

pub trait LogoutLocalPort: Send + Sync {
    /// Clear the exact account-bound authorization observed before remote
    /// teardown. Implementations must reject a concurrently replaced account.
    fn clear<'a>(
        &'a self,
        expected: &'a AuthCredentialBundle,
    ) -> BoxFuture<'a, Result<(), PortError>>;
}

pub struct Logout<'a> {
    remote: &'a dyn LogoutRemotePort,
    local: &'a dyn LogoutLocalPort,
}

impl<'a> Logout<'a> {
    #[must_use]
    pub const fn new(remote: &'a dyn LogoutRemotePort, local: &'a dyn LogoutLocalPort) -> Self {
        Self { remote, local }
    }

    /// Revoke the current link and device before the app session that
    /// authenticates those calls. No remote mutation is retried implicitly.
    /// Local credentials are cleared regardless of remote failures.
    pub async fn execute(
        &self,
        credentials: &AuthCredentialBundle,
        cancellation: CancellationToken,
    ) -> Result<LogoutOutcome, PortError> {
        let link = match self
            .remote
            .current_link(credentials, OperationId::new(), cancellation.clone())
            .await
        {
            Ok(Some(link_id)) => match self
                .remote
                .revoke_link(
                    credentials,
                    link_id,
                    OperationId::new(),
                    cancellation.clone(),
                )
                .await
            {
                Ok(()) => LogoutStep::succeeded(),
                Err(error) => LogoutStep::failed(&error),
            },
            Ok(None) => LogoutStep::skipped(),
            Err(error) => LogoutStep::failed(&error),
        };
        let device = match self
            .remote
            .revoke_device(credentials, OperationId::new(), cancellation.clone())
            .await
        {
            Ok(()) => LogoutStep::succeeded(),
            Err(error) => LogoutStep::failed(&error),
        };
        let session = match self
            .remote
            .revoke_session(credentials, OperationId::new(), cancellation)
            .await
        {
            Ok(()) => LogoutStep::succeeded(),
            Err(error) => LogoutStep::failed(&error),
        };
        let teardown = LogoutTeardown {
            link,
            device,
            session,
        };
        self.local.clear(credentials).await?;
        Ok(LogoutOutcome {
            ok: true,
            remote_complete: teardown.remote_complete(),
            teardown,
            local_credentials_cleared: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use heyfood_core::{
        AccountId, AuthCredentialBundle, ChannelCredentials, CredentialVersion, SensitiveString,
        SessionCredentials,
    };

    use super::*;

    struct Fixture {
        calls: Arc<Mutex<Vec<&'static str>>>,
        fail_link: bool,
    }

    impl LogoutRemotePort for Fixture {
        fn current_link<'a>(
            &'a self,
            _credentials: &'a AuthCredentialBundle,
            _operation_id: OperationId,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'a, Result<Option<String>, PortError>> {
            Box::pin(async {
                self.calls.lock().unwrap().push("whoami");
                Ok(Some("link-1".into()))
            })
        }

        fn revoke_link<'a>(
            &'a self,
            _credentials: &'a AuthCredentialBundle,
            _link_id: String,
            _operation_id: OperationId,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'a, Result<(), PortError>> {
            Box::pin(async {
                self.calls.lock().unwrap().push("link");
                if self.fail_link {
                    Err(PortError::uncertain("sentinel-secret", "sensitive"))
                } else {
                    Ok(())
                }
            })
        }

        fn revoke_device<'a>(
            &'a self,
            _credentials: &'a AuthCredentialBundle,
            _operation_id: OperationId,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'a, Result<(), PortError>> {
            Box::pin(async {
                self.calls.lock().unwrap().push("device");
                Ok(())
            })
        }

        fn revoke_session<'a>(
            &'a self,
            _credentials: &'a AuthCredentialBundle,
            _operation_id: OperationId,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'a, Result<(), PortError>> {
            Box::pin(async {
                self.calls.lock().unwrap().push("session");
                Ok(())
            })
        }
    }

    impl LogoutLocalPort for Fixture {
        fn clear<'a>(
            &'a self,
            _expected: &'a AuthCredentialBundle,
        ) -> BoxFuture<'a, Result<(), PortError>> {
            Box::pin(async {
                self.calls.lock().unwrap().push("local");
                Ok(())
            })
        }
    }

    fn credentials() -> AuthCredentialBundle {
        let account_id = AccountId::parse("user-1").unwrap();
        AuthCredentialBundle {
            channel: ChannelCredentials::from_rfc3339_expiry(
                "client-1",
                "device-1",
                SensitiveString::new("channel-access"),
                SensitiveString::new("channel-refresh"),
                "2099-01-01T00:00:00Z",
                "menu:watch",
            )
            .unwrap(),
            session: SessionCredentials::from_unix_expiry(
                account_id,
                SensitiveString::new("session-access"),
                SensitiveString::new("session-refresh"),
                CredentialVersion::new(1),
                4_102_444_800,
            )
            .unwrap(),
        }
    }

    #[tokio::test]
    async fn revokes_link_then_device_then_session_before_local_clear() {
        let fixture = Fixture {
            calls: Arc::new(Mutex::new(Vec::new())),
            fail_link: false,
        };
        let outcome = Logout::new(&fixture, &fixture)
            .execute(&credentials(), CancellationToken::new())
            .await
            .unwrap();
        assert!(outcome.remote_complete);
        assert_eq!(
            *fixture.calls.lock().unwrap(),
            ["whoami", "link", "device", "session", "local"]
        );
    }

    #[tokio::test]
    async fn remote_failure_is_sanitized_and_never_prevents_local_clear() {
        let fixture = Fixture {
            calls: Arc::new(Mutex::new(Vec::new())),
            fail_link: true,
        };
        let outcome = Logout::new(&fixture, &fixture)
            .execute(&credentials(), CancellationToken::new())
            .await
            .unwrap();
        assert!(!outcome.remote_complete);
        assert_eq!(outcome.teardown.link.error, Some("outcome_uncertain"));
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(!json.contains("sentinel"));
        assert!(!json.contains("sensitive"));
        assert_eq!(fixture.calls.lock().unwrap().last(), Some(&"local"));
    }
}
