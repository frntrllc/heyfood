//! Thin native composition seams for the Phase 0 qualification build.

#![forbid(unsafe_code)]

use std::{fmt, io, time::Duration};

use heyfood_tui::{Effect, ExitReason, RuntimeEvent, TuiError};
use tokio::sync::mpsc;

pub const QUALIFICATION_MESSAGE: &str = "The native interactive client is a Phase 0 qualification build and cannot start without validated native credentials and bootstrap state. Continue using the released Python client until cutover.";
pub const QUALIFIED_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

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
        assert!(QUALIFICATION_MESSAGE.contains("released Python client"));
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
