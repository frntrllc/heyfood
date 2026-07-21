use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use heyfood_core::{
    AccountId, AuthCapabilities, AuthCredentialBundle, ChannelCredentials, CredentialVersion,
    NetworkPolicy, OperationId, ProfileStatus, SensitiveString, ServiceUrl, SessionCredentials,
};
use reqwest::{Client, Response, StatusCode};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

const OFFICIAL_CLIENT_ID: &str = "hf_cid_heyfood_cli";
const APP_CLIENT_ID: &str = "heyfood-cli";
const LOGIN_SCOPES: &[&str] = &[
    "account:link",
    "account:delete",
    "knowledge:read",
    "menu:read",
    "recommend:read",
    "recipes:read",
    "recipes:write",
    "claims:read_derived",
    "profile:read",
    "profile:write",
    "meals:read",
    "meals:write",
    "audio:transcribe",
];

#[derive(Clone, Debug)]
pub struct RegistrationClient {
    base_url: ServiceUrl,
    client: Client,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceAuthorization {
    pub verification_uri: String,
    pub user_code: String,
    device_code: SensitiveString,
    expires_in: Duration,
    interval: Duration,
    issued_at: tokio::time::Instant,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistrationOutcome {
    pub credentials: AuthCredentialBundle,
    pub profile_status: ProfileStatus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistrationError {
    pub code: &'static str,
    pub public_message: String,
    pub retryable: bool,
    /// The server may have accepted a security mutation even though the client
    /// could not observe or persist its terminal response. Automated retry is
    /// unsafe until account state is reconciled.
    pub outcome_uncertain: bool,
}

impl RegistrationError {
    fn new(code: &'static str, public_message: impl Into<String>) -> Self {
        Self {
            code,
            public_message: public_message.into(),
            retryable: false,
            outcome_uncertain: false,
        }
    }

    fn retryable(code: &'static str, public_message: impl Into<String>) -> Self {
        Self {
            code,
            public_message: public_message.into(),
            retryable: true,
            outcome_uncertain: false,
        }
    }

    fn uncertain(code: &'static str, public_message: impl Into<String>) -> Self {
        Self {
            code,
            public_message: public_message.into(),
            retryable: false,
            outcome_uncertain: true,
        }
    }
}

impl fmt::Display for RegistrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.public_message)
    }
}

impl std::error::Error for RegistrationError {}

#[derive(Serialize)]
struct DeviceAuthorizationRequest<'a> {
    client_id: &'a str,
    scope: String,
    intent: &'a str,
}

#[derive(Deserialize)]
struct DeviceAuthorizationResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: Option<String>,
    expires_in: u64,
    interval: u64,
}

#[derive(Serialize)]
struct DeviceTokenRequest<'a> {
    client_id: &'a str,
    device_code: &'a str,
}

#[derive(Serialize)]
struct ChannelRefreshRequest<'a> {
    grant_type: &'static str,
    client_id: &'a str,
    refresh_token: &'a str,
}

#[derive(Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
    scope: String,
}

#[derive(Deserialize)]
struct OAuthErrorResponse {
    error: Option<String>,
}

#[derive(Serialize)]
struct CliSessionRequest<'a> {
    device_id: &'a str,
}

#[derive(Deserialize)]
struct CliSessionResponse {
    user_id: String,
    device_id: String,
    session_id: String,
    access_token: String,
    refresh_token: String,
    access_expires_at: String,
    scopes: Vec<String>,
    #[serde(default)]
    is_anonymous: bool,
}

#[derive(Deserialize)]
struct ProfileReadinessResponse {
    schema_version: u16,
    status: String,
    member_id: String,
    has_profile_sync_consent: Option<bool>,
    profile_version: Option<u64>,
}

impl RegistrationClient {
    pub fn new(base_url: ServiceUrl, policy: NetworkPolicy) -> Result<Self, RegistrationError> {
        if base_url.is_plaintext_loopback() && !policy.allow_plaintext_loopback {
            return Err(RegistrationError::new(
                "network_policy",
                "The selected hello.food service URL is not allowed.",
            ));
        }
        let client = Client::builder()
            .use_rustls_tls()
            .https_only(!policy.allow_plaintext_loopback)
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .user_agent(format!("heyfood-cli/{}", crate::VERSION))
            .build()
            .map_err(|_| {
                RegistrationError::new("http_client", "Could not initialize secure HTTP.")
            })?;
        Ok(Self { base_url, client })
    }

