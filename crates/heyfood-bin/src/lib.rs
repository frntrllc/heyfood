//! Thin native composition seams for the Phase 0 qualification build.

#![forbid(unsafe_code)]

use std::{fmt, io, sync::Arc, time::Duration};

use heyfood_agent_runtime::{GroceryExport, HttpService};
use heyfood_application::{
    EnsureSession, EnsureSessionError, EnsureSessionOutcome, RefreshPolicy, RunTurnOutcome,
    ServicePort, TurnContext, TurnRequest, execute_one_shot_turn,
};
use heyfood_cli::{
    AskArgs, Command, GroceryCommand, HealthCommand, ItemArgs, LogArgs, OutputMode,
    render_agent_result, render_grocery_list, render_grocery_proposal, render_health_context,
    render_item_result, render_json,
};
use heyfood_core::{
    AddItemsRequestWire, AgentEvent, GroceryConfirmationToken, GroceryDecisionWire,
    GroceryEntityId, GroceryItemInputWire, GroceryListVersion, GroceryMutationConfirmRequestWire,
    ImportedPythonState, OperationId, RemoveItemsRequestWire, SessionCredentials, SessionSnapshot,
    UpdateItemStateRequestWire, terminal_safe_text,
};
use heyfood_tui::{Effect, ExitReason, RuntimeEvent, TuiError};
use serde_json::{Map, Value, json};
use tokio::{
    runtime::Runtime,
    sync::{Mutex, mpsc},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

pub const QUALIFIED_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
pub const MAX_CONFIRMATION_STDIN_BYTES: usize = 1024 * 1024;

#[derive(Clone, Eq, PartialEq)]
pub struct OneShotError {
    pub code: &'static str,
    pub message: String,
    pub outcome_uncertain: bool,
}

impl fmt::Debug for OneShotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OneShotError")
            .field("code", &self.code)
            .field("message", &"[REDACTED]")
            .field("outcome_uncertain", &self.outcome_uncertain)
            .finish()
    }
}

impl fmt::Display for OneShotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for OneShotError {}

impl From<heyfood_application::PortError> for OneShotError {
    fn from(value: heyfood_application::PortError) -> Self {
        Self {
            code: value.code,
            message: value.message,
            outcome_uncertain: value.outcome_uncertain,
        }
    }
}

impl From<EnsureSessionError> for OneShotError {
    fn from(value: EnsureSessionError) -> Self {
        let (code, outcome_uncertain) = match &value {
            EnsureSessionError::ReconciliationRequired => ("session_reconciliation_required", true),
            EnsureSessionError::Service(error) => (error.code, error.outcome_uncertain),
            EnsureSessionError::ServiceReconciliationRequired(_) => {
                ("session_refresh_outcome_uncertain", true)
            }
            EnsureSessionError::CredentialReconciliationRequired(_) => {
                ("session_refresh_persistence_uncertain", true)
            }
            EnsureSessionError::ReconciliationMarkerWrite { .. } => {
                ("session_reconciliation_marker_write", true)
            }
        };
        Self {
            code,
            message: terminal_safe_text(&value.to_string()),
            outcome_uncertain,
        }
    }
}

/// Phase 2 executor over explicit, already-validated native state. The public
/// binary constructs this for the native command families it advertises.
pub struct OneShotExecutor<'a> {
    service: &'a HttpService,
    credentials: &'a SessionCredentials,
    output_mode: OutputMode,
    imported_state: Option<&'a ImportedPythonState>,
}

