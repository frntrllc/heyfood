use std::{fmt, io, time::Duration};

use crossterm::event::{self, Event};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc;

use crate::{
    Action, AppModel, CrosstermTerminalControl, Effect, ExitReason, GuardedError, RuntimeEvent,
    action_from_key, dispatch, render, run_guarded,
};

const INPUT_POLL: Duration = Duration::from_millis(16);

#[derive(Debug)]
pub enum TuiError {
    Terminal(io::Error),
    Effect(io::Error),
    Panic(String),
}

impl fmt::Display for TuiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Terminal(error) => write!(formatter, "terminal operation failed: {error}"),
            Self::Effect(error) => write!(formatter, "TUI effect delivery failed: {error}"),
            Self::Panic(message) => write!(formatter, "TUI panicked: {message}"),
        }
    }
}

impl std::error::Error for TuiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Terminal(error) | Self::Effect(error) => Some(error),
            Self::Panic(_) => None,
        }
    }
}

/// Run the retained terminal surface.
///
/// Runtime work stays outside this function. Its bounded receiver is drained
/// between short terminal polls, and every reducer effect is synchronously
/// handed to the composition root. The sink must enqueue or supervise work; it
/// must never perform network IO on the UI thread.
pub fn run_terminal(
    runtime_events: &mut mpsc::Receiver<RuntimeEvent>,
    mut effect_sink: impl FnMut(Effect) -> io::Result<()>,
) -> Result<ExitReason, TuiError> {
    let result = run_guarded(CrosstermTerminalControl, || {
        terminal_body(runtime_events, &mut effect_sink)
    });
    match result {
        Ok(reason) => Ok(reason),
        Err(GuardedError::Enter(error)) => Err(TuiError::Terminal(error)),
        Err(GuardedError::Body(error)) => Err(error),
        Err(GuardedError::Panic(message)) => Err(TuiError::Panic(message)),
    }
}

fn terminal_body(
    runtime_events: &mut mpsc::Receiver<RuntimeEvent>,
    effect_sink: &mut impl FnMut(Effect) -> io::Result<()>,
) -> Result<ExitReason, TuiError> {
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).map_err(TuiError::Terminal)?;
    let size = terminal.size().map_err(TuiError::Terminal)?;
    let mut model = AppModel::default();
    let _ = dispatch(
        &mut model,
        Action::Resize {
            width: size.width,
            height: size.height,
        },
    );
    terminal.clear().map_err(TuiError::Terminal)?;

    loop {
        while let Ok(runtime) = runtime_events.try_recv() {
            if let Some(reason) = apply(&mut model, Action::Runtime(runtime), effect_sink)? {
                return Ok(reason);
            }
        }

        terminal
            .draw(|frame| render(frame, &model))
            .map_err(TuiError::Terminal)?;

        if !event::poll(INPUT_POLL).map_err(TuiError::Terminal)? {
            continue;
        }
        let action = match event::read().map_err(TuiError::Terminal)? {
            Event::Key(key) => action_from_key(key),
            Event::Paste(text) => Some(Action::InsertText(text)),
            Event::Resize(width, height) => Some(Action::Resize { width, height }),
            Event::FocusGained | Event::FocusLost | Event::Mouse(_) => None,
        };
        if let Some(action) = action
            && let Some(reason) = apply(&mut model, action, effect_sink)?
        {
            return Ok(reason);
        }
    }
}

fn apply(
    model: &mut AppModel,
    action: Action,
    effect_sink: &mut impl FnMut(Effect) -> io::Result<()>,
) -> Result<Option<ExitReason>, TuiError> {
    let effects = dispatch(model, action);
    let mut exit = None;
    for effect in effects {
        if let Effect::Exit(reason) = effect {
            exit = Some(reason);
            effect_sink(Effect::Exit(reason)).map_err(TuiError::Effect)?;
        } else {
            effect_sink(effect).map_err(TuiError::Effect)?;
        }
    }
    Ok(exit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_delivery_preserves_cancel_before_exit_order() {
        let mut model = AppModel::default();
        model.draft = "question".into();
        model.cursor = 8;
        let mut effects = Vec::new();
        apply(&mut model, Action::Submit, &mut |effect| {
            effects.push(effect);
            Ok(())
        })
        .unwrap();
        effects.clear();
        let exit = apply(
            &mut model,
            Action::Runtime(RuntimeEvent::ExternalSignal(ExitReason::Interrupt)),
            &mut |effect| {
                effects.push(effect);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(exit, Some(ExitReason::Interrupt));
        assert_eq!(
            effects,
            [
                Effect::CancelTurn { operation_id: 1 },
                Effect::Exit(ExitReason::Interrupt),
            ]
        );
    }

    #[test]
    fn sink_failure_is_typed() {
        let mut model = AppModel::default();
        model.draft = "question".into();
        model.cursor = 8;
        let error = apply(&mut model, Action::Submit, &mut |_| {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
        })
        .unwrap_err();
        assert!(
            matches!(error, TuiError::Effect(error) if error.kind() == io::ErrorKind::BrokenPipe)
        );
    }
}
