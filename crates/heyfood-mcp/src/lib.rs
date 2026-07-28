//! Bounded, renderer-neutral MCP presentation over heyfood application ports.

#![forbid(unsafe_code)]
#![recursion_limit = "512"]

use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use heyfood_application::{
    BoxFuture, CapabilityPort, CapabilitySnapshot, DiscoverCapabilities, GroceryReadPort,
    ListMenuWatches, MenuWatchReadPort, OptionalCapabilityStatus, PortError,
    ProfileReadinessStatus, ReadActiveGroceryDisplay, ReadGroceryExclusions, ReadStatus,
    RegistrationAvailability, StatusPort, VoiceReadinessStatus,
};
use heyfood_core::{GroceryCapability, OperationId, SessionCredentials};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ErrorCode, ErrorData, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
};
use rmcp::service::{RequestContext, RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::{RoleServer, ServerHandler, ServiceExt};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, Semaphore};
use tokio_util::sync::CancellationToken;

pub const MAX_INBOUND_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_TOOL_ARGUMENT_BYTES: usize = 1024 * 1024;
pub const MAX_STRUCTURED_RESULT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_OUTBOUND_FRAME_BYTES: usize = MAX_STRUCTURED_RESULT_BYTES + 64 * 1024;
pub const MAX_OUTSTANDING_REQUESTS: usize = 8;
pub const OUTBOUND_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

pub const TOOL_GET_MANIFEST: &str = "heyfood_get_manifest";
pub const TOOL_GET_STATUS: &str = "heyfood_get_status";
pub const TOOL_GET_CAPABILITIES: &str = "heyfood_get_capabilities";
pub const TOOL_GET_GROCERY_LIST: &str = "heyfood_get_grocery_list";
pub const TOOL_GET_GROCERY_EXCLUSIONS: &str = "heyfood_get_grocery_exclusions";
pub const TOOL_LIST_MENU_WATCHES: &str = "heyfood_list_menu_watches";

pub const TOOLS: [&str; 6] = [
    TOOL_GET_MANIFEST,
    TOOL_GET_STATUS,
    TOOL_GET_CAPABILITIES,
    TOOL_GET_GROCERY_LIST,
    TOOL_GET_GROCERY_EXCLUSIONS,
    TOOL_LIST_MENU_WATCHES,
];

/// Narrow object-safe composition used by MCP. It intentionally exposes no
/// mutation ports, raw HTTP client, filesystem access, or command execution.
pub trait McpReadService:
    CapabilityPort + StatusPort + GroceryReadPort + MenuWatchReadPort + Send + Sync
{
}

impl<T> McpReadService for T where
    T: CapabilityPort + StatusPort + GroceryReadPort + MenuWatchReadPort + Send + Sync
{
}

/// Account-bound session source composed by the executable. Implementations
/// may rotate and durably persist a session credential, which is why the MCP
/// tools deliberately do not claim the `readOnlyHint`.
pub trait McpSessionProvider: Send + Sync {
    fn current(
        &self,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<McpSessionContext, PortError>>;
}

#[derive(Clone)]
pub struct McpSessionContext {
    pub credentials: SessionCredentials,
    pub authorization_scope: Arc<str>,
}

impl McpSessionContext {
    #[must_use]
    pub fn new(credentials: SessionCredentials, authorization_scope: impl Into<Arc<str>>) -> Self {
        Self {
            credentials,
            authorization_scope: authorization_scope.into(),
        }
    }
}

#[derive(Clone)]
pub struct HeyfoodMcpServer {
    service: Arc<dyn McpReadService>,
    sessions: Arc<dyn McpSessionProvider>,
    outstanding: Arc<Semaphore>,
    remote: Arc<Semaphore>,
}

impl HeyfoodMcpServer {
    #[must_use]
    pub fn new(service: Arc<dyn McpReadService>, sessions: Arc<dyn McpSessionProvider>) -> Self {
        Self {
            service,
            sessions,
            outstanding: Arc::new(Semaphore::new(MAX_OUTSTANDING_REQUESTS)),
            remote: Arc::new(Semaphore::new(1)),
        }
    }

    #[must_use]
    pub fn tools() -> Vec<Tool> {
        TOOLS
            .into_iter()
            .map(|name| tool_definition(name).expect("frozen MCP tool has a definition"))
            .collect()
    }

