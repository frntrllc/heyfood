use std::{collections::VecDeque, fmt::Write as _};

use heyfood_application::{RunTurnOutcome, agent_result_text};
use heyfood_core::{AgentEvent, terminal_safe_text};

pub const MAX_SCROLLBACK_ENTRIES: usize = 1_000;
pub const MAX_RENDERED_LINES: usize = 20_000;
pub const MAX_SCROLLBACK_BYTES: usize = 4 * 1024 * 1024;
const TRUNCATION_NOTICE: &str = "[… earlier content truncated …]\n";
const MAX_PROMPT_HISTORY: usize = 100;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SlashCommandKind {
    Help,
    New,
    Grocery,
    Health,
    Household,
    Profile,
    Location,
    Status,
    Clear,
    Exit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanelRequest {
    Grocery,
    Health,
    Household,
    Profile,
    Location,
}

impl PanelRequest {
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Grocery => "Grocery",
            Self::Health => "Health",
            Self::Household => "Household",
            Self::Profile => "Dietary profile",
            Self::Location => "Location",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SlashCommandSpec {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub usage: &'static str,
    pub description: &'static str,
    kind: SlashCommandKind,
}

pub const SLASH_COMMAND_REGISTRY: &[SlashCommandSpec] = &[
    SlashCommandSpec {
        name: "/help",
        aliases: &["/?"],
        usage: "/help",
        description: "Show commands and keyboard help",
        kind: SlashCommandKind::Help,
    },
    SlashCommandSpec {
        name: "/new",
        aliases: &[],
        usage: "/new",
        description: "Start a fresh conversation",
        kind: SlashCommandKind::New,
    },
    SlashCommandSpec {
        name: "/grocery",
        aliases: &[],
        usage: "/grocery",
        description: "Open the screened active Grocery list",
        kind: SlashCommandKind::Grocery,
    },
    SlashCommandSpec {
        name: "/health",
        aliases: &[],
        usage: "/health",
        description: "Open connected health context",
        kind: SlashCommandKind::Health,
    },
    SlashCommandSpec {
        name: "/household",
        aliases: &[],
        usage: "/household",
        description: "Open household targeting",
        kind: SlashCommandKind::Household,
    },
    SlashCommandSpec {
        name: "/profile",
        aliases: &[],
        usage: "/profile",
        description: "Open dietary profile readiness",
        kind: SlashCommandKind::Profile,
    },
    SlashCommandSpec {
        name: "/location",
        aliases: &[],
        usage: "/location",
        description: "Open active location context",
        kind: SlashCommandKind::Location,
    },
    SlashCommandSpec {
        name: "/status",
        aliases: &[],
        usage: "/status",
        description: "Show session readiness",
        kind: SlashCommandKind::Status,
    },
    SlashCommandSpec {
        name: "/clear",
        aliases: &[],
        usage: "/clear",
        description: "Clear visible scrollback",
        kind: SlashCommandKind::Clear,
    },
    SlashCommandSpec {
        name: "/exit",
        aliases: &["/quit"],
        usage: "/exit",
        description: "Close hey.food",
        kind: SlashCommandKind::Exit,
    },
];

#[must_use]
pub fn slash_suggestions(model: &AppModel, limit: usize) -> Vec<&'static SlashCommandSpec> {
    let query = model.draft.trim();
    if !query.starts_with('/') || query.contains(char::is_whitespace) {
        return Vec::new();
    }
    SLASH_COMMAND_REGISTRY
        .iter()
        .filter(|spec| {
            spec.name.starts_with(query)
                || spec.aliases.iter().any(|alias| alias.starts_with(query))
        })
        .take(limit)
        .collect()
}

fn resolve_slash_command(name: &str) -> Option<&'static SlashCommandSpec> {
    SLASH_COMMAND_REGISTRY
        .iter()
        .find(|spec| spec.name == name || spec.aliases.contains(&name))
}

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
    rendered_bytes: usize,
    maximum_entries: usize,
    maximum_lines: usize,
    maximum_bytes: usize,
}

