use std::{collections::VecDeque, fmt::Write as _};

use heyfood_application::{RunTurnOutcome, agent_result_text};
use heyfood_core::{
    ActionConfirmationEnvelopeWire, AgentConfirmationCommandWire, AgentEvent,
    ConfirmationDecisionWire, GroceryEditPatch, OnboardingOption, OnboardingProfileInput,
    activity_options, allergy_options, condition_options, cuisine_options, diet_options,
    required_text, terminal_safe_text,
};

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
    For,
    Profile,
    Onboard,
    Location,
    Status,
    Clear,
    Exit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanelRequest {
    Status,
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
            Self::Status => "Status",
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
        name: "/for",
        aliases: &[],
        usage: "/for MEMBER|everyone",
        description: "Target future turns to a household scope",
        kind: SlashCommandKind::For,
    },
    SlashCommandSpec {
        name: "/profile",
        aliases: &[],
        usage: "/profile",
        description: "Open dietary profile readiness",
        kind: SlashCommandKind::Profile,
    },
    SlashCommandSpec {
        name: "/onboard",
        aliases: &[],
        usage: "/onboard",
        description: "Build or replace your synced dietary profile",
        kind: SlashCommandKind::Onboard,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OnboardingStep {
    Diets,
    Allergies,
    Conditions,
    Severity,
    AvoidIngredients,
    Activity,
    Cuisines,
    Notes,
    Review,
    Saving,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OnboardingFlow {
    step: OnboardingStep,
    profile: OnboardingProfileInput,
}

struct MultiSelection {
    ids: Vec<String>,
    custom: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingActionConfirmation {
    confirmation_id: heyfood_core::GroceryConfirmationId,
    idempotency_key: heyfood_core::GroceryIdempotencyKey,
    editable_items: Option<Vec<serde_json::Map<String, serde_json::Value>>>,
}

impl PendingActionConfirmation {
    fn command(
        &self,
        decision: ConfirmationDecisionWire,
        edits: Option<GroceryEditPatch>,
    ) -> AgentConfirmationCommandWire {
        AgentConfirmationCommandWire {
            confirmation_id: self.confirmation_id,
            idempotency_key: self.idempotency_key,
            decision,
            edits,
        }
    }
}

impl Default for OnboardingFlow {
    fn default() -> Self {
        Self {
            step: OnboardingStep::Diets,
            profile: OnboardingProfileInput::default(),
        }
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
    pending_choice_labels: Vec<String>,
    pending_confirmation: Option<PendingActionConfirmation>,
    onboarding: Option<OnboardingFlow>,
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
            pending_choice_labels: Vec::new(),
            pending_confirmation: None,
            onboarding: None,
            next_operation_id: 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RuntimeEvent {
    BeginOnboarding {
        message: String,
    },
    OnboardingSaved {
        operation_id: u64,
    },
    OnboardingFailed {
        operation_id: u64,
        message: String,
    },
    OnboardingCancelled {
        operation_id: u64,
        outcome: RunTurnOutcome,
    },
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
    HouseholdScopeReady {
        operation_id: u64,
        label: String,
    },
    HouseholdScopeFailed {
        operation_id: u64,
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
    SaveOnboarding {
        operation_id: u64,
        profile: Box<OnboardingProfileInput>,
    },
    SubmitTurn {
        operation_id: u64,
        prompt: String,
    },
    ConfirmAction {
        operation_id: u64,
        command: AgentConfirmationCommandWire,
    },
    OpenPanel {
        operation_id: u64,
        panel: PanelRequest,
    },
    SelectHousehold {
        operation_id: u64,
        selector: String,
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
        Action::Insert('?') if model.draft.is_empty() && model.onboarding.is_none() => {
            show_help(model)
        }
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
    if model.onboarding.is_some() {
        return submit_onboarding(model);
    }
    if model.pending_confirmation.is_some() {
        return submit_confirmation_answer(model);
    }
    if model.draft.trim_start().starts_with('/') {
        return submit_slash_command(model);
    }
    if model.operation.is_active() {
        return Vec::new();
    }
    let prompt = std::mem::take(&mut model.draft);
    model.pending_choice_labels.clear();
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

fn submit_confirmation_answer(model: &mut AppModel) -> Vec<Effect> {
    if model.operation.is_active() {
        return Vec::new();
    }
    let answer = model.draft.trim().to_owned();
    let normalized = answer.to_ascii_lowercase();
    let decision = match normalized.as_str() {
        "y" | "yes" | "confirm" | "accept" => ConfirmationDecisionWire::Accept,
        "n" | "no" | "cancel" => ConfirmationDecisionWire::Cancel,
        value if value.starts_with("edit ") => return submit_confirmation_edit(model, &answer),
        _ => {
            push_notice(
                model,
                "A write is awaiting your decision. Type `y` to confirm, `n` to cancel, or use the edit instruction shown on the card.",
            );
            return Vec::new();
        }
    };
    submit_confirmation(model, decision, None)
}

fn submit_confirmation_edit(model: &mut AppModel, answer: &str) -> Vec<Effect> {
    let Some(pending) = model.pending_confirmation.as_ref() else {
        return Vec::new();
    };
    let Some(editable_items) = pending.editable_items.as_ref() else {
        push_notice(
            model,
            "This proposal does not expose a contract-backed item edit.",
        );
        return Vec::new();
    };
    let mut words = answer.split_whitespace();
    let command = words.next();
    let reference = words.next();
    let replacement = words.collect::<Vec<_>>().join(" ");
    let index = command
        .filter(|value| value.eq_ignore_ascii_case("edit"))
        .and(reference)
        .and_then(|value| value.strip_prefix('#'))
        .and_then(|value| value.parse::<usize>().ok());
    let replacement = required_text(&replacement, 255).ok();
    let (Some(index), Some(replacement)) = (index, replacement) else {
        push_notice(model, "Use `edit #N <replacement item name>`.");
        return Vec::new();
    };
    if index == 0 || index > editable_items.len() {
        push_notice(model, "That item number is outside the pending proposal.");
        return Vec::new();
    }
    let mut items = editable_items.clone();
    items[index - 1].insert("name".into(), serde_json::Value::String(replacement));
    let edits = GroceryEditPatch::new(serde_json::Map::from_iter([(
        "items".into(),
        serde_json::Value::Array(items.into_iter().map(serde_json::Value::Object).collect()),
    )]));
    let Ok(edits) = edits else {
        push_notice(model, "The corrected proposal is too large or invalid.");
        return Vec::new();
    };
    submit_confirmation(model, ConfirmationDecisionWire::Accept, Some(edits))
}

fn submit_confirmation(
    model: &mut AppModel,
    decision: ConfirmationDecisionWire,
    edits: Option<GroceryEditPatch>,
) -> Vec<Effect> {
    if model.operation.is_active() {
        return Vec::new();
    }
    let Some(pending) = model.pending_confirmation.as_ref() else {
        return Vec::new();
    };
    let editing = edits.is_some();
    let command = pending.command(decision, edits);
    model.draft.clear();
    model.cursor = 0;
    let operation_id = model.next_operation_id;
    model.next_operation_id = model.next_operation_id.saturating_add(1);
    let label = match (decision, editing) {
        (ConfirmationDecisionWire::Accept, true) => "Edit and confirm",
        (ConfirmationDecisionWire::Accept, false) => "Confirm",
        (ConfirmationDecisionWire::Cancel, _) => "Cancel",
    };
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::User,
        text: label.into(),
        streaming: false,
    });
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::Assistant,
        text: String::new(),
        streaming: true,
    });
    model.operation = OperationState::Running(operation_id);
    model.activity = Some(match (decision, editing) {
        (ConfirmationDecisionWire::Accept, true) => "Applying correction…".into(),
        (ConfirmationDecisionWire::Accept, false) => "Confirming…".into(),
        (ConfirmationDecisionWire::Cancel, _) => "Cancelling proposal…".into(),
    });
    model.idle_exit_armed = false;
    follow_tail(model);
    vec![Effect::ConfirmAction {
        operation_id,
        command,
    }]
}

fn begin_onboarding(model: &mut AppModel, message: &str) {
    if model.onboarding.is_some() {
        push_notice(model, "Dietary onboarding is already in progress.");
        return;
    }
    model.onboarding = Some(OnboardingFlow::default());
    model.idle_exit_armed = false;
    push_notice(model, message);
    push_onboarding_prompt(model);
}

fn submit_onboarding(model: &mut AppModel) -> Vec<Effect> {
    if model.operation.is_active() {
        return Vec::new();
    }
    let answer = std::mem::take(&mut model.draft);
    model.cursor = 0;
    let answer = answer.trim();
    if matches!(answer.to_ascii_lowercase().as_str(), "cancel" | "/cancel") {
        model.onboarding = None;
        push_notice(
            model,
            "Dietary onboarding cancelled. Nothing was sent or saved.",
        );
        return Vec::new();
    }

    let mut flow = model
        .onboarding
        .take()
        .expect("onboarding submission requires an active flow");
    if answer.eq_ignore_ascii_case("back") {
        flow.step = previous_onboarding_step(flow.step, &flow.profile);
        model.onboarding = Some(flow);
        push_onboarding_prompt(model);
        return Vec::new();
    }

    let result = apply_onboarding_answer(&mut flow, answer);
    if let Err(message) = result {
        model.onboarding = Some(flow);
        push_notice(model, &message);
        push_onboarding_prompt(model);
        return Vec::new();
    }

    if flow.step == OnboardingStep::Saving {
        let profile = flow.profile.clone();
        if let Err(message) = profile.profile_data() {
            flow.step = OnboardingStep::Review;
            model.onboarding = Some(flow);
            push_notice(model, &format!("Unable to review this profile: {message}"));
            return Vec::new();
        }
        let operation_id = model.next_operation_id;
        model.next_operation_id = model.next_operation_id.saturating_add(1);
        model.scrollback.push(SemanticEntry {
            speaker: Speaker::User,
            text: "Save dietary profile".into(),
            streaming: false,
        });
        model.scrollback.push(SemanticEntry {
            speaker: Speaker::Assistant,
            text: String::new(),
            streaming: true,
        });
        model.onboarding = Some(flow);
        model.operation = OperationState::Running(operation_id);
        model.activity = Some("Saving dietary profile…".into());
        follow_tail(model);
        return vec![Effect::SaveOnboarding {
            operation_id,
            profile: Box::new(profile),
        }];
    }

    model.scrollback.push(SemanticEntry {
        speaker: Speaker::User,
        text: terminal_safe_text(answer),
        streaming: false,
    });
    model.onboarding = Some(flow);
    push_onboarding_prompt(model);
    Vec::new()
}

fn apply_onboarding_answer(flow: &mut OnboardingFlow, answer: &str) -> Result<(), String> {
    match flow.step {
        OnboardingStep::Diets => {
            let selected = parse_multi_options(answer, diet_options(), 10, 40)?;
            flow.profile.diet_style_ids = selected.ids;
            flow.profile.custom_diet_styles = selected.custom;
            flow.step = OnboardingStep::Allergies;
        }
        OnboardingStep::Allergies => {
            let selected = parse_multi_options(answer, allergy_options(), 10, 60)?;
            flow.profile.allergy_ids = selected.ids;
            flow.profile.custom_restrictions = selected.custom;
            flow.step = OnboardingStep::Conditions;
        }
        OnboardingStep::Conditions => {
            let selected = parse_multi_options(answer, condition_options(), 10, 60)?;
            flow.profile.health_condition_ids = selected.ids;
            flow.profile.custom_health_conditions = selected.custom;
            flow.step = if flow.profile.health_condition_ids.is_empty() {
                flow.profile.severity_level = None;
                OnboardingStep::AvoidIngredients
            } else {
                OnboardingStep::Severity
            };
        }
        OnboardingStep::Severity => {
            let severity = answer
                .parse::<u8>()
                .ok()
                .filter(|value| (1..=5).contains(value))
                .ok_or_else(|| "Enter a condition severity from 1 to 5.".to_owned())?;
            flow.profile.severity_level = Some(severity);
            flow.step = OnboardingStep::AvoidIngredients;
        }
        OnboardingStep::AvoidIngredients => {
            flow.profile.avoid_ingredients = parse_free_text_list(answer, 20, 40)?;
            flow.step = OnboardingStep::Activity;
        }
        OnboardingStep::Activity => {
            flow.profile.activity_level = parse_single_option(answer, activity_options())?;
            flow.step = OnboardingStep::Cuisines;
        }
        OnboardingStep::Cuisines => {
            let selected = parse_multi_options(answer, cuisine_options(), 10, 40)?;
            flow.profile.cuisine_preferences = selected.ids;
            flow.profile.custom_cuisines = selected.custom;
            flow.step = OnboardingStep::Notes;
        }
        OnboardingStep::Notes => {
            flow.profile.notes = parse_optional_text(answer, 280)?;
            flow.step = OnboardingStep::Review;
        }
        OnboardingStep::Review if answer.eq_ignore_ascii_case("save") => {
            flow.step = OnboardingStep::Saving;
        }
        OnboardingStep::Review => {
            return Err(
                "Type `save` to confirm, `back` to edit, or `cancel` to discard it.".into(),
            );
        }
        OnboardingStep::Saving => return Err("The dietary profile is already being saved.".into()),
    }
    Ok(())
}

fn previous_onboarding_step(
    step: OnboardingStep,
    profile: &OnboardingProfileInput,
) -> OnboardingStep {
    match step {
        OnboardingStep::Diets => OnboardingStep::Diets,
        OnboardingStep::Allergies => OnboardingStep::Diets,
        OnboardingStep::Conditions => OnboardingStep::Allergies,
        OnboardingStep::Severity => OnboardingStep::Conditions,
        OnboardingStep::AvoidIngredients if profile.health_condition_ids.is_empty() => {
            OnboardingStep::Conditions
        }
        OnboardingStep::AvoidIngredients => OnboardingStep::Severity,
        OnboardingStep::Activity => OnboardingStep::AvoidIngredients,
        OnboardingStep::Cuisines => OnboardingStep::Activity,
        OnboardingStep::Notes => OnboardingStep::Cuisines,
        OnboardingStep::Review | OnboardingStep::Saving => OnboardingStep::Notes,
    }
}

fn parse_multi_options(
    answer: &str,
    options: &[OnboardingOption],
    custom_maximum: usize,
    custom_max_length: usize,
) -> Result<MultiSelection, String> {
    if is_none_answer(answer) {
        return Ok(MultiSelection {
            ids: Vec::new(),
            custom: Vec::new(),
        });
    }
    if let Some(option) = resolve_onboarding_option(answer.trim(), options) {
        return Ok(MultiSelection {
            ids: vec![option.id.clone()],
            custom: Vec::new(),
        });
    }
    let mut selected = MultiSelection {
        ids: Vec::new(),
        custom: Vec::new(),
    };
    for token in answer
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if is_none_answer(token) {
            return Err("Use `none` by itself to clear this section.".into());
        }
        if let Some((start, end)) = numeric_range(token)? {
            if start == 0 || start > end || end > options.len() {
                return Err(
                    "A numeric range must refer to listed options in ascending order.".into(),
                );
            }
            for index in start..=end {
                let id = &options[index - 1].id;
                if !selected.ids.contains(id) {
                    selected.ids.push(id.clone());
                }
            }
            continue;
        }
        if let Some(option) = resolve_onboarding_option(token, options) {
            if !selected.ids.contains(&option.id) {
                selected.ids.push(option.id.clone());
            }
            continue;
        }
        if token.parse::<usize>().is_ok() {
            return Err("A numeric choice must refer to one of the listed options.".into());
        }
        if token.chars().count() > custom_max_length || token.chars().any(char::is_control) {
            return Err(format!(
                "Custom entries must be at most {custom_max_length} characters."
            ));
        }
        if !selected.custom.iter().any(|value| value == token) {
            selected.custom.push(token.to_owned());
        }
    }
    if selected.ids.is_empty() && selected.custom.is_empty() {
        return Err("Choose at least one option, or type `none`.".into());
    }
    if selected.custom.len() > custom_maximum {
        return Err(format!("Enter at most {custom_maximum} custom selections."));
    }
    Ok(selected)
}

fn numeric_range(token: &str) -> Result<Option<(usize, usize)>, String> {
    let Some((start, end)) = token.split_once('-') else {
        return Ok(None);
    };
    if start.trim().chars().all(|value| value.is_ascii_digit())
        && end.trim().chars().all(|value| value.is_ascii_digit())
    {
        let start = start
            .trim()
            .parse()
            .map_err(|_| "The numeric range is too large.".to_owned())?;
        let end = end
            .trim()
            .parse()
            .map_err(|_| "The numeric range is too large.".to_owned())?;
        Ok(Some((start, end)))
    } else {
        Ok(None)
    }
}

fn parse_single_option(
    answer: &str,
    options: &[OnboardingOption],
) -> Result<Option<String>, String> {
    if is_none_answer(answer) {
        return Ok(None);
    }
    if answer.contains(',') {
        return Err("Choose one activity level, or type `none`.".into());
    }
    resolve_onboarding_option(answer.trim(), options)
        .map(|option| Some(option.id.clone()))
        .ok_or_else(|| "Choose an activity by number, exact label, or canonical ID.".into())
}

fn resolve_onboarding_option<'a>(
    token: &str,
    options: &'a [OnboardingOption],
) -> Option<&'a OnboardingOption> {
    if let Ok(number) = token.parse::<usize>() {
        return number.checked_sub(1).and_then(|index| options.get(index));
    }
    let normalized = normalize_choice(token);
    options.iter().find(|option| {
        normalize_choice(&option.id) == normalized || normalize_choice(&option.label) == normalized
    })
}

fn normalize_choice(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn parse_free_text_list(
    answer: &str,
    maximum: usize,
    max_length: usize,
) -> Result<Vec<String>, String> {
    if is_none_answer(answer) {
        return Ok(Vec::new());
    }
    let mut values = Vec::new();
    for value in answer
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if value.chars().count() > max_length || value.chars().any(char::is_control) {
            return Err(format!(
                "Each entry must be at most {max_length} characters."
            ));
        }
        if !values.iter().any(|current| current == value) {
            values.push(value.to_owned());
        }
    }
    if values.is_empty() {
        return Err("Enter comma-separated ingredients, or type `none`.".into());
    }
    if values.len() > maximum {
        return Err(format!("Enter at most {maximum} ingredients."));
    }
    Ok(values)
}

fn parse_optional_text(answer: &str, maximum: usize) -> Result<Option<String>, String> {
    if is_none_answer(answer) {
        return Ok(None);
    }
    if answer.chars().count() > maximum || answer.chars().any(char::is_control) {
        return Err(format!("Notes must be at most {maximum} characters."));
    }
    Ok(Some(answer.to_owned()))
}

fn is_none_answer(answer: &str) -> bool {
    matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "none" | "0" | "skip"
    )
}