/// Refresh and durably reconcile the session before entering any authenticated
/// one-shot command. A refresh cancellation observed before dispatch never
/// reaches the command; accepted rotations are committed by `EnsureSession`
/// before this function constructs the executor.
pub async fn execute_qualified_one_shot(
    service: &HttpService,
    ensure_session: &EnsureSession,
    snapshot: heyfood_core::SessionSnapshot,
    output_mode: OutputMode,
    command: Command,
    stdin: &[u8],
    cancellation: CancellationToken,
) -> Result<String, OneShotError> {
    execute_qualified_one_shot_with_state(
        service,
        ensure_session,
        snapshot,
        output_mode,
        command,
        stdin,
        cancellation,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn execute_qualified_one_shot_with_state(
    service: &HttpService,
    ensure_session: &EnsureSession,
    snapshot: heyfood_core::SessionSnapshot,
    output_mode: OutputMode,
    command: Command,
    stdin: &[u8],
    cancellation: CancellationToken,
    imported_state: Option<&ImportedPythonState>,
) -> Result<String, OneShotError> {
    let credentials = match ensure_session
        .execute(snapshot, cancellation.child_token())
        .await
        .map_err(OneShotError::from)?
    {
        EnsureSessionOutcome::Current(credentials)
        | EnsureSessionOutcome::Refreshed(credentials) => credentials,
        EnsureSessionOutcome::CancelledBeforeDispatch => {
            return Err(OneShotError::new(
                "session_cancelled_before_dispatch",
                "session refresh was cancelled before dispatch",
            ));
        }
    };
    OneShotExecutor::new(service, &credentials, output_mode)
        .with_imported_state(imported_state)
        .execute(command, stdin, cancellation)
        .await
}

impl<'a> OneShotExecutor<'a> {
    #[must_use]
    pub const fn new(
        service: &'a HttpService,
        credentials: &'a SessionCredentials,
        output_mode: OutputMode,
    ) -> Self {
        Self {
            service,
            credentials,
            output_mode,
            imported_state: None,
        }
    }

    #[must_use]
    pub const fn with_imported_state(
        mut self,
        imported_state: Option<&'a ImportedPythonState>,
    ) -> Self {
        self.imported_state = imported_state;
        self
    }

    pub async fn execute(
        &self,
        command: Command,
        stdin: &[u8],
        cancellation: CancellationToken,
    ) -> Result<String, OneShotError> {
        match command {
            Command::Ask(arguments) => self.execute_agent(arguments, stdin, cancellation).await,
            Command::Reply(arguments) => {
                if arguments.conversation_id.is_none() {
                    return Err(OneShotError::new(
                        "conversation_required",
                        "native reply requires --conversation-id until conversation persistence is implemented",
                    ));
                }
                self.execute_agent(arguments, stdin, cancellation).await
            }
            Command::Log(arguments) => self.execute_log(arguments, stdin, cancellation).await,
            Command::Item(arguments) => self.execute_item(arguments, cancellation).await,
            Command::Grocery { command } => {
                self.execute_grocery(command, stdin, cancellation).await
            }
            Command::Health { command } => self.execute_health(command, cancellation).await,
            Command::Completion { shell } => {
                String::from_utf8(heyfood_cli::generate_completion(shell)).map_err(|_| {
                    OneShotError::new("completion_encoding", "completion output is invalid UTF-8")
                })
            }
            _ => Err(OneShotError::new(
                "phase2_parity_pending",
                "this command is present for topology parity but its Phase 2 use case is not yet qualified",
            )),
        }
    }

    async fn execute_agent(
        &self,
        arguments: AskArgs,
        stdin: &[u8],
        cancellation: CancellationToken,
    ) -> Result<String, OneShotError> {
        let prompt = if arguments.text.is_empty() {
            if stdin.is_empty() || stdin.len() > MAX_CONFIRMATION_STDIN_BYTES {
                return Err(OneShotError::new(
                    "invalid_prompt",
                    "prompt text or at most 1 MiB of UTF-8 stdin is required",
                ));
            }
            std::str::from_utf8(stdin)
                .map_err(|_| OneShotError::new("invalid_prompt", "prompt stdin is not UTF-8"))?
                .trim_end_matches(['\r', '\n'])
                .to_owned()
        } else {
            arguments.prompt()
        };
        let prompt = required_text(prompt, 500, "prompt")?;
        self.execute_prompt(
            prompt,
            arguments.conversation_id,
            TurnContext {
                latitude: arguments.latitude,
                longitude: arguments.longitude,
                ..TurnContext::default()
            },
            cancellation,
        )
        .await
    }

    async fn execute_log(
        &self,
        arguments: LogArgs,
        stdin: &[u8],
        cancellation: CancellationToken,
    ) -> Result<String, OneShotError> {
        let meal = if arguments.meal.is_empty() {
            if stdin.is_empty() || stdin.len() > MAX_CONFIRMATION_STDIN_BYTES {
                return Err(OneShotError::new(
                    "invalid_meal",
                    "meal text or at most 1 MiB of UTF-8 stdin is required",
                ));
            }
            std::str::from_utf8(stdin)
                .map_err(|_| OneShotError::new("invalid_meal", "meal stdin is not UTF-8"))?
                .trim_end_matches(['\r', '\n'])
                .to_owned()
        } else {
            arguments.meal_text()
        };
        let meal = required_text(meal, 500, "meal")?;
        let mut prompt = format!("Log this meal: {meal}");
        if let Some(meal_type) = arguments.meal_type {
            prompt.push_str(". Meal type: ");
            prompt.push_str(meal_type.as_str());
            prompt.push('.');
        }
        let prompt = required_text(prompt, 500, "query")?;
        let context = self
            .household_turn_context(
                arguments.checking_for.as_deref(),
                cancellation.child_token(),
            )
            .await?;
        self.execute_prompt(prompt, None, context, cancellation)
            .await
    }

    async fn execute_item(
        &self,
        arguments: ItemArgs,
        cancellation: CancellationToken,
    ) -> Result<String, OneShotError> {
        let item_name = required_text(arguments.item_name(), 200, "item name")?;
        let mut restaurant = arguments
            .restaurant
            .map(|value| optional_text(Some(value), 200, "restaurant name"))
            .transpose()?
            .flatten();
        if let Some(selector) = arguments.at.as_deref()
            && selector.trim().bytes().all(|byte| byte.is_ascii_digit())
            && !selector.trim().is_empty()
        {
            restaurant = Some(self.restaurant_from_selector(selector)?);
        }
        let document = self
            .service
            .explain_item(
                &item_name,
                restaurant.as_deref(),
                OperationId::new(),
                cancellation,
            )
            .await?;
        Ok(render_item_result(&document, self.output_mode))
    }

    fn restaurant_from_selector(&self, selector: &str) -> Result<String, OneShotError> {
        let normalized = selector.trim();
        let index = normalized
            .parse::<usize>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| {
                OneShotError::new(
                    "restaurant_selector",
                    "restaurant selection is out of range",
                )
            })?;
        let state = self.bound_imported_state()?;
        let restaurants = state
            .account_scoped
            .get("last_restaurant_search")
            .and_then(|value| value.get("restaurants"))
            .and_then(Value::as_array)
            .ok_or_else(|| {
                OneShotError::new(
                    "restaurant_search_missing",
                    "no previous restaurant search was imported; run search before using --at",
                )
            })?;
        let restaurant = restaurants
            .get(index - 1)
            .and_then(Value::as_object)
            .ok_or_else(|| {
                OneShotError::new(
                    "restaurant_selector",
                    format!("restaurant selection {index} is out of range for the last search"),
                )
            })?;
        let name = restaurant
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                OneShotError::new(
                    "restaurant_selector",
                    "the selected restaurant does not contain a name",
                )
            })?;
        required_text(name.to_owned(), 200, "restaurant name")
    }

    fn bound_imported_state(&self) -> Result<&ImportedPythonState, OneShotError> {
        let state = self.imported_state.ok_or_else(|| {
            OneShotError::new(
                "python_state_required",
                "this selector requires account-bound state imported from the Python client",
            )
        })?;
        if state.account_user_id.as_deref() != Some(self.credentials.account_id.as_str()) {
            return Err(OneShotError::new(
                "python_state_account_mismatch",
                "imported Python state does not belong to the authenticated account",
            ));
        }
        Ok(state)
    }

    async fn household_turn_context(
        &self,
        selector: Option<&str>,
        cancellation: CancellationToken,
    ) -> Result<TurnContext, OneShotError> {
        let state = self.bound_imported_state()?;
        let household = normalized_household(state)?;
        let selected = resolve_household_scope(&household, selector)?;
        let consent = self
            .service
            .profile_consent_status(
                self.credentials,
                OperationId::new(),
                cancellation.child_token(),
            )
            .await?;
        let has_consent = consent
            .get("has_consent")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let active = active_household_members(&household);
        let owner = member_by_id(&active, "_self")
            .or_else(|| active.first().copied())
            .ok_or_else(|| OneShotError::new("household_state", "household has no active owner"))?;
        let local_profiles = state
            .account_scoped
            .get("household_local_profiles")
            .and_then(Value::as_object);
        let profile_outbox = state
            .account_scoped
            .get("household_profile_outbox")
            .and_then(Value::as_object);

        let dietary = if selected == "__everyone__" {
            let mut members = Vec::with_capacity(active.len());
            for member in &active {
                let profile = self
                    .profile_for_household_member(
                        member,
                        local_profiles,
                        profile_outbox,
                        has_consent,
                        cancellation.child_token(),
                    )
                    .await?;
                let mut context = member_dietary_context(member, &profile, owner)?;
                context.insert(
                    "member_id".into(),
                    Value::String(member_id(member)?.to_owned()),
                );
                context.insert(
                    "label".into(),
                    Value::String(member_name(member)?.to_owned()),
                );
                members.push(Value::Object(context));
            }
            json!({"mode": "household", "members": members})
        } else {
            let member = member_by_id(&active, &selected).ok_or_else(|| {
                OneShotError::new(
                    "household_scope",
                    "selected household member is unavailable",
                )
            })?;
            let profile = self
                .profile_for_household_member(
                    member,
                    local_profiles,
                    profile_outbox,
                    has_consent,
                    cancellation.child_token(),
                )
                .await?;
            Value::Object(member_dietary_context(member, &profile, owner)?)
        };
        let selected_member = member_by_id(&active, &selected);
        let scope_label = if selected == "__everyone__" {
            "Everyone".to_owned()
        } else {
            member_name(selected_member.ok_or_else(|| {
                OneShotError::new(
                    "household_scope",
                    "selected household member is unavailable",
                )
            })?)?
            .to_owned()
        };
        let device = has_consent.then(|| {
            json!({
                "household": {
                    "owner_id": "_self",
                    "members": active.iter().filter_map(|member| {
                        Some(json!({
                            "id": member.get("id")?.as_str()?,
                            "name": member.get("name")?.as_str()?,
                            "relationship": member.get("relationship").and_then(Value::as_str).unwrap_or("other"),
                            "is_owner": member.get("id").and_then(Value::as_str) == Some("_self")
                        }))
                    }).collect::<Vec<_>>()
                }
            })
        });
        let meal = if selected == "__everyone__" {
            json!({"is_cook_mode": true})
        } else {
            json!({
                "active_member_id": selected,
                "active_member_name": scope_label,
                "is_cook_mode": false
            })
        };
        Ok(TurnContext {
            dietary: Some(dietary),
            device,
            meal: Some(meal),
            ..TurnContext::default()
        })
    }

    async fn profile_for_household_member(
        &self,
        member: &Map<String, Value>,
        local_profiles: Option<&Map<String, Value>>,
        profile_outbox: Option<&Map<String, Value>>,
        has_consent: bool,
        cancellation: CancellationToken,
    ) -> Result<Value, OneShotError> {
        let id = member_id(member)?;
        if member.get("relationship").and_then(Value::as_str) == Some("child") {
            return Ok(local_profiles
                .and_then(|profiles| profiles.get(id))
                .cloned()
                .unwrap_or_else(|| json!({})));
        }
        if let Some(pending) = profile_outbox.and_then(|outbox| outbox.get(id)) {
            return Ok(pending
                .get("local_context")
                .filter(|value| value.is_object())
                .cloned()
                .unwrap_or_else(|| json!({})));
        }
        if !has_consent {
            return Ok(json!({}));
        }
        let downloaded = self
            .service
            .download_profile(self.credentials, id, OperationId::new(), cancellation)
            .await?;
        Ok(downloaded
            .get("profile_data")
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(|| json!({})))
    }

    async fn execute_prompt(
        &self,
        prompt: String,
        conversation_id: Option<String>,
        context: TurnContext,
        cancellation: CancellationToken,
    ) -> Result<String, OneShotError> {
        let result = execute_one_shot_turn(
            self.service,
            TurnRequest {
                prompt,
                conversation_id,
                context,
                refresh: RefreshPolicy::Never,
            },
            self.credentials.clone(),
            OperationId::new(),
            cancellation,
        )
        .await?;
        Ok(render_agent_result(&result.document, self.output_mode))
    }

    async fn execute_grocery(
        &self,
        command: GroceryCommand,
        stdin: &[u8],
        cancellation: CancellationToken,
    ) -> Result<String, OneShotError> {
        let capabilities = self
            .service
            .discover_capabilities(cancellation.child_token())
            .await?;
        HttpService::require_grocery_v1(&capabilities)?;
        match command {
            GroceryCommand::List => {
                let list = self
                    .service
                    .grocery_list(
                        &capabilities,
                        self.credentials,
                        OperationId::new(),
                        cancellation,
                    )
                    .await?;
                Ok(render_grocery_list(&list, self.output_mode))
            }
            GroceryCommand::Add(arguments) => {
                if arguments.items.len() > 25 {
                    return Err(OneShotError::new(
                        "grocery_item_count",
                        "a Grocery add request may contain at most 25 items",
                    ));
                }
                let request = AddItemsRequestWire {
                    list_id: parse_list_id(&arguments.list.list_id)?,
                    expected_version: parse_list_version(arguments.list.version)?,
                    items: arguments
                        .items
                        .into_iter()
                        .map(|name| {
                            let name = bounded_text(name, 255, "grocery item name")?;
                            Ok(GroceryItemInputWire {
                                name,
                                quantity: None,
                                unit: None,
                                package_quantity: None,
                                note: None,
                                intended_for: arguments.intended_for.clone(),
                                source_type: "manual".into(),
                                source_ref: None,
                                source_detail: None,
                            })
                        })
                        .collect::<Result<_, OneShotError>>()?,
                };
                let proposal = self
                    .service
                    .grocery_prepare_add(
                        &capabilities,
                        self.credentials,
                        OperationId::new(),
                        &request,
                        cancellation,
                    )
                    .await?;
                Ok(render_grocery_proposal(&proposal, self.output_mode))
            }
            GroceryCommand::Remove(arguments) => {
                let (list_id, version, item_ids) = self
                    .resolve_references(
                        &capabilities,
                        &arguments.list.list_id,
                        arguments.list.version,
                        &arguments.items,
                        cancellation.child_token(),
                    )
                    .await?;
                let proposal = self
                    .service
                    .grocery_prepare_remove(
                        &capabilities,
                        self.credentials,
                        OperationId::new(),
                        &RemoveItemsRequestWire {
                            list_id,
                            expected_version: version,
                            item_ids,
                        },
                        cancellation,
                    )
                    .await?;
                Ok(render_grocery_proposal(&proposal, self.output_mode))
            }
            GroceryCommand::State(arguments) => {
                let (list_id, version, item_ids) = self
                    .resolve_references(
                        &capabilities,
                        &arguments.list.list_id,
                        arguments.list.version,
                        std::slice::from_ref(&arguments.item),
                        cancellation.child_token(),
                    )
                    .await?;
                let proposal = self
                    .service
                    .grocery_prepare_state(
                        &capabilities,
                        self.credentials,
                        OperationId::new(),
                        &UpdateItemStateRequestWire {
                            list_id,
                            expected_version: version,
                            item_id: item_ids.into_iter().next().ok_or_else(|| {
                                OneShotError::new("grocery_item_reference", "item is required")
                            })?,
                            state: arguments.state.into(),
                        },
                        cancellation,
                    )
                    .await?;
                Ok(render_grocery_proposal(&proposal, self.output_mode))
            }
            GroceryCommand::Export(arguments) => {
                let export = self
                    .service
                    .grocery_export(
                        &capabilities,
                        self.credentials,
                        OperationId::new(),
                        parse_list_id(&arguments.list_id)?,
                        arguments.format.as_wire_value(),
                        cancellation,
                    )
                    .await?;
                match export {
                    GroceryExport::Json(list) => render_json(&list).map_err(|_| {
                        OneShotError::new("output_json", "could not encode Grocery export")
                    }),
                    GroceryExport::Markdown(text) | GroceryExport::Text(text) => Ok(text),
                }
            }
            GroceryCommand::Confirm(arguments) => {
                if !arguments.proposal_stdin {
                    return Err(OneShotError::new(
                        "confirmation_input",
                        "confirmation proposals must be read from stdin",
                    ));
                }
                if stdin.is_empty() || stdin.len() > MAX_CONFIRMATION_STDIN_BYTES {
                    return Err(OneShotError::new(
                        "confirmation_input",
                        "confirmation proposal stdin must contain at most 1 MiB",
                    ));
                }
                let proposal: heyfood_core::GroceryMutationProposalWire =
                    serde_json::from_slice(stdin).map_err(|_| {
                        OneShotError::new(
                            "confirmation_input",
                            "confirmation proposal stdin is invalid JSON",
                        )
                    })?;
                let result = self
                    .service
                    .grocery_confirm(
                        &capabilities,
                        self.credentials,
                        OperationId::new(),
                        &GroceryMutationConfirmRequestWire {
                            confirmation_token: GroceryConfirmationToken::parse(
                                proposal
                                    .confirmation_token
                                    .expose_at_transport_boundary()
                                    .to_owned(),
                            )
                            .map_err(|message| OneShotError::new("confirmation_input", message))?,
                            decision: GroceryDecisionWire::from(arguments.decision),
                        },
                        cancellation,
                    )
                    .await?;
                render_json(&result).map_err(|_| {
                    OneShotError::new("output_json", "could not encode confirmation result")
                })
            }
        }
    }

    async fn execute_health(
        &self,
        command: HealthCommand,
        cancellation: CancellationToken,
    ) -> Result<String, OneShotError> {
        match command {
            HealthCommand::Status => {
                let integrations = self
                    .service
                    .health_integrations(self.credentials, OperationId::new(), cancellation)
                    .await?;
                if self.output_mode == OutputMode::Json {
                    return render_json(&integrations).map_err(|_| {
                        OneShotError::new("output_json", "could not encode integration status")
                    });
                }
                let mut output = String::new();
                if integrations.integrations.is_empty() {
                    output.push_str("No health integrations connected.\n");
                }
                for integration in integrations.integrations {
                    let provider = serde_json::to_value(integration.provider)
                        .ok()
                        .and_then(|value| value.as_str().map(str::to_owned))
                        .unwrap_or_else(|| "provider".into());
                    let status = serde_json::to_value(integration.status)
                        .ok()
                        .and_then(|value| value.as_str().map(str::to_owned))
                        .unwrap_or_else(|| "unknown".into());
                    output.push_str(&format!("{provider}: {status}\n"));
                }
                Ok(output)
            }
            HealthCommand::Show => {
                let context = self
                    .service
                    .health_context(self.credentials, OperationId::new(), cancellation)
                    .await?;
                Ok(render_health_context(&context, self.output_mode))
            }
            HealthCommand::Connect(arguments) => {
                ensure_oura(arguments.provider)?;
                let authorization = self
                    .service
                    .health_authorize_oura(self.credentials, OperationId::new(), cancellation)
                    .await?;
                if self.output_mode == OutputMode::Json {
                    render_json(&authorization).map_err(|_| {
                        OneShotError::new("output_json", "could not encode authorization")
                    })
                } else {
                    Ok(format!(
                        "Open this authorization URL in your browser:\n{}\n",
                        terminal_safe_text(&authorization.auth_url)
                    ))
                }
            }
            HealthCommand::Sync(arguments) => {
                ensure_oura(arguments.provider)?;
                let result = self
                    .service
                    .health_sync_oura(self.credentials, OperationId::new(), cancellation)
                    .await?;
                render_json(&result)
                    .map_err(|_| OneShotError::new("output_json", "could not encode sync result"))
            }
            HealthCommand::Disconnect(arguments) => {
                ensure_oura(arguments.provider.provider)?;
                if !arguments.yes {
                    return Err(OneShotError::new(
                        "confirmation_required",
                        "health disconnect requires --yes",
                    ));
                }
                let result = self
                    .service
                    .health_disconnect_oura(self.credentials, OperationId::new(), cancellation)
                    .await?;
                render_json(&result).map_err(|_| {
                    OneShotError::new("output_json", "could not encode disconnect result")
                })
            }
        }
    }

    async fn resolve_references(
        &self,
        capabilities: &heyfood_core::ApplicationCapabilitiesWire,
        requested_list_id: &str,
        requested_version: u64,
        references: &[String],
        cancellation: CancellationToken,
    ) -> Result<(GroceryEntityId, GroceryListVersion, Vec<String>), OneShotError> {
        let list_id = parse_list_id(requested_list_id)?;
        let version = parse_list_version(requested_version)?;
        let list = self
            .service
            .grocery_list(
                capabilities,
                self.credentials,
                OperationId::new(),
                cancellation,
            )
            .await?;
        if list.id != list_id.as_uuid().hyphenated().to_string() || list.version != version.get() {
            return Err(OneShotError::new(
                "version_conflict",
                "the active Grocery list identity or version changed; fetch it again",
            ));
        }
        let item_ids = references
            .iter()
            .map(|reference| {
                if let Some(index) = reference.strip_prefix('#') {
                    let index = index.parse::<usize>().map_err(|_| {
                        OneShotError::new(
                            "grocery_item_reference",
                            "Grocery item index must be written as #N",
                        )
                    })?;
                    if index == 0 {
                        return Err(OneShotError::new(
                            "grocery_item_reference",
                            "Grocery item indexes are one-based",
                        ));
                    }
                    list.items
                        .get(index - 1)
                        .map(|item| item.id.clone())
                        .ok_or_else(|| {
                            OneShotError::new(
                                "grocery_item_reference",
                                "Grocery item index is outside the current list",
                            )
                        })
                } else {
                    bounded_text(reference.clone(), 255, "grocery item ID")
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((list_id, version, item_ids))
    }
}

impl OneShotError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            outcome_uncertain: false,
        }
    }
}

