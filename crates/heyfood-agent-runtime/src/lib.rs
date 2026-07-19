//! Authenticated HTTP and server-sent-event adapters.

#![forbid(unsafe_code)]

mod sse;

use std::time::Duration;

use heyfood_application::{AcceptedTurn, BoxFuture, PortError, ServicePort, TurnRequest};
use heyfood_core::{
    AccountId, CredentialVersion, NetworkPolicy, RefreshRequest, RefreshResult, SensitiveString,
    ServiceUrl, SessionCredentials,
};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

pub use sse::SseEventStream;

/// The package version shared by the native workspace.
pub const VERSION: &str = heyfood_core::VERSION;

/// Finite transport bounds. The streaming deadline is an inactivity bound,
/// not a limit on the total duration of a healthy conversation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HttpDeadlines {
    pub connect: Duration,
    pub request: Duration,
    pub pool_idle: Duration,
    pub sse_inactivity: Duration,
}

impl Default for HttpDeadlines {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(5),
            request: Duration::from_secs(15),
            pool_idle: Duration::from_secs(15),
            sse_inactivity: Duration::from_secs(30),
        }
    }
}

impl HttpDeadlines {
    fn validate(self) -> Result<Self, PortError> {
        if [
            self.connect,
            self.request,
            self.pool_idle,
            self.sse_inactivity,
        ]
        .contains(&Duration::ZERO)
        {
            return Err(PortError::new(
                "invalid_deadline",
                "HTTP deadlines must be finite and greater than zero",
            ));
        }
        Ok(self)
    }
}

/// Reqwest/Rustls implementation of the hosted-service boundary.
///
/// A new client is constructed for each operation. Redirects and automatic
/// retries are disabled, which is especially important for the conversational
/// POST whose acceptance can be uncertain after a transport failure.
#[derive(Clone, Debug)]
pub struct HttpService {
    base_url: ServiceUrl,
    policy: NetworkPolicy,
    deadlines: HttpDeadlines,
}

impl HttpService {
    pub fn new(
        base_url: ServiceUrl,
        policy: NetworkPolicy,
        deadlines: HttpDeadlines,
    ) -> Result<Self, PortError> {
        let deadlines = deadlines.validate()?;
        if base_url.is_plaintext_loopback() && !policy.allow_plaintext_loopback {
            return Err(PortError::new(
                "network_policy",
                "service URL is not allowed by the selected network policy",
            ));
        }
        Ok(Self {
            base_url,
            policy,
            deadlines,
        })
    }

    fn client(&self, streaming: bool) -> Result<Client, PortError> {
        let mut builder = Client::builder()
            .use_rustls_tls()
            .https_only(!self.policy.allow_plaintext_loopback)
            .connect_timeout(self.deadlines.connect)
            .pool_idle_timeout(self.deadlines.pool_idle)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .user_agent(format!("heyfood/{}", VERSION));
        if !streaming {
            builder = builder.timeout(self.deadlines.request);
        }
        builder
            .build()
            .map_err(|error| PortError::new("http_client", sanitized_reqwest_error(&error)))
    }

    fn endpoint(&self, path: &str) -> Result<reqwest::Url, PortError> {
        self.base_url
            .as_url()
            .join(path)
            .map_err(|_| PortError::new("service_url", "could not construct service endpoint"))
    }
}

#[derive(Serialize)]
struct RefreshBody<'a> {
    account_id: &'a str,
    refresh_token: &'a str,
    current_version: u64,
}

#[derive(Deserialize)]
struct RefreshBodyResponse {
    account_id: String,
    access_token: String,
    refresh_token: String,
    #[serde(alias = "credential_version", alias = "version")]
    credential_version: u64,
    #[serde(alias = "expires_at", alias = "expires_at_unix")]
    expires_at_unix: i64,
}