fn push_onboarding_prompt(model: &mut AppModel) {
    let Some(flow) = model.onboarding.as_ref() else {
        return;
    };
    let prompt = onboarding_prompt(flow);
    push_notice(model, &prompt);
}

fn onboarding_prompt(flow: &OnboardingFlow) -> String {
    match flow.step {
        OnboardingStep::Diets => option_prompt(
            "Diet styles · 1/8",
            "Choose any that apply by number, range, ID, label, or custom text. Separate choices with commas; type `none` for no restrictions.",
            diet_options(),
        ),
        OnboardingStep::Allergies => option_prompt(
            "Allergies & restrictions · 2/8",
            "Choose every option that must be avoided by number, range, ID, label, or custom text; type `none` if there are none.",
            allergy_options(),
        ),
        OnboardingStep::Conditions => option_prompt(
            "Health conditions · 3/8",
            "Choose conditions by number, range, ID, label, or custom text; type `none` if there are none.",
            condition_options(),
        ),
        OnboardingStep::Severity => {
            "Condition severity · 4/8\nChoose a shared severity from 1 (mild) to 5 (critical).".into()
        }
        OnboardingStep::AvoidIngredients => "Ingredients to avoid · 5/8\nEnter up to 20 ingredients separated by commas, or type `none`.".into(),
        OnboardingStep::Activity => option_prompt(
            "Activity level · 6/8",
            "Choose one option by number, ID, or label; type `none` to leave it unset.",
            activity_options(),
        ),
        OnboardingStep::Cuisines => option_prompt(
            "Cuisines you love · 7/8",
            "Choose favorites by number, range, ID, label, or custom text; type `none` to skip.",
            cuisine_options(),
        ),
        OnboardingStep::Notes => "Additional notes · 8/8\nAdd anything else the food guide should know (280 characters maximum), or type `none`.".into(),
        OnboardingStep::Review => onboarding_review(&flow.profile),
        OnboardingStep::Saving => "Saving your dietary profile…".into(),
    }
}