fn parse_list_id(value: &str) -> Result<GroceryEntityId, OneShotError> {
    GroceryEntityId::parse(value).map_err(|message| OneShotError::new("grocery_list_id", message))
}

fn parse_list_version(value: u64) -> Result<GroceryListVersion, OneShotError> {
    GroceryListVersion::new(value)
        .map_err(|message| OneShotError::new("grocery_list_version", message))
}

fn bounded_text(
    value: String,
    maximum: usize,
    label: &'static str,
) -> Result<String, OneShotError> {
    if value.trim() != value || value.is_empty() || value.len() > maximum {
        return Err(OneShotError::new(
            "invalid_argument",
            format!("{label} is invalid"),
        ));
    }
    let value = terminal_safe_text(&value);
    if value.is_empty() {
        return Err(OneShotError::new(
            "invalid_argument",
            format!("{label} is invalid"),
        ));
    }
    Ok(value)
}

fn required_text(
    value: String,
    maximum_characters: usize,
    label: &'static str,
) -> Result<String, OneShotError> {
    heyfood_core::required_text(&value, maximum_characters).map_err(|_| {
        OneShotError::new(
            "invalid_argument",
            format!("{label} must contain 1 to {maximum_characters} characters"),
        )
    })
}

fn optional_text(
    value: Option<String>,
    maximum_characters: usize,
    label: &'static str,
) -> Result<Option<String>, OneShotError> {
    heyfood_core::optional_text(value.as_deref(), maximum_characters).map_err(|_| {
        OneShotError::new(
            "invalid_argument",
            format!("{label} must contain at most {maximum_characters} characters"),
        )
    })
}

