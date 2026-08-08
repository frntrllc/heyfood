//! Contract-derived Phase 2 REST operations.

use heyfood_application::{
    AuthoritativeConsentStateV1, BoxFuture, CapabilityPort, CapabilitySnapshot,
    CreateMenuWatchRequest, DeployedGroceryMutationRequest, DietPort, GroceryDisplayItem,
    GroceryDisplayList, GroceryDisplayMemberFlag, GroceryDisplaySafety, GroceryDisplaySource,
    GroceryExclusions, GroceryExport, GroceryExportPort, GroceryMutationPort, GroceryReadPort,
    LogoutRemotePort, MenuWatchChangeEvent, MenuWatchChangeSummary, MenuWatchList, MenuWatchPort,
    MenuWatchReadPort, MenuWatchSnapshot, PortError, RegistrationAvailability, StatusPort,
};
use heyfood_core::{
    AddItemsRequestWire, ApplicationCapabilitiesWire, AuthCredentialBundle,
    AuthorizationServerMetadataWire, ConsentVersionV1, DietCapability, DietCatalog,
    DietCatalogResponseWire, DietDetail, DietDetailResponseWire, ExclusionListResponseWire,
    ExclusionMutationRequestWire, GroceryCapability, GroceryEntityId, GroceryListWire,
    GroceryMutationConfirmRequestWire, GroceryMutationProposalWire, GroceryMutationResultWire,
    HealthContextWire, IntegrationAuthorizeRequestWire, IntegrationAuthorizeResponseWire,
    IntegrationDisconnectResponseWire, IntegrationListWire, IntegrationRedirectTargetWire,
    IntegrationSyncResponseWire, MAX_OWNER_SYNC_REQUEST_BODY_BYTES, MenuWatchCreateRequestWire,
    MenuWatchId, MenuWatchListResponseWire, MenuWatchResponseWire, OperationId,
    RemoveItemsRequestWire, SelfRegistrationStatusWire, SessionCredentials, TRANSCRIPTION_CHANNELS,
    TRANSCRIPTION_CLIENT_ERROR_KINDS, TRANSCRIPTION_MAX_AUDIO_BYTES,
    TRANSCRIPTION_MAX_DURATION_SECONDS, TRANSCRIPTION_MAX_LANGUAGE_CHARACTERS,
    TRANSCRIPTION_MAX_REQUEST_BYTES, TRANSCRIPTION_SAMPLE_WIDTH_BYTES,
    TRANSCRIPTION_WAV_HEADER_BYTES, Transcription, TranscriptionPurpose, TranscriptionWire,
    UpdateItemStateRequestWire, required_text, terminal_safe_text,
    transcription_sample_rate_supported,
};
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode, header};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{HttpService, uncertain_transport};

pub const MAX_JSON_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_EXPORT_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_TRANSCRIPTION_RESPONSE_BYTES: usize = 128 * 1024;
const MAX_TRANSCRIPTION_ERROR_BYTES: usize = 16 * 1024;
const MAX_MENU_WATCH_ERROR_BYTES: usize = 16 * 1024;
const MAX_DIET_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_DIET_ERROR_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy)]
enum DispatchKind {
    Safe,
    Mutation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerSyncOutcomeUncertainReasonV1 {
    Timeout,
    CancelledAfterSend,
    Transport,
    BodyRead,
    BodyTooLarge,
}

#[derive(Clone, Eq, PartialEq)]
pub enum OwnerSyncTransportResultV1 {
    CancelledBeforeSend,
    Response {
        status: u16,
        body: Vec<u8>,
    },
    OutcomeUncertain {
        reason: OwnerSyncOutcomeUncertainReasonV1,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileConsentStatusWireV1 {
    has_consent: bool,
    #[serde(default)]
    granted_at: Option<String>,
    #[serde(default)]
    consent_version: Option<ConsentVersionV1>,
}

#[derive(Default, Deserialize)]
struct DietErrorEnvelopeWire {
    #[serde(default)]
    details: Option<DietErrorDetailsWire>,
}

#[derive(Default, Deserialize)]
struct DietErrorDetailsWire {
    #[serde(default)]
    reason: Option<String>,
}

impl ProfileConsentStatusWireV1 {
    fn authoritative_state(&self) -> Option<AuthoritativeConsentStateV1> {
        let _ = &self.granted_at;
        match (self.has_consent, self.consent_version) {
            (true, Some(version)) => Some(AuthoritativeConsentStateV1::Active(version)),
            (false, None) => Some(AuthoritativeConsentStateV1::Absent),
            (true, None) | (false, Some(_)) => None,
        }
    }
}

impl std::fmt::Debug for OwnerSyncTransportResultV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CancelledBeforeSend => formatter.write_str("CancelledBeforeSend"),
            Self::Response { status, body } => formatter
                .debug_struct("Response")
                .field("status", status)
                .field("body_bytes", &body.len())
                .finish(),
            Self::OutcomeUncertain { reason } => formatter
                .debug_struct("OutcomeUncertain")
                .field("reason", reason)
                .finish(),
        }
    }
}

impl DispatchKind {
    const fn uncertain(self) -> bool {
        matches!(self, Self::Mutation)
    }
}