fn option_prompt(title: &str, instructions: &str, options: &[OnboardingOption]) -> String {
    let mut output = format!("{title}\n{instructions}\n\n");
    for (index, option) in options.iter().enumerate() {
        let _ = writeln!(output, "{:>2}. {}", index + 1, option.label);
    }
    output
        .push_str("\nType `back` to revisit the previous step or `cancel` to discard onboarding.");
    output
}

fn onboarding_review(profile: &OnboardingProfileInput) -> String {
    format!(
        "Review dietary profile\n\nDiet styles: {}\nAllergies: {}\nHealth conditions: {}\nCondition severity: {}\nAvoid ingredients: {}\nActivity: {}\nCuisines: {}\nNotes: {}\n\nNo profile data has been sent yet. Type `save` to grant profile-sync consent and replace the synced profile, `back` to edit, or `cancel` to discard it.",
        labels_and_custom(
            &profile.diet_style_ids,
            &profile.custom_diet_styles,
            diet_options()
        ),
        labels_and_custom(
            &profile.allergy_ids,
            &profile.custom_restrictions,
            allergy_options()
        ),
        labels_and_custom(
            &profile.health_condition_ids,
            &profile.custom_health_conditions,
            condition_options()
        ),
        profile
            .severity_level
            .map_or_else(|| "None".into(), |value| value.to_string()),
        display_values(&profile.avoid_ingredients),
        profile.activity_level.as_deref().map_or_else(
            || "None".into(),
            |value| labels_for(&[value.to_owned()], activity_options())
        ),
        labels_and_custom(
            &profile.cuisine_preferences,
            &profile.custom_cuisines,
            cuisine_options()
        ),
        profile.notes.clone().unwrap_or_else(|| "None".into()),
    )
}