    async fn execute(
        &self,
        request: CallToolRequestParams,
        cancellation: CancellationToken,
    ) -> Result<CallToolResult, ErrorData> {
        validate_empty_arguments(&request)?;
        if !TOOLS.contains(&request.name.as_ref()) {
            return Err(ErrorData::new(
                ErrorCode::METHOD_NOT_FOUND,
                format!("unknown heyfood tool {:?}", request.name),
                None,
            ));
        }
        let _outstanding = self.outstanding.clone().try_acquire_owned().map_err(|_| {
            ErrorData::new(
                ErrorCode(-32000),
                "heyfood MCP is at its bounded request limit",
                Some(json!({"code": "mcp_overloaded", "retryable": true})),
            )
        })?;

        if request.name == TOOL_GET_MANIFEST {
            return validated_bounded_success(
                TOOL_GET_MANIFEST,
                heyfood_agent_contract::manifest(),
            );
        }

        let _remote = tokio::select! {
            permit = self.remote.clone().acquire_owned() => permit.map_err(|_| {
                ErrorData::internal_error("heyfood MCP is shutting down", None)
            })?,
            () = cancellation.cancelled() => {
                return Ok(structured_error(PortError::new(
                    "mcp_cancelled_before_dispatch",
                    "The MCP request was cancelled before remote dispatch",
                )));
            }
        };
        if cancellation.is_cancelled() {
            return Ok(structured_error(PortError::new(
                "mcp_cancelled_before_dispatch",
                "The MCP request was cancelled before remote dispatch",
            )));
        }

        let result: Result<Value, PortError> = async {
            match request.name.as_ref() {
                TOOL_GET_CAPABILITIES => DiscoverCapabilities::new(self.service.as_ref())
                    .execute(cancellation)
                    .await
                    .map(capabilities_document),
                TOOL_GET_STATUS => {
                    let session = self.sessions.current(cancellation.child_token()).await?;
                    ReadStatus::new(self.service.as_ref())
                        .execute(
                            session.credentials,
                            &session.authorization_scope,
                            false,
                            cancellation,
                        )
                        .await
                        .map(status_document)
                }
                TOOL_GET_GROCERY_LIST => {
                    let session = self.sessions.current(cancellation.child_token()).await?;
                    let capabilities = DiscoverCapabilities::new(self.service.as_ref())
                        .execute(cancellation.child_token())
                        .await?;
                    ReadActiveGroceryDisplay::new(self.service.as_ref())
                        .execute(
                            capabilities,
                            session.credentials,
                            OperationId::new(),
                            cancellation,
                        )
                        .await
                        .and_then(|list| serialize_document("list", list))
                }
                TOOL_GET_GROCERY_EXCLUSIONS => {
                    let session = self.sessions.current(cancellation.child_token()).await?;
                    let capabilities = DiscoverCapabilities::new(self.service.as_ref())
                        .execute(cancellation.child_token())
                        .await?;
                    ReadGroceryExclusions::new(self.service.as_ref())
                        .execute(
                            capabilities,
                            session.credentials,
                            OperationId::new(),
                            cancellation,
                        )
                        .await
                        .and_then(|exclusions| serialize_document("grocery", exclusions))
                }
                TOOL_LIST_MENU_WATCHES => {
                    let session = self.sessions.current(cancellation.child_token()).await?;
                    ListMenuWatches::new(self.service.as_ref())
                        .execute(session.credentials, OperationId::new(), cancellation)
                        .await
                        .and_then(|watches| serialize_document("menu_watch", watches))
                }
                TOOL_GET_MANIFEST => unreachable!("manifest returns before remote dispatch"),
                _ => unreachable!("tool allowlist checked before dispatch"),
            }
        }
        .await;

        match result {
            Ok(value) => validated_bounded_success(request.name.as_ref(), value),
            Err(error) => Ok(structured_error(error)),
        }
    }
}

impl ServerHandler for HeyfoodMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("heyfood", heyfood_core::VERSION))
            .with_instructions(
                "Six bounded heyfood read/discovery tools are available. No mutation, shell, file, raw API, credential, or TUI-control tool exists.",
            )
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        tool_definition(name)
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        if request
            .as_ref()
            .and_then(|request| request.cursor.as_ref())
            .is_some()
        {
            return Err(ErrorData::invalid_params(
                "heyfood exposes a single bounded tool page",
                None,
            ));
        }
        Ok(ListToolsResult::with_all_items(Self::tools()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(request, context.ct).await
    }
}

pub async fn serve_stdio(server: HeyfoodMcpServer) -> Result<(), McpServeError> {
    let transport = BoundedStdioTransport::new(tokio::io::stdin(), tokio::io::stdout());
    let running = server
        .serve(transport)
        .await
        .map_err(|_| McpServeError::Startup)?;
    running
        .waiting()
        .await
        .map_err(|_| McpServeError::Runtime)?;
    Ok(())
}

#[derive(Debug)]
pub enum McpServeError {
    Startup,
    Runtime,
}

impl std::fmt::Display for McpServeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Startup => formatter.write_str("MCP protocol startup failed"),
            Self::Runtime => formatter.write_str("MCP protocol runtime failed"),
        }
    }
}

impl std::error::Error for McpServeError {}

fn validate_empty_arguments(request: &CallToolRequestParams) -> Result<(), ErrorData> {
    if request
        .arguments
        .as_ref()
        .is_some_and(|arguments| !arguments.is_empty())
    {
        return Err(ErrorData::invalid_params(
            "this heyfood tool accepts no arguments",
            None,
        ));
    }
    let argument_bytes = request.arguments.as_ref().map_or(0, |arguments| {
        serde_json::to_vec(arguments).map_or(usize::MAX, |v| v.len())
    });
    if argument_bytes > MAX_TOOL_ARGUMENT_BYTES {
        return Err(ErrorData::invalid_params(
            "tool arguments exceed the 1 MiB limit",
            None,
        ));
    }
    Ok(())
}

fn capabilities_document(snapshot: CapabilitySnapshot) -> Value {
    json!({
        "schema_version": 1,
        "registration": match snapshot.registration {
            RegistrationAvailability::Available => "available",
            RegistrationAvailability::Disabled => "disabled",
            RegistrationAvailability::Unavailable => "unavailable",
        },
        "profile_readiness": snapshot.profile_readiness,
        "loopback_pkce": snapshot.loopback_pkce,
        "device_code": snapshot.device_code,
        "grocery": match snapshot.grocery {
            GroceryCapability::V1 => "v1".to_owned(),
            GroceryCapability::Unavailable => "unavailable".to_owned(),
            GroceryCapability::UnsupportedVersion(version) => format!("unsupported:{version}"),
        }
    })
}

fn status_document(snapshot: heyfood_application::StatusSnapshot) -> Value {
    json!({
        "schema_version": 1,
        "service_reachable": snapshot.service_reachable,
        "registration": registration_status(snapshot.registration),
        "profile": profile_status(snapshot.profile),
        "grocery": optional_status(snapshot.grocery),
        "menu_watch": optional_status(snapshot.menu_watch),
        "voice": voice_status(snapshot.voice)
    })
}

