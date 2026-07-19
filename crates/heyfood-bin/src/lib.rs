//! Thin native composition seams for the Phase 0 qualification build.

#![forbid(unsafe_code)]

use std::{fmt, io};

use heyfood_tui::{Effect, ExitReason, RuntimeEvent, TuiError};
use tokio::sync::mpsc;

pub const QUALIFICATION_MESSAGE: &str = "The native interactive client is a Phase 0 qualification build and cannot start without validated native credentials and bootstrap state. Continue using the released Python client until cutover.";

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
}

#[derive(Debug)]
pub enum CompositionError {
    Tui(TuiError),
    Driver(io::Error),
}

impl fmt::Display for CompositionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tui(error) => error.fmt(formatter),
            Self::Driver(error) => write!(formatter, "turn supervisor failed: {error}"),
        }
    }
}

impl std::error::Error for CompositionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Tui(error) => Some(error),
            Self::Driver(error) => Some(error),
        }
    }
}

/// Enter the terminal only after the caller has constructed a qualified driver
/// from explicit, validated native state.
pub fn run_qualified_session(
    driver: &mut impl QualifiedTurnDriver,
) -> Result<ExitReason, CompositionError> {
    let (runtime_sender, mut runtime_receiver) = mpsc::channel(64);
    heyfood_tui::run_terminal(&mut runtime_receiver, |effect| {
        route_effect(driver, &runtime_sender, effect).map_err(|error| match error {
            CompositionError::Driver(error) => error,
            CompositionError::Tui(_) => unreachable!("effect routing does not enter the TUI"),
        })
    })
    .map_err(CompositionError::Tui)
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
}