    pub async fn capabilities(&self) -> Result<AuthCapabilities, RegistrationError> {
        let response = self
            .client
            .get(self.endpoint("/v1/auth/capabilities")?)
            .header("Accept", "application/json")
            .header("X-App-Client-ID", APP_CLIENT_ID)
            .header("X-Request-ID", OperationId::new().as_uuid().to_string())
            .send()
            .await
            .map_err(|_| {
                RegistrationError::retryable(
                    "registration_preflight",
                    "Could not reach hello.food to check registration availability.",
                )
            })?;
        if matches!(
            response.status(),
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
        ) {
            return Err(RegistrationError::new(
                "registration_unavailable",
                "This hello.food service does not support self registration.",
            ));
        }
        let response = successful(response, "registration_preflight").await?;
        let capabilities: AuthCapabilities = response.json().await.map_err(|_| {
            RegistrationError::new(
                "auth_contract_error",
                "hello.food returned an unsupported registration capability response.",
            )
        })?;
        capabilities
            .validate_native_registration_launch()
            .map_err(|message| {
                let code = if capabilities.self_registration.status
                    == heyfood_core::RegistrationStatus::Available
                {
                    "auth_contract_error"
                } else {
                    "registration_unavailable"
                };
                RegistrationError::new(code, message)
            })?;
        Ok(capabilities)
    }

    /// Rotate an expired channel grant before it is needed for app-session
    /// re-exchange. Callers must durably replace the complete auth bundle
    /// before using the returned access token.
    pub async fn refresh_channel(
        &self,
        current: &ChannelCredentials,
    ) -> Result<ChannelCredentials, RegistrationError> {
        let response = self
            .client
            .post(self.endpoint("/v1/channel/oauth/token")?)
            .header("Accept", "application/json")
            .header("X-Request-ID", OperationId::new().as_uuid().to_string())
            .json(&ChannelRefreshRequest {
                grant_type: "refresh_token",
                client_id: &current.client_id,
                refresh_token: current.refresh_token.expose_secret(),
            })
            .send()
            .await
            .map_err(|_| {
                RegistrationError::uncertain(
                    "channel_refresh_outcome_uncertain",
                    "The channel credential refresh response was not observed. Reconcile account state before retrying.",
                )
            })?;
        if !response.status().is_success() {
            return Err(RegistrationError::new(
                "login_required",
                "The channel authorization expired. Reconnect the hello.food account.",
            ));
        }
        let refreshed: OAuthTokenResponse = response.json().await.map_err(|_| {
            RegistrationError::uncertain(
                "channel_refresh_contract_uncertain",
                "The channel credential was rotated but its response was invalid. Reconcile account state before retrying.",
            )
        })?;
        if refreshed.access_token.is_empty()
            || refreshed.refresh_token.is_empty()
            || refreshed.expires_in == 0
            || refreshed.scope.split_whitespace().collect::<Vec<_>>()
                != current.scope.split_whitespace().collect::<Vec<_>>()
        {
            return Err(RegistrationError::uncertain(
                "channel_refresh_contract_uncertain",
                "The channel credential was rotated with an invalid contract. Reconcile account state before retrying.",
            ));
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |value| value.as_secs());
        ChannelCredentials::from_unix_expiry(
            current.client_id.clone(),
            current.device_id.clone(),
            SensitiveString::new(refreshed.access_token),
            SensitiveString::new(refreshed.refresh_token),
            i64::try_from(now.saturating_add(refreshed.expires_in)).unwrap_or(i64::MAX),
            refreshed.scope,
        )
        .map_err(|_| {
            RegistrationError::uncertain(
                "channel_refresh_contract_uncertain",
                "The channel credential was rotated with invalid bounds. Reconcile account state before retrying.",
            )
        })
    }

