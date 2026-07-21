//! Contract-derived Phase 2 REST operations.

use heyfood_application::PortError;
use heyfood_core::{
    AddItemsRequestWire, ApplicationCapabilitiesWire, AuthorizationServerMetadataWire,
    GroceryEntityId, GroceryListWire, GroceryMutationConfirmRequestWire,
    GroceryMutationProposalWire, GroceryMutationResultWire, HealthContextWire,
    IntegrationAuthorizeRequestWire, IntegrationAuthorizeResponseWire,
    IntegrationDisconnectResponseWire, IntegrationListWire, IntegrationRedirectTargetWire,
    IntegrationSyncResponseWire, OperationId, RemoveItemsRequestWire, SessionCredentials,
    UpdateItemStateRequestWire, terminal_safe_text,
};
use reqwest::{Method, RequestBuilder, Response, StatusCode, header};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio_util::sync::CancellationToken;

use crate::{HttpService, uncertain_transport};

const MAX_JSON_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_EXPORT_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, PartialEq)]
pub enum GroceryExport {
    Json(GroceryListWire),
    Markdown(String),
    Text(String),
}

impl std::fmt::Debug for GroceryExport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(list) => formatter
                .debug_struct("GroceryExport::Json")
                .field("item_count", &list.items.len())
                .finish(),
            Self::Markdown(_) => formatter.write_str("GroceryExport::Markdown([REDACTED])"),
            Self::Text(_) => formatter.write_str("GroceryExport::Text([REDACTED])"),
        }
    }
}

#[derive(Clone, Copy)]
enum DispatchKind {
    Safe,
    Mutation,
}

impl DispatchKind {
    const fn uncertain(self) -> bool {
        matches!(self, Self::Mutation)
    }
}

impl HttpService {
    pub async fn discover_authorization_metadata(
        &self,
        cancellation: CancellationToken,
    ) -> Result<AuthorizationServerMetadataWire, PortError> {
        let builder = self
            .request(
                Method::GET,
                "/.well-known/oauth-authorization-server",
                None,
                OperationId::new(),
            )?
            .header(header::ACCEPT, "application/json");
        let metadata: AuthorizationServerMetadataWire = self
            .dispatch_json(builder, cancellation, DispatchKind::Safe)
            .await?;
        if metadata.scopes_supported.is_empty() {
            return Err(PortError::new(
                "authorization_metadata",
                "authorization metadata did not publish any scopes",
            ));
        }
        Ok(metadata)
    }

    pub async fn discover_capabilities(
        &self,
        cancellation: CancellationToken,
    ) -> Result<ApplicationCapabilitiesWire, PortError> {
        let builder = self
            .request(
                Method::GET,
                "/v1/auth/capabilities",
                None,
                OperationId::new(),
            )?
            .header(header::ACCEPT, "application/json");
        self.dispatch_json(builder, cancellation, DispatchKind::Safe)
            .await
    }

    pub fn require_grocery_v1(capabilities: &ApplicationCapabilitiesWire) -> Result<(), PortError> {
        match capabilities.application_version("grocery") {
            Some("v1") => Ok(()),
            None => Err(PortError::new(
                "grocery_capability_unavailable",
                "Grocery is not advertised by this deployment",
            )),
            Some(_) => Err(PortError::new(
                "grocery_capability_unsupported",
                "Grocery advertises an unsupported contract version",
            )),
        }
    }