impl Default for Scrollback {
    fn default() -> Self {
        Self::bounded(
            MAX_SCROLLBACK_ENTRIES,
            MAX_RENDERED_LINES,
            MAX_SCROLLBACK_BYTES,
        )
    }
}

impl Scrollback {
    #[must_use]
    pub fn bounded(maximum_entries: usize, maximum_lines: usize, maximum_bytes: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            rendered_lines: 0,
            rendered_bytes: 0,
            maximum_entries: maximum_entries.max(1),
            maximum_lines: maximum_lines.max(1),
            maximum_bytes: maximum_bytes.max(1),
        }
    }

    pub fn push(&mut self, entry: SemanticEntry) {
        self.rendered_lines = self.rendered_lines.saturating_add(entry.line_count());
        self.rendered_bytes = self.rendered_bytes.saturating_add(entry.text.len());
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

    #[must_use]
    pub const fn rendered_bytes(&self) -> usize {
        self.rendered_bytes
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.rendered_lines = 0;
        self.rendered_bytes = 0;
    }

    fn mutate_last_assistant(&mut self, mutate: impl FnOnce(&mut SemanticEntry)) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .rev()
            .find(|entry| entry.speaker == Speaker::Assistant && entry.streaming)
        {
            let before = entry.line_count();
            let before_bytes = entry.text.len();
            mutate(entry);
            let after = entry.line_count();
            let after_bytes = entry.text.len();
            self.rendered_lines = self
                .rendered_lines
                .saturating_sub(before)
                .saturating_add(after);
            self.rendered_bytes = self
                .rendered_bytes
                .saturating_sub(before_bytes)
                .saturating_add(after_bytes);
        }
        self.enforce_bounds();
    }

    fn enforce_bounds(&mut self) {
        while self.entries.len() > self.maximum_entries
            || (self.rendered_lines > self.maximum_lines && self.entries.len() > 1)
            || (self.rendered_bytes > self.maximum_bytes && self.entries.len() > 1)
        {
            if let Some(removed) = self.entries.pop_front() {
                self.rendered_lines = self.rendered_lines.saturating_sub(removed.line_count());
                self.rendered_bytes = self.rendered_bytes.saturating_sub(removed.text.len());
            }
        }
        if let Some(entry) = self.entries.back_mut() {
            if self.rendered_lines > self.maximum_lines {
                let mut retained = entry
                    .text
                    .lines()
                    .rev()
                    .take(self.maximum_lines)
                    .collect::<Vec<_>>();
                retained.reverse();
                entry.text = retained.join("\n");
            }
            retain_utf8_tail(&mut entry.text, self.maximum_bytes);
            self.rendered_lines = self.entries.iter().map(SemanticEntry::line_count).sum();
            self.rendered_bytes = self.entries.iter().map(|entry| entry.text.len()).sum();
        }
    }
}