    pub async fn start_device_registration(
        &self,
    ) -> Result<DeviceAuthorization, RegistrationError> {
        self.capabilities().await?;
        let request = DeviceAuthorizationRequest {
            client_id: OFFICIAL_CLIENT_ID,
            scope: LOGIN_SCOPES.join(" "),
            intent: "create_account",
        };
        let response = self
            .client
            .post(self.endpoint("/v1/channel/oauth/device/authorize")?)
            .header("Accept", "application/json")
            .header("X-Request-ID", OperationId::new().as_uuid().to_string())
            .json(&request)
            .send()
            .await
            .map_err(|_| {
                RegistrationError::retryable(
                    "device_authorization",
                    "Could not start hello.food device authorization.",
                )
            })?;
        let response = successful(response, "device_authorization").await?;
        let decoded: DeviceAuthorizationResponse = response.json().await.map_err(|_| {
            RegistrationError::new(
                "auth_contract_error",
                "Device authorization returned an unsupported response.",
            )
        })?;
        if decoded.device_code.len() < 20
            || decoded.user_code.len() < 8
            || decoded.expires_in == 0
            || decoded.interval == 0
            || decoded.interval > decoded.expires_in
        {
            return Err(RegistrationError::new(
                "auth_contract_error",
                "Device authorization returned invalid bounds.",
            ));
        }
        let verification_uri = decoded
            .verification_uri_complete
            .unwrap_or(decoded.verification_uri);
        let mut url = reqwest::Url::parse(&verification_uri).map_err(|_| {
            RegistrationError::new(
                "auth_contract_error",
                "Device authorization returned an invalid verification URL.",
            )
        })?;
        if url.scheme() != "https" && !self.base_url.is_plaintext_loopback() {
            return Err(RegistrationError::new(
                "auth_contract_error",
                "Device authorization did not return a secure verification URL.",
            ));
        }
        url.query_pairs_mut()
            .append_pair("intent", "create_account");
        Ok(DeviceAuthorization {
            verification_uri: url.into(),
            user_code: decoded.user_code,
            device_code: SensitiveString::new(decoded.device_code),
            expires_in: Duration::from_secs(decoded.expires_in),
            interval: Duration::from_secs(decoded.interval),
            issued_at: tokio::time::Instant::now(),
        })
    }