const fn registration_status(value: RegistrationAvailability) -> &'static str {
    match value {
        RegistrationAvailability::Available => "available",
        RegistrationAvailability::Disabled => "disabled",
        RegistrationAvailability::Unavailable => "unavailable",
    }
}

const fn profile_status(value: ProfileReadinessStatus) -> &'static str {
    match value {
        ProfileReadinessStatus::NotAuthorized => "not_authorized",
        ProfileReadinessStatus::AuthorizedConsentGranted => "authorized_consent_granted",
        ProfileReadinessStatus::AuthorizedConsentNotGranted => "authorized_consent_not_granted",
    }
}

const fn optional_status(value: OptionalCapabilityStatus) -> &'static str {
    match value {
        OptionalCapabilityStatus::NotAdvertised => "not_advertised",
        OptionalCapabilityStatus::AuthorizationRequired => "authorization_required",
        OptionalCapabilityStatus::Authorized => "authorized",
    }
}

const fn voice_status(value: VoiceReadinessStatus) -> &'static str {
    match value {
        VoiceReadinessStatus::AuthorizationRequiredCaptureAvailable => {
            "authorization_required_capture_available"
        }
        VoiceReadinessStatus::AuthorizationRequiredCaptureUnavailable => {
            "authorization_required_capture_unavailable"
        }
        VoiceReadinessStatus::AuthorizedCaptureAvailable => "authorized_capture_available",
        VoiceReadinessStatus::AuthorizedCaptureUnavailable => "authorized_capture_unavailable",
    }
}

fn serialize_document(
    kind: &'static str,
    value: impl serde::Serialize,
) -> Result<Value, PortError> {
    let value = serde_json::to_value(value).map_err(|_| {
        PortError::new(
            "mcp_serialization",
            "The typed application result could not be serialized",
        )
    })?;
    Ok(json!({
        "schema_version": 1,
        "kind": kind,
        "data": value,
    }))
}

fn structured_error(error: PortError) -> CallToolResult {
    let value = json!({
        "schema_version": 1,
        "ok": false,
        "error": {
            "code": error.code,
            "message": "The heyfood service could not complete this read.",
            "outcome_uncertain": error.outcome_uncertain,
            "retryable": false,
        }
    });
    let mut result = CallToolResult::structured_error(value);
    result.content = vec![rmcp::model::ContentBlock::text(
        "The heyfood read did not complete. Inspect structuredContent for the typed error.",
    )];
    result
}

fn bounded_success(value: Value) -> Result<CallToolResult, ErrorData> {
    let bytes = serde_json::to_vec(&value).map_err(|_| {
        ErrorData::internal_error("heyfood could not serialize its typed result", None)
    })?;
    if bytes.len() > MAX_STRUCTURED_RESULT_BYTES {
        return Ok(structured_error(PortError::new(
            "mcp_result_too_large",
            "The structured result exceeds the 4 MiB MCP limit",
        )));
    }
    let mut result = CallToolResult::structured(value);
    result.content = vec![rmcp::model::ContentBlock::text(
        "Structured heyfood result attached.",
    )];
    Ok(result)
}

fn validated_bounded_success(name: &str, value: Value) -> Result<CallToolResult, ErrorData> {
    static VALIDATORS: LazyLock<BTreeMap<&'static str, jsonschema::Validator>> =
        LazyLock::new(|| {
            TOOLS
                .into_iter()
                .map(|name| {
                    let schema = tool_definition(name)
                        .and_then(|tool| tool.output_schema)
                        .map(|schema| Value::Object((*schema).clone()))
                        .expect("every frozen MCP tool has an output schema");
                    let validator = jsonschema::draft202012::new(&schema)
                        .expect("every frozen MCP output schema compiles");
                    (name, validator)
                })
                .collect()
        });
    let validator = VALIDATORS
        .get(name)
        .ok_or_else(|| ErrorData::internal_error("heyfood tool schema is unavailable", None))?;
    if !validator.is_valid(&value) {
        return Ok(structured_error(PortError::new(
            "mcp_output_schema_mismatch",
            "The typed application result did not match the frozen MCP output schema",
        )));
    }
    bounded_success(value)
}

fn tool_definition(name: &str) -> Option<Tool> {
    let description = match name {
        TOOL_GET_MANIFEST => "Return the exact embedded heyfood agent contract and build identity.",
        TOOL_GET_STATUS => {
            "Return authenticated service, profile, Grocery, Menu Watch, and local voice readiness."
        }
        TOOL_GET_CAPABILITIES => {
            "Discover the deployed hello.food capabilities without mutating product state."
        }
        TOOL_GET_GROCERY_LIST => {
            "Read the active household-aware Grocery list with safety and provenance."
        }
        TOOL_GET_GROCERY_EXCLUSIONS => "Read the account's Grocery never-buy exclusions.",
        TOOL_LIST_MENU_WATCHES => {
            "List recurring Menu Watch subscriptions and their latest change summaries."
        }
        _ => return None,
    };
    let input_schema = object_schema(json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {}
    }));
    let success_schema = match name {
        TOOL_GET_MANIFEST => serde_json::from_str(heyfood_agent_contract::MANIFEST_SCHEMA)
            .expect("embedded manifest schema is valid"),
        TOOL_GET_CAPABILITIES => capabilities_output_schema(),
        TOOL_GET_STATUS => status_output_schema(),
        TOOL_GET_GROCERY_LIST => grocery_list_output_schema(),
        TOOL_GET_GROCERY_EXCLUSIONS => grocery_exclusions_output_schema(),
        TOOL_LIST_MENU_WATCHES => menu_watches_output_schema(),
        _ => unreachable!("tool description match already rejected unknown names"),
    };
    let output_schema = object_schema(tool_output_schema(success_schema));
    let local = name == TOOL_GET_MANIFEST;
    Some(
        Tool::new(name.to_owned(), description, input_schema)
            .with_raw_output_schema(output_schema)
            .with_annotations(
                ToolAnnotations::new()
                    .destructive(false)
                    .idempotent(true)
                    .read_only(local)
                    .open_world(!local),
            ),
    )
}