fn normalized_household(state: &ImportedPythonState) -> Result<Map<String, Value>, OneShotError> {
    let owner_name = state
        .account_scoped
        .get("first_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Me");
    let mut household = state
        .account_scoped
        .get("household")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(Map::new);
    let raw_members = household
        .remove("members")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let mut members = Vec::new();
    let mut identifiers = std::collections::BTreeSet::new();
    for raw in raw_members {
        let Some(mut member) = raw.as_object().cloned() else {
            continue;
        };
        let Some(id) = member
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != "__everyone__")
            .map(str::to_owned)
        else {
            continue;
        };
        if !identifiers.insert(id.clone()) {
            continue;
        }
        let name = member
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(if id == "_self" { owner_name } else { &id })
            .to_owned();
        let relationship = member
            .get("relationship")
            .and_then(Value::as_str)
            .unwrap_or(if id == "_self" { "self" } else { "other" })
            .to_owned();
        member.insert("id".into(), Value::String(id.clone()));
        member.insert("name".into(), Value::String(name));
        member.insert(
            "relationship".into(),
            Value::String(if id == "_self" {
                "self".to_owned()
            } else {
                relationship
            }),
        );
        member.insert("is_owner".into(), Value::Bool(id == "_self"));
        members.push(Value::Object(member));
    }
    if !identifiers.contains("_self") {
        members.insert(
            0,
            json!({
                "id": "_self",
                "name": owner_name,
                "relationship": "self",
                "is_owner": true,
                "archived": false
            }),
        );
    }
    let active_ids = members
        .iter()
        .filter_map(Value::as_object)
        .filter(|member| {
            !member
                .get("archived")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .filter_map(|member| member.get("id").and_then(Value::as_str))
        .collect::<std::collections::BTreeSet<_>>();
    let active_scope = household
        .get("active_scope")
        .and_then(Value::as_str)
        .filter(|scope| {
            active_ids.contains(*scope) || (*scope == "__everyone__" && active_ids.len() >= 2)
        })
        .unwrap_or("_self")
        .to_owned();
    household.insert("owner_id".into(), Value::String("_self".into()));
    household.insert("active_scope".into(), Value::String(active_scope));
    household.insert("members".into(), Value::Array(members));
    Ok(household)
}

fn active_household_members(household: &Map<String, Value>) -> Vec<&Map<String, Value>> {
    household
        .get("members")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter(|member| {
            !member
                .get("archived")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .collect()
}

fn member_by_id<'a>(
    members: &'a [&Map<String, Value>],
    identifier: &str,
) -> Option<&'a Map<String, Value>> {
    members
        .iter()
        .copied()
        .find(|member| member.get("id").and_then(Value::as_str) == Some(identifier))
}

fn member_id(member: &Map<String, Value>) -> Result<&str, OneShotError> {
    member
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| OneShotError::new("household_state", "household member ID is missing"))
}