fn retain_utf8_tail(text: &mut String, maximum_bytes: usize) {
    if text.len() <= maximum_bytes {
        return;
    }
    if maximum_bytes <= TRUNCATION_NOTICE.len() {
        let mut end = maximum_bytes;
        while !text.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        text.truncate(end);
        return;
    }
    let tail_bytes = maximum_bytes - TRUNCATION_NOTICE.len();
    let mut start = text.len().saturating_sub(tail_bytes);
    while !text.is_char_boundary(start) {
        start = start.saturating_add(1);
    }
    let tail = text[start..].to_owned();
    text.clear();
    text.push_str(TRUNCATION_NOTICE);
    text.push_str(&tail);
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
    prompt_history: VecDeque<String>,
    history_index: Option<usize>,
    history_draft: String,
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
            prompt_history: VecDeque::new(),
            history_index: None,
            history_draft: String::new(),
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
    PanelReady {
        operation_id: u64,
        panel: PanelRequest,
        body: String,
    },
    PanelFailed {
        operation_id: u64,
        panel: PanelRequest,
        message: String,
    },
    Notice {
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
    HistoryPrevious,
    HistoryNext,
    CompleteSlash,
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
    SubmitTurn {
        operation_id: u64,
        prompt: String,
    },
    OpenPanel {
        operation_id: u64,
        panel: PanelRequest,
    },
    CancelTurn {
        operation_id: u64,
    },
    ResetConversation,
    Exit(ExitReason),
}

#[must_use]
pub fn dispatch(model: &mut AppModel, action: Action) -> Vec<Effect> {
    match action {
        Action::Insert('?') if model.draft.is_empty() => show_help(model),
        Action::Insert(character) => {
            reset_history_navigation(model);
            insert_at_cursor(model, &character.to_string());
            model.idle_exit_armed = false;
        }
        Action::InsertText(text) => {
            reset_history_navigation(model);
            insert_at_cursor(model, &text);
            model.idle_exit_armed = false;
        }
        Action::Backspace => {
            reset_history_navigation(model);
            backspace(model);
        }
        Action::Delete => {
            reset_history_navigation(model);
            delete(model);
        }
        Action::MoveLeft => model.cursor = model.cursor.saturating_sub(1),
        Action::MoveRight => model.cursor = (model.cursor + 1).min(model.draft.chars().count()),
        Action::HistoryPrevious => history_previous(model),
        Action::HistoryNext => history_next(model),
        Action::CompleteSlash => complete_slash(model),
        Action::InsertNewline => {
            reset_history_navigation(model);
            insert_at_cursor(model, "\n");
        }
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
    if model.draft.trim().is_empty() {
        return Vec::new();
    }
    if model.draft.trim_start().starts_with('/') {
        return submit_slash_command(model);
    }
    if model.operation.is_active() {
        return Vec::new();
    }
    let prompt = std::mem::take(&mut model.draft);
    remember_prompt(model, &prompt);
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

fn submit_slash_command(model: &mut AppModel) -> Vec<Effect> {
    let command = model.draft.trim().to_owned();
    remember_prompt(model, &command);
    model.draft.clear();
    model.cursor = 0;
    let (name, arguments) = command
        .split_once(char::is_whitespace)
        .map_or((command.as_str(), ""), |(name, arguments)| {
            (name, arguments.trim())
        });
    let Some(spec) = resolve_slash_command(name) else {
        push_notice(
            model,
            "Unknown command. Use /help to see the interactive command registry.",
        );
        return Vec::new();
    };
    match spec.kind {
        SlashCommandKind::Help => show_help(model),
        SlashCommandKind::Status => push_notice(
            model,
            if model.operation.is_active() {
                "Connected · a turn is active · Ctrl+C stops it"
            } else {
                "Connected · Rust TUI ready · credentials and service are checked before every turn"
            },
        ),
        SlashCommandKind::Clear if model.operation.is_active() => push_notice(
            model,
            "Finish or stop the active turn before clearing the visible transcript.",
        ),
        SlashCommandKind::Clear => {
            model.scrollback.clear();
            model.activity = None;
            follow_tail(model);
        }
        SlashCommandKind::New if !arguments.is_empty() => {
            push_notice(model, &format!("Usage: {}", spec.usage));
        }
        SlashCommandKind::New if model.operation.is_active() => push_notice(
            model,
            "Stop the active turn with Ctrl+C, then run /new again.",
        ),
        SlashCommandKind::New => {
            push_notice(model, "Started a fresh conversation.");
            return vec![Effect::ResetConversation];
        }
        SlashCommandKind::Grocery
        | SlashCommandKind::Health
        | SlashCommandKind::Household
        | SlashCommandKind::Profile
        | SlashCommandKind::Location
            if !arguments.is_empty() =>
        {
            push_notice(model, &format!("Usage: {}", spec.usage));
        }
        SlashCommandKind::Grocery => return open_panel(model, PanelRequest::Grocery),
        SlashCommandKind::Health => return open_panel(model, PanelRequest::Health),
        SlashCommandKind::Household => return open_panel(model, PanelRequest::Household),
        SlashCommandKind::Profile => return open_panel(model, PanelRequest::Profile),
        SlashCommandKind::Location => return open_panel(model, PanelRequest::Location),
        SlashCommandKind::Exit => return begin_exit(model, ExitReason::Requested),
    }
    Vec::new()
}

fn open_panel(model: &mut AppModel, panel: PanelRequest) -> Vec<Effect> {
    if model.operation.is_active() {
        push_notice(
            model,
            "Finish or stop the active work before opening another panel.",
        );
        return Vec::new();
    }
    let operation_id = model.next_operation_id;
    model.next_operation_id = model.next_operation_id.saturating_add(1);
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::User,
        text: format!("/{}", panel.title().to_ascii_lowercase()),
        streaming: false,
    });
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::Assistant,
        text: String::new(),
        streaming: true,
    });
    model.operation = OperationState::Running(operation_id);
    model.activity = Some(format!("Loading {}…", panel.title()));
    model.idle_exit_armed = false;
    follow_tail(model);
    vec![Effect::OpenPanel {
        operation_id,
        panel,
    }]
}