fn tool_output_schema(success: Value) -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "oneOf": [success, tool_error_output_schema()]
    })
}

fn tool_error_output_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["schema_version", "ok", "error"],
        "properties": {
            "schema_version": {"const": 1},
            "ok": {"const": false},
            "error": {
                "type": "object",
                "additionalProperties": false,
                "required": ["code", "message", "outcome_uncertain", "retryable"],
                "properties": {
                    "code": {
                        "type": "string",
                        "pattern": "^[a-z][a-z0-9_]{0,127}$"
                    },
                    "message": {
                        "const": "The heyfood service could not complete this read."
                    },
                    "outcome_uncertain": {"type": "boolean"},
                    "retryable": {"const": false}
                }
            }
        }
    })
}

fn capabilities_output_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "additionalProperties": false,
        "required": [
            "schema_version", "registration", "profile_readiness",
            "loopback_pkce", "device_code", "grocery"
        ],
        "properties": {
            "schema_version": {"const": 1},
            "registration": {"enum": ["available", "disabled", "unavailable"]},
            "profile_readiness": {"type": "boolean"},
            "loopback_pkce": {"type": "boolean"},
            "device_code": {"type": "boolean"},
            "grocery": {
                "type": "string",
                "pattern": "^(v1|unavailable|unsupported:[A-Za-z0-9._-]{1,64})$"
            }
        }
    })
}

fn status_output_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "additionalProperties": false,
        "required": [
            "schema_version", "service_reachable", "registration", "profile",
            "grocery", "menu_watch", "voice"
        ],
        "properties": {
            "schema_version": {"const": 1},
            "service_reachable": {"type": "boolean"},
            "registration": {"enum": ["available", "disabled", "unavailable"]},
            "profile": {
                "enum": [
                    "not_authorized", "authorized_consent_granted",
                    "authorized_consent_not_granted"
                ]
            },
            "grocery": {
                "enum": ["not_advertised", "authorization_required", "authorized"]
            },
            "menu_watch": {
                "enum": ["not_advertised", "authorization_required", "authorized"]
            },
            "voice": {
                "enum": [
                    "authorization_required_capture_available",
                    "authorization_required_capture_unavailable",
                    "authorized_capture_available", "authorized_capture_unavailable"
                ]
            }
        }
    })
}

fn grocery_list_output_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "additionalProperties": false,
        "required": ["schema_version", "kind", "data"],
        "properties": {
            "schema_version": {"const": 1},
            "kind": {"const": "list"},
            "data": {
                "type": "object",
                "additionalProperties": false,
                "required": [
                    "id", "title", "state", "version", "items", "created_at", "updated_at"
                ],
                "properties": {
                    "id": bounded_string(),
                    "title": bounded_string(),
                    "state": bounded_string(),
                    "version": {"type": "integer", "minimum": 0},
                    "items": {
                        "type": "array",
                        "maxItems": 100,
                        "items": grocery_item_schema()
                    },
                    "created_at": bounded_string(),
                    "updated_at": bounded_string()
                }
            }
        }
    })
}

fn grocery_item_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": [
            "id", "requested_name", "canonical_name", "quantity", "unit",
            "package_quantity", "note", "state", "intended_for", "sources",
            "safety", "created_at", "updated_at"
        ],
        "properties": {
            "id": bounded_string(),
            "requested_name": bounded_string(),
            "canonical_name": bounded_string(),
            "quantity": nullable_number(),
            "unit": nullable_string(),
            "package_quantity": {
                "type": ["integer", "null"],
                "minimum": 0
            },
            "note": nullable_string(),
            "state": {"enum": ["active", "purchased", "dismissed"]},
            "intended_for": nullable_string(),
            "sources": {
                "type": "array",
                "maxItems": 100,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["source_type", "source_ref", "source_detail"],
                    "properties": {
                        "source_type": bounded_string(),
                        "source_ref": nullable_string(),
                        "source_detail": nullable_string()
                    }
                }
            },
            "safety": {
                "oneOf": [
                    {"type": "null"},
                    grocery_safety_schema()
                ]
            },
            "created_at": bounded_string(),
            "updated_at": bounded_string()
        }
    })
}

fn grocery_safety_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": [
            "basis", "status", "member_flags", "model_version", "rules_version",
            "confidence", "context_hash", "context_hash_version", "label_hint"
        ],
        "properties": {
            "basis": bounded_string(),
            "status": safety_status_schema(),
            "member_flags": {
                "type": "array",
                "maxItems": 100,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["member_id", "status", "reason", "substitutions"],
                    "properties": {
                        "member_id": bounded_string(),
                        "status": safety_status_schema(),
                        "reason": nullable_string(),
                        "substitutions": {
                            "type": "array",
                            "maxItems": 100,
                            "items": bounded_string()
                        }
                    }
                }
            },
            "model_version": nullable_string(),
            "rules_version": nullable_string(),
            "confidence": nullable_number(),
            "context_hash": nullable_string(),
            "context_hash_version": {"type": ["integer", "null"]},
            "label_hint": bounded_string()
        }
    })
}

