//! Retained terminal presentation for the native heyfood client.
//!
//! The reducer in this crate is deliberately independent from Crossterm and
//! Ratatui. Runtime adapters feed [`RuntimeEvent`] values into it and execute
//! the returned [`Effect`] values; rendering is a read-only projection.

#![forbid(unsafe_code)]

mod input;
mod loop_driver;
mod model;
mod render;
mod terminal;

pub use input::action_from_key;
pub use loop_driver::{TuiError, run_terminal};
pub use model::{
    Action, AppModel, Effect, ExitReason, MAX_RENDERED_LINES, MAX_SCROLLBACK_BYTES,
    MAX_SCROLLBACK_ENTRIES, OperationState, RuntimeEvent, Scrollback, SemanticEntry, Speaker,
    dispatch,
};
pub use render::{ResponsiveMode, composer_height, render, responsive_mode};
pub use terminal::{
    CrosstermTerminalControl, GuardedError, TerminalControl, TerminalGuard, run_guarded,
};

/// The package version shared by the native workspace.
pub const VERSION: &str = heyfood_core::VERSION;