    pub async fn grocery_list(
        &self,
        capabilities: &ApplicationCapabilitiesWire,
        credentials: &SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<GroceryListWire, PortError> {
        Self::require_grocery_v1(capabilities)?;
        let builder = self
            .request(
                Method::GET,
                "/v1/grocery/list",
                Some(credentials),
                operation_id,
            )?
            .header(header::ACCEPT, "application/json");
        self.dispatch_json(builder, cancellation, DispatchKind::Safe)
            .await
    }

    pub async fn grocery_prepare_add(
        &self,
        capabilities: &ApplicationCapabilitiesWire,
        credentials: &SessionCredentials,
        operation_id: OperationId,
        request: &AddItemsRequestWire,
        cancellation: CancellationToken,
    ) -> Result<GroceryMutationProposalWire, PortError> {
        self.grocery_mutation(
            capabilities,
            credentials,
            operation_id,
            "/v1/grocery/items",
            request,
            cancellation,
        )
        .await
    }

    pub async fn grocery_prepare_remove(
        &self,
        capabilities: &ApplicationCapabilitiesWire,
        credentials: &SessionCredentials,
        operation_id: OperationId,
        request: &RemoveItemsRequestWire,
        cancellation: CancellationToken,
    ) -> Result<GroceryMutationProposalWire, PortError> {
        self.grocery_mutation(
            capabilities,
            credentials,
            operation_id,
            "/v1/grocery/items/remove",
            request,
            cancellation,
        )
        .await
    }

    pub async fn grocery_prepare_state(
        &self,
        capabilities: &ApplicationCapabilitiesWire,
        credentials: &SessionCredentials,
        operation_id: OperationId,
        request: &UpdateItemStateRequestWire,
        cancellation: CancellationToken,
    ) -> Result<GroceryMutationProposalWire, PortError> {
        self.grocery_mutation(
            capabilities,
            credentials,
            operation_id,
            "/v1/grocery/items/state",
            request,
            cancellation,
        )
        .await
    }

    pub async fn grocery_confirm(
        &self,
        capabilities: &ApplicationCapabilitiesWire,
        credentials: &SessionCredentials,
        operation_id: OperationId,
        request: &GroceryMutationConfirmRequestWire,
        cancellation: CancellationToken,
    ) -> Result<GroceryMutationResultWire, PortError> {
        self.grocery_mutation(
            capabilities,
            credentials,
            operation_id,
            "/v1/grocery/confirm",
            request,
            cancellation,
        )
        .await
    }

    pub async fn grocery_export(
        &self,
        capabilities: &ApplicationCapabilitiesWire,
        credentials: &SessionCredentials,
        operation_id: OperationId,
        list_id: GroceryEntityId,
        format: &str,
        cancellation: CancellationToken,
    ) -> Result<GroceryExport, PortError> {
        Self::require_grocery_v1(capabilities)?;
        if !matches!(format, "json" | "markdown" | "text") {
            return Err(PortError::new(
                "grocery_export_format",
                "Grocery export format must be json, markdown, or text",
            ));
        }
        let path = format!(
            "/v1/grocery/lists/{}/export",
            list_id.as_uuid().hyphenated()
        );
        let builder = self
            .request(Method::GET, &path, Some(credentials), operation_id)?
            .query(&[("format", format)]);
        let response = self
            .dispatch(builder, &cancellation, DispatchKind::Safe)
            .await?;
        let content_type = media_type(&response);
        let bytes = read_limited(
            response,
            &cancellation,
            DispatchKind::Safe,
            MAX_EXPORT_RESPONSE_BYTES,
        )
        .await?;
        match format {
            "json" if is_json_media_type(&content_type) => serde_json::from_slice(&bytes)
                .map(GroceryExport::Json)
                .map_err(|_| PortError::new("response_json", "service response is invalid JSON")),
            "markdown" if content_type == "text/markdown" => {
                decode_text(bytes).map(GroceryExport::Markdown)
            }
            "text" if content_type == "text/plain" => decode_text(bytes).map(GroceryExport::Text),
            _ => Err(PortError::new(
                "response_content_type",
                "service returned an unexpected export content type",
            )),
        }
    }

    pub async fn health_context(
        &self,
        credentials: &SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<HealthContextWire, PortError> {
        let builder = self
            .request(
                Method::GET,
                "/v1/health/context",
                Some(credentials),
                operation_id,
            )?
            .header(header::ACCEPT, "application/json");
        self.dispatch_json(builder, cancellation, DispatchKind::Safe)
            .await
    }

    pub async fn health_integrations(
        &self,
        credentials: &SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<IntegrationListWire, PortError> {
        let builder = self
            .request(
                Method::GET,
                "/v1/integrations",
                Some(credentials),
                operation_id,
            )?
            .header(header::ACCEPT, "application/json");
        self.dispatch_json(builder, cancellation, DispatchKind::Safe)
            .await
    }

    pub async fn health_authorize_oura(
        &self,
        credentials: &SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<IntegrationAuthorizeResponseWire, PortError> {
        let cli_auth = self.cli_auth.as_ref().ok_or_else(|| {
            PortError::new(
                "integration_context",
                "CLI auth context is required for integration authorization",
            )
        })?;
        let body = IntegrationAuthorizeRequestWire {
            device_id: cli_auth.device_id.clone(),
            provider: heyfood_core::HealthProvider::Oura,
            redirect_target: IntegrationRedirectTargetWire::Cli,
        };
        let builder = self
            .request(
                Method::POST,
                "/v1/integrations/authorize",
                Some(credentials),
                operation_id,
            )?
            .header(header::ACCEPT, "application/json")
            .json(&body);
        self.dispatch_json(builder, cancellation, DispatchKind::Mutation)
            .await
    }

    pub async fn health_sync_oura(
        &self,
        credentials: &SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<IntegrationSyncResponseWire, PortError> {
        let builder = self
            .request(
                Method::POST,
                "/v1/integrations/oura/sync",
                Some(credentials),
                operation_id,
            )?
            .header(header::ACCEPT, "application/json");
        self.dispatch_json(builder, cancellation, DispatchKind::Mutation)
            .await
    }

    pub async fn health_disconnect_oura(
        &self,
        credentials: &SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<IntegrationDisconnectResponseWire, PortError> {
        let builder = self
            .request(
                Method::DELETE,
                "/v1/integrations/oura",
                Some(credentials),
                operation_id,
            )?
            .header(header::ACCEPT, "application/json");
        self.dispatch_json(builder, cancellation, DispatchKind::Mutation)
            .await
    }

    async fn grocery_mutation<B, T>(
        &self,
        capabilities: &ApplicationCapabilitiesWire,
        credentials: &SessionCredentials,
        operation_id: OperationId,
        path: &str,
        request: &B,
        cancellation: CancellationToken,
    ) -> Result<T, PortError>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        Self::require_grocery_v1(capabilities)?;
        let builder = self
            .request(Method::POST, path, Some(credentials), operation_id)?
            .header(header::ACCEPT, "application/json")
            .json(request);
        self.dispatch_json(builder, cancellation, DispatchKind::Mutation)
            .await
    }

    fn request(
        &self,
        method: Method,
        path: &str,
        credentials: Option<&SessionCredentials>,
        operation_id: OperationId,
    ) -> Result<RequestBuilder, PortError> {
        let client = self.client(false)?;
        let endpoint = self.endpoint(path)?;
        let mut builder = client
            .request(method, endpoint)
            .header("X-App-Client-ID", "heyfood-cli")
            .header("X-Request-ID", operation_id.as_uuid().to_string());
        if let Some(cli_auth) = self.cli_auth.as_ref() {
            builder = builder.header("X-Device-ID", &cli_auth.device_id);
            if let Some(api_key) = cli_auth.api_key.as_ref() {
                builder = builder.header("X-API-Key", api_key.expose_secret());
            }
        }
        if let Some(credentials) = credentials {
            builder = builder.bearer_auth(credentials.access_token.expose_secret());
        }
        Ok(builder)
    }

    async fn dispatch_json<T: DeserializeOwned>(
        &self,
        builder: RequestBuilder,
        cancellation: CancellationToken,
        kind: DispatchKind,
    ) -> Result<T, PortError> {
        let response = self.dispatch(builder, &cancellation, kind).await?;
        let content_type = media_type(&response);
        if !is_json_media_type(&content_type) {
            return Err(dispatch_error(
                kind,
                "response_content_type",
                "service response is not JSON",
            ));
        }
        let bytes = read_limited(response, &cancellation, kind, MAX_JSON_RESPONSE_BYTES).await?;
        serde_json::from_slice(&bytes)
            .map_err(|_| dispatch_error(kind, "response_json", "service response is invalid JSON"))
    }

    async fn dispatch(
        &self,
        builder: RequestBuilder,
        cancellation: &CancellationToken,
        kind: DispatchKind,
    ) -> Result<Response, PortError> {
        if cancellation.is_cancelled() {
            return Err(PortError::new(
                "request_cancelled_before_dispatch",
                "request was cancelled before dispatch",
            ));
        }
        let send = builder.send();
        let response = tokio::select! {
            () = cancellation.cancelled() => {
                return Err(dispatch_error(kind, "request_cancelled_after_dispatch", "service response was not observed after request dispatch"));
            }
            result = send => result.map_err(|error| {
                if kind.uncertain() {
                    uncertain_transport("request_transport", &error)
                } else {
                    PortError::new("request_transport", crate::sanitized_reqwest_error(&error))
                }
            })?,
        };
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(http_status_error(response.status()))
        }
    }
}

fn dispatch_error(kind: DispatchKind, code: &'static str, message: &'static str) -> PortError {
    if kind.uncertain() {
        PortError::uncertain(code, message)
    } else {
        PortError::new(code, message)
    }
}

fn http_status_error(status: StatusCode) -> PortError {
    let (code, message) = match status.as_u16() {
        401 => ("login_required", "authentication is required"),
        403 => ("scope_required", "the session lacks a required scope"),
        404 => ("resource_not_found", "the requested resource was not found"),
        409 => (
            "version_conflict",
            "the resource version changed; fetch it again",
        ),
        422 => ("invalid_request", "the service rejected the request"),
        429 => ("rate_limited", "the service rate limit was reached"),
        503 => (
            "service_unavailable",
            "the service is temporarily unavailable",
        ),
        _ => ("http_status", "the service returned an unsuccessful status"),
    };
    PortError::new(code, message)
}

async fn read_limited(
    mut response: Response,
    cancellation: &CancellationToken,
    kind: DispatchKind,
    maximum: usize,
) -> Result<Vec<u8>, PortError> {
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(dispatch_error(
            kind,
            "response_too_large",
            "service response exceeds its size limit",
        ));
    }
    let mut bytes = Vec::new();
    loop {
        let chunk = tokio::select! {
            () = cancellation.cancelled() => {
                return Err(dispatch_error(kind, "response_cancelled", "response body was not observed after request dispatch"));
            }
            result = response.chunk() => result.map_err(|error| {
                if kind.uncertain() {
                    uncertain_transport("response_transport", &error)
                } else {
                    PortError::new("response_transport", crate::sanitized_reqwest_error(&error))
                }
            })?,
        };
        let Some(chunk) = chunk else {
            return Ok(bytes);
        };
        if bytes.len().saturating_add(chunk.len()) > maximum {
            return Err(dispatch_error(
                kind,
                "response_too_large",
                "service response exceeds its size limit",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
}

fn media_type(response: &Response) -> String {
    response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn is_json_media_type(value: &str) -> bool {
    value == "application/json" || value.ends_with("+json")
}

fn decode_text(bytes: Vec<u8>) -> Result<String, PortError> {
    let text = String::from_utf8(bytes)
        .map_err(|_| PortError::new("response_utf8", "service response is not valid UTF-8"))?;
    Ok(terminal_safe_text(&text))
}