impl HttpService {
    /// Dispatch an already-frozen owner-profile sync request without
    /// classifying its HTTP result. Every observed status and bounded raw body
    /// crosses this boundary unchanged so the household application state
    /// machine can reconcile before any retry.
    pub async fn send_owner_profile_sync_v1(
        &self,
        credentials: &SessionCredentials,
        request_body: &[u8],
        remote_request_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<OwnerSyncTransportResultV1, PortError> {
        if request_body.is_empty() || request_body.len() > MAX_OWNER_SYNC_REQUEST_BODY_BYTES {
            return Err(PortError::new(
                "owner_sync_request_body",
                "frozen owner profile sync request body is invalid",
            ));
        }
        let builder = self
            .request(
                Method::PUT,
                "/v1/profile/sync",
                Some(credentials),
                remote_request_id,
            )?
            .header(header::ACCEPT, "application/json")
            .header(header::CONTENT_TYPE, "application/json")
            .body(request_body.to_vec());
        Ok(dispatch_owner_sync_transport(builder, cancellation).await)
    }

    /// Upload one bounded in-memory WAV using channel authority. This endpoint
    /// performs transcription only; it never submits the transcript to the
    /// agent or converts it into mutation consent.
    pub async fn transcribe_audio(
        &self,
        wav_bytes: &[u8],
        purpose: TranscriptionPurpose,
        language: Option<&str>,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<Transcription, PortError> {
        self.transcribe_audio_inner(wav_bytes, purpose, language, operation_id, cancellation)
            .await
            .map_err(transcription_public_error)
    }

    async fn transcribe_audio_inner(
        &self,
        wav_bytes: &[u8],
        purpose: TranscriptionPurpose,
        language: Option<&str>,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<Transcription, PortError> {
        validate_transcription_wav(wav_bytes)?;
        let language = language
            .map(|value| required_text(value, TRANSCRIPTION_MAX_LANGUAGE_CHARACTERS))
            .transpose()
            .map_err(|_| {
                PortError::new(
                    "transcription_language",
                    "the transcription language tag is invalid",
                )
            })?;
        let cli_auth = self.cli_auth.as_ref().ok_or_else(|| {
            PortError::new(
                "transcription_context",
                "CLI channel authorization is required for transcription",
            )
        })?;
        let boundary = format!("heyfood-{}", operation_id.as_uuid().simple());
        let body = transcription_multipart(
            &boundary,
            wav_bytes,
            purpose.as_contract_value(),
            language.as_deref(),
        )?;
        let client = self.client_with_timeout(Some(self.deadlines.transcription))?;
        let builder = self
            .request_with_client(
                &client,
                Method::POST,
                "/v1/audio/transcriptions",
                None,
                operation_id,
            )?
            .header(header::ACCEPT, "application/json")
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .bearer_auth(cli_auth.channel_access_token.expose_secret())
            .body(body);
        let response = self.dispatch_transcription(builder, &cancellation).await?;
        if !is_json_media_type(&media_type(&response)) {
            return Err(PortError::new(
                "transcription_contract_error",
                "transcription response is not JSON",
            ));
        }
        let bytes = read_limited(
            response,
            &cancellation,
            DispatchKind::Safe,
            MAX_TRANSCRIPTION_RESPONSE_BYTES,
        )
        .await?;
        let wire: TranscriptionWire = serde_json::from_slice(&bytes).map_err(|_| {
            PortError::new(
                "transcription_contract_error",
                "transcription response is invalid JSON",
            )
        })?;
        Transcription::from_wire(wire)
            .map_err(|error| PortError::new("transcription_contract_error", error.to_string()))
    }

    /// Read profile consent for household-context construction. This is a
    /// safe, authenticated GET and never changes consent state.
    pub async fn profile_consent_status(
        &self,
        credentials: &SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<Value, PortError> {
        let builder = self
            .request(Method::GET, "/v1/profile/consent", None, operation_id)?
            .header(header::ACCEPT, "application/json")
            .bearer_auth(credentials.access_token.expose_secret());
        self.dispatch_json(builder, cancellation, DispatchKind::Safe)
            .await
    }

    /// Read and strictly validate the authoritative profile-sync consent
    /// state. Active consent always carries the exact PostgreSQL-compatible
    /// integer version; absence cannot carry a contradictory version.
    pub async fn profile_consent_authority_v1(
        &self,
        credentials: &SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<AuthoritativeConsentStateV1, PortError> {
        let builder = self
            .request(Method::GET, "/v1/profile/consent", None, operation_id)?
            .header(header::ACCEPT, "application/json")
            .bearer_auth(credentials.access_token.expose_secret());
        let response: ProfileConsentStatusWireV1 = self
            .dispatch_profile_consent_wire(builder, cancellation, DispatchKind::Safe)
            .await?;
        response
            .authoritative_state()
            .ok_or_else(|| profile_consent_contract_error(DispatchKind::Safe))
    }

    /// Read one member's synchronized dietary profile. Household metadata
    /// remains local and this request is made only after positive consent.
    pub async fn download_profile(
        &self,
        credentials: &SessionCredentials,
        member_id: &str,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<Value, PortError> {
        let builder = self
            .request(Method::GET, "/v1/profile/sync", None, operation_id)?
            .header(header::ACCEPT, "application/json")
            .bearer_auth(credentials.access_token.expose_secret())
            .query(&[("member_id", member_id)]);
        self.dispatch_json(builder, cancellation, DispatchKind::Safe)
            .await
    }

    /// Grant the versioned account-level consent required for synchronized
    /// dietary profiles. This mutation is never retried automatically.
    pub async fn grant_profile_consent(
        &self,
        credentials: &SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<Value, PortError> {
        let builder = self
            .request(
                Method::POST,
                "/v1/profile/consent",
                Some(credentials),
                operation_id,
            )?
            .header(header::ACCEPT, "application/json")
            .json(&serde_json::json!({"consent_version": 1}));
        self.dispatch_json(builder, cancellation, DispatchKind::Mutation)
            .await
    }

    /// Grant owner profile-sync consent and return the exact authoritative
    /// version from the response. This method never substitutes the requested
    /// version for a missing, stale, or malformed response value.
    pub async fn grant_owner_profile_consent_v1(
        &self,
        credentials: &SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<ConsentVersionV1, PortError> {
        let builder = self
            .request(
                Method::POST,
                "/v1/profile/consent",
                Some(credentials),
                operation_id,
            )?
            .header(header::ACCEPT, "application/json")
            .json(&serde_json::json!({"consent_version": 1}));
        let response: ProfileConsentStatusWireV1 = self
            .dispatch_profile_consent_wire(builder, cancellation, DispatchKind::Mutation)
            .await?;
        match response
            .authoritative_state()
            .ok_or_else(|| profile_consent_contract_error(DispatchKind::Mutation))?
        {
            AuthoritativeConsentStateV1::Active(version) => Ok(version),
            AuthoritativeConsentStateV1::Absent | AuthoritativeConsentStateV1::Malformed => {
                Err(PortError::uncertain(
                    "profile_consent_contract",
                    "profile consent grant response is not authoritative",
                ))
            }
        }
    }

    /// Replace one synchronized dietary profile using the last observed
    /// version when present. This mutation is never retried automatically.
    pub async fn upload_profile(
        &self,
        credentials: &SessionCredentials,
        member_id: &str,
        profile_data: &Value,
        expected_version: Option<u64>,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<Value, PortError> {
        let mut body = serde_json::json!({
            "member_id": member_id,
            "profile_data": profile_data,
        });
        if let Some(expected_version) = expected_version {
            body["expected_version"] = Value::from(expected_version);
        }
        let builder = self
            .request(
                Method::PUT,
                "/v1/profile/sync",
                Some(credentials),
                operation_id,
            )?
            .header(header::ACCEPT, "application/json")
            .json(&body);
        self.dispatch_json(builder, cancellation, DispatchKind::Mutation)
            .await
    }

    /// Evaluate an item through the released provider-neutral channel-tool
    /// contract. This deliberately uses channel authority, matching the
    /// Python CLI, rather than converting the request into an agent prompt.
    pub async fn explain_item(
        &self,
        item_name: &str,
        restaurant_name: Option<&str>,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<Value, PortError> {
        let cli_auth = self.cli_auth.as_ref().ok_or_else(|| {
            PortError::new(
                "channel_tool_context",
                "CLI channel authorization is required for item evaluation",
            )
        })?;
        let body = serde_json::json!({
            "item_name": item_name,
            "restaurant_name": restaurant_name,
        });
        let builder = self
            .request(
                Method::POST,
                "/v1/channel/tools/explain_item",
                None,
                operation_id,
            )?
            .header(header::ACCEPT, "application/json")
            .bearer_auth(cli_auth.channel_access_token.expose_secret())
            .json(&body);
        self.dispatch_json(builder, cancellation, DispatchKind::Safe)
            .await
    }

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

    pub async fn menu_watch_list(
        &self,
        credentials: &SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<MenuWatchListResponseWire, PortError> {
        let builder = self
            .request(
                Method::GET,
                "/v1/menu/watch",
                Some(credentials),
                operation_id,
            )?
            .header(header::ACCEPT, "application/json");
        self.dispatch_json(builder, cancellation, DispatchKind::Safe)
            .await
    }

    pub async fn menu_watch_create(
        &self,
        credentials: &SessionCredentials,
        operation_id: OperationId,
        request: &MenuWatchCreateRequestWire,
        cancellation: CancellationToken,
    ) -> Result<MenuWatchResponseWire, PortError> {
        let builder = self
            .request(
                Method::POST,
                "/v1/menu/watch",
                Some(credentials),
                operation_id,
            )?
            .header(header::ACCEPT, "application/json")
            .json(request);
        self.dispatch_menu_watch_create(builder, cancellation).await
    }

    pub async fn menu_watch_delete(
        &self,
        credentials: &SessionCredentials,
        operation_id: OperationId,
        watch_id: MenuWatchId,
        cancellation: CancellationToken,
    ) -> Result<(), PortError> {
        let endpoint = format!("/v1/menu/watch/{}", watch_id.as_uuid().hyphenated());
        let builder = self.request(Method::DELETE, &endpoint, Some(credentials), operation_id)?;
        self.dispatch(builder, &cancellation, DispatchKind::Mutation)
            .await?;
        Ok(())
    }

    pub fn require_grocery_v1(capabilities: &CapabilitySnapshot) -> Result<(), PortError> {
        match &capabilities.grocery {
            heyfood_core::GroceryCapability::V1 => Ok(()),
            heyfood_core::GroceryCapability::Unavailable => Err(PortError::new(
                "grocery_capability_unavailable",
                "Grocery is not advertised by this deployment",
            )),
            heyfood_core::GroceryCapability::UnsupportedVersion(_) => Err(PortError::new(
                "grocery_capability_unsupported",
                "Grocery advertises an unsupported contract version",
            )),
        }
    }

    pub fn require_diet_v1(capabilities: &CapabilitySnapshot) -> Result<(), PortError> {
        match &capabilities.diet {
            DietCapability::V1 => Ok(()),
            DietCapability::Unavailable => Err(PortError::new(
                "diet_capability_unavailable",
                "Diet guidance is not advertised by this deployment",
            )),
            DietCapability::UnsupportedVersion(_) => Err(PortError::new(
                "diet_capability_unsupported",
                "Diet guidance advertises an unsupported contract version",
            )),
        }
    }

    pub async fn diet_catalog(
        &self,
        capabilities: &CapabilitySnapshot,
        credentials: &SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<DietCatalog, PortError> {
        Self::require_diet_v1(capabilities)?;
        let builder = self
            .request(Method::GET, "/v1/diets", Some(credentials), operation_id)?
            .header(header::ACCEPT, "application/json");
        let wire: DietCatalogResponseWire = self.dispatch_diet_json(builder, cancellation).await?;
        DietCatalog::try_from(wire).map_err(|_| {
            PortError::new(
                "diet_contract_error",
                "the diet catalog response violates contract v1",
            )
        })
    }

    pub async fn diet_detail(
        &self,
        capabilities: &CapabilitySnapshot,
        credentials: &SessionCredentials,
        operation_id: OperationId,
        diet_id: &str,
        cancellation: CancellationToken,
    ) -> Result<DietDetail, PortError> {
        Self::require_diet_v1(capabilities)?;
        let segment = encode_diet_path_segment(diet_id)?;
        let path = format!("/v1/diets/{segment}");
        let builder = self
            .request(Method::GET, &path, Some(credentials), operation_id)?
            .header(header::ACCEPT, "application/json");
        let wire: DietDetailResponseWire = self.dispatch_diet_json(builder, cancellation).await?;
        DietDetail::try_from(wire).map_err(|_| {
            PortError::new(
                "diet_contract_error",
                "the diet detail response violates contract v1",
            )
        })
    }

    pub async fn grocery_list(
        &self,
        capabilities: &CapabilitySnapshot,
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
        capabilities: &CapabilitySnapshot,
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
        capabilities: &CapabilitySnapshot,
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
        capabilities: &CapabilitySnapshot,
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

    pub async fn grocery_exclusions(
        &self,
        capabilities: &CapabilitySnapshot,
        credentials: &SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<ExclusionListResponseWire, PortError> {
        Self::require_grocery_v1(capabilities)?;
        let builder = self
            .request(
                Method::GET,
                "/v1/grocery/exclusions",
                Some(credentials),
                operation_id,
            )?
            .header(header::ACCEPT, "application/json");
        self.dispatch_json(builder, cancellation, DispatchKind::Safe)
            .await
    }

    pub async fn grocery_prepare_add_exclusion(
        &self,
        capabilities: &CapabilitySnapshot,
        credentials: &SessionCredentials,
        operation_id: OperationId,
        request: &ExclusionMutationRequestWire,
        cancellation: CancellationToken,
    ) -> Result<GroceryMutationProposalWire, PortError> {
        self.grocery_mutation(
            capabilities,
            credentials,
            operation_id,
            "/v1/grocery/exclusions",
            request,
            cancellation,
        )
        .await
    }

    pub async fn grocery_prepare_remove_exclusion(
        &self,
        capabilities: &CapabilitySnapshot,
        credentials: &SessionCredentials,
        operation_id: OperationId,
        request: &ExclusionMutationRequestWire,
        cancellation: CancellationToken,
    ) -> Result<GroceryMutationProposalWire, PortError> {
        self.grocery_mutation(
            capabilities,
            credentials,
            operation_id,
            "/v1/grocery/exclusions/remove",
            request,
            cancellation,
        )
        .await
    }

    pub async fn grocery_confirm(
        &self,
        capabilities: &CapabilitySnapshot,
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
        capabilities: &CapabilitySnapshot,
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
        capabilities: &CapabilitySnapshot,
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
        self.request_with_client(&client, method, path, credentials, operation_id)
    }

    fn request_with_client(
        &self,
        client: &Client,
        method: Method,
        path: &str,
        credentials: Option<&SessionCredentials>,
        operation_id: OperationId,
    ) -> Result<RequestBuilder, PortError> {
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

    async fn dispatch_diet_json<T: DeserializeOwned>(
        &self,
        builder: RequestBuilder,
        cancellation: CancellationToken,
    ) -> Result<T, PortError> {
        if cancellation.is_cancelled() {
            return Err(PortError::new(
                "request_cancelled_before_dispatch",
                "request was cancelled before dispatch",
            ));
        }
        let response = tokio::select! {
            () = cancellation.cancelled() => {
                return Err(PortError::new(
                    "request_cancelled_after_dispatch",
                    "service response was not observed after request dispatch",
                ));
            }
            result = builder.send() => result.map_err(|error| {
                PortError::new("request_transport", crate::sanitized_reqwest_error(&error))
            })?,
        };
        let status = response.status();
        if !is_json_media_type(&media_type(&response)) {
            return Err(PortError::new(
                "response_content_type",
                "diet service response is not JSON",
            ));
        }
        let maximum = if status.is_success() {
            MAX_DIET_RESPONSE_BYTES
        } else {
            MAX_DIET_ERROR_BYTES
        };
        let bytes = read_limited(response, &cancellation, DispatchKind::Safe, maximum).await?;
        if status.is_success() {
            return serde_json::from_slice(&bytes).map_err(|_| {
                PortError::new(
                    "diet_contract_error",
                    "diet service response is invalid JSON",
                )
            });
        }
        let body = serde_json::from_slice::<DietErrorEnvelopeWire>(&bytes).unwrap_or_default();
        Err(diet_http_error(
            status,
            body.details.and_then(|details| details.reason),
        ))
    }

    async fn dispatch_profile_consent_wire(
        &self,
        builder: RequestBuilder,
        cancellation: CancellationToken,
        kind: DispatchKind,
    ) -> Result<ProfileConsentStatusWireV1, PortError> {
        let response = self.dispatch(builder, &cancellation, kind).await?;
        if !is_json_media_type(&media_type(&response)) {
            return Err(profile_consent_contract_error(kind));
        }
        let bytes = read_limited(response, &cancellation, kind, MAX_JSON_RESPONSE_BYTES).await?;
        serde_json::from_slice(&bytes).map_err(|_| profile_consent_contract_error(kind))
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

    async fn dispatch_transcription(
        &self,
        builder: RequestBuilder,
        cancellation: &CancellationToken,
    ) -> Result<Response, PortError> {
        if cancellation.is_cancelled() {
            return Err(PortError::new(
                "request_cancelled_before_dispatch",
                "request was cancelled before dispatch",
            ));
        }
        let response = tokio::select! {
            () = cancellation.cancelled() => {
                return Err(PortError::new(
                    "request_cancelled_after_dispatch",
                    "transcription response was not observed after request dispatch",
                ));
            }
            result = builder.send() => result.map_err(|error| {
                PortError::new("request_transport", crate::sanitized_reqwest_error(&error))
            })?,
        };
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let bytes = read_limited(
            response,
            cancellation,
            DispatchKind::Safe,
            MAX_TRANSCRIPTION_ERROR_BYTES,
        )
        .await?;
        let body = serde_json::from_slice::<TranscriptionErrorBody>(&bytes).unwrap_or_default();
        Err(transcription_http_error(status, body.error.as_deref()))
    }

    async fn dispatch_menu_watch_create<T: DeserializeOwned>(
        &self,
        builder: RequestBuilder,
        cancellation: CancellationToken,
    ) -> Result<T, PortError> {
        if cancellation.is_cancelled() {
            return Err(PortError::new(
                "request_cancelled_before_dispatch",
                "request was cancelled before dispatch",
            ));
        }
        let response = tokio::select! {
            () = cancellation.cancelled() => {
                return Err(PortError::uncertain(
                    "request_cancelled_after_dispatch",
                    "service response was not observed after request dispatch",
                ));
            }
            result = builder.send() => result.map_err(|error| {
                uncertain_transport("request_transport", &error)
            })?,
        };
        if !response.status().is_success() {
            let status = response.status();
            if status != StatusCode::CONFLICT {
                return Err(menu_watch_create_error(http_status_error(status)));
            }
            let fallback = || {
                PortError::new(
                    "menu_watch_conflict",
                    "the watch conflicts with current Menu Watch state; review existing watches and retry",
                )
            };
            let bytes = match read_limited(
                response,
                &cancellation,
                DispatchKind::Safe,
                MAX_MENU_WATCH_ERROR_BYTES,
            )
            .await
            {
                Ok(bytes) => bytes,
                Err(_) => return Err(fallback()),
            };
            let body = serde_json::from_slice::<MenuWatchErrorBody>(&bytes).unwrap_or_default();
            return Err(menu_watch_conflict_error(&body));
        }
        let content_type = media_type(&response);
        if !is_json_media_type(&content_type) {
            return Err(dispatch_error(
                DispatchKind::Mutation,
                "response_content_type",
                "service response is not JSON",
            ));
        }
        let bytes = read_limited(
            response,
            &cancellation,
            DispatchKind::Mutation,
            MAX_JSON_RESPONSE_BYTES,
        )
        .await?;
        serde_json::from_slice(&bytes).map_err(|_| {
            dispatch_error(
                DispatchKind::Mutation,
                "response_json",
                "service response is invalid JSON",
            )
        })
    }
}

async fn dispatch_owner_sync_transport(
    builder: RequestBuilder,
    cancellation: CancellationToken,
) -> OwnerSyncTransportResultV1 {
    if cancellation.is_cancelled() {
        return OwnerSyncTransportResultV1::CancelledBeforeSend;
    }

    let send = builder.send();
    let mut response = tokio::select! {
        biased;
        () = cancellation.cancelled() => {
            return OwnerSyncTransportResultV1::OutcomeUncertain {
                reason: OwnerSyncOutcomeUncertainReasonV1::CancelledAfterSend,
            };
        }
        result = send => match result {
            Ok(response) => response,
            Err(error) => {
                return OwnerSyncTransportResultV1::OutcomeUncertain {
                    reason: if error.is_timeout() {
                        OwnerSyncOutcomeUncertainReasonV1::Timeout
                    } else {
                        OwnerSyncOutcomeUncertainReasonV1::Transport
                    },
                };
            }
        },
    };
    let status = response.status().as_u16();
    if response
        .content_length()
        .is_some_and(|length| length > MAX_JSON_RESPONSE_BYTES as u64)
    {
        return OwnerSyncTransportResultV1::OutcomeUncertain {
            reason: OwnerSyncOutcomeUncertainReasonV1::BodyTooLarge,
        };
    }

    let mut body = Vec::with_capacity(
        response
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or(0)
            .min(MAX_JSON_RESPONSE_BYTES),
    );
    loop {
        let chunk = tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                return OwnerSyncTransportResultV1::OutcomeUncertain {
                    reason: OwnerSyncOutcomeUncertainReasonV1::CancelledAfterSend,
                };
            }
            result = response.chunk() => match result {
                Ok(chunk) => chunk,
                Err(error) => {
                    return OwnerSyncTransportResultV1::OutcomeUncertain {
                        reason: if error.is_timeout() {
                            OwnerSyncOutcomeUncertainReasonV1::Timeout
                        } else {
                            OwnerSyncOutcomeUncertainReasonV1::BodyRead
                        },
                    };
                }
            },
        };
        let Some(chunk) = chunk else {
            break;
        };
        let Some(total) = body.len().checked_add(chunk.len()) else {
            return OwnerSyncTransportResultV1::OutcomeUncertain {
                reason: OwnerSyncOutcomeUncertainReasonV1::BodyTooLarge,
            };
        };
        if total > MAX_JSON_RESPONSE_BYTES {
            return OwnerSyncTransportResultV1::OutcomeUncertain {
                reason: OwnerSyncOutcomeUncertainReasonV1::BodyTooLarge,
            };
        }
        body.extend_from_slice(&chunk);
    }
    OwnerSyncTransportResultV1::Response { status, body }
}

impl CapabilityPort for HttpService {
    fn discover(
        &self,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<CapabilitySnapshot, PortError>> {
        Box::pin(async move {
            self.discover_capabilities(cancellation)
                .await
                .map(capability_snapshot_from_wire)
        })
    }
}

impl StatusPort for HttpService {
    fn profile_consent_granted(
        &self,
        credentials: SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<bool, PortError>> {
        Box::pin(async move {
            self.profile_consent_status(&credentials, operation_id, cancellation)
                .await
                .and_then(|document| {
                    document
                        .get("has_consent")
                        .and_then(Value::as_bool)
                        .ok_or_else(|| {
                            PortError::new(
                                "profile_consent_contract",
                                "The profile consent response omitted boolean `has_consent`",
                            )
                        })
                })
        })
    }
}

impl MenuWatchReadPort for HttpService {
    fn list(
        &self,
        credentials: SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<MenuWatchList, PortError>> {
        Box::pin(async move {
            self.menu_watch_list(&credentials, operation_id, cancellation)
                .await
                .map(menu_watch_list_from_wire)
        })
    }
}

impl MenuWatchPort for HttpService {
    fn create(
        &self,
        credentials: SessionCredentials,
        operation_id: OperationId,
        request: CreateMenuWatchRequest,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<MenuWatchSnapshot, PortError>> {
        Box::pin(async move {
            self.menu_watch_create(
                &credentials,
                operation_id,
                &menu_watch_request_to_wire(request),
                cancellation,
            )
            .await
            .map(menu_watch_snapshot_from_wire)
        })
    }

    fn remove(
        &self,
        credentials: SessionCredentials,
        operation_id: OperationId,
        watch_id: MenuWatchId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            self.menu_watch_delete(&credentials, operation_id, watch_id, cancellation)
                .await
        })
    }
}

impl GroceryReadPort for HttpService {
    fn read_active_display(
        &self,
        capabilities: CapabilitySnapshot,
        credentials: SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<GroceryDisplayList, PortError>> {
        Box::pin(async move {
            self.grocery_list(&capabilities, &credentials, operation_id, cancellation)
                .await
                .map(grocery_display_from_wire)
        })
    }

    fn read_exclusions(
        &self,
        capabilities: CapabilitySnapshot,
        credentials: SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<GroceryExclusions, PortError>> {
        Box::pin(async move {
            self.grocery_exclusions(&capabilities, &credentials, operation_id, cancellation)
                .await
                .map(|wire| GroceryExclusions {
                    exclusions: wire.exclusions,
                })
        })
    }
}

impl DietPort for HttpService {
    fn list(
        &self,
        capabilities: CapabilitySnapshot,
        credentials: SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<DietCatalog, PortError>> {
        Box::pin(async move {
            self.diet_catalog(&capabilities, &credentials, operation_id, cancellation)
                .await
        })
    }

    fn detail(
        &self,
        capabilities: CapabilitySnapshot,
        credentials: SessionCredentials,
        operation_id: OperationId,
        diet_id: String,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<DietDetail, PortError>> {
        Box::pin(async move {
            self.diet_detail(
                &capabilities,
                &credentials,
                operation_id,
                &diet_id,
                cancellation,
            )
            .await
        })
    }
}

impl GroceryExportPort for HttpService {
    fn export(
        &self,
        capabilities: CapabilitySnapshot,
        credentials: SessionCredentials,
        operation_id: OperationId,
        list_id: GroceryEntityId,
        format: String,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<GroceryExport, PortError>> {
        Box::pin(async move {
            self.grocery_export(
                &capabilities,
                &credentials,
                operation_id,
                list_id,
                &format,
                cancellation,
            )
            .await
        })
    }
}

impl GroceryMutationPort for HttpService {
    fn prepare(
        &self,
        capabilities: CapabilitySnapshot,
        credentials: SessionCredentials,
        operation_id: OperationId,
        request: DeployedGroceryMutationRequest,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<GroceryMutationProposalWire, PortError>> {
        Box::pin(async move {
            match request {
                DeployedGroceryMutationRequest::Add(request) => {
                    self.grocery_prepare_add(
                        &capabilities,
                        &credentials,
                        operation_id,
                        &request,
                        cancellation,
                    )
                    .await
                }
                DeployedGroceryMutationRequest::Remove(request) => {
                    self.grocery_prepare_remove(
                        &capabilities,
                        &credentials,
                        operation_id,
                        &request,
                        cancellation,
                    )
                    .await
                }
                DeployedGroceryMutationRequest::UpdateState(request) => {
                    self.grocery_prepare_state(
                        &capabilities,
                        &credentials,
                        operation_id,
                        &request,
                        cancellation,
                    )
                    .await
                }
                DeployedGroceryMutationRequest::AddExclusion(request) => {
                    self.grocery_prepare_add_exclusion(
                        &capabilities,
                        &credentials,
                        operation_id,
                        &request,
                        cancellation,
                    )
                    .await
                }
                DeployedGroceryMutationRequest::RemoveExclusion(request) => {
                    self.grocery_prepare_remove_exclusion(
                        &capabilities,
                        &credentials,
                        operation_id,
                        &request,
                        cancellation,
                    )
                    .await
                }
            }
        })
    }

    fn confirm(
        &self,
        capabilities: CapabilitySnapshot,
        credentials: SessionCredentials,
        operation_id: OperationId,
        request: GroceryMutationConfirmRequestWire,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<GroceryMutationResultWire, PortError>> {
        Box::pin(async move {
            self.grocery_confirm(
                &capabilities,
                &credentials,
                operation_id,
                &request,
                cancellation,
            )
            .await
        })
    }
}

fn capability_snapshot_from_wire(wire: ApplicationCapabilitiesWire) -> CapabilitySnapshot {
    let grocery = GroceryCapability::from_advertised(wire.application_version("grocery"));
    let diet = DietCapability::from_advertised(wire.application_version("diet"));
    let registration = match wire.self_registration.status {
        SelfRegistrationStatusWire::Available => RegistrationAvailability::Available,
        SelfRegistrationStatusWire::Disabled => RegistrationAvailability::Disabled,
        SelfRegistrationStatusWire::Unavailable => RegistrationAvailability::Unavailable,
    };
    CapabilitySnapshot {
        schema_version: wire.schema_version,
        registration,
        profile_readiness: wire.profile_readiness,
        loopback_pkce: wire.authorization.loopback_pkce,
        device_code: wire.authorization.device_code,
        grocery,
        diet,
    }
}

fn menu_watch_request_to_wire(request: CreateMenuWatchRequest) -> MenuWatchCreateRequestWire {
    MenuWatchCreateRequestWire {
        restaurant_id: request.restaurant_id,
        cadence: request.cadence,
        notify: request.notify,
        menu_url: request.menu_url,
        confirm_menu_url: request.confirm_menu_url,
        tz: request.tz,
    }
}

fn menu_watch_list_from_wire(wire: MenuWatchListResponseWire) -> MenuWatchList {
    MenuWatchList {
        watches: wire
            .watches
            .into_iter()
            .map(menu_watch_snapshot_from_wire)
            .collect(),
        count: wire.count,
    }
}

fn menu_watch_snapshot_from_wire(wire: MenuWatchResponseWire) -> MenuWatchSnapshot {
    MenuWatchSnapshot {
        id: wire.id,
        restaurant_id: wire.restaurant_id,
        cadence: wire.cadence,
        tz: wire.tz,
        active: wire.active,
        notify: wire.notify,
        next_run_at: wire.next_run_at,
        last_run_at: wire.last_run_at,
        last_snapshot_id: wire.last_snapshot_id,
        created_at: wire.created_at,
        menu_url: wire.menu_url,
        identity_verdict: wire.identity_verdict,
        identity_confidence: wire.identity_confidence,
        identity_reasoning: wire.identity_reasoning,
        identity_confirmed: wire.identity_confirmed,
        last_change: wire.last_change.map(|change| MenuWatchChangeEvent {
            changed_at: change.changed_at,
            previous_snapshot_id: change.previous_snapshot_id,
            new_snapshot_id: change.new_snapshot_id,
            summary: MenuWatchChangeSummary {
                added: change.summary.added,
                removed: change.summary.removed,
                modified: change.summary.modified,
                price_increases: change.summary.price_increases,
                price_decreases: change.summary.price_decreases,
            },
        }),
    }
}

fn grocery_display_from_wire(wire: GroceryListWire) -> GroceryDisplayList {
    GroceryDisplayList {
        id: wire.id,
        title: wire.title,
        state: wire.state,
        version: wire.version,
        items: wire
            .items
            .into_iter()
            .map(|item| GroceryDisplayItem {
                id: item.id,
                requested_name: item.requested_name,
                canonical_name: item.canonical_name,
                quantity: item.quantity,
                unit: item.unit,
                package_quantity: item.package_quantity,
                note: item.note,
                state: item.state,
                intended_for: item.intended_for,
                sources: item
                    .sources
                    .into_iter()
                    .map(|source| GroceryDisplaySource {
                        source_type: source.source_type,
                        source_ref: source.source_ref,
                        source_detail: source.source_detail,
                    })
                    .collect(),
                safety: item.safety.map(|safety| GroceryDisplaySafety {
                    basis: safety.basis,
                    status: safety.status,
                    member_flags: safety
                        .member_flags
                        .into_iter()
                        .map(|flag| GroceryDisplayMemberFlag {
                            member_id: flag.member_id,
                            status: flag.status,
                            reason: flag.reason,
                            substitutions: flag.substitutions,
                        })
                        .collect(),
                    model_version: safety.model_version,
                    rules_version: safety.rules_version,
                    confidence: safety.confidence,
                    context_hash: safety.context_hash,
                    context_hash_version: safety.context_hash_version,
                    label_hint: safety.label_hint,
                }),
                created_at: item.created_at,
                updated_at: item.updated_at,
            })
            .collect(),
        created_at: wire.created_at,
        updated_at: wire.updated_at,
    }
}

fn dispatch_error(kind: DispatchKind, code: &'static str, message: &'static str) -> PortError {
    if kind.uncertain() {
        PortError::uncertain(code, message)
    } else {
        PortError::new(code, message)
    }
}

fn profile_consent_contract_error(kind: DispatchKind) -> PortError {
    dispatch_error(
        kind,
        "profile_consent_contract",
        "profile consent response is malformed or contradictory",
    )
}

fn http_status_error(status: StatusCode) -> PortError {
    let (code, message) = match status.as_u16() {
        400 => ("invalid_request", "the service rejected the request"),
        401 => ("login_required", "authentication is required"),
        403 => ("scope_required", "the session lacks a required scope"),
        404 => ("resource_not_found", "the requested resource was not found"),
        409 => (
            "version_conflict",
            "the resource version changed; fetch it again",
        ),
        413 => ("payload_too_large", "the service rejected the request size"),
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

fn diet_http_error(status: StatusCode, reason: Option<String>) -> PortError {
    match (status, reason.as_deref()) {
        (StatusCode::NOT_FOUND, Some("unknown_diet")) => PortError::new(
            "diet_unknown_diet",
            "the requested diet is not in the served catalog",
        ),
        (StatusCode::NOT_FOUND, Some("feature_disabled")) => PortError::new(
            "diet_feature_disabled",
            "diet guidance is not available on this deployment",
        ),
        (StatusCode::FORBIDDEN, Some("scope_not_permitted")) => PortError::new(
            "scope_required",
            "the session lacks the knowledge:read scope required for diet guidance",
        ),
        _ => http_status_error(status),
    }
}

fn encode_diet_path_segment(value: &str) -> Result<String, PortError> {
    if value.is_empty() || value.chars().count() > 64 || value.chars().any(char::is_control) {
        return Err(PortError::new(
            "diet_id_invalid",
            "diet id must contain 1 to 64 non-control characters",
        ));
    }
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(*byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    Ok(encoded)
}

#[derive(Default, Deserialize)]
struct TranscriptionErrorBody {
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct LogoutWhoami {
    link_id: Option<String>,
}

#[derive(Serialize)]
struct LogoutReason<'a> {
    reason: &'a str,
}

#[derive(Serialize)]
struct LogoutDeviceReason<'a> {
    device_id: &'a str,
    reason: &'a str,
}

fn logout_id(value: &str) -> Result<&str, PortError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(PortError::new(
            "logout_contract_error",
            "service returned an invalid channel link identifier",
        ));
    }
    Ok(value)
}

impl LogoutRemotePort for HttpService {
    fn current_link<'a>(
        &'a self,
        credentials: &'a AuthCredentialBundle,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<Option<String>, PortError>> {
        Box::pin(async move {
            let builder = self
                .request(Method::GET, "/v1/channel/oauth/whoami", None, operation_id)?
                .header(header::ACCEPT, "application/json")
                .bearer_auth(credentials.channel.access_token.expose_secret());
            let response: LogoutWhoami = self
                .dispatch_json(builder, cancellation, DispatchKind::Safe)
                .await?;
            response
                .link_id
                .map(|value| logout_id(&value).map(str::to_owned))
                .transpose()
        })
    }

    fn revoke_link<'a>(
        &'a self,
        credentials: &'a AuthCredentialBundle,
        link_id: String,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<(), PortError>> {
        Box::pin(async move {
            let link_id = logout_id(&link_id)?;
            let builder = self.request(
                Method::DELETE,
                &format!("/v1/channel/links/{link_id}"),
                Some(&credentials.session),
                operation_id,
            )?;
            match self
                .dispatch(builder, &cancellation, DispatchKind::Mutation)
                .await
            {
                Ok(_) => Ok(()),
                Err(error) if error.code == "resource_not_found" => Ok(()),
                Err(error) => Err(error),
            }
        })
    }

    fn revoke_device<'a>(
        &'a self,
        credentials: &'a AuthCredentialBundle,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<(), PortError>> {
        Box::pin(async move {
            let builder = self
                .request(
                    Method::POST,
                    "/v1/auth/device/revoke",
                    Some(&credentials.session),
                    operation_id,
                )?
                .json(&LogoutDeviceReason {
                    device_id: &credentials.channel.device_id,
                    reason: "cli_logout",
                });
            match self
                .dispatch(builder, &cancellation, DispatchKind::Mutation)
                .await
            {
                Ok(_) => Ok(()),
                Err(error) if error.code == "resource_not_found" => Ok(()),
                Err(error) => Err(error),
            }
        })
    }

    fn revoke_session<'a>(
        &'a self,
        credentials: &'a AuthCredentialBundle,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'a, Result<(), PortError>> {
        Box::pin(async move {
            let builder = self
                .request(
                    Method::POST,
                    "/v1/auth/session/revoke",
                    Some(&credentials.session),
                    operation_id,
                )?
                .json(&LogoutReason {
                    reason: "cli_logout",
                });
            match self
                .dispatch(builder, &cancellation, DispatchKind::Mutation)
                .await
            {
                Ok(_) => Ok(()),
                Err(error) if error.code == "resource_not_found" => Ok(()),
                Err(error) => Err(error),
            }
        })
    }
}

#[derive(Default, Deserialize)]
struct MenuWatchErrorBody {
    #[serde(default)]
    error_code: Option<String>,
    #[serde(default)]
    details: Option<MenuWatchErrorDetails>,
}

#[derive(Default, Deserialize)]
struct MenuWatchErrorDetails {
    #[serde(default)]
    requires_confirmation: Option<bool>,
    #[serde(default)]
    cap: Option<u64>,
}

fn transcription_http_error(status: StatusCode, error: Option<&str>) -> PortError {
    let (code, message) = match status.as_u16() {
        400 | 413 => (
            "audio_rejected",
            "the service rejected the recording size or format",
        ),
        401 => ("login_required", "voice transcription requires login"),
        403 if error == Some("insufficient_scope") => (
            "insufficient_scope",
            "voice transcription requires additional authorization",
        ),
        429 => (
            "rate_limited",
            "the transcription service rate limit was reached",
        ),
        404 | 503 => (
            "transcription_unavailable",
            "the transcription service is currently unavailable",
        ),
        _ => (
            "transcription_unavailable",
            "the transcription service rejected the request",
        ),
    };
    PortError::new(code, message)
}

fn transcription_public_error(error: PortError) -> PortError {
    if TRANSCRIPTION_CLIENT_ERROR_KINDS.contains(&error.code) {
        return error;
    }
    match error.code {
        "transcription_context" => PortError::new(
            "login_required",
            "voice transcription requires an authenticated CLI channel",
        ),
        "transcription_language" => PortError::new(
            "audio_rejected",
            "the transcription language tag is invalid",
        ),
        "response_content_type" | "response_json" | "response_too_large" => PortError::new(
            "transcription_contract_error",
            "the transcription service returned an invalid success response",
        ),
        _ => PortError::new(
            "transcription_unavailable",
            "the transcription service is currently unavailable",
        ),
    }
}

fn menu_watch_create_error(error: PortError) -> PortError {
    match error.code {
        "invalid_request" => PortError::new(
            "menu_watch_rejected",
            "the service rejected the restaurant, menu identity, schedule, or timezone",
        ),
        _ => error,
    }
}

fn menu_watch_conflict_error(body: &MenuWatchErrorBody) -> PortError {
    let details = body.details.as_ref();
    if details.and_then(|value| value.requires_confirmation) == Some(true) {
        return PortError::new(
            "menu_watch_confirmation_required",
            "the menu identity requires review; retry with --confirm-menu-url only after verifying the URL",
        );
    }
    if details.and_then(|value| value.cap).is_some()
        || body.error_code.as_deref() == Some("daily_limit_exceeded")
    {
        return PortError::new(
            "menu_watch_limit_reached",
            "the Menu Watch limit was reached; remove an existing watch before adding another",
        );
    }
    if body.error_code.as_deref() == Some("invalid_request") {
        return PortError::new(
            "menu_watch_already_exists",
            "a watch with this cadence already exists for this restaurant",
        );
    }
    PortError::new(
        "menu_watch_conflict",
        "the watch conflicts with current Menu Watch state; review existing watches and retry",
    )
}

fn transcription_multipart(
    boundary: &str,
    wav_bytes: &[u8],
    purpose: &str,
    language: Option<&str>,
) -> Result<Vec<u8>, PortError> {
    let mut body = Vec::with_capacity(wav_bytes.len().saturating_add(1_024));
    append_multipart_text(&mut body, boundary, "purpose", purpose);
    if let Some(language) = language {
        append_multipart_text(&mut body, boundary, "language", language);
    }
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: audio/wav\r\n\r\n");
    body.extend_from_slice(wav_bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    if body.len() > TRANSCRIPTION_MAX_REQUEST_BYTES {
        return Err(PortError::new(
            "audio_rejected",
            "the multipart recording exceeds the transcription request limit",
        ));
    }
    Ok(body)
}

fn validate_transcription_wav(wav_bytes: &[u8]) -> Result<(), PortError> {
    let invalid = || {
        PortError::new(
            "audio_rejected",
            "the recording is not a bounded mono PCM WAV accepted by the transcription contract",
        )
    };
    if wav_bytes.len() <= TRANSCRIPTION_WAV_HEADER_BYTES
        || wav_bytes.len() > TRANSCRIPTION_MAX_AUDIO_BYTES
        || wav_bytes.get(..4) != Some(b"RIFF")
        || wav_bytes.get(8..12) != Some(b"WAVE")
        || wav_bytes.get(12..16) != Some(b"fmt ")
        || wav_bytes.get(36..40) != Some(b"data")
    {
        return Err(invalid());
    }
    let riff_size = read_wav_u32(wav_bytes, 4);
    let fmt_size = read_wav_u32(wav_bytes, 16);
    let audio_format = read_wav_u16(wav_bytes, 20);
    let channels = read_wav_u16(wav_bytes, 22);
    let sample_rate = read_wav_u32(wav_bytes, 24);
    let byte_rate = read_wav_u32(wav_bytes, 28);
    let block_align = read_wav_u16(wav_bytes, 32);
    let bits_per_sample = read_wav_u16(wav_bytes, 34);
    let data_size = read_wav_u32(wav_bytes, 40);
    let expected_data_size =
        u32::try_from(wav_bytes.len() - TRANSCRIPTION_WAV_HEADER_BYTES).map_err(|_| invalid())?;
    let expected_riff_size = u32::try_from(wav_bytes.len() - 8).map_err(|_| invalid())?;
    let sample_width = u16::try_from(TRANSCRIPTION_SAMPLE_WIDTH_BYTES).map_err(|_| invalid())?;
    let expected_block_align = TRANSCRIPTION_CHANNELS
        .checked_mul(sample_width)
        .ok_or_else(invalid)?;
    let expected_bits_per_sample = sample_width.checked_mul(8).ok_or_else(invalid)?;
    let expected_byte_rate = sample_rate
        .checked_mul(u32::from(expected_block_align))
        .ok_or_else(invalid)?;
    let samples = u64::from(data_size) / u64::from(expected_block_align);
    if riff_size != expected_riff_size
        || fmt_size != 16
        || audio_format != 1
        || channels != TRANSCRIPTION_CHANNELS
        || !transcription_sample_rate_supported(sample_rate)
        || byte_rate != expected_byte_rate
        || block_align != expected_block_align
        || bits_per_sample != expected_bits_per_sample
        || data_size != expected_data_size
        || !data_size.is_multiple_of(u32::from(expected_block_align))
        || samples > u64::from(sample_rate) * TRANSCRIPTION_MAX_DURATION_SECONDS
    {
        return Err(invalid());
    }
    Ok(())
}

fn read_wav_u16(wav_bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([wav_bytes[offset], wav_bytes[offset + 1]])
}

fn read_wav_u32(wav_bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        wav_bytes[offset],
        wav_bytes[offset + 1],
        wav_bytes[offset + 2],
        wav_bytes[offset + 3],
    ])
}

fn append_multipart_text(body: &mut Vec<u8>, boundary: &str, name: &str, value: &str) {
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(value.as_bytes());
    body.extend_from_slice(b"\r\n");
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