fn labels_for(values: &[String], options: &[OnboardingOption]) -> String {
    if values.is_empty() {
        return "None".into();
    }
    values
        .iter()
        .map(|value| {
            options
                .iter()
                .find(|option| option.id == *value)
                .map_or(value.as_str(), |option| option.label.as_str())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn labels_and_custom(values: &[String], custom: &[String], options: &[OnboardingOption]) -> String {
    let canonical = labels_for(values, options);
    match (canonical.as_str(), custom.is_empty()) {
        ("None", true) => canonical,
        ("None", false) => custom.join(", "),
        (_, true) => canonical,
        (_, false) => format!("{canonical}, {}", custom.join(", ")),
    }
}

fn display_values(values: &[String]) -> String {
    if values.is_empty() {
        "None".into()
    } else {
        values.join(", ")
    }
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
        SlashCommandKind::Status
        | SlashCommandKind::Grocery
        | SlashCommandKind::Health
        | SlashCommandKind::Household
        | SlashCommandKind::Profile
        | SlashCommandKind::Onboard
        | SlashCommandKind::Location
            if !arguments.is_empty() =>
        {
            push_notice(model, &format!("Usage: {}", spec.usage));
        }
        SlashCommandKind::Status => return open_panel(model, PanelRequest::Status),
        SlashCommandKind::Grocery => return open_panel(model, PanelRequest::Grocery),
        SlashCommandKind::Health => return open_panel(model, PanelRequest::Health),
        SlashCommandKind::Household => return open_panel(model, PanelRequest::Household),
        SlashCommandKind::For if arguments.is_empty() => {
            push_notice(model, &format!("Usage: {}", spec.usage));
        }
        SlashCommandKind::For => return select_household(model, arguments),
        SlashCommandKind::Profile => return open_panel(model, PanelRequest::Profile),
        SlashCommandKind::Onboard if model.operation.is_active() => push_notice(
            model,
            "Finish or stop the active work before starting dietary onboarding.",
        ),
        SlashCommandKind::Onboard => begin_onboarding(
            model,
            "Dietary onboarding replaces your synced profile only after you review and save it.",
        ),
        SlashCommandKind::Location => return open_panel(model, PanelRequest::Location),
        SlashCommandKind::Exit => return begin_exit(model, ExitReason::Requested),
    }
    Vec::new()
}

fn select_household(model: &mut AppModel, selector: &str) -> Vec<Effect> {
    if model.operation.is_active() {
        push_notice(
            model,
            "Finish or stop the active work before changing the household target.",
        );
        return Vec::new();
    }
    let operation_id = model.next_operation_id;
    model.next_operation_id = model.next_operation_id.saturating_add(1);
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::User,
        text: format!("/for {selector}"),
        streaming: false,
    });
    model.scrollback.push(SemanticEntry {
        speaker: Speaker::Assistant,
        text: String::new(),
        streaming: true,
    });
    model.operation = OperationState::Running(operation_id);
    model.activity = Some("Changing household target…".into());
    model.idle_exit_armed = false;
    follow_tail(model);
    vec![Effect::SelectHousehold {
        operation_id,
        selector: selector.to_owned(),
    }]
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
    if model.pending_confirmation.is_some() && !model.operation.is_active() {
        return submit_confirmation(model, ConfirmationDecisionWire::Cancel, None);
    }
    if !model.draft.is_empty() {
        model.draft.clear();
        model.cursor = 0;
        model.idle_exit_armed = false;
        return Vec::new();
    }
    if model.onboarding.is_some() && !model.operation.is_active() {
        model.onboarding = None;
        model.idle_exit_armed = false;
        model.activity = None;
        push_notice(
            model,
            "Dietary onboarding cancelled. Nothing was sent or saved.",
        );
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
        RuntimeEvent::BeginOnboarding { message }
            if model.operation == OperationState::Idle && model.onboarding.is_none() =>
        {
            begin_onboarding(model, &terminal_safe_text(&message));
        }
        RuntimeEvent::OnboardingSaved { operation_id }
            if model.operation.operation_id() == Some(operation_id) =>
        {
            finish_onboarding(model, Ok(()));
        }
        RuntimeEvent::OnboardingFailed {
            operation_id,
            message,
        } if model.operation.operation_id() == Some(operation_id) => {
            finish_onboarding(model, Err(message));
        }
        RuntimeEvent::OnboardingCancelled {
            operation_id,
            outcome,
        } if model.operation.operation_id() == Some(operation_id) => {
            model.onboarding = None;
            finish_onboarding_cancel(model, outcome);
        }
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
        RuntimeEvent::HouseholdScopeReady {
            operation_id,
            label,
        } if model.operation.operation_id() == Some(operation_id) => {
            finish_household_scope(model, Ok(label));
        }
        RuntimeEvent::HouseholdScopeFailed {
            operation_id,
            message,
        } if model.operation.operation_id() == Some(operation_id) => {
            finish_household_scope(model, Err(message));
        }
        RuntimeEvent::BeginOnboarding { .. }
        | RuntimeEvent::OnboardingSaved { .. }
        | RuntimeEvent::OnboardingFailed { .. }
        | RuntimeEvent::OnboardingCancelled { .. }
        | RuntimeEvent::TurnEvent { .. }
        | RuntimeEvent::TurnFinished { .. }
        | RuntimeEvent::TurnFailed { .. }
        | RuntimeEvent::PanelReady { .. }
        | RuntimeEvent::PanelFailed { .. }
        | RuntimeEvent::HouseholdScopeReady { .. }
        | RuntimeEvent::HouseholdScopeFailed { .. } => {}
    }
    Vec::new()
}