fn grocery_exclusions_output_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "additionalProperties": false,
        "required": ["schema_version", "kind", "data"],
        "properties": {
            "schema_version": {"const": 1},
            "kind": {"const": "grocery"},
            "data": {
                "type": "object",
                "additionalProperties": false,
                "required": ["exclusions"],
                "properties": {
                    "exclusions": {
                        "type": "array",
                        "maxItems": 100,
                        "items": bounded_string()
                    }
                }
            }
        }
    })
}

fn menu_watches_output_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "additionalProperties": false,
        "required": ["schema_version", "kind", "data"],
        "properties": {
            "schema_version": {"const": 1},
            "kind": {"const": "menu_watch"},
            "data": {
                "type": "object",
                "additionalProperties": false,
                "required": ["watches", "count"],
                "properties": {
                    "count": {"type": "integer", "minimum": 0, "maximum": 100},
                    "watches": {
                        "type": "array",
                        "maxItems": 100,
                        "items": menu_watch_schema()
                    }
                }
            }
        }
    })
}

fn menu_watch_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": [
            "id", "restaurant_id", "cadence", "tz", "active", "notify",
            "next_run_at", "last_run_at", "last_snapshot_id", "created_at",
            "menu_url", "identity_verdict", "identity_confidence",
            "identity_reasoning", "identity_confirmed", "last_change"
        ],
        "properties": {
            "id": uuid_schema(),
            "restaurant_id": uuid_schema(),
            "cadence": {
                "type": "object",
                "additionalProperties": false,
                "required": ["weekday", "hour"],
                "properties": {
                    "weekday": {"type": "integer", "minimum": 0, "maximum": 6},
                    "hour": {"type": "integer", "minimum": 0, "maximum": 23}
                }
            },
            "tz": bounded_string(),
            "active": {"type": "boolean"},
            "notify": {"type": "boolean"},
            "next_run_at": bounded_string(),
            "last_run_at": nullable_string(),
            "last_snapshot_id": nullable_string(),
            "created_at": bounded_string(),
            "menu_url": nullable_string(),
            "identity_verdict": nullable_string(),
            "identity_confidence": nullable_number(),
            "identity_reasoning": nullable_string(),
            "identity_confirmed": {"type": ["boolean", "null"]},
            "last_change": {
                "oneOf": [
                    {"type": "null"},
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "required": [
                            "changed_at", "previous_snapshot_id", "new_snapshot_id", "summary"
                        ],
                        "properties": {
                            "changed_at": bounded_string(),
                            "previous_snapshot_id": bounded_string(),
                            "new_snapshot_id": bounded_string(),
                            "summary": {
                                "type": "object",
                                "additionalProperties": false,
                                "required": [
                                    "added", "removed", "modified",
                                    "price_increases", "price_decreases"
                                ],
                                "properties": {
                                    "added": {"type": "integer", "minimum": 0},
                                    "removed": {"type": "integer", "minimum": 0},
                                    "modified": {"type": "integer", "minimum": 0},
                                    "price_increases": {"type": "integer", "minimum": 0},
                                    "price_decreases": {"type": "integer", "minimum": 0}
                                }
                            }
                        }
                    }
                ]
            }
        }
    })
}

fn safety_status_schema() -> Value {
    json!({"enum": ["generally_safer", "risky", "avoid", "unable_to_evaluate"]})
}

fn bounded_string() -> Value {
    json!({"type": "string", "maxLength": 65536})
}

fn nullable_string() -> Value {
    json!({"type": ["string", "null"], "maxLength": 65536})
}

fn nullable_number() -> Value {
    json!({"type": ["number", "null"]})
}

fn uuid_schema() -> Value {
    json!({
        "type": "string",
        "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
    })
}

fn object_schema(value: Value) -> Arc<Map<String, Value>> {
    Arc::new(
        value
            .as_object()
            .expect("frozen MCP schema must be an object")
            .clone(),
    )
}

/// Newline-delimited JSON-RPC transport with a hard inbound bound. The
/// upstream SDK's stock stdio receiver uses an unbounded `read_until`; this
/// implementation keeps the official message/service types while bounding
/// allocation before parsing.
pub struct BoundedStdioTransport<R, W> {
    read: BufReader<R>,
    line: Vec<u8>,
    write: Arc<Mutex<Option<W>>>,
}

impl<R, W> BoundedStdioTransport<R, W>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    #[must_use]
    pub fn new(read: R, write: W) -> Self {
        Self {
            read: BufReader::with_capacity(16 * 1024, read),
            line: Vec::with_capacity(16 * 1024),
            write: Arc::new(Mutex::new(Some(write))),
        }
    }
}

async fn write_encoded_frame<W>(
    write: Arc<Mutex<Option<W>>>,
    encoded: Vec<u8>,
) -> Result<(), io::Error>
where
    W: AsyncWrite + Unpin,
{
    if encoded.len() > MAX_OUTBOUND_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "outbound MCP frame exceeds the configured bound",
        ));
    }
    tokio::time::timeout(OUTBOUND_WRITE_TIMEOUT, async {
        let mut guard = write.lock().await;
        let writer = guard
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "MCP is closed"))?;
        writer.write_all(&encoded).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await
    })
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "outbound MCP frame exceeded the 5 second write deadline",
        )
    })?
}

