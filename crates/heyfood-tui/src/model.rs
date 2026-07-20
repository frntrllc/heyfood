use std::collections::VecDeque;

use heyfood_application::RunTurnOutcome;
use heyfood_core::AgentEvent;

pub const MAX_SCROLLBACK_ENTRIES: usize = 1_000;
pub const MAX_RENDERED_LINES: usize = 20_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Speaker {
    User,
    Assistant,
    Notice,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticEntry {
    pub speaker: Speaker,
    pub text: String,
    pub streaming: bool,
}

impl SemanticEntry {
    fn line_count(&self) -> usize {
        self.text.lines().count().max(1)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Scrollback {
    entries: VecDeque<SemanticEntry>,
    rendered_lines: usize,
    maximum_entries: usize,
    maximum_lines: usize,
}

impl Default for Scrollback {
    fn default() -> Self {
        Self::bounded(MAX_SCROLLBACK_ENTRIES, MAX_RENDERED_LINES)
    }
}

impl Scrollback {
    #[must_use]
    pub fn bounded(maximum_entries: usize, maximum_lines: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            rendered_lines: 0,
            maximum_entries: maximum_entries.max(1),
            maximum_lines: maximum_lines.max(1),
        }
    }

    pub fn push(&mut self, entry: SemanticEntry) {
        self.rendered_lines = self.rendered_lines.saturating_add(entry.line_count());
        self.entries.push_back(entry);
        self.enforce_bounds();
    }

    #[must_use]
    pub fn entries(&self) -> &VecDeque<SemanticEntry> {
        &self.entries
    }

    #[must_use]
    pub const fn rendered_lines(&self) -> usize {
        self.rendered_lines
    }

    fn mutate_last_assistant(&mut self, mutate: impl FnOnce(&mut SemanticEntry)) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .rev()
            .find(|entry| entry.speaker == Speaker::Assistant && entry.streaming)
        {
            let before = entry.line_count();
            mutate(entry);
            let after = entry.line_count();
            self.rendered_lines = self
                .rendered_lines
                .saturating_sub(before)
                .saturating_add(after);
        }
        self.enforce_bounds();
    }

    fn enforce_bounds(&mut self) {
        while self.entries.len() > self.maximum_entries
            || (self.rendered_lines > self.maximum_lines && self.entries.len() > 1)
        {
            if let Some(removed) = self.entries.pop_front() {
                self.rendered_lines = self.rendered_lines.saturating_sub(removed.line_count());
            }
        }
        if self.rendered_lines > self.maximum_lines
            && let Some(entry) = self.entries.back_mut()
        {
            let mut retained = entry
                .text
                .lines()
                .rev()
                .take(self.maximum_lines)
                .collect::<Vec<_>>();
            retained.reverse();
            entry.text = retained.join("\n");
            self.rendered_lines = entry.line_count();
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExitReason {
    Requested,
    Interrupt,
    Terminate,
    Hangup,
}

impl ExitReason {
    #[must_use]
    pub const fn exit_code(self) -> i32 {
        match self {
            Self::Requested => 0,
            Self::Interrupt => 130,
            Self::Terminate => 143,
            Self::Hangup => 129,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationState {
    Idle,
    Running(u64),
    Cancelling(u64),
    Finishing(u64),
    Exiting(ExitReason),
}

impl OperationState {
    #[must_use]
    pub const fn operation_id(self) -> Option<u64> {
        match self {
            Self::Running(id) | Self::Cancelling(id) | Self::Finishing(id) => Some(id),
            Self::Idle | Self::Exiting(_) => None,
        }
    }

    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(
            self,
            Self::Running(_) | Self::Cancelling(_) | Self::Finishing(_)
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppModel {
    pub scrollback: Scrollback,
    pub draft: String,
    /// Character index, not byte index.
    pub cursor: usize,
    pub width: u16,
    pub height: u16,
    pub operation: OperationState,
    pub activity: Option<String>,
    pub follow_tail: bool,
    pub scroll_from_tail: usize,
    pub unseen_lines: usize,
    pub idle_exit_armed: bool,
    next_operation_id: u64,
}

impl Default for AppModel {
    fn default() -> Self {
        Self {
            scrollback: Scrollback::default(),
            draft: String::new(),
            cursor: 0,
            width: 80,
            height: 24,
            operation: OperationState::Idle,
            activity: None,
            follow_tail: true,
            scroll_from_tail: 0,
            unseen_lines: 0,
            idle_exit_armed: false,
            next_operation_id: 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RuntimeEvent {
    TurnEvent {
        operation_id: u64,
        event: AgentEvent,
    },
    TurnFinished {
        operation_id: u64,
        outcome: RunTurnOutcome,
    },
    TurnFailed {
        operation_id: u64,
        message: String,
    },
    ExternalSignal(ExitReason),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Insert(char),
    InsertText(String),
    Backspace,
    Delete,
    MoveLeft,
    MoveRight,
    InsertNewline,
    Submit,
    CancelOrExit,
    Exit,
    ScrollUp(usize),
    ScrollDown(usize),
    ScrollTop,
    FollowTail,
    Resize { width: u16, height: u16 },
    Runtime(RuntimeEvent),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Effect {
    SubmitTurn { operation_id: u64, prompt: String },
    CancelTurn { operation_id: u64 },
    Exit(ExitReason),
}

#[must_use]
pub fn dispatch(model: &mut AppModel, action: Action) -> Vec<Effect> {
    match action {
        Action::Insert(character) => {
            insert_at_cursor(model, &character.to_string());
            model.idle_exit_armed = false;
        }
        Action::InsertText(text) => {
            insert_at_cursor(model, &text);
            model.idle_exit_armed = false;
        }
        Action::Backspace => backspace(model),
        Action::Delete => delete(model),
        Action::MoveLeft => model.cursor = model.cursor.saturating_sub(1),
        Action::MoveRight => model.cursor = (model.cursor + 1).min(model.draft.chars().count()),
        Action::InsertNewline => insert_at_cursor(model, "\n"),
        Action::Submit => return submit(model),
        Action::CancelOrExit => return cancel_or_exit(model),
        Action::Exit if model.draft.is_empty() => {
            return begin_exit(model, ExitReason::Requested);
        }
        Action::Exit => {}
        Action::ScrollUp(lines) => {
            model.follow_tail = false;
            model.scroll_from_tail = model.scroll_from_tail.saturating_add(lines.max(1));
        }
        Action::ScrollDown(lines) => {
            model.scroll_from_tail = model.scroll_from_tail.saturating_sub(lines.max(1));
            if model.scroll_from_tail == 0 {
                follow_tail(model);
            }
        }
        Action::ScrollTop => {
            model.follow_tail = false;
            model.scroll_from_tail = usize::MAX / 2;
        }
        Action::FollowTail => follow_tail(model),
        Action::Resize { width, height } => {
            model.width = width;
            model.height = height;
        }
        Action::Runtime(event) => return runtime_event(model, event),
    }
    Vec::new()
}

fn submit(model: &mut AppModel) -> Vec<Effect> {
    if model.operation.is_active() || model.draft.trim().is_empty() {
        return Vec::new();
    }
    let prompt = std::mem::take(&mut model.draft);
    model.cursor = 0;
    let operation_id = model.next_operation_id;
    model.next_operation_id = model.next_operation_id.saturating_add(1);
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::User,
        text: prompt.clone(),
        streaming: false,
    });
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::Assistant,
        text: String::new(),
        streaming: true,
    });
    model.operation = OperationState::Running(operation_id);
    model.activity = Some("Connecting…".into());
    follow_tail(model);
    vec![Effect::SubmitTurn {
        operation_id,
        prompt,
    }]
}

fn cancel_or_exit(model: &mut AppModel) -> Vec<Effect> {
    if !model.draft.is_empty() {
        model.draft.clear();
        model.cursor = 0;
        model.idle_exit_armed = false;
        return Vec::new();
    }
    match model.operation {
        OperationState::Running(operation_id) => {
            model.operation = OperationState::Cancelling(operation_id);
            model.activity = Some("Stopping…".into());
            vec![Effect::CancelTurn { operation_id }]
        }
        OperationState::Cancelling(_) | OperationState::Finishing(_) => Vec::new(),
        OperationState::Idle if model.idle_exit_armed => begin_exit(model, ExitReason::Requested),
        OperationState::Idle => {
            model.idle_exit_armed = true;
            model.activity = Some("Press Ctrl+C again to exit".into());
            Vec::new()
        }
        OperationState::Exiting(_) => Vec::new(),
    }
}

fn begin_exit(model: &mut AppModel, reason: ExitReason) -> Vec<Effect> {
    let mut effects = Vec::new();
    if let Some(operation_id) = model.operation.operation_id() {
        effects.push(Effect::CancelTurn { operation_id });
    }
    model.operation = OperationState::Exiting(reason);
    effects.push(Effect::Exit(reason));
    effects
}

fn runtime_event(model: &mut AppModel, runtime: RuntimeEvent) -> Vec<Effect> {
    match runtime {
        RuntimeEvent::ExternalSignal(reason) => return begin_exit(model, reason),
        RuntimeEvent::TurnEvent {
            operation_id,
            event,
        } if model.operation.operation_id() == Some(operation_id) => {
            apply_agent_event(model, event)
        }
        RuntimeEvent::TurnFinished {
            operation_id,
            outcome,
        } if model.operation.operation_id() == Some(operation_id) => {
            let cancelled = !matches!(outcome, RunTurnOutcome::Completed);
            finish_stream(model, cancelled);
        }
        RuntimeEvent::TurnFailed {
            operation_id,
            message,
        } if model.operation.operation_id() == Some(operation_id) => {
            model.scrollback.mutate_last_assistant(|entry| {
                if !entry.text.is_empty() {
                    entry.text.push_str("\n\n");
                }
                entry
                    .text
                    .push_str(&format!("Unable to complete this turn: {message}"));
            });
            finish_stream(model, false);
        }
        RuntimeEvent::TurnEvent { .. }
        | RuntimeEvent::TurnFinished { .. }
        | RuntimeEvent::TurnFailed { .. } => {}
    }
    Vec::new()
}

fn apply_agent_event(model: &mut AppModel, event: AgentEvent) {
    let old_lines = model.scrollback.rendered_lines();
    match event {
        AgentEvent::Thinking { stage, message } => {
            model.activity = message.or(stage).or_else(|| Some("Thinking…".into()));
        }
        AgentEvent::Progress {
            message,
            current,
            total,
        } => {
            model.activity = match (current, total) {
                (Some(current), Some(total)) => Some(format!("{message} ({current}/{total})")),
                _ => Some(message),
            };
        }
        AgentEvent::Partial { text } => {
            model
                .scrollback
                .mutate_last_assistant(|entry| entry.text.push_str(&text));
            model.activity = Some("Responding…".into());
        }
        AgentEvent::Choices { choices, .. } => {
            model.scrollback.mutate_last_assistant(|entry| {
                if !entry.text.is_empty() {
                    entry.text.push('\n');
                }
                for choice in choices {
                    entry.text.push_str("• ");
                    entry.text.push_str(&choice.label);
                    entry.text.push('\n');
                }
            });
            model.activity = Some("Choose an option".into());
        }
        AgentEvent::Result { document, .. } => {
            let result = document
                .as_str()
                .or_else(|| document.get("text").and_then(|value| value.as_str()))
                .map(str::to_owned)
                .unwrap_or_else(|| document.to_string());
            model.scrollback.mutate_last_assistant(|entry| {
                entry.text = result;
                entry.streaming = false;
            });
            mark_finishing(model);
            model.activity = Some("Finishing…".into());
            model.idle_exit_armed = false;
        }
        AgentEvent::Error { error } => {
            model.scrollback.mutate_last_assistant(|entry| {
                if !entry.text.is_empty() {
                    entry.text.push_str("\n\n");
                }
                entry
                    .text
                    .push_str(&format!("{}: {}", error.code, error.message));
                entry.streaming = false;
            });
            mark_finishing(model);
            model.activity = Some("Finishing…".into());
        }
    }
    account_for_new_lines(model, old_lines);
}

fn mark_finishing(model: &mut AppModel) {
    if let Some(operation_id) = model.operation.operation_id() {
        model.operation = OperationState::Finishing(operation_id);
    }
}

fn finish_stream(model: &mut AppModel, cancelled: bool) {
    let old_lines = model.scrollback.rendered_lines();
    model.scrollback.mutate_last_assistant(|entry| {
        if cancelled && entry.text.is_empty() {
            entry.text = "Turn cancelled.".into();
        }
        entry.streaming = false;
    });
    model.operation = OperationState::Idle;
    model.activity = None;
    model.idle_exit_armed = false;
    account_for_new_lines(model, old_lines);
}

fn account_for_new_lines(model: &mut AppModel, old_lines: usize) {
    if model.follow_tail {
        return;
    }
    let added = model
        .scrollback
        .rendered_lines()
        .saturating_sub(old_lines)
        .max(1);
    model.scroll_from_tail = model.scroll_from_tail.saturating_add(added);
    model.unseen_lines = model.unseen_lines.saturating_add(added);
}

fn follow_tail(model: &mut AppModel) {
    model.follow_tail = true;
    model.scroll_from_tail = 0;
    model.unseen_lines = 0;
}

fn insert_at_cursor(model: &mut AppModel, text: &str) {
    let byte = byte_index(&model.draft, model.cursor);
    model.draft.insert_str(byte, text);
    model.cursor += text.chars().count();
}

fn backspace(model: &mut AppModel) {
    if model.cursor == 0 {
        return;
    }
    let start = byte_index(&model.draft, model.cursor - 1);
    let end = byte_index(&model.draft, model.cursor);
    model.draft.replace_range(start..end, "");
    model.cursor -= 1;
    model.idle_exit_armed = false;
}

fn delete(model: &mut AppModel) {
    let characters = model.draft.chars().count();
    if model.cursor >= characters {
        return;
    }
    let start = byte_index(&model.draft, model.cursor);
    let end = byte_index(&model.draft, model.cursor + 1);
    model.draft.replace_range(start..end, "");
    model.idle_exit_armed = false;
}

fn byte_index(text: &str, character_index: usize) -> usize {
    text.char_indices()
        .nth(character_index)
        .map_or(text.len(), |(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use heyfood_core::AgentFailure;

    #[test]
    fn draft_remains_editable_while_streaming_and_is_not_auto_submitted() {
        let mut model = AppModel {
            draft: "lunch".into(),
            cursor: 5,
            ..AppModel::default()
        };
        let effects = dispatch(&mut model, Action::Submit);
        assert_eq!(effects.len(), 1);
        let _ = dispatch(&mut model, Action::InsertText("follow up".into()));
        assert_eq!(model.draft, "follow up");
        assert!(dispatch(&mut model, Action::Submit).is_empty());

        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Partial {
                    text: "Your meal ".into(),
                },
            }),
        );
        assert_eq!(model.draft, "follow up");
        assert_eq!(
            model.scrollback.entries().back().unwrap().text,
            "Your meal "
        );
    }

    #[test]
    fn stale_runtime_events_are_ignored() {
        let mut model = AppModel {
            draft: "question".into(),
            cursor: 8,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 99,
                event: AgentEvent::Error {
                    error: AgentFailure {
                        code: "stale".into(),
                        message: "must not appear".into(),
                        retryable: false,
                    },
                },
            }),
        );
        assert!(model.scrollback.entries().back().unwrap().text.is_empty());
        assert_eq!(model.operation, OperationState::Running(1));
    }

    #[test]
    fn scrollback_is_bounded_by_entries_and_lines() {
        let mut scrollback = Scrollback::bounded(3, 4);
        for number in 0..8 {
            scrollback.push(SemanticEntry {
                speaker: Speaker::Notice,
                text: format!("entry {number}\nline"),
                streaming: false,
            });
        }
        assert!(scrollback.entries().len() <= 3);
        assert!(scrollback.rendered_lines() <= 4);
        assert!(scrollback.entries().back().unwrap().text.contains('7'));

        scrollback.push(SemanticEntry {
            speaker: Speaker::Assistant,
            text: (0..20)
                .map(|line| format!("line {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
            streaming: true,
        });
        assert_eq!(scrollback.rendered_lines(), 4);
        assert_eq!(scrollback.entries().len(), 1);
        assert!(
            scrollback
                .entries()
                .back()
                .unwrap()
                .text
                .contains("line 19")
        );
    }

    #[test]
    fn scrolling_away_preserves_position_and_counts_streamed_updates() {
        let mut model = AppModel {
            draft: "question".into(),
            cursor: 8,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(&mut model, Action::ScrollUp(5));
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Partial {
                    text: "one\ntwo".into(),
                },
            }),
        );
        assert!(!model.follow_tail);
        assert!(model.scroll_from_tail > 5);
        assert!(model.unseen_lines > 0);
        let _ = dispatch(&mut model, Action::FollowTail);
        assert!(model.follow_tail);
        assert_eq!(model.unseen_lines, 0);
    }

    #[test]
    fn keyboard_cancel_has_clear_cancel_and_double_exit_states() {
        let mut model = AppModel {
            draft: "draft".into(),
            cursor: 5,
            ..AppModel::default()
        };
        assert!(dispatch(&mut model, Action::CancelOrExit).is_empty());
        assert!(model.draft.is_empty());

        model.draft = "turn".into();
        model.cursor = 4;
        let _ = dispatch(&mut model, Action::Submit);
        assert_eq!(
            dispatch(&mut model, Action::CancelOrExit),
            vec![Effect::CancelTurn { operation_id: 1 }]
        );
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnFinished {
                operation_id: 1,
                outcome: RunTurnOutcome::CancelledAfterServerAcceptance,
            }),
        );
        assert!(dispatch(&mut model, Action::CancelOrExit).is_empty());
        assert!(model.idle_exit_armed);
        assert_eq!(
            dispatch(&mut model, Action::CancelOrExit),
            vec![Effect::Exit(ExitReason::Requested)]
        );
    }

    #[test]
    fn external_signal_cancels_and_exits_with_platform_code() {
        let mut model = AppModel {
            draft: "turn".into(),
            cursor: 4,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        assert_eq!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::ExternalSignal(ExitReason::Terminate))
            ),
            vec![
                Effect::CancelTurn { operation_id: 1 },
                Effect::Exit(ExitReason::Terminate),
            ]
        );
        assert_eq!(ExitReason::Terminate.exit_code(), 143);
    }

    #[test]
    fn terminal_event_keeps_single_flight_closed_until_turn_finished() {
        let mut model = AppModel {
            draft: "first".into(),
            cursor: 5,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Result {
                    document: Default::default(),
                    conversation_id: Some("conversation-1".into()),
                },
            }),
        );
        assert_eq!(model.operation, OperationState::Finishing(1));

        let _ = dispatch(&mut model, Action::InsertText("second".into()));
        assert!(dispatch(&mut model, Action::Submit).is_empty());

        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnFinished {
                operation_id: 1,
                outcome: RunTurnOutcome::Completed,
            }),
        );
        assert_eq!(model.operation, OperationState::Idle);
        assert_eq!(dispatch(&mut model, Action::Submit).len(), 1);
    }
}