impl ServicePort for HttpService {
    fn refresh_session(
        &self,
        request: RefreshRequest,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<RefreshResult, PortError>> {
        Box::pin(async move {
            let client = self.client(false)?;
            let endpoint = self.endpoint("/v1/auth/session/refresh")?;
            let body = RefreshBody {
                account_id: request.account_id.as_str(),
                refresh_token: request.refresh_token.expose_secret(),
                current_version: request.current_version.get(),
            };
            let send = client.post(endpoint).json(&body).send();
            let response = tokio::select! {
                () = cancellation.cancelled() => cancellation_pending().await,
                result = send => result.map_err(|error| uncertain_transport("refresh_transport", &error))?,
            };
            ensure_success(response.status(), "refresh_http_status")?;
            let decoded = tokio::select! {
                () = cancellation.cancelled() => cancellation_pending().await,
                result = response.json::<RefreshBodyResponse>() => result.map_err(|error| PortError::uncertain("refresh_response", sanitized_reqwest_error(&error)))?,
            };
            let account_id = AccountId::parse(decoded.account_id)
                .map_err(|message| PortError::uncertain("refresh_response", message))?;
            let credentials = SessionCredentials::from_unix_expiry(
                account_id,
                SensitiveString::new(decoded.access_token),
                SensitiveString::new(decoded.refresh_token),
                CredentialVersion::new(decoded.credential_version),
                decoded.expires_at_unix,
            )
            .map_err(|message| PortError::uncertain("refresh_response", message))?;
            RefreshResult::validated(&request, credentials)
                .map_err(|message| PortError::uncertain("refresh_response", message))
        })
    }

    fn open_turn(
        &self,
        request: TurnRequest,
        credentials: SessionCredentials,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<AcceptedTurn, PortError>> {
        Box::pin(async move {
            let client = self.client(true)?;
            let endpoint = self.endpoint("/v1/agent/converse")?;
            let mut body = serde_json::json!({
                "input_mode": "text",
                "query": request.prompt,
            });
            if let Some(conversation_id) = request.conversation_id {
                body["conversation_id"] = serde_json::Value::String(conversation_id);
            }
            let send = client
                .post(endpoint)
                .header("Accept", "text/event-stream")
                .bearer_auth(credentials.access_token.expose_secret())
                .json(&body)
                .send();
            let response = tokio::select! {
                () = cancellation.cancelled() => cancellation_pending().await,
                result = tokio::time::timeout(self.deadlines.request, send) => {
                    match result {
                        Ok(Ok(response)) => response,
                        Ok(Err(error)) => return Err(uncertain_transport("converse_transport", &error)),
                        Err(_) => return Err(PortError::uncertain("converse_timeout", "conversational POST acceptance is uncertain after its deadline")),
                    }
                }
            };
            ensure_success(response.status(), "converse_http_status")?;
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            if !content_type
                .split(';')
                .next()
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"))
            {
                return Err(PortError::new(
                    "converse_content_type",
                    "conversational response is not an event stream",
                ));
            }
            Ok(AcceptedTurn {
                events: Box::new(SseEventStream::new(
                    response,
                    cancellation,
                    self.deadlines.sse_inactivity,
                )),
            })
        })
    }
}

fn ensure_success(status: StatusCode, code: &'static str) -> Result<(), PortError> {
    if status.is_success() {
        Ok(())
    } else {
        Err(PortError::new(
            code,
            format!("service returned HTTP status {}", status.as_u16()),
        ))
    }
}

async fn cancellation_pending<T>() -> T {
    // The application layer owns the user-visible cancellation outcome. By
    // entering this branch, `select!` first drops the in-flight Reqwest future
    // (and therefore its request/response resource); remaining pending lets the
    // application's outer cancellation race complete deterministically.
    std::future::pending().await
}

fn uncertain_transport(code: &'static str, error: &reqwest::Error) -> PortError {
    PortError::uncertain(code, sanitized_reqwest_error(error))
}

fn sanitized_reqwest_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "HTTP operation timed out".into()
    } else if error.is_connect() {
        "could not connect to service".into()
    } else if error.is_decode() {
        "service response could not be decoded".into()
    } else {
        "HTTP transport failed".into()
    }
}