    pub async fn complete_device_registration(
        &self,
        authorization: DeviceAuthorization,
        device_id: String,
        maximum_wait: Duration,
        cancellation: CancellationToken,
    ) -> Result<RegistrationOutcome, RegistrationError> {
        if device_id.len() < 3 || device_id.len() > 255 || device_id.trim() != device_id {
            return Err(RegistrationError::new(
                "device_id",
                "The native device identifier is invalid.",
            ));
        }
        let advertised_deadline = authorization.issued_at + authorization.expires_in;
        let deadline = std::cmp::min(
            advertised_deadline,
            tokio::time::Instant::now() + maximum_wait.max(Duration::from_secs(1)),
        );
        let mut interval = authorization.interval;
        let mut consecutive_server_failures = 0_u8;
        let oauth = loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(RegistrationError::new(
                    "authorization_expired",
                    "The approval window ended. Run heyfood register again.",
                ));
            }
            // Cancellation is clean only before the consuming token request is
            // dispatched. Once dispatched, observe its bounded response rather
            // than dropping the future: a successful exchange consumes the
            // device grant, so its outcome must not be reported as retryable.
            if cancellation.is_cancelled() {
                return Err(RegistrationError::new(
                    "cancelled",
                    "Registration canceled. Nothing was saved.",
                ));
            }
            let response = self.poll_device_token(&authorization).await?;
            if response.status().is_success() {
                break response.json::<OAuthTokenResponse>().await.map_err(|_| {
                    RegistrationError::uncertain(
                        "device_token_contract_uncertain",
                        "The device authorization was consumed, but its token response was unsupported. Reconcile account state before retrying registration.",
                    )
                })?;
            }
            let status = response.status();
            let error = response
                .json::<OAuthErrorResponse>()
                .await
                .ok()
                .and_then(|value| value.error);
            if status.is_server_error() || error.as_deref() == Some("temporarily_unavailable") {
                consecutive_server_failures = consecutive_server_failures.saturating_add(1);
                if consecutive_server_failures >= 10 {
                    return Err(RegistrationError::retryable(
                        "device_token_unavailable",
                        "hello.food could not complete registration after repeated service errors.",
                    ));
                }
                sleep_before_next_poll(interval, deadline, &cancellation).await?;
                continue;
            }
            match error.as_deref() {
                Some("authorization_pending") => consecutive_server_failures = 0,
                Some("slow_down") => {
                    consecutive_server_failures = 0;
                    interval = interval.saturating_add(Duration::from_secs(5));
                }
                Some("access_denied") => {
                    return Err(RegistrationError::new(
                        "access_denied",
                        "The registration request was declined.",
                    ));
                }
                Some("expired_token") | Some("invalid_grant") => {
                    return Err(RegistrationError::new(
                        "authorization_expired",
                        "The approval window ended. Run heyfood register again.",
                    ));
                }
                _ => {
                    return Err(RegistrationError::new(
                        "device_token",
                        format!("Device authorization failed with HTTP {}.", status.as_u16()),
                    ));
                }
            }
            sleep_before_next_poll(interval, deadline, &cancellation).await?;
        };
        if oauth.access_token.is_empty() || oauth.refresh_token.is_empty() || oauth.scope.is_empty()
        {
            return Err(RegistrationError::uncertain(
                "device_token_contract_uncertain",
                "The device authorization was consumed, but its token response was incomplete. Reconcile account state before retrying registration.",
            ));
        }
        // A successful device-token response consumed the grant. Finish the
        // bounded session exchange even if cancellation arrived while that
        // response was in flight, preserving the known authorization outcome.
        let session = self
            .exchange_cli_session(&oauth.access_token, &device_id)
            .await?;
        let session_credentials = validate_cli_session(session, &device_id)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |value| value.as_secs());
        let channel_credentials = ChannelCredentials::from_unix_expiry(
            OFFICIAL_CLIENT_ID,
            device_id,
            SensitiveString::new(oauth.access_token),
            SensitiveString::new(oauth.refresh_token),
            i64::try_from(now.saturating_add(oauth.expires_in)).unwrap_or(i64::MAX),
            oauth.scope,
        )
        .map_err(|_| {
            RegistrationError::uncertain(
                "device_token_contract_uncertain",
                "The device authorization was consumed, but its token credentials were invalid. Reconcile account state before retrying registration.",
            )
        })?;
        let profile_status = self
            .profile_readiness(channel_credentials.access_token.expose_secret())
            .await
            .unwrap_or(ProfileStatus::Unknown);
        Ok(RegistrationOutcome {
            credentials: AuthCredentialBundle {
                channel: channel_credentials,
                session: session_credentials,
            },
            profile_status,
        })
    }

    async fn poll_device_token(
        &self,
        authorization: &DeviceAuthorization,
    ) -> Result<Response, RegistrationError> {
        self.client
            .post(self.endpoint("/v1/channel/oauth/device/token")?)
            .header("Accept", "application/json")
            .header("X-Request-ID", OperationId::new().as_uuid().to_string())
            .json(&DeviceTokenRequest {
                client_id: OFFICIAL_CLIENT_ID,
                device_code: authorization.device_code.expose_secret(),
            })
            .send()
            .await
            .map_err(|_| {
                RegistrationError::uncertain(
                    "device_token_outcome_uncertain",
                    "hello.food may have consumed the device authorization, but the native client could not observe the token response. Do not retry registration until account state is reconciled.",
                )
            })
    }

    async fn exchange_cli_session(
        &self,
        channel_access_token: &str,
        device_id: &str,
    ) -> Result<CliSessionResponse, RegistrationError> {
        let response = self
            .client
            .post(self.endpoint("/v1/channel/oauth/cli/session")?)
            .header("Accept", "application/json")
            .header("X-App-Client-ID", APP_CLIENT_ID)
            .header("X-Device-ID", device_id)
            .header("X-Request-ID", OperationId::new().as_uuid().to_string())
            .bearer_auth(channel_access_token)
            .json(&CliSessionRequest { device_id })
            .send()
            .await
            .map_err(|_| {
                RegistrationError::uncertain(
                    "session_exchange_outcome_uncertain",
                    "The server may have connected the account, but the native client could not observe the session response. Do not retry registration until account state is reconciled.",
                )
            })?;
        if response.status().is_server_error() {
            return Err(RegistrationError::uncertain(
                "session_exchange_outcome_uncertain",
                "The server may have connected the account before returning an error. Do not retry registration until account state is reconciled.",
            ));
        }
        successful(response, "session_exchange")
            .await?
            .json()
            .await
            .map_err(|_| {
                RegistrationError::uncertain(
                    "session_exchange_contract_uncertain",
                    "The server accepted the session exchange, but returned an unsupported response. Do not retry registration until account state is reconciled.",
                )
            })
    }

    async fn profile_readiness(
        &self,
        channel_access_token: &str,
    ) -> Result<ProfileStatus, RegistrationError> {
        let response = self
            .client
            .get(self.endpoint("/v1/channel/tools/profile/readiness")?)
            .header("Accept", "application/json")
            .header("X-App-Client-ID", APP_CLIENT_ID)
            .header("X-Request-ID", OperationId::new().as_uuid().to_string())
            .bearer_auth(channel_access_token)
            .send()
            .await
            .map_err(|_| {
                RegistrationError::retryable(
                    "profile_readiness",
                    "Could not determine profile readiness.",
                )
            })?;
        let response = successful(response, "profile_readiness").await?;
        let value: ProfileReadinessResponse = response.json().await.map_err(|_| {
            RegistrationError::new(
                "auth_contract_error",
                "Profile readiness returned an unsupported response.",
            )
        })?;
        if value.schema_version != 1 || value.member_id != "_self" {
            return Err(RegistrationError::new(
                "auth_contract_error",
                "Profile readiness returned an unsupported response.",
            ));
        }
        match value.status.as_str() {
            "ready"
                if value.has_profile_sync_consent == Some(true)
                    && value.profile_version.is_some() =>
            {
                Ok(ProfileStatus::Ready)
            }
            "missing"
                if value.has_profile_sync_consent.is_some() && value.profile_version.is_none() =>
            {
                Ok(ProfileStatus::Missing)
            }
            "unknown" if value.profile_version.is_none() => Ok(ProfileStatus::Unknown),
            _ => Err(RegistrationError::new(
                "auth_contract_error",
                "Profile readiness returned inconsistent state.",
            )),
        }
    }

    fn endpoint(&self, path: &str) -> Result<reqwest::Url, RegistrationError> {
        self.base_url.as_url().join(path).map_err(|_| {
            RegistrationError::new(
                "service_url",
                "Could not construct the hello.food endpoint.",
            )
        })
    }
}