fn finish_onboarding(model: &mut AppModel, result: Result<(), String>) {
    let old_lines = model.scrollback.rendered_lines();
    match result {
        Ok(()) => {
            model.scrollback.mutate_last_assistant(|entry| {
                entry.text = "Dietary profile saved\n\nYour hello.food guidance now uses this synced profile across supported experiences.".into();
                entry.streaming = false;
            });
            model.onboarding = None;
        }
        Err(message) => {
            if let Some(flow) = model.onboarding.as_mut() {
                flow.step = OnboardingStep::Review;
            }
            let review = model
                .onboarding
                .as_ref()
                .map(|flow| onboarding_review(&flow.profile))
                .unwrap_or_default();
            model.scrollback.mutate_last_assistant(|entry| {
                entry.text = format!(
                    "Dietary profile was not saved: {}\n\n{}",
                    terminal_safe_text(&message),
                    review
                );
                entry.streaming = false;
            });
        }
    }
    model.operation = OperationState::Idle;
    model.activity = None;
    model.idle_exit_armed = false;
    account_for_new_lines(model, old_lines);
}

fn finish_onboarding_cancel(model: &mut AppModel, outcome: RunTurnOutcome) {
    let old_lines = model.scrollback.rendered_lines();
    model.scrollback.mutate_last_assistant(|entry| {
        entry.text = match outcome {
            RunTurnOutcome::CancelledAfterDispatchOutcomeUnknown => "Dietary profile save stopped after dispatch, and the server outcome is unknown. Open `/profile` to inspect current state before starting onboarding again.".into(),
            RunTurnOutcome::CancelledBeforeServerAcceptance
            | RunTurnOutcome::CancelledAfterServerAcceptance
            | RunTurnOutcome::StaleGeneration
            | RunTurnOutcome::Completed => "Dietary profile save cancelled. The profile upload was not dispatched; profile-sync consent may already have been granted.".into(),
        };
        entry.streaming = false;
    });
    model.operation = OperationState::Idle;
    model.activity = None;
    model.idle_exit_armed = false;
    account_for_new_lines(model, old_lines);
}