fn member_name(member: &Map<String, Value>) -> Result<&str, OneShotError> {
    member
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| OneShotError::new("household_state", "household member name is missing"))
}

fn resolve_household_scope(
    household: &Map<String, Value>,
    selector: Option<&str>,
) -> Result<String, OneShotError> {
    let selector = selector
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| household.get("active_scope").and_then(Value::as_str))
        .unwrap_or("_self");
    let members = active_household_members(household);
    let folded = selector.to_lowercase();
    if matches!(folded.as_str(), "me" | "myself" | "self" | "_self") {
        return Ok("_self".into());
    }
    if matches!(
        folded.as_str(),
        "all" | "everyone" | "household" | "family" | "__everyone__"
    ) {
        if members.len() < 2 {
            return Err(OneShotError::new(
                "household_scope",
                "add or import another household member before selecting everyone",
            ));
        }
        return Ok("__everyone__".into());
    }
    if member_by_id(&members, selector).is_some() {
        return Ok(selector.to_owned());
    }
    let matches = members
        .iter()
        .filter(|member| {
            member
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.to_lowercase() == folded)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [member] => Ok(member_id(member)?.to_owned()),
        [] => Err(OneShotError::new(
            "household_scope",
            format!("unknown household scope '{selector}'"),
        )),
        _ => Err(OneShotError::new(
            "household_scope",
            format!("more than one household member is named '{selector}'; use a member ID"),
        )),
    }
}