fn validate_cli_session(
    session: CliSessionResponse,
    requested_device_id: &str,
) -> Result<SessionCredentials, RegistrationError> {
    if session.device_id != requested_device_id {
        return Err(RegistrationError::uncertain(
            "session_exchange_contract_uncertain",
            "The server accepted the session exchange but returned a different device identifier. Do not retry registration until account state is reconciled.",
        ));
    }
    if session.is_anonymous {
        return Err(RegistrationError::uncertain(
            "session_exchange_contract_uncertain",
            "The server accepted the session exchange but returned an anonymous session. Do not retry registration until account state is reconciled.",
        ));
    }
    if session.session_id.is_empty()
        || session.session_id.len() > 256
        || session.session_id.chars().any(char::is_control)
    {
        return Err(RegistrationError::uncertain(
            "session_exchange_contract_uncertain",
            "The server accepted the session exchange but returned an invalid session identifier. Do not retry registration until account state is reconciled.",
        ));
    }
    if session.access_token.is_empty()
        || session.refresh_token.is_empty()
        || session.access_token.len() > 16 * 1024
        || session.refresh_token.len() > 16 * 1024
    {
        return Err(RegistrationError::uncertain(
            "session_exchange_contract_uncertain",
            "The server accepted the session exchange but returned incomplete credentials. Do not retry registration until account state is reconciled.",
        ));
    }
    if session.scopes.is_empty()
        || session.scopes.len() > LOGIN_SCOPES.len()
        || session.scopes.iter().any(|scope| {
            scope.is_empty()
                || scope.len() > 128
                || scope.chars().any(char::is_control)
                || !LOGIN_SCOPES.contains(&scope.as_str())
        })
        || session
            .scopes
            .iter()
            .enumerate()
            .any(|(index, scope)| session.scopes[..index].contains(scope))
    {
        return Err(RegistrationError::uncertain(
            "session_exchange_contract_uncertain",
            "The server accepted the session exchange but returned invalid scopes. Do not retry registration until account state is reconciled.",
        ));
    }
    let account_id = AccountId::parse(session.user_id).map_err(|_| {
        RegistrationError::uncertain(
            "session_exchange_contract_uncertain",
            "The server accepted the session exchange but returned an invalid account. Do not retry registration until account state is reconciled.",
        )
    })?;
    let credentials = SessionCredentials::from_rfc3339_expiry(
        account_id,
        SensitiveString::new(session.access_token),
        SensitiveString::new(session.refresh_token),
        CredentialVersion::new(1),
        &session.access_expires_at,
    )
    .map_err(|_| {
        RegistrationError::uncertain(
            "session_exchange_contract_uncertain",
            "The server accepted the session exchange but returned an invalid expiry. Do not retry registration until account state is reconciled.",
        )
    })?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| {
            i64::try_from(value.as_secs()).unwrap_or(i64::MAX)
        });
    if credentials.expires_at_unix() <= now {
        return Err(RegistrationError::uncertain(
            "session_exchange_contract_uncertain",
            "The server accepted the session exchange but returned an expired session. Do not retry registration until account state is reconciled.",
        ));
    }
    Ok(credentials)
}

async fn successful(response: Response, code: &'static str) -> Result<Response, RegistrationError> {
    if response.status().is_success() {
        return Ok(response);
    }
    Err(RegistrationError::new(
        code,
        format!("hello.food returned HTTP {}.", response.status().as_u16()),
    ))
}