fn finish_household_scope(model: &mut AppModel, result: Result<String, String>) {
    let old_lines = model.scrollback.rendered_lines();
    model.scrollback.mutate_last_assistant(|entry| {
        entry.text = match result {
            Ok(label) => format!(
                "Household target\n\nFuture turns will consider {}.",
                terminal_safe_text(&label)
            ),
            Err(message) => format!(
                "Unable to change the household target: {}",
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
            model.pending_choice_labels = choices
                .iter()
                .map(|choice| terminal_safe_text(&choice.label))
                .collect();
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
            let confirmation = ActionConfirmationEnvelopeWire::from_result_document(&document);
            let result = agent_result_text(&document).map(terminal_safe_text);
            let choice_labels = std::mem::take(&mut model.pending_choice_labels);
            model.scrollback.mutate_last_assistant(|entry| {
                match confirmation.as_ref() {
                    Ok(Some(envelope)) => {
                        entry.text = render_action_confirmation(envelope);
                    }
                    Err(message) => {
                        entry.text = format!(
                            "Unable to present this confirmation safely: {}",
                            terminal_safe_text(message)
                        );
                    }
                    Ok(None) => {
                        if let Some(result) = result {
                            if !result.is_empty() {
                                entry.text = result;
                                append_choice_labels(&mut entry.text, &choice_labels);
                            }
                        } else if entry.text.is_empty() {
                            entry.text = terminal_safe_text(&document.to_string());
                        }
                    }
                }
                entry.streaming = false;
            });
            match confirmation {
                Ok(Some(envelope)) => {
                    let editable_items = editable_grocery_items(&envelope);
                    model.pending_confirmation = Some(PendingActionConfirmation {
                        confirmation_id: envelope.confirmation_id,
                        idempotency_key: envelope.idempotency_key,
                        editable_items,
                    });
                }
                Ok(None) | Err(_) => model.pending_confirmation = None,
            }
            mark_finishing(model);
            model.activity = Some("Finishing…".into());
            model.idle_exit_armed = false;
        }
        AgentEvent::Error { error } => {
            model.pending_choice_labels.clear();
            if !confirmation_error_preserves_pending(&error.code) {
                model.pending_confirmation = None;
            }
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

fn confirmation_error_preserves_pending(code: &str) -> bool {
    matches!(code, "edit_invalid" | "temporarily_unavailable")
}

fn render_action_confirmation(envelope: &ActionConfirmationEnvelopeWire) -> String {
    let mut output = format!(
        "Review before changing anything\n\n{}\n",
        terminal_safe_text(&envelope.preview)
    );
    if let Some(items) = envelope
        .structured_preview
        .as_ref()
        .and_then(|preview| preview.get("items"))
        .and_then(serde_json::Value::as_array)
    {
        for (index, item) in items.iter().enumerate() {
            let name = ["name", "requested_name", "canonical_name"]
                .into_iter()
                .find_map(|key| item.get(key).and_then(serde_json::Value::as_str))
                .map(terminal_safe_text)
                .unwrap_or_else(|| "item".into());
            let intended_for = item.get("intended_for").and_then(serde_json::Value::as_str);
            let intended = intended_for
                .map(terminal_safe_text)
                .map(|member| format!(" for {member}"))
                .unwrap_or_default();
            let quantity = item.get("quantity").and_then(|value| {
                value
                    .as_str()
                    .map(terminal_safe_text)
                    .or_else(|| value.as_f64().map(|value| value.to_string()))
            });
            let unit = item
                .get("unit")
                .and_then(serde_json::Value::as_str)
                .map(terminal_safe_text);
            let amount = match (quantity, unit) {
                (Some(quantity), Some(unit)) => format!(" · {quantity} {unit}"),
                (Some(quantity), None) => format!(" · {quantity}"),
                _ => String::new(),
            };
            let _ = writeln!(output, "{}. {name}{intended}{amount}", index + 1);
            if let Some(provenance) = item.get("provenance").and_then(serde_json::Value::as_str) {
                let _ = writeln!(output, "   source: {}", terminal_safe_text(provenance));
            }
            render_confirmation_safety(&mut output, item, intended_for);
        }
    }
    if let Some(expires_at) = envelope.expires_at.as_deref() {
        let _ = writeln!(output, "\nExpires: {}", terminal_safe_text(expires_at));
    }
    output.push_str(
        "\nNothing has changed yet. Type `y` to confirm or `n` to cancel. Ctrl+C cancels.",
    );
    if editable_grocery_items(envelope).is_some() {
        output.push_str(
            "\nTo replace one item name and confirm the correction, type `edit #N <replacement>`.",
        );
    }
    output
}

fn editable_grocery_items(
    envelope: &ActionConfirmationEnvelopeWire,
) -> Option<Vec<serde_json::Map<String, serde_json::Value>>> {
    if !matches!(
        envelope.action.as_str(),
        "grocery_list_add_items" | "add_items"
    ) {
        return None;
    }
    let items = envelope
        .structured_preview
        .as_ref()?
        .get("items")?
        .as_array()?;
    if items.is_empty() || items.len() > 25 {
        return None;
    }
    items.iter().map(editable_grocery_item).collect()
}

fn editable_grocery_item(
    item: &serde_json::Value,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let name = ["name", "requested_name"]
        .into_iter()
        .find_map(|key| item.get(key).and_then(serde_json::Value::as_str))
        .and_then(|value| required_text(value, 255).ok())?;
    let mut editable = serde_json::Map::new();
    editable.insert("name".into(), serde_json::Value::String(name));

    if let Some(quantity) = item.get("quantity").and_then(serde_json::Value::as_f64)
        && quantity.is_finite()
        && quantity >= 0.0
        && let Some(quantity) = serde_json::Number::from_f64(quantity)
    {
        editable.insert("quantity".into(), serde_json::Value::Number(quantity));
    }
    if let Some(package_quantity) = item
        .get("package_quantity")
        .and_then(serde_json::Value::as_i64)
        .filter(|value| *value >= 0)
    {
        editable.insert(
            "package_quantity".into(),
            serde_json::Value::Number(package_quantity.into()),
        );
    }
    for (field, maximum) in [("unit", 40), ("note", 255), ("intended_for", 64)] {
        if let Some(value) = item
            .get(field)
            .and_then(serde_json::Value::as_str)
            .and_then(|value| required_text(value, maximum).ok())
        {
            editable.insert(field.into(), serde_json::Value::String(value));
        }
    }
    editable.insert(
        "source_type".into(),
        serde_json::Value::String("manual".into()),
    );
    Some(editable)
}

fn render_confirmation_safety(
    output: &mut String,
    item: &serde_json::Value,
    intended_for: Option<&str>,
) {
    // The generic C3 v1 item card placed flags at `item.safety_flags`.
    // Grocery Phase A's frozen production fixture specializes that shape as
    // `item.safety.{status,member_flags,label_hint}`. Prefer the production
    // Grocery shape while retaining the additive generic-C3 compatibility.
    let nested_safety = item.get("safety");
    if let Some(status) = nested_safety
        .and_then(|safety| safety.get("status"))
        .and_then(serde_json::Value::as_str)
    {
        let status = terminal_safe_text(status).replace('_', " ");
        let _ = writeln!(output, "   ingredient screening: {status}");
    }
    let flags = nested_safety
        .and_then(|safety| safety.get("member_flags"))
        .and_then(serde_json::Value::as_array)
        .or_else(|| {
            item.get("safety_flags")
                .and_then(serde_json::Value::as_array)
        });
    if let Some(flags) = flags {
        for flag in flags {
            let member_id = flag.get("member_id").and_then(serde_json::Value::as_str);
            let member = member_id
                .map(terminal_safe_text)
                .unwrap_or_else(|| "member".into());
            let status = flag
                .get("status")
                .and_then(serde_json::Value::as_str)
                .map(terminal_safe_text)
                .map(|value| value.replace('_', " "))
                .unwrap_or_else(|| "unable to evaluate".into());
            let intended = member_id
                .filter(|member| Some(*member) == intended_for)
                .map_or("", |_| " · intended");
            let _ = writeln!(output, "   • {member}: {status}{intended}");
            if let Some(reason) = flag.get("reason").and_then(serde_json::Value::as_str) {
                let _ = writeln!(output, "     {}", terminal_safe_text(reason));
            }
            if let Some(substitutions) = flag
                .get("substitutions")
                .and_then(serde_json::Value::as_array)
            {
                let substitutions = substitutions
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(terminal_safe_text)
                    .collect::<Vec<_>>()
                    .join(", ");
                if !substitutions.is_empty() {
                    let _ = writeln!(output, "     try: {substitutions}");
                }
            }
        }
    }
    if let Some(label_hint) = nested_safety
        .and_then(|safety| safety.get("label_hint"))
        .and_then(serde_json::Value::as_str)
    {
        let _ = writeln!(output, "   {}", terminal_safe_text(label_hint));
    }
}

fn append_choice_labels(output: &mut String, choices: &[String]) {
    if choices.is_empty() {
        return;
    }
    if !output.is_empty() {
        output.push_str("\n\n");
    }
    output.push_str("Options\n");
    for choice in choices {
        output.push_str("• ");
        output.push_str(choice);
        output.push('\n');
    }
    output.pop();
}

fn mark_finishing(model: &mut AppModel) {
    if let Some(operation_id) = model.operation.operation_id() {
        model.operation = OperationState::Finishing(operation_id);
    }
}

fn finish_stream(model: &mut AppModel, outcome: RunTurnOutcome) {
    model.pending_choice_labels.clear();
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

    fn submit_text(model: &mut AppModel, value: &str) -> Vec<Effect> {
        model.draft = value.into();
        model.cursor = value.chars().count();
        dispatch(model, Action::Submit)
    }

    fn advance_to_onboarding_review(model: &mut AppModel) {
        assert!(submit_text(model, "1, vegan").is_empty());
        assert!(submit_text(model, "none").is_empty());
        assert!(submit_text(model, "celiac").is_empty());
        assert!(submit_text(model, "5").is_empty());
        assert!(submit_text(model, "raw onion").is_empty());
        assert!(submit_text(model, "2").is_empty());
        assert!(submit_text(model, "Mexican, 2").is_empty());
        assert!(submit_text(model, "none").is_empty());
        assert_eq!(
            model.onboarding.as_ref().map(|flow| flow.step),
            Some(OnboardingStep::Review)
        );
    }

    #[test]
    fn onboarding_is_local_until_explicit_review_and_save() {
        let mut model = AppModel::default();
        assert!(
            dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::BeginOnboarding {
                    message: "Complete your dietary profile.".into(),
                })
            )
            .is_empty()
        );
        advance_to_onboarding_review(&mut model);

        let effects = submit_text(&mut model, "save");
        assert_eq!(effects.len(), 1);
        let Effect::SaveOnboarding {
            operation_id,
            profile,
        } = &effects[0]
        else {
            panic!("expected an onboarding save effect");
        };
        assert_eq!(*operation_id, 1);
        assert_eq!(profile.diet_style_ids, ["gluten_free", "vegan"]);
        assert_eq!(profile.health_condition_ids, ["celiac"]);
        assert_eq!(profile.severity_level, Some(5));
        assert_eq!(profile.avoid_ingredients, ["raw onion"]);
        assert_eq!(profile.activity_level.as_deref(), Some("moderate"));
        assert_eq!(profile.cuisine_preferences, ["mexican", "italian"]);
        assert_eq!(model.operation, OperationState::Running(1));
    }

    #[test]
    fn onboarding_cancel_discards_local_answers_without_a_mutation_effect() {
        let mut model = AppModel::default();
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::BeginOnboarding {
                message: "Complete your dietary profile.".into(),
            }),
        );
        assert!(submit_text(&mut model, "vegan").is_empty());
        assert!(submit_text(&mut model, "cancel").is_empty());
        assert!(model.onboarding.is_none());
        assert_eq!(model.operation, OperationState::Idle);
        assert!(
            model
                .scrollback
                .entries()
                .back()
                .is_some_and(|entry| entry.text.contains("Nothing was sent or saved"))
        );
    }

    #[test]
    fn failed_onboarding_save_returns_to_the_review_for_an_explicit_retry() {
        let mut model = AppModel::default();
        begin_onboarding(&mut model, "Complete your dietary profile.");
        advance_to_onboarding_review(&mut model);
        let _ = submit_text(&mut model, "save");
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::OnboardingFailed {
                operation_id: 1,
                message: "profile version changed".into(),
            }),
        );
        assert_eq!(model.operation, OperationState::Idle);
        assert_eq!(
            model.onboarding.as_ref().map(|flow| flow.step),
            Some(OnboardingStep::Review)
        );
        assert!(
            model
                .scrollback
                .entries()
                .back()
                .unwrap()
                .text
                .contains("profile version changed")
        );
        assert!(matches!(
            submit_text(&mut model, "save").as_slice(),
            [Effect::SaveOnboarding {
                operation_id: 2,
                ..
            }]
        ));
    }

    #[test]
    fn invalid_onboarding_selection_stays_on_the_same_step() {
        let mut model = AppModel::default();
        begin_onboarding(&mut model, "Complete your dietary profile.");
        assert!(submit_text(&mut model, "99").is_empty());
        assert_eq!(
            model.onboarding.as_ref().map(|flow| flow.step),
            Some(OnboardingStep::Diets)
        );
        assert!(
            model
                .scrollback
                .entries()
                .iter()
                .rev()
                .any(|entry| entry.text.contains("numeric choice"))
        );
    }

    #[test]
    fn onboarding_accepts_numeric_ranges_and_bounded_custom_entries() {
        let selected = parse_multi_options("1-3, family recipe diet", diet_options(), 10, 40)
            .expect("valid range and custom diet");
        assert_eq!(selected.ids, ["gluten_free", "dairy_free", "vegetarian"]);
        assert_eq!(selected.custom, ["family recipe diet"]);
    }

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
    fn terminal_result_preserves_choices_after_partial_content() {
        for field in ["message", "text", "response"] {
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
                        text: "Review the available paths.".into(),
                    },
                }),
            );
            let _ = dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::TurnEvent {
                    operation_id: 1,
                    event: AgentEvent::Choices {
                        choices: vec![
                            heyfood_core::AgentChoice::from_untrusted(
                                "Cook at home".into(),
                                Some("cook".into()),
                            )
                            .unwrap(),
                            heyfood_core::AgentChoice::from_untrusted(
                                "Eat out".into(),
                                Some("restaurant".into()),
                            )
                            .unwrap(),
                        ],
                        allow_multiple: false,
                    },
                }),
            );
            let mut document = serde_json::json!({});
            document[field] = serde_json::Value::String("Which path works for you?".into());
            let _ = dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::TurnEvent {
                    operation_id: 1,
                    event: AgentEvent::Result {
                        document,
                        conversation_id: Some("conversation-choices".into()),
                    },
                }),
            );
            let entry = model.scrollback.entries().back().unwrap();
            assert_eq!(
                entry.text,
                "Which path works for you?\n\nOptions\n• Cook at home\n• Eat out"
            );
            assert!(!entry.streaming);
        }
    }

    #[test]
    fn production_grocery_confirmation_renders_safety_and_requires_typed_accept_or_cancel() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/contracts/grocery-backend/phase-a/fixtures/grocery/confirmation_round_trip.json"
        ))
        .unwrap();
        let mut structured = fixture["card"].clone();
        let structured = structured.as_object_mut().unwrap();
        structured.insert(
            "confirmation_id".into(),
            fixture["accept_payload"]["confirmation_id"].clone(),
        );
        structured.insert(
            "idempotency_key".into(),
            fixture["accept_payload"]["idempotency_key"].clone(),
        );
        structured.insert(
            "preview".into(),
            serde_json::json!("Add one screened ingredient"),
        );
        structured.insert(
            "expires_at".into(),
            serde_json::json!("2026-07-22T12:05:00Z"),
        );
        let confirmation_document = serde_json::json!({
            "text": "I prepared a grocery update.",
            "structured": structured
        });
        let mut model = AppModel {
            draft: "add ingredients".into(),
            cursor: 15,
            ..AppModel::default()
        };
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Result {
                    document: confirmation_document,
                    conversation_id: Some("conversation-grocery".into()),
                },
            }),
        );
        let card = &model.scrollback.entries().back().unwrap().text;
        assert!(card.contains("Review before changing anything"));
        assert!(card.contains("1. onion · 1"));
        assert!(card.contains("source: manual"));
        assert!(card.contains("ingredient screening: risky"));
        assert!(card.contains("maya-uuid: risky"));
        assert!(card.contains("Onion is high-FODMAP."));
        assert!(card.contains("try: scallion greens"));
        assert!(card.contains("Screened at ingredient level — verify the product label."));
        assert!(card.contains("Type `y` to confirm or `n` to cancel"));
        assert!(card.contains("edit #N <replacement>"));
        assert!(!card.contains("confirmation_id"));
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnFinished {
                operation_id: 1,
                outcome: RunTurnOutcome::Completed,
            }),
        );

        model.draft = "y".into();
        model.cursor = 1;
        let effects = dispatch(&mut model, Action::Submit);
        assert!(matches!(
            effects.as_slice(),
            [Effect::ConfirmAction { operation_id: 2, command }]
                if command.decision == ConfirmationDecisionWire::Accept
                    && command.edits.is_none()
                    && command.confirmation_id.as_uuid().to_string()
                        == "00000000-0000-0000-0000-000000000001"
        ));
    }

    #[test]
    fn grocery_add_item_edit_is_bounded_explicit_and_sent_as_c3_edits() {
        let envelope: ActionConfirmationEnvelopeWire = serde_json::from_value(serde_json::json!({
            "type": "action_confirmation",
            "confirmation_id": "00000000-0000-4000-8000-000000000001",
            "idempotency_key": "00000000-0000-4000-8000-000000000002",
            "action": "grocery_list_add_items",
            "preview": "Add two screened ingredients",
            "card_form": "item_list",
            "structured_preview": {
                "items": [
                    {
                        "requested_name": "onion",
                        "quantity": 1,
                        "unit": "each",
                        "intended_for": "maya",
                        "safety": {"status": "risky"}
                    },
                    {
                        "name": "milk",
                        "quantity": 2,
                        "unit": "cartons"
                    }
                ]
            }
        }))
        .unwrap();
        let editable_items = editable_grocery_items(&envelope).unwrap();
        let mut model = AppModel {
            draft: "edit #1 scallion greens".into(),
            cursor: 23,
            pending_confirmation: Some(PendingActionConfirmation {
                confirmation_id: envelope.confirmation_id,
                idempotency_key: envelope.idempotency_key,
                editable_items: Some(editable_items),
            }),
            ..AppModel::default()
        };

        let effects = dispatch(&mut model, Action::Submit);
        let command = match effects.as_slice() {
            [
                Effect::ConfirmAction {
                    operation_id: 1,
                    command,
                },
            ] => command,
            effects => panic!("expected edited confirmation, got {effects:?}"),
        };
        assert_eq!(command.decision, ConfirmationDecisionWire::Accept);
        assert_eq!(
            serde_json::to_value(command.edits.as_ref().unwrap()).unwrap(),
            serde_json::json!({
                "items": [
                    {
                        "name": "scallion greens",
                        "quantity": 1.0,
                        "unit": "each",
                        "intended_for": "maya",
                        "source_type": "manual"
                    },
                    {
                        "name": "milk",
                        "quantity": 2.0,
                        "unit": "cartons",
                        "source_type": "manual"
                    }
                ]
            })
        );
        assert!(model.pending_confirmation.is_some());
        assert!(model.draft.is_empty());
        assert_eq!(
            model.scrollback.entries().iter().rev().nth(1).unwrap().text,
            "Edit and confirm"
        );
    }

    #[test]
    fn generic_c3_safety_flags_and_targeting_remain_visible() {
        let envelope: ActionConfirmationEnvelopeWire = serde_json::from_value(serde_json::json!({
            "type": "action_confirmation",
            "confirmation_id": "00000000-0000-4000-8000-000000000001",
            "idempotency_key": "00000000-0000-4000-8000-000000000002",
            "action": "grocery_list_add_items",
            "preview": "Add a targeted ingredient",
            "card_form": "item_list",
            "structured_preview": {
                "items": [{
                    "name": "tomato",
                    "quantity": 2,
                    "unit": "each",
                    "intended_for": "maya",
                    "provenance": "menu",
                    "safety_flags": [{
                        "member_id": "maya",
                        "status": "avoid",
                        "reason": "Member-specific conflict"
                    }]
                }]
            }
        }))
        .unwrap();
        let card = render_action_confirmation(&envelope);
        assert!(card.contains("1. tomato for maya · 2 each"));
        assert!(card.contains("source: menu"));
        assert!(card.contains("maya: avoid · intended"));
        assert!(card.contains("Member-specific conflict"));
    }

    #[test]
    fn ctrl_c_cancels_a_pending_action_confirmation_through_the_server() {
        let mut model = AppModel {
            draft: "an unsubmitted answer".into(),
            cursor: 21,
            pending_confirmation: Some(PendingActionConfirmation {
                confirmation_id: heyfood_core::GroceryConfirmationId::parse(
                    "00000000-0000-4000-8000-000000000001",
                )
                .unwrap(),
                idempotency_key: heyfood_core::GroceryIdempotencyKey::parse(
                    "00000000-0000-4000-8000-000000000002",
                )
                .unwrap(),
                editable_items: None,
            }),
            ..AppModel::default()
        };
        let effects = dispatch(&mut model, Action::CancelOrExit);
        assert!(matches!(
            effects.as_slice(),
            [Effect::ConfirmAction { operation_id: 1, command }]
                if command.decision == ConfirmationDecisionWire::Cancel
        ));
        assert!(model.draft.is_empty());
        assert!(!model.idle_exit_armed);
    }

    #[test]
    fn confirmation_store_outage_preserves_exact_ids_for_accept_and_cancel_replay() {
        for (answer, decision) in [
            ("y", ConfirmationDecisionWire::Accept),
            ("n", ConfirmationDecisionWire::Cancel),
        ] {
            let mut model = AppModel {
                draft: answer.into(),
                cursor: 1,
                pending_confirmation: Some(PendingActionConfirmation {
                    confirmation_id: heyfood_core::GroceryConfirmationId::parse(
                        "00000000-0000-4000-8000-000000000001",
                    )
                    .unwrap(),
                    idempotency_key: heyfood_core::GroceryIdempotencyKey::parse(
                        "00000000-0000-4000-8000-000000000002",
                    )
                    .unwrap(),
                    editable_items: None,
                }),
                ..AppModel::default()
            };
            let first = dispatch(&mut model, Action::Submit);
            let first_command = match first.as_slice() {
                [Effect::ConfirmAction { command, .. }] => command.clone(),
                effects => panic!("expected confirmation effect, got {effects:?}"),
            };
            assert_eq!(first_command.decision, decision);

            let _ = dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::TurnEvent {
                    operation_id: 1,
                    event: AgentEvent::Error {
                        error: AgentFailure {
                            code: "temporarily_unavailable".into(),
                            message: "confirmation store unavailable".into(),
                            retryable: true,
                        },
                    },
                }),
            );
            let _ = dispatch(
                &mut model,
                Action::Runtime(RuntimeEvent::TurnFinished {
                    operation_id: 1,
                    outcome: RunTurnOutcome::Completed,
                }),
            );

            model.draft = answer.into();
            model.cursor = 1;
            let replay = dispatch(&mut model, Action::Submit);
            assert!(matches!(
                replay.as_slice(),
                [Effect::ConfirmAction { operation_id: 2, command }]
                    if command == &first_command
            ));
        }
    }

    #[test]
    fn edit_invalid_keeps_pending_confirmation_authority() {
        let pending = PendingActionConfirmation {
            confirmation_id: heyfood_core::GroceryConfirmationId::parse(
                "00000000-0000-4000-8000-000000000001",
            )
            .unwrap(),
            idempotency_key: heyfood_core::GroceryIdempotencyKey::parse(
                "00000000-0000-4000-8000-000000000002",
            )
            .unwrap(),
            editable_items: None,
        };
        let mut model = AppModel {
            operation: OperationState::Running(1),
            pending_confirmation: Some(pending.clone()),
            ..AppModel::default()
        };
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Error {
                    error: AgentFailure {
                        code: "edit_invalid".into(),
                        message: "invalid edit".into(),
                        retryable: false,
                    },
                },
            }),
        );
        assert_eq!(model.pending_confirmation, Some(pending));
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
    fn incomplete_voice_command_is_not_advertised() {
        assert!(resolve_slash_command("/voice").is_none());
    }

    #[test]
    fn household_target_dispatches_and_reports_the_resolved_scope() {
        let mut model = AppModel {
            draft: "/for Sarah".into(),
            cursor: 10,
            ..AppModel::default()
        };
        assert_eq!(
            dispatch(&mut model, Action::Submit),
            vec![Effect::SelectHousehold {
                operation_id: 1,
                selector: "Sarah".into(),
            }]
        );
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::HouseholdScopeReady {
                operation_id: 1,
                label: "Sarah".into(),
            }),
        );
        assert_eq!(model.operation, OperationState::Idle);
        assert_eq!(
            model.scrollback.entries().back().unwrap().text,
            "Household target\n\nFuture turns will consider Sarah."
        );
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
            ("/status", PanelRequest::Status),
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
