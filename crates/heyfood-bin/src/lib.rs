//! Thin native composition seams for the Phase 0 qualification build.

#![forbid(unsafe_code)]

use std::{fmt, io, time::Duration};

use heyfood_agent_runtime::{GroceryExport, HttpService};
use heyfood_cli::{
    Command, GroceryCommand, HealthCommand, OutputMode, render_grocery_list,
    render_grocery_proposal, render_health_context, render_json,
};
use heyfood_core::{
    AddItemsRequestWire, GroceryConfirmationToken, GroceryDecisionWire, GroceryEntityId,
    GroceryItemInputWire, GroceryListVersion, GroceryMutationConfirmRequestWire, OperationId,
    RemoveItemsRequestWire, SessionCredentials, UpdateItemStateRequestWire, terminal_safe_text,
};
use heyfood_tui::{Effect, ExitReason, RuntimeEvent, TuiError};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub const QUALIFICATION_MESSAGE: &str = "The native interactive session cannot start in this build. Use a native one-shot command such as `heyfood register`; run `heyfood -h` for available commands.";
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

/// Phase 2 qualification executor over explicit, already-validated native
/// state. The public binary does not yet construct this executor, so repository
/// integration cannot activate or cut over the Rust client.
pub struct OneShotExecutor<'a> {
    service: &'a HttpService,
    credentials: &'a SessionCredentials,
    output_mode: OutputMode,
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
        }
    }

    pub async fn execute(
        &self,
        command: Command,
        stdin: &[u8],
        cancellation: CancellationToken,
    ) -> Result<String, OneShotError> {
        match command {
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
    fn new(code: &'static str, message: impl Into<String>) -> Self {
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

fn ensure_oura(provider: heyfood_cli::HealthProviderArgument) -> Result<(), OneShotError> {
    if !matches!(provider, heyfood_cli::HealthProviderArgument::Oura) {
        return Err(OneShotError::new(
            "health_provider",
            "only provider-neutral Oura management is implemented",
        ));
    }
    Ok(())
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
        Effect::Exit(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use heyfood_core::AgentEvent;

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
    fn qualification_message_is_fail_closed_and_does_not_advertise_a_spike_flag() {
        assert!(QUALIFICATION_MESSAGE.contains("cannot start"));
        assert!(QUALIFICATION_MESSAGE.contains("heyfood register"));
        assert!(!QUALIFICATION_MESSAGE.contains("Python"));
        assert!(!QUALIFICATION_MESSAGE.contains("--"));
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