impl<R, W> rmcp::transport::Transport<RoleServer> for BoundedStdioTransport<R, W>
where
    R: AsyncRead + Send + Unpin,
    W: AsyncWrite + Send + Unpin + 'static,
{
    type Error = io::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let write = self.write.clone();
        async move {
            let encoded = serde_json::to_vec(&item)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            write_encoded_frame(write, encoded).await
        }
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        loop {
            let available = match self.read.fill_buf().await {
                Ok(bytes) => bytes,
                Err(_) => return None,
            };
            if available.is_empty() {
                self.line.clear();
                return None;
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let take = newline.map_or(available.len(), |index| index + 1);
            let framed_bytes = self
                .line
                .len()
                .saturating_add(take)
                .saturating_sub(usize::from(newline.is_some()));
            if framed_bytes > MAX_INBOUND_FRAME_BYTES {
                self.read.consume(take);
                self.line.clear();
                let response = TxJsonRpcMessage::<RoleServer>::error(
                    ErrorData::invalid_request("MCP frame exceeds the 1 MiB limit", None),
                    None,
                );
                if let Ok(encoded) = serde_json::to_vec(&response) {
                    let _ = write_encoded_frame(self.write.clone(), encoded).await;
                }
                return None;
            }
            self.line.extend_from_slice(&available[..take]);
            self.read.consume(take);
            if newline.is_none() {
                continue;
            }
            if self.line.last() == Some(&b'\n') {
                self.line.pop();
            }
            if self.line.last() == Some(&b'\r') {
                self.line.pop();
            }
            if self.line.is_empty() {
                continue;
            }
            let parsed = serde_json::from_slice::<RxJsonRpcMessage<RoleServer>>(&self.line);
            self.line.clear();
            match parsed {
                Ok(message) => return Some(message),
                Err(_) => {
                    let response = TxJsonRpcMessage::<RoleServer>::error(
                        ErrorData::new(ErrorCode::PARSE_ERROR, "Parse error", None),
                        None,
                    );
                    let Ok(encoded) = serde_json::to_vec(&response) else {
                        return None;
                    };
                    if write_encoded_frame(self.write.clone(), encoded)
                        .await
                        .is_err()
                    {
                        return None;
                    }
                }
            }
        }
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        let mut writer = self.write.lock().await;
        if let Some(mut writer) = writer.take() {
            writer.shutdown().await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use heyfood_application::{
        CapabilitySnapshot, GroceryDisplayList, GroceryExclusions, MenuWatchList,
    };
    use heyfood_core::{AccountId, CredentialVersion, SensitiveString};
    use tokio::io::AsyncReadExt;
    use tokio::sync::Notify;

    use super::*;

    struct FakeService {
        calls: AtomicUsize,
    }

    impl CapabilityPort for FakeService {
        fn discover(
            &self,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<CapabilitySnapshot, PortError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {
                Ok(CapabilitySnapshot {
                    schema_version: 1,
                    registration: RegistrationAvailability::Available,
                    profile_readiness: true,
                    loopback_pkce: true,
                    device_code: true,
                    grocery: GroceryCapability::V1,
                })
            })
        }
    }

    impl StatusPort for FakeService {
        fn profile_consent_granted(
            &self,
            _credentials: SessionCredentials,
            _operation_id: OperationId,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<bool, PortError>> {
            Box::pin(async { Ok(true) })
        }
    }

    impl GroceryReadPort for FakeService {
        fn read_active_display(
            &self,
            _capabilities: CapabilitySnapshot,
            _credentials: SessionCredentials,
            _operation_id: OperationId,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<GroceryDisplayList, PortError>> {
            Box::pin(async {
                Ok(GroceryDisplayList {
                    id: "list".into(),
                    title: "Groceries".into(),
                    state: "active".into(),
                    version: 7,
                    items: vec![],
                    created_at: "2026-07-27T00:00:00Z".into(),
                    updated_at: "2026-07-27T00:00:00Z".into(),
                })
            })
        }

        fn read_exclusions(
            &self,
            _capabilities: CapabilitySnapshot,
            _credentials: SessionCredentials,
            _operation_id: OperationId,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<GroceryExclusions, PortError>> {
            Box::pin(async {
                Ok(GroceryExclusions {
                    exclusions: vec!["anchovies".into()],
                })
            })
        }
    }

    impl MenuWatchReadPort for FakeService {
        fn list(
            &self,
            _credentials: SessionCredentials,
            _operation_id: OperationId,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<MenuWatchList, PortError>> {
            Box::pin(async {
                Ok(MenuWatchList {
                    watches: vec![],
                    count: 0,
                })
            })
        }
    }

    struct FakeSessions;

    impl McpSessionProvider for FakeSessions {
        fn current(
            &self,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<McpSessionContext, PortError>> {
            Box::pin(async {
                let credentials = SessionCredentials::from_unix_expiry(
                    AccountId::parse("mcp-test").unwrap(),
                    SensitiveString::new("access"),
                    SensitiveString::new("refresh"),
                    CredentialVersion::new(1),
                    4_102_444_800,
                )
                .map_err(|message| PortError::new("test_credentials", message))?;
                Ok(McpSessionContext::new(
                    credentials,
                    "profile:read grocery:read menu:watch",
                ))
            })
        }
    }

    struct BlockingService {
        calls: AtomicUsize,
        cancelled: AtomicBool,
        entered: Arc<Notify>,
        release: Arc<Semaphore>,
    }

    impl CapabilityPort for BlockingService {
        fn discover(
            &self,
            cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<CapabilitySnapshot, PortError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            Box::pin(async move {
                tokio::select! {
                    permit = self.release.acquire() => {
                        let _permit = permit.expect("test release semaphore remains open");
                        Ok(CapabilitySnapshot {
                        schema_version: 1,
                        registration: RegistrationAvailability::Available,
                        profile_readiness: true,
                        loopback_pkce: true,
                        device_code: true,
                        grocery: GroceryCapability::V1,
                        })
                    },
                    () = cancellation.cancelled() => {
                        self.cancelled.store(true, Ordering::SeqCst);
                        Err(PortError::new("cancelled", "private cancellation detail"))
                    }
                }
            })
        }
    }

    impl StatusPort for BlockingService {
        fn profile_consent_granted(
            &self,
            _credentials: SessionCredentials,
            _operation_id: OperationId,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<bool, PortError>> {
            Box::pin(async { Err(PortError::new("not_called", "not called")) })
        }
    }

    impl GroceryReadPort for BlockingService {
        fn read_active_display(
            &self,
            _capabilities: CapabilitySnapshot,
            _credentials: SessionCredentials,
            _operation_id: OperationId,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<GroceryDisplayList, PortError>> {
            Box::pin(async { Err(PortError::new("not_called", "not called")) })
        }

        fn read_exclusions(
            &self,
            _capabilities: CapabilitySnapshot,
            _credentials: SessionCredentials,
            _operation_id: OperationId,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<GroceryExclusions, PortError>> {
            Box::pin(async { Err(PortError::new("not_called", "not called")) })
        }
    }

    impl MenuWatchReadPort for BlockingService {
        fn list(
            &self,
            _credentials: SessionCredentials,
            _operation_id: OperationId,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<MenuWatchList, PortError>> {
            Box::pin(async { Err(PortError::new("not_called", "not called")) })
        }
    }

    fn server() -> HeyfoodMcpServer {
        HeyfoodMcpServer::new(
            Arc::new(FakeService {
                calls: AtomicUsize::new(0),
            }),
            Arc::new(FakeSessions),
        )
    }

    #[test]
    fn exact_six_tool_allowlist_has_no_mutation_or_generic_escape_hatch() {
        let tools = HeyfoodMcpServer::tools();
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.name.as_ref())
                .collect::<Vec<_>>(),
            TOOLS
        );
        for tool in tools {
            let annotations = tool.annotations.unwrap();
            assert_eq!(
                annotations.read_only_hint,
                Some(tool.name == TOOL_GET_MANIFEST)
            );
            assert_eq!(annotations.destructive_hint, Some(false));
            assert!(!tool.name.contains("shell"));
            assert!(!tool.name.contains("file"));
            assert!(!tool.name.contains("confirm"));
            assert!(!tool.name.contains("create"));
            assert!(!tool.name.contains("remove"));
        }
    }

    #[tokio::test]
    async fn manifest_is_network_and_session_free() {
        let service = Arc::new(FakeService {
            calls: AtomicUsize::new(0),
        });
        let server = HeyfoodMcpServer::new(service.clone(), Arc::new(FakeSessions));
        let result = server
            .execute(
                CallToolRequestParams::new(TOOL_GET_MANIFEST),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(false));
        assert_eq!(service.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn all_remote_tools_return_bounded_structured_documents() {
        for name in &TOOLS[1..] {
            let tool = tool_definition(name).unwrap();
            let result = server()
                .execute(CallToolRequestParams::new(*name), CancellationToken::new())
                .await
                .unwrap();
            assert_eq!(result.is_error, Some(false), "{name}");
            let value = result.structured_content.unwrap();
            assert_eq!(value["schema_version"], 1);
            assert!(serde_json::to_vec(&value).unwrap().len() <= MAX_STRUCTURED_RESULT_BYTES);
            let schema = Value::Object((*tool.output_schema.unwrap()).clone());
            jsonschema::draft202012::validate(&schema, &value)
                .unwrap_or_else(|error| panic!("{name} output schema mismatch: {error}"));
        }
    }

    #[tokio::test]
    async fn cancellation_before_remote_permit_dispatches_nothing() {
        let service = Arc::new(FakeService {
            calls: AtomicUsize::new(0),
        });
        let server = HeyfoodMcpServer::new(service.clone(), Arc::new(FakeSessions));
        let permit = server.remote.clone().acquire_owned().await.unwrap();
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let result = server
            .execute(
                CallToolRequestParams::new(TOOL_GET_CAPABILITIES),
                cancellation,
            )
            .await
            .unwrap();
        drop(permit);
        assert_eq!(result.is_error, Some(true));
        assert_eq!(service.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn bounded_transport_accepts_split_frames_and_rejects_oversize() {
        let (mut input_writer, input_reader) = tokio::io::duplex(MAX_INBOUND_FRAME_BYTES + 4096);
        let (output_writer, mut output_reader) = tokio::io::duplex(4096);
        let mut transport = BoundedStdioTransport::new(input_reader, output_writer);

        let frame = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n";
        input_writer.write_all(&frame[..15]).await.unwrap();
        input_writer.write_all(&frame[15..]).await.unwrap();
        assert!(
            rmcp::transport::Transport::<RoleServer>::receive(&mut transport)
                .await
                .is_some()
        );

        input_writer
            .write_all(&vec![b'x'; MAX_INBOUND_FRAME_BYTES + 1])
            .await
            .unwrap();
        assert!(
            rmcp::transport::Transport::<RoleServer>::receive(&mut transport)
                .await
                .is_none()
        );
        let mut error = vec![0; 4096];
        let read = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            tokio::io::AsyncReadExt::read(&mut output_reader, &mut error),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            std::str::from_utf8(&error[..read])
                .unwrap()
                .contains("1 MiB")
        );
    }

    #[tokio::test]
    async fn transport_handles_coalesced_malformed_and_invalid_utf8_frames() {
        let (mut input_writer, input_reader) = tokio::io::duplex(4096);
        let (output_writer, mut output_reader) = tokio::io::duplex(4096);
        let mut transport = BoundedStdioTransport::new(input_reader, output_writer);
        input_writer
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n",
            )
            .await
            .unwrap();
        assert!(
            rmcp::transport::Transport::<RoleServer>::receive(&mut transport)
                .await
                .is_some()
        );
        assert!(
            rmcp::transport::Transport::<RoleServer>::receive(&mut transport)
                .await
                .is_some()
        );

        input_writer.write_all(b"{not-json}\n").await.unwrap();
        input_writer.write_all(&[0xff, b'\n']).await.unwrap();
        input_writer.shutdown().await.unwrap();
        assert!(
            rmcp::transport::Transport::<RoleServer>::receive(&mut transport)
                .await
                .is_none()
        );
        let mut output = vec![0; 2048];
        let read = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            tokio::io::AsyncReadExt::read(&mut output_reader, &mut output),
        )
        .await
        .unwrap()
        .unwrap();
        let output = std::str::from_utf8(&output[..read]).unwrap();
        assert_eq!(output.matches("Parse error").count(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn slow_reader_hits_the_bounded_outbound_deadline() {
        let (_input_writer, input_reader) = tokio::io::duplex(16);
        let (output_writer, _output_reader) = tokio::io::duplex(1);
        let mut transport = BoundedStdioTransport::new(input_reader, output_writer);
        let message = TxJsonRpcMessage::<RoleServer>::error(
            ErrorData::internal_error("deliberately larger than one byte", None),
            None,
        );
        let error = rmcp::transport::Transport::<RoleServer>::send(&mut transport, message)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    #[tokio::test(start_paused = true)]
    async fn malformed_input_error_also_obeys_the_outbound_deadline() {
        let (mut input_writer, input_reader) = tokio::io::duplex(16);
        let (output_writer, _output_reader) = tokio::io::duplex(1);
        let mut transport = BoundedStdioTransport::new(input_reader, output_writer);
        input_writer.write_all(b"x\n").await.unwrap();
        assert!(
            rmcp::transport::Transport::<RoleServer>::receive(&mut transport)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn clean_eof_terminates_the_transport_without_output() {
        let (input_writer, input_reader) = tokio::io::duplex(16);
        let (output_writer, mut output_reader) = tokio::io::duplex(16);
        let mut transport = BoundedStdioTransport::new(input_reader, output_writer);
        drop(input_writer);
        assert!(
            rmcp::transport::Transport::<RoleServer>::receive(&mut transport)
                .await
                .is_none()
        );
        rmcp::transport::Transport::<RoleServer>::close(&mut transport)
            .await
            .unwrap();
        let mut output = Vec::new();
        output_reader.read_to_end(&mut output).await.unwrap();
        assert!(output.is_empty());
    }

    #[tokio::test]
    async fn ninth_request_is_rejected_while_eight_are_bounded() {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Semaphore::new(0));
        let service = Arc::new(BlockingService {
            calls: AtomicUsize::new(0),
            cancelled: AtomicBool::new(false),
            entered: entered.clone(),
            release: release.clone(),
        });
        let server = HeyfoodMcpServer::new(service, Arc::new(FakeSessions));
        let mut requests = Vec::new();
        for _ in 0..MAX_OUTSTANDING_REQUESTS {
            let server = server.clone();
            requests.push(tokio::spawn(async move {
                server
                    .execute(
                        CallToolRequestParams::new(TOOL_GET_CAPABILITIES),
                        CancellationToken::new(),
                    )
                    .await
            }));
        }
        entered.notified().await;
        let overloaded = server
            .execute(
                CallToolRequestParams::new(TOOL_GET_CAPABILITIES),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(overloaded.data.unwrap()["code"], "mcp_overloaded");
        release.add_permits(MAX_OUTSTANDING_REQUESTS);
        for request in requests {
            assert!(request.await.unwrap().is_ok());
        }
    }

    #[tokio::test]
    async fn in_flight_cancellation_reaches_the_application_port_and_redacts_details() {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Semaphore::new(0));
        let service = Arc::new(BlockingService {
            calls: AtomicUsize::new(0),
            cancelled: AtomicBool::new(false),
            entered: entered.clone(),
            release,
        });
        let server = HeyfoodMcpServer::new(service.clone(), Arc::new(FakeSessions));
        let cancellation = CancellationToken::new();
        let task = tokio::spawn({
            let server = server.clone();
            let cancellation = cancellation.clone();
            async move {
                server
                    .execute(
                        CallToolRequestParams::new(TOOL_GET_CAPABILITIES),
                        cancellation,
                    )
                    .await
                    .unwrap()
            }
        });
        entered.notified().await;
        cancellation.cancel();
        let result = task.await.unwrap();
        assert_eq!(result.is_error, Some(true));
        let encoded = serde_json::to_string(&result).unwrap();
        assert!(!encoded.contains("private cancellation detail"));
        assert!(service.cancelled.load(Ordering::SeqCst));
    }

    #[test]
    fn oversized_results_fail_as_typed_errors_without_partial_content() {
        let result =
            bounded_success(json!({"value": "x".repeat(MAX_STRUCTURED_RESULT_BYTES)})).unwrap();
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content.unwrap()["error"]["code"],
            "mcp_result_too_large"
        );
    }

    #[test]
    fn out_of_contract_success_is_converted_to_a_typed_error() {
        let result = validated_bounded_success(
            TOOL_LIST_MENU_WATCHES,
            json!({
                "schema_version": 1,
                "kind": "menu_watch",
                "data": {
                    "count": 101,
                    "watches": []
                }
            }),
        )
        .unwrap();
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content.unwrap()["error"]["code"],
            "mcp_output_schema_mismatch"
        );
    }

    #[test]
    fn typed_tool_errors_conform_to_every_advertised_output_schema() {
        let result = structured_error(PortError::uncertain(
            "backend_outcome_uncertain",
            "private backend detail",
        ));
        let value = result.structured_content.unwrap();
        for tool in HeyfoodMcpServer::tools() {
            let schema = Value::Object((*tool.output_schema.unwrap()).clone());
            jsonschema::draft202012::validate(&schema, &value)
                .unwrap_or_else(|error| panic!("{} error schema mismatch: {error}", tool.name));
        }
    }
}