fn show_help(model: &mut AppModel) {
    let mut help = String::from("Commands\n");
    for spec in SLASH_COMMAND_REGISTRY {
        let _ = writeln!(help, "  {:<14} {}", spec.usage, spec.description);
    }
    help.push_str(
        "\nKeys\n  Enter send · Shift+Enter/Ctrl+J newline · Up/Down history\n  Tab complete · PageUp/PageDown scroll · End follow\n  Ctrl+C stop · Ctrl+D exit",
    );
    push_notice(model, &help);
}

fn push_notice(model: &mut AppModel, text: &str) {
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::Notice,
        text: text.into(),
        streaming: false,
    });
    follow_tail(model);
}

fn remember_prompt(model: &mut AppModel, prompt: &str) {
    if model
        .prompt_history
        .back()
        .is_none_or(|last| last != prompt)
    {
        model.prompt_history.push_back(prompt.to_owned());
        while model.prompt_history.len() > MAX_PROMPT_HISTORY {
            model.prompt_history.pop_front();
        }
    }
    reset_history_navigation(model);
}

fn reset_history_navigation(model: &mut AppModel) {
    model.history_index = None;
    model.history_draft.clear();
}

fn history_previous(model: &mut AppModel) {
    if model.prompt_history.is_empty() {
        return;
    }
    let next = match model.history_index {
        None => {
            model.history_draft = model.draft.clone();
            model.prompt_history.len() - 1
        }
        Some(index) => index.saturating_sub(1),
    };
    model.history_index = Some(next);
    model.draft = model.prompt_history[next].clone();
    model.cursor = model.draft.chars().count();
}

fn history_next(model: &mut AppModel) {
    let Some(index) = model.history_index else {
        return;
    };
    if index + 1 < model.prompt_history.len() {
        let next = index + 1;
        model.history_index = Some(next);
        model.draft = model.prompt_history[next].clone();
    } else {
        model.history_index = None;
        model.draft = std::mem::take(&mut model.history_draft);
    }
    model.cursor = model.draft.chars().count();
}