async fn sleep_before_next_poll(
    interval: Duration,
    deadline: tokio::time::Instant,
    cancellation: &CancellationToken,
) -> Result<(), RegistrationError> {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    tokio::select! {
        () = cancellation.cancelled() => Err(RegistrationError::new("cancelled", "Registration canceled. Nothing was saved.")),
        () = tokio::time::sleep(interval.min(remaining)) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::Notify;

    #[tokio::test]
    async fn device_registration_requires_both_identity_methods_and_builds_credentials() {
        let (base_url, server) = fixture_server().await;
        let service_url = ServiceUrl::parse(&base_url, NetworkPolicy::DEVELOPMENT).unwrap();
        let client = RegistrationClient::new(service_url, NetworkPolicy::DEVELOPMENT).unwrap();
        let authorization = client.start_device_registration().await.unwrap();
        assert_eq!(authorization.user_code, "ABCD-EFGH");
        assert!(
            authorization
                .verification_uri
                .contains("intent=create_account")
        );

        let result = client
            .complete_device_registration(
                authorization,
                "heyfood-test-device".into(),
                Duration::from_secs(5),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(result.profile_status, ProfileStatus::Ready);
        assert_eq!(result.credentials.session.account_id.as_str(), "user-test");
        assert_eq!(result.credentials.channel.device_id, "heyfood-test-device");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_after_session_dispatch_waits_for_response_and_completes() {
        let session_seen = Arc::new(Notify::new());
        let release_session = Arc::new(Notify::new());
        let (base_url, server) = fixture_server_with(SessionBehavior::Wait {
            seen: session_seen.clone(),
            release: release_session.clone(),
        })
        .await;
        let service_url = ServiceUrl::parse(&base_url, NetworkPolicy::DEVELOPMENT).unwrap();
        let client = RegistrationClient::new(service_url, NetworkPolicy::DEVELOPMENT).unwrap();
        let authorization = client.start_device_registration().await.unwrap();
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let task = tokio::spawn(async move {
            client
                .complete_device_registration(
                    authorization,
                    "heyfood-test-device".into(),
                    Duration::from_secs(5),
                    task_cancellation,
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), session_seen.notified())
            .await
            .unwrap();
        cancellation.cancel();
        release_session.notify_one();
        let result = task.await.unwrap().unwrap();
        assert_eq!(result.profile_status, ProfileStatus::Ready);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_after_device_token_dispatch_waits_for_response_and_completes() {
        let token_seen = Arc::new(Notify::new());
        let release_token = Arc::new(Notify::new());
        let (base_url, server) = fixture_server_with_token(DeviceTokenBehavior::Wait {
            seen: token_seen.clone(),
            release: release_token.clone(),
        })
        .await;
        let service_url = ServiceUrl::parse(&base_url, NetworkPolicy::DEVELOPMENT).unwrap();
        let client = RegistrationClient::new(service_url, NetworkPolicy::DEVELOPMENT).unwrap();
        let authorization = client.start_device_registration().await.unwrap();
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let task = tokio::spawn(async move {
            client
                .complete_device_registration(
                    authorization,
                    "heyfood-test-device".into(),
                    Duration::from_secs(5),
                    task_cancellation,
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), token_seen.notified())
            .await
            .unwrap();
        cancellation.cancel();
        release_token.notify_one();
        let result = task.await.unwrap().unwrap();
        assert_eq!(result.profile_status, ProfileStatus::Ready);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn eof_after_device_token_dispatch_is_explicitly_outcome_uncertain() {
        let (base_url, server) = fixture_server_with_token(DeviceTokenBehavior::Eof).await;
        let service_url = ServiceUrl::parse(&base_url, NetworkPolicy::DEVELOPMENT).unwrap();
        let client = RegistrationClient::new(service_url, NetworkPolicy::DEVELOPMENT).unwrap();
        let authorization = client.start_device_registration().await.unwrap();
        let error = client
            .complete_device_registration(
                authorization,
                "heyfood-test-device".into(),
                Duration::from_secs(5),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "device_token_outcome_uncertain");
        assert!(error.outcome_uncertain);
        assert!(!error.retryable);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn malformed_success_after_device_grant_consumption_is_outcome_uncertain() {
        let (base_url, server) =
            fixture_server_with_token(DeviceTokenBehavior::MalformedSuccess).await;
        let service_url = ServiceUrl::parse(&base_url, NetworkPolicy::DEVELOPMENT).unwrap();
        let client = RegistrationClient::new(service_url, NetworkPolicy::DEVELOPMENT).unwrap();
        let authorization = client.start_device_registration().await.unwrap();
        let error = client
            .complete_device_registration(
                authorization,
                "heyfood-test-device".into(),
                Duration::from_secs(5),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "device_token_contract_uncertain");
        assert!(error.outcome_uncertain);
        assert!(!error.retryable);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn eof_after_session_dispatch_is_explicitly_outcome_uncertain() {
        let (base_url, server) = fixture_server_with(SessionBehavior::Eof).await;
        let service_url = ServiceUrl::parse(&base_url, NetworkPolicy::DEVELOPMENT).unwrap();
        let client = RegistrationClient::new(service_url, NetworkPolicy::DEVELOPMENT).unwrap();
        let authorization = client.start_device_registration().await.unwrap();
        let error = client
            .complete_device_registration(
                authorization,
                "heyfood-test-device".into(),
                Duration::from_secs(5),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "session_exchange_outcome_uncertain");
        assert!(error.outcome_uncertain);
        assert!(!error.retryable);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn expired_channel_grant_rotates_without_changing_scope_or_device() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let count = socket.read(&mut buffer).await.unwrap();
                request.extend_from_slice(&buffer[..count]);
                if complete_http_request(&request) {
                    break;
                }
            }
            let text = String::from_utf8(request).unwrap();
            assert!(text.starts_with("POST /v1/channel/oauth/token "));
            assert!(text.contains("\"grant_type\":\"refresh_token\""));
            assert!(text.contains("\"client_id\":\"hf_cid_heyfood_cli\""));
            assert!(text.contains("\"refresh_token\":\"channel-refresh-old\""));
            let body = serde_json::to_vec(&serde_json::json!({
                "access_token": "channel-access-new",
                "refresh_token": "channel-refresh-new",
                "expires_in": 3600,
                "scope": "account:link profile:read"
            }))
            .unwrap();
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(header.as_bytes()).await.unwrap();
            socket.write_all(&body).await.unwrap();
        });
        let service_url = ServiceUrl::parse(&base_url, NetworkPolicy::DEVELOPMENT).unwrap();
        let client = RegistrationClient::new(service_url, NetworkPolicy::DEVELOPMENT).unwrap();
        let current = ChannelCredentials::from_unix_expiry(
            "hf_cid_heyfood_cli",
            "heyfood-device",
            SensitiveString::new("channel-access-old"),
            SensitiveString::new("channel-refresh-old"),
            1,
            "account:link profile:read",
        )
        .unwrap();
        let refreshed = client.refresh_channel(&current).await.unwrap();
        assert_eq!(refreshed.client_id, current.client_id);
        assert_eq!(refreshed.device_id, current.device_id);
        assert_eq!(refreshed.scope, current.scope);
        assert_eq!(refreshed.access_token.expose_secret(), "channel-access-new");
        assert_eq!(
            refreshed.refresh_token.expose_secret(),
            "channel-refresh-new"
        );
        server.await.unwrap();
    }

    #[test]
    fn session_validation_rejects_device_mismatch_and_empty_tokens() {
        let mut mismatch = valid_session_response();
        mismatch.device_id = "another-device".into();
        let error = validate_cli_session(mismatch, "heyfood-test-device").unwrap_err();
        assert_eq!(error.code, "session_exchange_contract_uncertain");
        assert!(error.outcome_uncertain);

        for (access_token, refresh_token) in [("", "refresh"), ("access", "")] {
            let mut response = valid_session_response();
            response.access_token = access_token.into();
            response.refresh_token = refresh_token.into();
            let error = validate_cli_session(response, "heyfood-test-device").unwrap_err();
            assert_eq!(error.code, "session_exchange_contract_uncertain");
            assert!(error.outcome_uncertain);
        }
    }

    fn valid_session_response() -> CliSessionResponse {
        CliSessionResponse {
            user_id: "user-test".into(),
            device_id: "heyfood-test-device".into(),
            session_id: "session-test".into(),
            access_token: "hf_at_test".into(),
            refresh_token: "hf_rt_test".into(),
            access_expires_at: "2999-01-01T00:00:00Z".into(),
            scopes: vec!["profile:read".into(), "profile:write".into()],
            is_anonymous: false,
        }
    }

    enum SessionBehavior {
        Immediate,
        Wait {
            seen: Arc<Notify>,
            release: Arc<Notify>,
        },
        Eof,
    }

    enum DeviceTokenBehavior {
        Immediate,
        Wait {
            seen: Arc<Notify>,
            release: Arc<Notify>,
        },
        Eof,
        MalformedSuccess,
    }

    async fn fixture_server() -> (String, tokio::task::JoinHandle<()>) {
        fixture_server_with(SessionBehavior::Immediate).await
    }

    async fn fixture_server_with(
        behavior: SessionBehavior,
    ) -> (String, tokio::task::JoinHandle<()>) {
        fixture_server_with_behaviors(DeviceTokenBehavior::Immediate, behavior).await
    }

    async fn fixture_server_with_token(
        behavior: DeviceTokenBehavior,
    ) -> (String, tokio::task::JoinHandle<()>) {
        fixture_server_with_behaviors(behavior, SessionBehavior::Immediate).await
    }

    async fn fixture_server_with_behaviors(
        token_behavior: DeviceTokenBehavior,
        session_behavior: SessionBehavior,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let base_url = format!("http://{address}");
        let verification_uri = format!("{base_url}/authorize?flow=device");
        let server = tokio::spawn(async move {
            let request_count = match (&token_behavior, &session_behavior) {
                (DeviceTokenBehavior::Eof | DeviceTokenBehavior::MalformedSuccess, _) => 3,
                (_, SessionBehavior::Eof) => 4,
                _ => 5,
            };
            for _ in 0..request_count {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 4096];
                loop {
                    let count = socket.read(&mut buffer).await.unwrap();
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                    if complete_http_request(&request) {
                        break;
                    }
                }
                let text = String::from_utf8(request).unwrap();
                let path = text
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap();
                if path == "/v1/channel/oauth/device/token" {
                    match &token_behavior {
                        DeviceTokenBehavior::Immediate => {}
                        DeviceTokenBehavior::Wait { seen, release } => {
                            seen.notify_one();
                            release.notified().await;
                        }
                        DeviceTokenBehavior::Eof => {
                            socket.shutdown().await.unwrap();
                            continue;
                        }
                        DeviceTokenBehavior::MalformedSuccess => {
                            let body = b"not-json";
                            let header = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            socket.write_all(header.as_bytes()).await.unwrap();
                            socket.write_all(body).await.unwrap();
                            socket.shutdown().await.unwrap();
                            continue;
                        }
                    }
                }
                if path == "/v1/channel/oauth/cli/session" {
                    match &session_behavior {
                        SessionBehavior::Immediate => {}
                        SessionBehavior::Wait { seen, release } => {
                            seen.notify_one();
                            release.notified().await;
                        }
                        SessionBehavior::Eof => {
                            socket.shutdown().await.unwrap();
                            continue;
                        }
                    }
                }
                let body = match path {
                    "/v1/auth/capabilities" => serde_json::json!({
                        "schema_version": 1,
                        "self_registration": {
                            "status": "available",
                            "regions": ["US"],
                            "identity_methods": ["sms", "email"]
                        },
                        "authorization": {
                            "loopback_pkce": true,
                            "device_code": true,
                            "identity_methods": ["sms", "email"]
                        },
                        "profile_readiness": true,
                        "application_capabilities": {}
                    }),
                    "/v1/channel/oauth/device/authorize" => {
                        assert!(text.contains("create_account"));
                        serde_json::json!({
                            "device_code": "hf_dc_01234567890123456789",
                            "user_code": "ABCD-EFGH",
                            "verification_uri": verification_uri,
                            "verification_uri_complete": null,
                            "expires_in": 600,
                            "interval": 1
                        })
                    }
                    "/v1/channel/oauth/device/token" => serde_json::json!({
                        "access_token": "hf_ct_test",
                        "refresh_token": "hf_cr_test",
                        "expires_in": 3600,
                        "scope": LOGIN_SCOPES.join(" ")
                    }),
                    "/v1/channel/oauth/cli/session" => valid_session_response_json(),
                    "/v1/channel/tools/profile/readiness" => serde_json::json!({
                        "schema_version": 1,
                        "status": "ready",
                        "member_id": "_self",
                        "has_profile_sync_consent": true,
                        "profile_version": 1
                    }),
                    _ => panic!("unexpected request path: {path}"),
                };
                let body = serde_json::to_vec(&body).unwrap();
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                socket.write_all(header.as_bytes()).await.unwrap();
                socket.write_all(&body).await.unwrap();
                socket.shutdown().await.unwrap();
            }
        });
        (base_url, server)
    }

    fn valid_session_response_json() -> serde_json::Value {
        serde_json::json!({
            "user_id": "user-test",
            "device_id": "heyfood-test-device",
            "session_id": "session-test",
            "access_token": "hf_at_test",
            "refresh_token": "hf_rt_test",
            "access_expires_at": "2999-01-01T00:00:00Z",
            "scopes": ["profile:read", "profile:write"],
            "is_anonymous": false
        })
    }

    fn complete_http_request(bytes: &[u8]) -> bool {
        let Some(header_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
            return false;
        };
        let headers = String::from_utf8_lossy(&bytes[..header_end]);
        let content_length = headers.lines().find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
        });
        bytes.len() >= header_end + 4 + content_length.unwrap_or(0)
    }
}
