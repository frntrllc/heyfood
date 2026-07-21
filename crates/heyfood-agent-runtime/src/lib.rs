//! Authenticated HTTP and server-sent-event adapters.

#![forbid(unsafe_code)]

mod registration;
mod sse;

use std::time::Duration;

use heyfood_application::{AcceptedTurn, BoxFuture, PortError, ServicePort, TurnRequest};
use heyfood_core::{
    AccountId, NetworkPolicy, OperationId, RefreshOutcome, RefreshRequest, RefreshResult,
    SensitiveString, ServiceUrl, SessionCredentials,
};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

pub use registration::{
    DeviceAuthorization, RegistrationClient, RegistrationError, RegistrationOutcome,
};
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
    cli_auth: Option<CliAuthContext>,
}

/// Python-compatible headers and fallback bearer used for CLI session refresh.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliAuthContext {
    device_id: String,
    channel_access_token: SensitiveString,
    api_key: Option<SensitiveString>,
}

impl CliAuthContext {
    pub fn new(
        device_id: impl Into<String>,
        channel_access_token: SensitiveString,
        api_key: Option<SensitiveString>,
    ) -> Result<Self, PortError> {
        let device_id = device_id.into();
        if device_id.trim() != device_id || device_id.len() < 3 || device_id.len() > 255 {
            return Err(PortError::new(
                "refresh_context",
                "CLI device ID must contain 3 to 255 characters without surrounding whitespace",
            ));
        }
        if channel_access_token.expose_secret().is_empty() {
            return Err(PortError::new(
                "refresh_context",
                "channel access token is required for session re-exchange",
            ));
        }
        if api_key
            .as_ref()
            .is_some_and(|value| value.expose_secret().is_empty())
        {
            return Err(PortError::new(
                "refresh_context",
                "API key must be omitted instead of empty",
            ));
        }
        Ok(Self {
            device_id,
            channel_access_token,
            api_key,
        })
    }
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
            cli_auth: None,
        })
    }

    /// Attach the channel/API-key material owned by the active CLI context.
    #[must_use]
    pub fn with_cli_auth(mut self, cli_auth: CliAuthContext) -> Self {
        self.cli_auth = Some(cli_auth);
        self
    }

    fn client(&self, streaming: bool) -> Result<Client, PortError> {
        let mut builder = Client::builder()
            .use_rustls_tls()
            .https_only(!self.policy.allow_plaintext_loopback)
            .connect_timeout(self.deadlines.connect)
            .pool_idle_timeout(self.deadlines.pool_idle)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .user_agent(format!("heyfood-cli/{}", VERSION));
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
    refresh_token: &'a str,
}

#[derive(Deserialize)]
struct RefreshBodyResponse {
    user_id: String,
    access_token: String,
    refresh_token: String,
    access_expires_at: String,
}

#[derive(Serialize)]
struct ReexchangeBody<'a> {
    device_id: &'a str,
}