fn member_dietary_context(
    member: &Map<String, Value>,
    profile: &Value,
    owner: &Map<String, Value>,
) -> Result<Map<String, Value>, OneShotError> {
    const PROFILE_KEYS: &[&str] = &[
        "preferences",
        "preference_strictness",
        "restrictions",
        "restriction_handling",
        "avoid_ingredients",
        "medical_constraints",
        "severity_level",
        "notes",
        "activity_level",
        "cuisine_preferences",
    ];
    let mut context = Map::new();
    if let Some(profile) = profile.as_object() {
        for key in PROFILE_KEYS {
            if let Some(value) = profile.get(*key).filter(|value| !value.is_null()) {
                context.insert((*key).to_owned(), value.clone());
            }
        }
        if let Some(value) = profile.get("medical_condition_id") {
            context.insert("medical_condition".into(), value.clone());
        }
    }
    context.insert(
        "name".into(),
        Value::String(member_name(member)?.to_owned()),
    );
    context.insert(
        "relationship".into(),
        Value::String(
            member
                .get("relationship")
                .and_then(Value::as_str)
                .unwrap_or("other")
                .to_owned(),
        ),
    );
    if member_id(member)? != "_self" {
        context.insert(
            "owner_name".into(),
            Value::String(member_name(owner)?.to_owned()),
        );
    }
    if let Some(birth_date) = member.get("date_of_birth").and_then(Value::as_str) {
        context.insert("date_of_birth".into(), Value::String(birth_date.to_owned()));
    }
    Ok(context)
}

fn ensure_oura(provider: heyfood_cli::HealthProviderArgument) -> Result<(), OneShotError> {
    if !matches!(provider, heyfood_cli::HealthProviderArgument::Oura) {
        return Err(OneShotError::new(
            "health_provider",
            "only provider-neutral Oura management is implemented",
        ));
    }
    Ok(())
}

struct OwnedInteractiveTurn {
    operation_id: u64,
    cancellation: CancellationToken,
    task: JoinHandle<()>,
}

/// Production driver for the retained terminal surface.
///
/// The terminal loop stays synchronous and owns stdout. Every authenticated
/// refresh and SSE operation runs on this driver's private Tokio runtime and
/// communicates with the reducer through the bounded runtime-event channel.
/// Conversation continuity is process-memory-only, matching the TUI privacy
/// contract.
pub struct InteractiveTurnDriver {
    runtime: Runtime,
    service: Arc<dyn ServicePort>,
    ensure_session: Arc<EnsureSession>,
    session: Arc<Mutex<SessionSnapshot>>,
    conversation_id: Arc<Mutex<Option<String>>>,
    turns: Vec<OwnedInteractiveTurn>,
}

impl InteractiveTurnDriver {
    pub fn new(
        service: Arc<dyn ServicePort>,
        ensure_session: Arc<EnsureSession>,
        session: SessionSnapshot,
    ) -> io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("heyfood-turn")
            .build()?;
        Ok(Self {
            runtime,
            service,
            ensure_session,
            session: Arc::new(Mutex::new(session)),
            conversation_id: Arc::new(Mutex::new(None)),
            turns: Vec::new(),
        })
    }

    fn reap_finished(&mut self) {
        self.turns.retain(|turn| !turn.task.is_finished());
    }
}

impl QualifiedTurnDriver for InteractiveTurnDriver {
    fn start_turn(
        &mut self,
        operation_id: u64,
        prompt: String,
        runtime_events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        self.reap_finished();
        if !self.turns.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "a conversational turn is already active",
            ));
        }

        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let service = self.service.clone();
        let ensure_session = self.ensure_session.clone();
        let session = self.session.clone();
        let conversation_id = self.conversation_id.clone();
        let task = self.runtime.spawn(async move {
            let outcome = run_interactive_turn(
                operation_id,
                prompt,
                service,
                ensure_session,
                session,
                conversation_id,
                task_cancellation,
                runtime_events.clone(),
            )
            .await;
            let terminal_event = match outcome {
                Ok(outcome) => RuntimeEvent::TurnFinished {
                    operation_id,
                    outcome,
                },
                Err(message) => RuntimeEvent::TurnFailed {
                    operation_id,
                    message,
                },
            };
            let _ = runtime_events.send(terminal_event).await;
        });
        self.turns.push(OwnedInteractiveTurn {
            operation_id,
            cancellation,
            task,
        });
        Ok(())
    }

    fn cancel_turn(&mut self, operation_id: u64) -> io::Result<()> {
        let turn = self
            .turns
            .iter()
            .find(|turn| turn.operation_id == operation_id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "active turn is missing"))?;
        turn.cancellation.cancel();
        Ok(())
    }

    fn reset_conversation(&mut self) -> io::Result<()> {
        self.runtime.block_on(async {
            *self.conversation_id.lock().await = None;
        });
        Ok(())
    }

    fn shutdown_and_join(&mut self, timeout: Duration) -> io::Result<()> {
        for turn in &self.turns {
            turn.cancellation.cancel();
        }
        let turns = std::mem::take(&mut self.turns);
        self.runtime.block_on(async move {
            tokio::time::timeout(timeout, async move {
                for turn in turns {
                    turn.task.await.map_err(|error| {
                        io::Error::other(format!("turn supervisor task failed: {error}"))
                    })?;
                }
                Ok(())
            })
            .await
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "turn supervisor exceeded its shutdown deadline",
                )
            })?
        })
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_interactive_turn(
    operation_id: u64,
    prompt: String,
    service: Arc<dyn ServicePort>,
    ensure_session: Arc<EnsureSession>,
    session: Arc<Mutex<SessionSnapshot>>,
    conversation_id: Arc<Mutex<Option<String>>>,
    cancellation: CancellationToken,
    runtime_events: mpsc::Sender<RuntimeEvent>,
) -> Result<RunTurnOutcome, String> {
    let snapshot = session.lock().await.clone();
    let credentials = match ensure_session
        .execute(snapshot.clone(), cancellation.child_token())
        .await
        .map_err(|error| terminal_safe_text(&error.to_string()))?
    {
        EnsureSessionOutcome::Current(credentials) => credentials,
        EnsureSessionOutcome::Refreshed(credentials) => {
            let mut current = session.lock().await;
            current.credentials = credentials.clone();
            current.reconciliation_required = false;
            credentials
        }
        EnsureSessionOutcome::CancelledBeforeDispatch => {
            return Ok(RunTurnOutcome::CancelledBeforeServerAcceptance);
        }
    };

    if cancellation.is_cancelled() {
        return Ok(RunTurnOutcome::CancelledBeforeServerAcceptance);
    }
    let request = TurnRequest {
        prompt,
        conversation_id: conversation_id.lock().await.clone(),
        context: TurnContext::default(),
        refresh: RefreshPolicy::Never,
    };
    let accepted = service
        .open_turn(
            request,
            credentials,
            OperationId::new(),
            cancellation.child_token(),
        )
        .await;
    let mut accepted = match accepted {
        Ok(accepted) => accepted,
        Err(error) if error.code == "converse_cancelled_before_dispatch" => {
            return Ok(RunTurnOutcome::CancelledBeforeServerAcceptance);
        }
        Err(error) if error.outcome_uncertain => {
            return Ok(RunTurnOutcome::CancelledAfterDispatchOutcomeUnknown);
        }
        Err(error) => return Err(terminal_safe_text(&error.message)),
    };

    loop {
        let next = accepted.events.next();
        let event = tokio::select! {
            () = cancellation.cancelled() => {
                let _ = accepted.events.close().await;
                return Ok(RunTurnOutcome::CancelledAfterServerAcceptance);
            }
            event = next => event.map_err(|error| terminal_safe_text(&error.message))?,
        };
        let Some(event) = event else {
            let _ = accepted.events.close().await;
            return Err("The response stream ended before a final result arrived.".into());
        };
        let terminal = matches!(event, AgentEvent::Result { .. } | AgentEvent::Error { .. });
        if let AgentEvent::Result {
            conversation_id: Some(next_conversation),
            ..
        } = &event
        {
            *conversation_id.lock().await = Some(next_conversation.clone());
        }
        if runtime_events
            .send(RuntimeEvent::TurnEvent {
                operation_id,
                event,
            })
            .await
            .is_err()
        {
            cancellation.cancel();
            let _ = accepted.events.close().await;
            return Ok(RunTurnOutcome::CancelledAfterServerAcceptance);
        }
        if terminal {
            accepted
                .events
                .close()
                .await
                .map_err(|error| terminal_safe_text(&error.message))?;
            return Ok(RunTurnOutcome::Completed);
        }
    }
}