fn complete_slash(model: &mut AppModel) {
    let suggestions = slash_suggestions(model, 2);
    if let [spec] = suggestions.as_slice() {
        model.draft = spec.name.to_owned();
        model.cursor = model.draft.chars().count();
    }
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
        RuntimeEvent::Notice { message } => push_notice(model, &terminal_safe_text(&message)),
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
            finish_stream(model, outcome);
        }
        RuntimeEvent::TurnFailed {
            operation_id,
            message,
        } if model.operation.operation_id() == Some(operation_id) => {
            let message = terminal_safe_text(&message);
            model.scrollback.mutate_last_assistant(|entry| {
                if !entry.text.is_empty() {
                    entry.text.push_str("\n\n");
                }
                entry
                    .text
                    .push_str(&format!("Unable to complete this turn: {message}"));
            });
            finish_stream(model, RunTurnOutcome::Completed);
        }
        RuntimeEvent::PanelReady {
            operation_id,
            panel,
            body,
        } if model.operation.operation_id() == Some(operation_id) => {
            finish_panel(model, panel, Ok(body));
        }
        RuntimeEvent::PanelFailed {
            operation_id,
            panel,
            message,
        } if model.operation.operation_id() == Some(operation_id) => {
            finish_panel(model, panel, Err(message));
        }
        RuntimeEvent::TurnEvent { .. }
        | RuntimeEvent::TurnFinished { .. }
        | RuntimeEvent::TurnFailed { .. }
        | RuntimeEvent::PanelReady { .. }
        | RuntimeEvent::PanelFailed { .. } => {}
    }
    Vec::new()
}

fn finish_panel(model: &mut AppModel, panel: PanelRequest, result: Result<String, String>) {
    let old_lines = model.scrollback.rendered_lines();
    model.scrollback.mutate_last_assistant(|entry| {
        entry.text = match result {
            Ok(body) => {
                let body = terminal_safe_text(&body);
                if body.trim().is_empty() {
                    format!("{}\n\nNo information is available.", panel.title())
                } else {
                    format!("{}\n\n{}", panel.title(), body.trim_end())
                }
            }
            Err(message) => format!(
                "Unable to open {}: {}",
                panel.title(),
                terminal_safe_text(&message)
            ),
        };
        entry.streaming = false;
    });
    model.operation = OperationState::Idle;
    model.activity = None;
    model.idle_exit_armed = false;
    account_for_new_lines(model, old_lines);
}