impl ServicePort for HttpService {
    fn refresh_session(
        &self,
        request: RefreshRequest,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<RefreshOutcome, PortError>> {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Ok(RefreshOutcome::CancelledBeforeDispatch);
            }
            let cli_auth = self.cli_auth.as_ref().ok_or_else(|| {
                PortError::new(
                    "refresh_context",
                    "CLI auth context is required for session refresh",
                )
            })?;
            let client = self.client(false)?;
            let response = if let Some(refresh_token) = request.refresh_token.as_ref() {
                let endpoint = self.endpoint("/v1/auth/session/refresh")?;
                let body = RefreshBody {
                    refresh_token: refresh_token.expose_secret(),
                };
                let mut builder = client
                    .post(endpoint)
                    .header("Accept", "application/json")
                    .header("X-App-Client-ID", "heyfood-cli")
                    .header("X-Device-ID", &cli_auth.device_id)
                    .header(
                        "X-Request-ID",
                        heyfood_core::OperationId::new().as_uuid().to_string(),
                    )
                    .json(&body);
                if let Some(api_key) = cli_auth.api_key.as_ref() {
                    builder = builder.header("X-API-Key", api_key.expose_secret());
                }
                let response = dispatch_refresh(
                    builder,
                    &cancellation,
                    "refresh_transport",
                    "refresh_cancelled_after_dispatch",
                )
                .await?;
                if response.status().is_success() {
                    response
                } else {
                    dispatch_reexchange(&client, self, cli_auth, &cancellation).await?
                }
            } else {
                dispatch_reexchange(&client, self, cli_auth, &cancellation).await?
            };
            if !response.status().is_success() {
                return Err(PortError::new(
                    "login_required",
                    "session expired and could not be renewed; login is required",
                ));
            }
            let decoded = tokio::select! {
                () = cancellation.cancelled() => return Err(PortError::uncertain("refresh_cancelled_after_dispatch", "session response was not observed after request dispatch")),
                result = response.json::<RefreshBodyResponse>() => result.map_err(|error| PortError::uncertain("refresh_response", sanitized_reqwest_error(&error)))?,
            };
            let account_id = AccountId::parse(decoded.user_id)
                .map_err(|message| PortError::uncertain("refresh_response", message))?;
            let credentials = SessionCredentials::from_rfc3339_expiry(
                account_id,
                SensitiveString::new(decoded.access_token),
                SensitiveString::new(decoded.refresh_token),
                request.current_version.next(),
                &decoded.access_expires_at,
            )
            .map_err(|message| PortError::uncertain("refresh_response", message))?;
            RefreshResult::validated(&request, credentials)
                .map(RefreshOutcome::Refreshed)
                .map_err(|message| PortError::uncertain("refresh_response", message))
        })
    }

    fn open_turn(
        &self,
        request: TurnRequest,
        credentials: SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<AcceptedTurn, PortError>> {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(PortError::new(
                    "converse_cancelled_before_dispatch",
                    "conversational POST was cancelled before dispatch",
                ));
            }
            let cli_auth = self.cli_auth.as_ref().ok_or_else(|| {
                PortError::new(
                    "converse_context",
                    "CLI auth context is required for conversational requests",
                )
            })?;
            let client = self.client(true)?;
            let endpoint = self.endpoint("/v1/agent/converse")?;
            let mut body = serde_json::json!({
                "input_mode": "text",
                "query": request.prompt,
            });
            if let Some(conversation_id) = request.conversation_id {
                body["conversation_id"] = serde_json::Value::String(conversation_id);
            }
            if let Some(value) = request.context.dietary {
                body["dietary_context"] = value;
            }
            if let Some(value) = request.context.device {
                body["device_context"] = value;
            }
            if let Some(value) = request.context.meal {
                body["meal_context"] = value;
            }
            if let Some(value) = request.context.latitude {
                body["lat"] = serde_json::Value::from(value);
            }
            if let Some(value) = request.context.longitude {
                body["lng"] = serde_json::Value::from(value);
            }
            let mut builder = client
                .post(endpoint)
                .header("Accept", "application/json")
                .header("X-App-Client-ID", "heyfood-cli")
                .header("X-Device-ID", &cli_auth.device_id)
                .header("X-Request-ID", operation_id.as_uuid().to_string())
                .bearer_auth(credentials.access_token.expose_secret())
                .json(&body);
            if let Some(api_key) = cli_auth.api_key.as_ref() {
                builder = builder.header("X-API-Key", api_key.expose_secret());
            }
            let send = builder.send();
            let response = tokio::select! {
                () = cancellation.cancelled() => {
                    return Err(PortError::uncertain(
                        "converse_cancelled_after_dispatch",
                        "conversational POST response was not observed after dispatch",
                    ));
                }
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

async fn dispatch_reexchange(
    client: &Client,
    service: &HttpService,
    cli_auth: &CliAuthContext,
    cancellation: &CancellationToken,
) -> Result<reqwest::Response, PortError> {
    if cancellation.is_cancelled() {
        return Err(PortError::uncertain(
            "reexchange_cancelled_after_refresh",
            "fallback was required after refresh dispatch but was interrupted",
        ));
    }
    let endpoint = service.endpoint("/v1/channel/oauth/cli/session")?;
    let mut builder = client
        .post(endpoint)
        .header("Accept", "application/json")
        .header("X-App-Client-ID", "heyfood-cli")
        .header("X-Device-ID", &cli_auth.device_id)
        .header(
            "X-Request-ID",
            heyfood_core::OperationId::new().as_uuid().to_string(),
        )
        .bearer_auth(cli_auth.channel_access_token.expose_secret())
        .json(&ReexchangeBody {
            device_id: &cli_auth.device_id,
        });
    if let Some(api_key) = cli_auth.api_key.as_ref() {
        builder = builder.header("X-API-Key", api_key.expose_secret());
    }
    dispatch_refresh(
        builder,
        cancellation,
        "reexchange_transport",
        "reexchange_cancelled_after_dispatch",
    )
    .await
}

async fn dispatch_refresh(
    builder: reqwest::RequestBuilder,
    cancellation: &CancellationToken,
    transport_code: &'static str,
    cancellation_code: &'static str,
) -> Result<reqwest::Response, PortError> {
    let send = builder.send();
    tokio::select! {
        () = cancellation.cancelled() => Err(PortError::uncertain(cancellation_code, "session response was not observed after request dispatch")),
        result = send => result.map_err(|error| uncertain_transport(transport_code, &error)),
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