/// Runtime supervisor boundary used only after bootstrap has validated every
/// required input. Implementations must enqueue work and return promptly; the
/// retained terminal thread must never perform network IO.
pub trait QualifiedTurnDriver {
    fn start_turn(
        &mut self,
        operation_id: u64,
        prompt: String,
        events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()>;

    fn cancel_turn(&mut self, operation_id: u64) -> io::Result<()>;

    /// Forget process-local conversation continuity without touching persisted
    /// credentials or server-side data.
    fn reset_conversation(&mut self) -> io::Result<()> {
        Ok(())
    }

    /// Cancel any remaining operations, close their transports, and join every
    /// owned worker before the deadline. Returning `Ok` certifies that no turn
    /// task or socket remains owned by this driver.
    fn shutdown_and_join(&mut self, timeout: Duration) -> io::Result<()>;
}

#[derive(Debug)]
pub enum CompositionError {
    Tui(TuiError),
    Driver(io::Error),
    TuiAndDriver { tui: TuiError, driver: io::Error },
}

impl fmt::Display for CompositionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tui(error) => error.fmt(formatter),
            Self::Driver(error) => write!(formatter, "turn supervisor failed: {error}"),
            Self::TuiAndDriver { tui, driver } => write!(
                formatter,
                "terminal session failed ({tui}) and turn supervisor shutdown also failed: {driver}"
            ),
        }
    }
}

impl std::error::Error for CompositionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Tui(error) => Some(error),
            Self::Driver(error) => Some(error),
            Self::TuiAndDriver { driver, .. } => Some(driver),
        }
    }
}

/// Enter the terminal only after the caller has constructed a qualified driver
/// from explicit, validated native state.
pub fn run_qualified_session(
    driver: &mut impl QualifiedTurnDriver,
) -> Result<ExitReason, CompositionError> {
    let (runtime_sender, mut runtime_receiver) = mpsc::channel(64);
    let terminal = heyfood_tui::run_terminal(&mut runtime_receiver, |effect| {
        route_effect(driver, &runtime_sender, effect).map_err(|error| match error {
            CompositionError::Driver(error) => error,
            CompositionError::Tui(_) | CompositionError::TuiAndDriver { .. } => {
                unreachable!("effect routing does not enter the TUI")
            }
        })
    });
    finish_session(
        terminal,
        driver.shutdown_and_join(QUALIFIED_SHUTDOWN_TIMEOUT),
    )
}

fn finish_session(
    terminal: Result<ExitReason, TuiError>,
    shutdown: io::Result<()>,
) -> Result<ExitReason, CompositionError> {
    match (terminal, shutdown) {
        (Ok(reason), Ok(())) => Ok(reason),
        (Err(error), Ok(())) => Err(CompositionError::Tui(error)),
        (Ok(_), Err(error)) => Err(CompositionError::Driver(error)),
        (Err(tui), Err(driver)) => Err(CompositionError::TuiAndDriver { tui, driver }),
    }
}