fn apply_agent_event(model: &mut AppModel, event: AgentEvent) {
    let old_lines = model.scrollback.rendered_lines();
    match event {
        AgentEvent::Thinking { stage, message } => {
            model.activity = message
                .or(stage)
                .map(|value| terminal_safe_text(&value))
                .or_else(|| Some("Thinking…".into()));
        }
        AgentEvent::Progress {
            message,
            current,
            total,
        } => {
            let message = terminal_safe_text(&message);
            model.activity = match (current, total) {
                (Some(current), Some(total)) => Some(format!("{message} ({current}/{total})")),
                _ => Some(message),
            };
        }
        AgentEvent::Partial { text } => {
            let text = terminal_safe_text(&text);
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
                    entry.text.push_str(&terminal_safe_text(&choice.label));
                    entry.text.push('\n');
                }
            });
            model.activity = Some("Choose an option".into());
        }
        AgentEvent::Result { document, .. } => {
            let result = agent_result_text(&document).map(terminal_safe_text);
            model.scrollback.mutate_last_assistant(|entry| {
                if let Some(result) = result {
                    if !result.is_empty() {
                        entry.text = result;
                    }
                } else if entry.text.is_empty() {
                    entry.text = terminal_safe_text(&document.to_string());
                }
                entry.streaming = false;
            });
            mark_finishing(model);
            model.activity = Some("Finishing…".into());
            model.idle_exit_armed = false;
        }
        AgentEvent::Error { error } => {
            let code = terminal_safe_text(&error.code);
            let message = terminal_safe_text(&error.message);
            model.scrollback.mutate_last_assistant(|entry| {
                if !entry.text.is_empty() {
                    entry.text.push_str("\n\n");
                }
                entry.text.push_str(&format!("{code}: {message}"));
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

fn finish_stream(model: &mut AppModel, outcome: RunTurnOutcome) {
    let old_lines = model.scrollback.rendered_lines();
    model.scrollback.mutate_last_assistant(|entry| {
        let notice = match outcome {
            RunTurnOutcome::Completed => None,
            RunTurnOutcome::CancelledBeforeServerAcceptance => Some("Turn cancelled."),
            RunTurnOutcome::CancelledAfterServerAcceptance => Some(
                "Turn cancelled after server acceptance. Check the conversation before retrying.",
            ),
            RunTurnOutcome::CancelledAfterDispatchOutcomeUnknown => Some(
                "Cancellation happened after dispatch and the server outcome is unknown. Check current state before retrying.",
            ),
            RunTurnOutcome::StaleGeneration => {
                Some("Turn stopped because the active account or context changed.")
            }
        };
        if let Some(notice) = notice {
            if !entry.text.is_empty() {
                entry.text.push_str("\n\n");
            }
            entry.text.push_str(notice);
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
    fn runtime_text_is_terminal_safe_even_when_an_adapter_constructs_events_directly() {
        let mut model = AppModel {
            draft: "question".into(),
            cursor: 8,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Partial {
                    text: "safe\u{1b}]52;clipboard\u{7}".into(),
                },
            }),
        );
        let text = &model.scrollback.entries().back().unwrap().text;
        assert_eq!(text, "safe]52;clipboard");
        assert!(!text.chars().any(|character| character == '\u{1b}'));
    }

    #[test]
    fn scrollback_is_bounded_by_entries_and_lines() {
        let mut scrollback = Scrollback::bounded(3, 4, 1_024);
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
    fn one_unbroken_stream_is_bounded_by_utf8_bytes() {
        let mut scrollback = Scrollback::bounded(3, 100, 96);
        scrollback.push(SemanticEntry {
            speaker: Speaker::Assistant,
            text: String::new(),
            streaming: true,
        });
        scrollback.mutate_last_assistant(|entry| {
            entry.text.push_str(&"é".repeat(1_000));
        });
        assert!(scrollback.rendered_bytes() <= 96);
        assert!(scrollback.entries().back().unwrap().text.ends_with('é'));
        assert!(
            scrollback
                .entries()
                .back()
                .unwrap()
                .text
                .starts_with(TRUNCATION_NOTICE)
        );
    }

    #[test]
    fn uncertain_post_dispatch_cancellation_is_not_presented_as_safe_to_retry() {
        let mut model = AppModel {
            draft: "mutating question".into(),
            cursor: 17,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnFinished {
                operation_id: 1,
                outcome: RunTurnOutcome::CancelledAfterDispatchOutcomeUnknown,
            }),
        );
        let text = &model.scrollback.entries().back().unwrap().text;
        assert!(text.contains("server outcome is unknown"));
        assert!(text.contains("Check current state before retrying"));
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
    fn slash_commands_are_local_and_new_resets_conversation() {
        let mut model = AppModel {
            draft: "/help".into(),
            cursor: 5,
            ..AppModel::default()
        };
        assert!(dispatch(&mut model, Action::Submit).is_empty());
        assert!(model.draft.is_empty());
        assert!(
            model
                .scrollback
                .entries()
                .back()
                .unwrap()
                .text
                .contains("/new")
        );

        model.draft = "/new".into();
        model.cursor = 4;
        assert_eq!(
            dispatch(&mut model, Action::Submit),
            vec![Effect::ResetConversation]
        );
    }

    #[test]
    fn prompt_history_restores_the_unsent_draft() {
        let mut model = AppModel {
            draft: "first".into(),
            cursor: 5,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnFinished {
                operation_id: 1,
                outcome: RunTurnOutcome::Completed,
            }),
        );
        model.draft = "working draft".into();
        model.cursor = model.draft.chars().count();
        let _ = dispatch(&mut model, Action::HistoryPrevious);
        assert_eq!(model.draft, "first");
        let _ = dispatch(&mut model, Action::HistoryNext);
        assert_eq!(model.draft, "working draft");
    }

    #[test]
    fn tab_completes_a_unique_slash_prefix() {
        let mut model = AppModel {
            draft: "/sta".into(),
            cursor: 4,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::CompleteSlash);
        assert_eq!(model.draft, "/status");
        assert_eq!(model.cursor, 7);
    }

    #[test]
    fn command_registry_drives_aliases_help_and_discovery() {
        let mut model = AppModel {
            draft: "/".into(),
            cursor: 1,
            ..AppModel::default()
        };
        assert_eq!(slash_suggestions(&model, 3).len(), 3);
        let _ = dispatch(&mut model, Action::CompleteSlash);
        assert_eq!(model.draft, "/", "ambiguous prefixes must remain editable");

        model.draft = "/quit".into();
        model.cursor = 5;
        assert_eq!(
            dispatch(&mut model, Action::Submit),
            vec![Effect::Exit(ExitReason::Requested)]
        );
    }

    #[test]
    fn terminal_message_and_response_fields_use_normalized_result_text() {
        for (document, expected) in [
            (
                serde_json::json!({"message": "final message"}),
                "final message",
            ),
            (
                serde_json::json!({"response": "final response"}),
                "final response",
            ),
        ] {
            let mut model = AppModel {
                draft: "question".into(),
                cursor: 8,
                ..AppModel::default()
            };
            let _ = dispatch(&mut model, Action::Submit);
            let _ = dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::TurnEvent {
                    operation_id: 1,
                    event: AgentEvent::Partial {
                        text: "streamed draft".into(),
                    },
                }),
            );
            let _ = dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::TurnEvent {
                    operation_id: 1,
                    event: AgentEvent::Result {
                        document,
                        conversation_id: None,
                    },
                }),
            );
            let entry = model.scrollback.entries().back().unwrap();
            assert_eq!(entry.text, expected);
            assert!(!entry.text.contains('{'));
            assert!(!entry.streaming);
        }
    }

    #[test]
    fn partial_only_terminal_document_preserves_the_streamed_answer() {
        let mut model = AppModel {
            draft: "question".into(),
            cursor: 8,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Partial {
                    text: "complete streamed answer".into(),
                },
            }),
        );
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Result {
                    document: serde_json::json!({"conversation_id": "conversation-1"}),
                    conversation_id: Some("conversation-1".into()),
                },
            }),
        );
        let entry = model.scrollback.entries().back().unwrap();
        assert_eq!(entry.text, "complete streamed answer");
        assert!(!entry.streaming);
    }

    #[test]
    fn incomplete_panels_are_not_advertised_as_available_commands() {
        for command in ["/voice", "/for"] {
            assert!(resolve_slash_command(command).is_none(), "{command}");
        }
    }

    #[test]
    fn runtime_notices_are_visible_and_terminal_safe() {
        let mut model = AppModel::default();
        assert!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::Notice {
                    message: "Account connected.\u{1b}[31m hidden".into(),
                }),
            )
            .is_empty()
        );
        let entry = model.scrollback.entries().back().unwrap();
        assert_eq!(entry.speaker, Speaker::Notice);
        assert_eq!(entry.text, "Account connected.[31m hidden");
    }

    #[test]
    fn available_panel_commands_dispatch_typed_effects() {
        for (command, panel) in [
            ("/grocery", PanelRequest::Grocery),
            ("/health", PanelRequest::Health),
            ("/household", PanelRequest::Household),
            ("/profile", PanelRequest::Profile),
            ("/location", PanelRequest::Location),
        ] {
            let mut model = AppModel {
                draft: command.into(),
                cursor: command.len(),
                ..AppModel::default()
            };
            assert_eq!(
                dispatch(&mut model, Action::Submit),
                vec![Effect::OpenPanel {
                    operation_id: 1,
                    panel,
                }]
            );
            assert_eq!(model.operation, OperationState::Running(1));
            assert_eq!(
                model.scrollback.entries().back().unwrap().speaker,
                Speaker::Assistant
            );

            let _ = dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::PanelReady {
                    operation_id: 1,
                    panel,
                    body: "Live service result".into(),
                }),
            );
            let result = model.scrollback.entries().back().unwrap();
            assert!(result.text.starts_with(panel.title()));
            assert!(result.text.contains("Live service result"));
            assert!(!result.streaming);
            assert_eq!(model.operation, OperationState::Idle);
        }
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
