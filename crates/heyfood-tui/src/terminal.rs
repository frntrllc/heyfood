use std::{
    any::Any,
    fmt, io,
    panic::{self, AssertUnwindSafe},
    sync::Mutex,
};

use crossterm::{
    cursor::{Hide, Show},
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};

static PANIC_HOOK_LOCK: Mutex<()> = Mutex::new(());
type PanicHook = Box<dyn Fn(&panic::PanicHookInfo<'_>) + Send + Sync + 'static>;

/// Testable boundary for the terminal bytes and mode changes owned by the TUI.
pub trait TerminalControl {
    type Error;

    fn enter(&mut self) -> Result<(), Self::Error>;
    fn restore(&mut self) -> Result<(), Self::Error>;
}

/// Sole RAII owner of raw mode, alternate screen, cursor, and bracketed paste.
pub struct TerminalGuard<C: TerminalControl> {
    control: Option<C>,
}

impl<C: TerminalControl> TerminalGuard<C> {
    pub fn new(mut control: C) -> Result<Self, C::Error> {
        if let Err(error) = control.enter() {
            // Entry can partially succeed (for example, raw mode before an IO
            // failure). Restoration is best effort and the original error wins.
            let _ = control.restore();
            return Err(error);
        }
        Ok(Self {
            control: Some(control),
        })
    }

    pub fn restore(&mut self) -> Result<(), C::Error> {
        if let Some(mut control) = self.control.take() {
            control.restore()
        } else {
            Ok(())
        }
    }
}

impl<C: TerminalControl> Drop for TerminalGuard<C> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[derive(Debug)]
pub enum GuardedError<E, T> {
    Enter(T),
    Body(E),
    Panic(String),
}

impl<E: fmt::Display, T: fmt::Display> fmt::Display for GuardedError<E, T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Enter(error) => write!(formatter, "could not enter terminal mode: {error}"),
            Self::Body(error) => error.fmt(formatter),
            Self::Panic(message) => write!(formatter, "terminal application panicked: {message}"),
        }
    }
}

/// Runs a catchable terminal body. The guard is dropped before a body error or
/// captured panic is returned to the composition root for reporting.
pub fn run_guarded<C, F, R, E>(control: C, body: F) -> Result<R, GuardedError<E, C::Error>>
where
    C: TerminalControl,
    F: FnOnce() -> Result<R, E>,
{
    // Panic diagnostics must not be written into the alternate screen. Worker
    // panics remain observable through their JoinHandle payloads, while a body
    // panic is returned below and can be reported after restoration.
    let _panic_hook_lock = PANIC_HOOK_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _panic_hook = MutedPanicHook::install();
    let guard = TerminalGuard::new(control).map_err(GuardedError::Enter)?;
    let result = panic::catch_unwind(AssertUnwindSafe(body));
    drop(guard);
    match result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(GuardedError::Body(error)),
        Err(payload) => Err(GuardedError::Panic(panic_message(payload))),
    }
}

struct MutedPanicHook {
    previous: Option<PanicHook>,
}

impl MutedPanicHook {
    fn install() -> Self {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(|_| {}));
        Self {
            previous: Some(previous),
        }
    }
}

impl Drop for MutedPanicHook {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            panic::set_hook(previous);
        }
    }
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_owned()
    }
}

#[derive(Default)]
pub struct CrosstermTerminalControl;

impl TerminalControl for CrosstermTerminalControl {
    type Error = io::Error;

    fn enter(&mut self) -> Result<(), Self::Error> {
        enable_raw_mode()?;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            Hide
        )
    }

    fn restore(&mut self) -> Result<(), Self::Error> {
        // Write presentation restoration in one ordered flush, then return the
        // terminal driver to cooked mode even if that write failed.
        let sequence = execute!(
            io::stdout(),
            DisableBracketedPaste,
            Show,
            LeaveAlternateScreen
        );
        let cooked = disable_raw_mode();
        sequence.and(cooked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct RecordingControl {
        events: Arc<Mutex<Vec<&'static str>>>,
        fail_enter: bool,
    }

    impl TerminalControl for RecordingControl {
        type Error = &'static str;

        fn enter(&mut self) -> Result<(), Self::Error> {
            self.events.lock().unwrap().push("enter");
            if self.fail_enter {
                Err("enter failed")
            } else {
                Ok(())
            }
        }

        fn restore(&mut self) -> Result<(), Self::Error> {
            self.events.lock().unwrap().push("restore");
            Ok(())
        }
    }

    fn control(events: &Arc<Mutex<Vec<&'static str>>>) -> RecordingControl {
        RecordingControl {
            events: Arc::clone(events),
            fail_enter: false,
        }
    }

    #[test]
    fn restores_after_normal_return() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let result = run_guarded(control(&events), || Ok::<_, &str>(7));
        assert_eq!(result.unwrap(), 7);
        assert_eq!(*events.lock().unwrap(), ["enter", "restore"]);
    }

    #[test]
    fn restores_before_returning_body_error() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let result = run_guarded(control(&events), || Err::<(), _>("body failed"));
        assert!(matches!(result, Err(GuardedError::Body("body failed"))));
        assert_eq!(*events.lock().unwrap(), ["enter", "restore"]);
    }

    #[test]
    fn restores_before_returning_catchable_panic() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let result = run_guarded(control(&events), || -> Result<(), &str> { panic!("boom") });
        assert!(matches!(result, Err(GuardedError::Panic(message)) if message == "boom"));
        assert_eq!(*events.lock().unwrap(), ["enter", "restore"]);
    }

    #[test]
    fn partially_failed_entry_attempts_restoration() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut failing = control(&events);
        failing.fail_enter = true;
        let result = run_guarded(failing, || Ok::<_, &str>(()));
        assert!(matches!(result, Err(GuardedError::Enter("enter failed"))));
        assert_eq!(*events.lock().unwrap(), ["enter", "restore"]);
    }

    #[test]
    fn explicit_restore_is_idempotent() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut guard = TerminalGuard::new(control(&events)).unwrap();
        guard.restore().unwrap();
        drop(guard);
        assert_eq!(*events.lock().unwrap(), ["enter", "restore"]);
    }

    #[test]
    fn restoration_sequence_is_single_flush_order() {
        let mut output = Vec::new();
        execute!(
            &mut output,
            DisableBracketedPaste,
            Show,
            LeaveAlternateScreen
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "\u{1b}[?2004l\u{1b}[?25h\u{1b}[?1049l"
        );
        io::sink().flush().unwrap();
    }
}