fn route_effect(
    driver: &mut impl QualifiedTurnDriver,
    runtime_sender: &mpsc::Sender<RuntimeEvent>,
    effect: Effect,
) -> Result<(), CompositionError> {
    match effect {
        Effect::SubmitTurn {
            operation_id,
            prompt,
        } => driver
            .start_turn(operation_id, prompt, runtime_sender.clone())
            .map_err(CompositionError::Driver),
        Effect::CancelTurn { operation_id } => driver
            .cancel_turn(operation_id)
            .map_err(CompositionError::Driver),
        Effect::ResetConversation => driver
            .reset_conversation()
            .map_err(CompositionError::Driver),
        Effect::Exit(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, sync::Mutex as StdMutex, thread};

    use heyfood_application::{
        AcceptedTurn, BoxFuture, ClockPort, CredentialCommit, CredentialPort, EventStream,
        PortError,
    };
    use heyfood_core::{
        AccountId, AgentEvent, CommitId, CredentialVersion, RefreshOutcome, RefreshRequest,
        SensitiveString,
    };

    struct FixedClock;

    impl ClockPort for FixedClock {
        fn unix_timestamp(&self) -> i64 {
            0
        }
    }

    struct MemoryCredentialPort;

    impl CredentialPort for MemoryCredentialPort {
        fn load(&self) -> BoxFuture<'_, Result<Option<SessionCredentials>, PortError>> {
            Box::pin(async { Ok(None) })
        }

        fn commit(&self, _commit: CredentialCommit) -> BoxFuture<'_, Result<(), PortError>> {
            Box::pin(async { Ok(()) })
        }

        fn mark_reconciliation_required(
            &self,
            _commit_id: CommitId,
        ) -> BoxFuture<'_, Result<(), PortError>> {
            Box::pin(async { Ok(()) })
        }

        fn clear_reconciliation_required(
            &self,
            _commit_id: CommitId,
        ) -> BoxFuture<'_, Result<(), PortError>> {
            Box::pin(async { Ok(()) })
        }
    }

    struct FixtureStream {
        events: VecDeque<AgentEvent>,
    }

    impl EventStream for FixtureStream {
        fn next(&mut self) -> BoxFuture<'_, Result<Option<AgentEvent>, PortError>> {
            Box::pin(async { Ok(self.events.pop_front()) })
        }

        fn close(self: Box<Self>) -> BoxFuture<'static, Result<(), PortError>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[derive(Default)]
    struct FixtureService {
        requests: StdMutex<Vec<TurnRequest>>,
    }

    impl ServicePort for FixtureService {
        fn refresh_session(
            &self,
            _request: RefreshRequest,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<RefreshOutcome, PortError>> {
            Box::pin(async {
                Err(PortError::new(
                    "unexpected_refresh",
                    "fixture credentials must remain current",
                ))
            })
        }

        fn open_turn(
            &self,
            request: TurnRequest,
            _credentials: SessionCredentials,
            _operation_id: OperationId,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<AcceptedTurn, PortError>> {
            self.requests.lock().unwrap().push(request);
            Box::pin(async {
                Ok(AcceptedTurn {
                    events: Box::new(FixtureStream {
                        events: VecDeque::from([
                            AgentEvent::Partial {
                                text: "Hello ".into(),
                            },
                            AgentEvent::Result {
                                document: serde_json::json!({"text": "Hello there"}),
                                conversation_id: Some("conversation-1".into()),
                            },
                        ]),
                    }),
                })
            })
        }
    }

    fn fixture_credentials() -> SessionCredentials {
        SessionCredentials::from_unix_expiry(
            AccountId::parse("account-1").unwrap(),
            SensitiveString::new("access"),
            SensitiveString::new("refresh"),
            CredentialVersion::new(1),
            4_102_444_800,
        )
        .unwrap()
    }

    #[derive(Default)]
    struct ControlledDriver {
        started: Vec<(u64, String)>,
        cancelled: Vec<u64>,
        joined: bool,
    }

    impl QualifiedTurnDriver for ControlledDriver {
        fn start_turn(
            &mut self,
            operation_id: u64,
            prompt: String,
            events: mpsc::Sender<RuntimeEvent>,
        ) -> io::Result<()> {
            self.started.push((operation_id, prompt));
            events
                .try_send(RuntimeEvent::TurnEvent {
                    operation_id,
                    event: AgentEvent::Partial {
                        text: "controlled partial".into(),
                    },
                })
                .map_err(io::Error::other)
        }

        fn cancel_turn(&mut self, operation_id: u64) -> io::Result<()> {
            self.cancelled.push(operation_id);
            Ok(())
        }

        fn shutdown_and_join(&mut self, _timeout: Duration) -> io::Result<()> {
            self.joined = true;
            Ok(())
        }
    }

    #[test]
    fn interactive_driver_streams_and_retains_conversation_in_memory() {
        let service = Arc::new(FixtureService::default());
        let service_port: Arc<dyn ServicePort> = service.clone();
        let ensure_session = Arc::new(EnsureSession::new(
            service_port.clone(),
            Arc::new(MemoryCredentialPort),
            Arc::new(FixedClock),
        ));
        let mut driver = InteractiveTurnDriver::new(
            service_port,
            ensure_session,
            SessionSnapshot {
                credentials: fixture_credentials(),
                reconciliation_required: false,
            },
        )
        .unwrap();
        let (sender, mut receiver) = mpsc::channel(16);

        driver
            .start_turn(1, "first question".into(), sender.clone())
            .unwrap();
        let mut first_events = Vec::new();
        loop {
            let event = receiver.blocking_recv().expect("first turn event");
            let finished = matches!(
                event,
                RuntimeEvent::TurnFinished {
                    operation_id: 1,
                    outcome: RunTurnOutcome::Completed
                }
            );
            first_events.push(event);
            if finished {
                break;
            }
        }
        assert!(first_events.iter().any(|event| matches!(
            event,
            RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Partial { text }
            } if text == "Hello "
        )));

        for _ in 0..100 {
            if driver.turns.iter().all(|turn| turn.task.is_finished()) {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        driver
            .start_turn(2, "follow up".into(), sender)
            .expect("completed turn is reaped before the next turn");
        loop {
            if matches!(
                receiver.blocking_recv().expect("second turn event"),
                RuntimeEvent::TurnFinished {
                    operation_id: 2,
                    outcome: RunTurnOutcome::Completed
                }
            ) {
                break;
            }
        }
        driver.shutdown_and_join(Duration::from_secs(1)).unwrap();

        let requests = service.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].conversation_id, None);
        assert_eq!(
            requests[1].conversation_id.as_deref(),
            Some("conversation-1")
        );
    }

    #[test]
    fn controlled_driver_is_available_as_a_test_seam_without_a_binary_flag() {
        let (sender, mut receiver) = mpsc::channel(4);
        let mut driver = ControlledDriver::default();
        route_effect(
            &mut driver,
            &sender,
            Effect::SubmitTurn {
                operation_id: 7,
                prompt: "lunch".into(),
            },
        )
        .unwrap();
        assert_eq!(driver.started, [(7, "lunch".into())]);
        assert!(matches!(
            receiver.try_recv(),
            Ok(RuntimeEvent::TurnEvent {
                operation_id: 7,
                event: AgentEvent::Partial { .. }
            })
        ));

        route_effect(&mut driver, &sender, Effect::CancelTurn { operation_id: 7 }).unwrap();
        assert_eq!(driver.cancelled, [7]);
    }

    #[test]
    fn supervisor_shutdown_failure_cannot_be_reported_as_a_clean_exit() {
        let error = finish_session(
            Ok(ExitReason::Requested),
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "worker did not join",
            )),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            CompositionError::Driver(error) if error.kind() == io::ErrorKind::TimedOut
        ));
    }
}
