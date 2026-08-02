use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use heyfood_core::{
    HouseholdLifecycleV1, HouseholdProfileStateV1, HouseholdScope, HouseholdSubjectId,
    RelationshipV1, terminal_safe_text,
};

use crate::model::{OnboardingChoicePanel, OnboardingSelectionMode};
use crate::{
    AppModel, HouseholdMemberPresentationV1, OperationState, ProfileCopyStateV1, Speaker,
    slash_suggestions,
};

fn semantic_style(model: &AppModel, color: Color) -> Style {
    if model.color_enabled() {
        Style::default().fg(color)
    } else {
        Style::default()
    }
}

#[must_use]
pub fn profile_copy(state: ProfileCopyStateV1) -> String {
    match state {
        ProfileCopyStateV1::OnboardingSaveReview => "Save this profile on this device? Saving does not grant profile-sync consent. If you already granted consent, sync may continue after the local save.".into(),
        ProfileCopyStateV1::OnboardingSaveCancelled => {
            "Profile not saved. Profile-sync consent was not changed.".into()
        }
        ProfileCopyStateV1::SavedWithAbsentConsent => "Saved on this device. Sync is off because profile-sync consent is not granted. Run /profile consent to review consent.".into(),
        ProfileCopyStateV1::ConsentReview => "Grant profile-sync consent for Me? This allows hello.food to store and update your owner dietary profile in your account. It does not sync household members, and granting consent does not upload a profile.".into(),
        ProfileCopyStateV1::ConsentReviewPrompt => {
            "Press y to grant consent; n or Esc to cancel.".into()
        }
        ProfileCopyStateV1::ConsentCancelled => {
            "Profile-sync consent was not changed. Nothing was uploaded.".into()
        }
        ProfileCopyStateV1::ConsentGranted { consent_version } => format!(
            "Profile-sync consent active for Me (version {}). No profile was uploaded.",
            consent_version.get()
        ),
        ProfileCopyStateV1::RetryOffered { consent_version } => format!(
            "Consent version {} is active. Your owner profile is still saved only on this device. Run /profile retry-sync to retry.",
            consent_version.get()
        ),
        ProfileCopyStateV1::InterruptedRetry => {
            "Sync was interrupted. Run /profile retry-sync to resume the exact saved owner profile."
                .into()
        }
        ProfileCopyStateV1::ConsentVersionChanged => {
            "Profile-sync consent changed. Review and save your owner profile again before syncing."
                .into()
        }
        ProfileCopyStateV1::ConsentRevoked => "Profile-sync consent is no longer active. Run /profile consent, then review and save your owner profile again.".into(),
        ProfileCopyStateV1::SyncPending => "Saved on this device; sync pending.".into(),
        ProfileCopyStateV1::RetryUnavailable => {
            "Owner profile sync retry is unavailable.".into()
        }
    }
}

#[must_use]
pub fn household_panel_copy(
    members: &[HouseholdMemberPresentationV1],
    active_scope: &HouseholdScope,
    management_enabled: bool,
) -> String {
    let mut output = String::from("Household\n\nActive\n");
    for member in members
        .iter()
        .filter(|member| member.lifecycle() == HouseholdLifecycleV1::Active)
    {
        let label = match member.subject() {
            HouseholdSubjectId::Self_ => "Me",
            HouseholdSubjectId::Member(_) => member.display_label(),
        };
        let current = match active_scope {
            HouseholdScope::Subject(subject) if subject == member.subject() => " · current",
            HouseholdScope::Subject(_) | HouseholdScope::Everyone => "",
        };
        output.push_str(&format!(
            "• {} · {} · {}{}\n",
            terminal_safe_text(label),
            relationship_copy(member.relationship()),
            profile_readiness_copy(member.profile_readiness()),
            current
        ));
    }
    let archived = members
        .iter()
        .filter(|member| member.lifecycle() == HouseholdLifecycleV1::Archived)
        .collect::<Vec<_>>();
    if !archived.is_empty() {
        output.push_str("\nArchived (not selectable)\n");
        for member in archived {
            output.push_str(&format!(
                "• {} · {} · {}\n",
                terminal_safe_text(member.display_label()),
                relationship_copy(member.relationship()),
                profile_readiness_copy(member.profile_readiness())
            ));
        }
    }
    if matches!(active_scope, HouseholdScope::Everyone) {
        output.push_str("\nCurrent target: Everyone\n");
    }
    if management_enabled {
        output.push_str("\nAdd a household member: /household add\n");
    }
    output.push_str(
        "\nEdit, archive, restore, and permanent member erasure are not yet available in the native TUI.",
    );
    output
}

fn relationship_copy(relationship: RelationshipV1) -> &'static str {
    match relationship {
        RelationshipV1::Self_ => "self",
        RelationshipV1::Spouse => "spouse",
        RelationshipV1::Partner => "partner",
        RelationshipV1::Parent => "parent",
        RelationshipV1::Child => "child",
        RelationshipV1::Sibling => "sibling",
        RelationshipV1::Grandparent => "grandparent",
        RelationshipV1::Friend => "friend",
        RelationshipV1::Other => "other",
    }
}

fn profile_readiness_copy(readiness: HouseholdProfileStateV1) -> &'static str {
    match readiness {
        HouseholdProfileStateV1::Incomplete => "incomplete",
        HouseholdProfileStateV1::LocalOnly => "saved on this device",
        HouseholdProfileStateV1::PendingSync => "pending sync",
        HouseholdProfileStateV1::Synced => "synced",
        HouseholdProfileStateV1::Conflicted => "conflicted",
    }
}

#[must_use]
pub fn household_chrome_copy(model: &AppModel, width: u16) -> Option<String> {
    let label = terminal_safe_text(model.household_chrome_label()?);
    let maximum = match responsive_mode(width) {
        ResponsiveMode::Compact => usize::from(width.saturating_sub(24)).max(4),
        ResponsiveMode::Standard => usize::from(width.saturating_sub(50)).max(8),
        ResponsiveMode::Wide => usize::from(width.saturating_sub(72)).max(12),
    };
    let mut characters = label.chars();
    let retained = characters.by_ref().take(maximum).collect::<String>();
    let label = if characters.next().is_some() && maximum > 1 {
        let mut shortened = retained.chars().take(maximum - 1).collect::<String>();
        shortened.push('…');
        shortened
    } else {
        retained
    };
    Some(format!("For: {label}"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResponsiveMode {
    Compact,
    Standard,
    Wide,
}

#[must_use]
pub const fn responsive_mode(width: u16) -> ResponsiveMode {
    if width < 60 {
        ResponsiveMode::Compact
    } else if width < 100 {
        ResponsiveMode::Standard
    } else {
        ResponsiveMode::Wide
    }
}

#[must_use]
pub fn composer_height(model: &AppModel, width: u16) -> u16 {
    let available = width.saturating_sub(4).max(1) as usize;
    let lines = model
        .draft
        .split('\n')
        .map(|line| line.chars().count().max(1).div_ceil(available))
        .sum::<usize>()
        .clamp(1, 6);
    let suggestions = slash_suggestion_line_count(model, available);
    u16::try_from(lines.saturating_add(suggestions))
        .unwrap_or(9)
        .saturating_add(2)
}

fn slash_suggestion_line_count(model: &AppModel, available: usize) -> usize {
    let suggestions = slash_suggestions(model, usize::MAX);
    if suggestions.is_empty() {
        return 0;
    }
    if model.draft.trim() == "/" {
        return suggestions
            .len()
            .div_ceil(slash_grid_columns(available, suggestions.len()));
    }
    suggestions
        .iter()
        .map(|spec| {
            format!("  {:<14}{}", spec.usage, spec.description)
                .chars()
                .count()
                .max(1)
                .div_ceil(available.max(1))
        })
        .sum()
}

fn slash_grid_columns(available: usize, suggestion_count: usize) -> usize {
    let cell_width = slash_grid_cell_width();
    (available.max(1) / cell_width)
        .max(1)
        .min(suggestion_count.max(1))
}

fn slash_grid_cell_width() -> usize {
    crate::SLASH_COMMAND_REGISTRY
        .iter()
        .map(|spec| spec.name.chars().count())
        .max()
        .unwrap_or(1)
        .saturating_add(3)
}

pub fn render(frame: &mut Frame<'_>, model: &AppModel) {
    let area = frame.area();
    let composer = composer_height(model, area.width);
    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(composer),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(frame, regions[0], model);
    render_transcript(frame, regions[1], model);
    render_composer(frame, regions[2], model);
    render_footer(frame, regions[3], model);
}

fn render_header(frame: &mut Frame<'_>, area: Rect, model: &AppModel) {
    let mode = responsive_mode(area.width);
    let status = match model.operation {
        OperationState::Idle => "ready",
        OperationState::Running(_) => "working",
        OperationState::Cancelling(_) => "stopping",
        OperationState::Finishing(_) => "finishing",
        OperationState::Exiting(_) => "closing",
    };
    let scope = household_chrome_copy(model, area.width)
        .map(|scope| format!(" · {scope}"))
        .unwrap_or_default();
    let line = match mode {
        ResponsiveMode::Compact => Line::from(vec![
            Span::styled(" hey.food", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                format!(" · {status}{scope}"),
                semantic_style(model, Color::DarkGray),
            ),
        ]),
        ResponsiveMode::Standard => Line::from(vec![
            Span::styled(" hey.food", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("  thoughtful food guidance · {status}{scope}"),
                semantic_style(model, Color::DarkGray),
            ),
        ]),
        ResponsiveMode::Wide => Line::from(vec![
            Span::styled(" hey.food", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("  Ask about food, meals, restaurants, or recipes · {status}{scope}"),
                semantic_style(model, Color::DarkGray),
            ),
        ]),
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn render_transcript(frame: &mut Frame<'_>, area: Rect, model: &AppModel) {
    if let Some(panel) = model.onboarding_choice_panel() {
        render_onboarding_choice_panel(frame, area, model, &panel);
        return;
    }
    let mut lines = Vec::new();
    let content_width = area.width.max(1) as usize;
    let mut latest_assistant_start = None;
    if model.scrollback.entries().is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Ask a question when you’re ready.",
            semantic_style(model, Color::DarkGray),
        )));
    }
    for entry in model.scrollback.entries() {
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        if entry.speaker == Speaker::Assistant {
            latest_assistant_start = Some(wrapped_line_count(&lines, content_width));
        }
        let (label, color) = match entry.speaker {
            Speaker::User => ("You", Color::Cyan),
            Speaker::Assistant => ("hey.food", Color::Green),
            Speaker::Notice => ("Notice", Color::Yellow),
        };
        lines.push(Line::from(Span::styled(
            label,
            semantic_style(model, color).add_modifier(Modifier::BOLD),
        )));
        if entry.text.is_empty() && entry.streaming {
            lines.push(Line::from(Span::styled(
                "…",
                semantic_style(model, Color::DarkGray),
            )));
        } else {
            lines.extend(entry.text.lines().map(|line| Line::from(line.to_owned())));
        }
    }
    if let Some(activity) = &model.activity {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            activity.clone(),
            semantic_style(model, Color::Yellow),
        )));
    }
    if model.unseen_lines > 0 {
        lines.push(Line::from(Span::styled(
            format!("{} new lines · End to follow", model.unseen_lines),
            semantic_style(model, Color::Yellow).add_modifier(Modifier::BOLD),
        )));
    }

    let total = wrapped_line_count(&lines, content_width);
    let visible = area.height as usize;
    let maximum_scroll = total.saturating_sub(visible);
    let scroll = if model.focus_latest_result_start {
        latest_assistant_start
            .unwrap_or(maximum_scroll)
            .saturating_add(model.latest_result_start_offset)
            .min(maximum_scroll)
    } else if model.follow_tail {
        maximum_scroll
    } else {
        maximum_scroll.saturating_sub(model.scroll_from_tail.min(maximum_scroll))
    };
    let scroll = u16::try_from(scroll).unwrap_or(u16::MAX);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        area,
    );
}

fn render_onboarding_choice_panel(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &AppModel,
    panel: &OnboardingChoicePanel,
) {
    let progress = panel
        .progress
        .map(|(step, total)| format!("Step {step} of {total}"));
    let context = match progress {
        Some(progress) => format!("{} · {progress}", panel.context),
        None => panel.context.clone(),
    };
    let header_height = area.height.min(3);
    let detail_height = area.height.saturating_sub(header_height).min(2);
    let body_height = area
        .height
        .saturating_sub(header_height)
        .saturating_sub(detail_height);
    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Length(body_height),
            Constraint::Length(detail_height),
        ])
        .split(area);

    let header = vec![
        Line::from(Span::styled(
            context,
            semantic_style(model, Color::Cyan).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            panel.title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            truncate_to_width(&panel.instruction, area.width as usize),
            semantic_style(model, Color::DarkGray),
        )),
    ];
    frame.render_widget(Paragraph::new(header), regions[0]);

    let columns = match responsive_mode(area.width) {
        ResponsiveMode::Compact => 1,
        ResponsiveMode::Standard => 2,
        ResponsiveMode::Wide => 3,
    };
    let rows = usize::from(regions[1].height).max(1);
    let page_size = rows.saturating_mul(columns).max(1);
    let page = panel.focused / page_size;
    let page_start = page.saturating_mul(page_size);
    let gap = 2usize;
    let total_gap = gap.saturating_mul(columns.saturating_sub(1));
    let cell_width = (usize::from(area.width).saturating_sub(total_gap) / columns).max(1);
    let mut choice_lines = Vec::with_capacity(rows);
    for row in 0..rows {
        let mut spans = Vec::new();
        for column in 0..columns {
            let choice_index = page_start + row * columns + column;
            if column > 0 {
                spans.push(Span::raw(" ".repeat(gap)));
            }
            let Some(choice) = panel.choices.get(choice_index) else {
                spans.push(Span::raw(" ".repeat(cell_width)));
                continue;
            };
            let focused = choice_index == panel.focused;
            let focus_marker = if focused { "›" } else { " " };
            let selected_marker = if choice.selected { "[✓]" } else { "[ ]" };
            let prefix = format!("{focus_marker} {selected_marker} {:>2} ", choice.number);
            let prefix_width = UnicodeWidthStr::width(prefix.as_str());
            let label = truncate_to_width(&choice.label, cell_width.saturating_sub(prefix_width));
            let content = pad_to_width(&format!("{prefix}{label}"), cell_width);
            let style = if focused {
                semantic_style(model, Color::Cyan).add_modifier(Modifier::BOLD)
            } else if choice.selected {
                semantic_style(model, Color::Green)
            } else {
                Style::default()
            };
            spans.push(Span::styled(content, style));
        }
        choice_lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(choice_lines), regions[1]);

    let selected_count = panel
        .choices
        .iter()
        .filter(|choice| choice.selected)
        .count();
    let focused = panel
        .choices
        .get(panel.focused)
        .map(|choice| format!("{} · {}", choice.number, choice.label))
        .unwrap_or_default();
    let page_count = panel.choices.len().max(1).div_ceil(page_size);
    let page_copy = (page_count > 1).then(|| format!(" · Page {} of {page_count}", page + 1));
    let summary = match panel.mode {
        OnboardingSelectionMode::Single => page_copy.unwrap_or_default(),
        OnboardingSelectionMode::Multiple => {
            format!("{selected_count} selected{}", page_copy.unwrap_or_default())
        }
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                truncate_to_width(&focused, area.width as usize),
                semantic_style(model, Color::Green),
            )),
            Line::from(Span::styled(
                truncate_to_width(&summary, area.width as usize),
                semantic_style(model, Color::DarkGray),
            )),
        ]),
        regions[2],
    );
}

fn truncate_to_width(value: &str, maximum: usize) -> String {
    if UnicodeWidthStr::width(value) <= maximum {
        return value.to_owned();
    }
    if maximum == 0 {
        return String::new();
    }
    if maximum == 1 {
        return "…".into();
    }
    let mut output = String::new();
    let target = maximum - 1;
    let mut width = 0usize;
    for character in value.chars() {
        let character_width = character.width().unwrap_or(0);
        if width.saturating_add(character_width) > target {
            break;
        }
        output.push(character);
        width = width.saturating_add(character_width);
    }
    output.push('…');
    output
}

fn pad_to_width(value: &str, width: usize) -> String {
    let mut output = truncate_to_width(value, width);
    let padding = width.saturating_sub(UnicodeWidthStr::width(output.as_str()));
    output.push_str(&" ".repeat(padding));
    output
}

fn render_composer(frame: &mut Frame<'_>, area: Rect, model: &AppModel) {
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(semantic_style(model, Color::DarkGray));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let choice_panel = model.onboarding_choice_panel();
    let hint = if model.draft.is_empty() {
        match choice_panel.as_ref().map(|panel| panel.mode) {
            Some(OnboardingSelectionMode::Single) => "Choose with ↑/↓, then press Enter…",
            Some(OnboardingSelectionMode::Multiple) => {
                "Press Space to select, or type comma-separated choices…"
            }
            None => "Ask about food, a meal, a restaurant, or a recipe…",
        }
    } else {
        &model.draft
    };
    let style = if model.draft.is_empty() {
        semantic_style(model, Color::DarkGray)
    } else {
        Style::default()
    };
    let mut lines = vec![Line::from(vec![
        Span::raw("> "),
        Span::styled(hint.to_owned(), style),
    ])];
    let suggestions = slash_suggestions(model, usize::MAX);
    if model.draft.trim() == "/" {
        let cell_width = slash_grid_cell_width();
        let columns = slash_grid_columns(
            inner.width.saturating_sub(4).max(1).into(),
            suggestions.len(),
        );
        for row in suggestions.chunks(columns) {
            let mut spans = vec![Span::raw("  ")];
            for (index, spec) in row.iter().enumerate() {
                let command = if index + 1 == row.len() {
                    spec.name.to_owned()
                } else {
                    format!("{:<cell_width$}", spec.name)
                };
                spans.push(Span::styled(command, semantic_style(model, Color::Cyan)));
            }
            lines.push(Line::from(spans));
        }
    } else {
        for spec in suggestions {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:<14}", spec.usage),
                    semantic_style(model, Color::Cyan),
                ),
                Span::styled(spec.description, semantic_style(model, Color::DarkGray)),
            ]));
        }
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
        inner,
    );

    let (cursor_x, cursor_y) = composer_cursor(model, inner.width.saturating_sub(2).max(1));
    frame.set_cursor_position(Position::new(
        inner.x.saturating_add(2).saturating_add(cursor_x),
        inner.y.saturating_add(cursor_y),
    ));
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, model: &AppModel) {
    let text = if let Some(panel) = model.onboarding_choice_panel() {
        match (responsive_mode(area.width), panel.mode) {
            (ResponsiveMode::Compact, OnboardingSelectionMode::Single) => {
                " ↑↓←→ move · Enter choose · Esc cancel"
            }
            (ResponsiveMode::Compact, OnboardingSelectionMode::Multiple) => {
                " ↑↓←→ move · Space toggle · Enter · Esc"
            }
            (_, OnboardingSelectionMode::Single) => " ↑↓←→ move · Enter choose · Esc cancel",
            (_, OnboardingSelectionMode::Multiple) => {
                " ↑↓←→ move · Space select · Enter continue · Esc cancel"
            }
        }
    } else {
        match responsive_mode(area.width) {
            ResponsiveMode::Compact => " / commands · ? help · ^C stop · ^D exit",
            ResponsiveMode::Standard => " / commands · Enter send · Ctrl+C stop · End follow",
            ResponsiveMode::Wide => {
                " / commands · Tab complete · Shift+Enter newline · Ctrl+C stop · Ctrl+D exit"
            }
        }
    };
    let text = if model.unseen_lines > 0 {
        format!(" {} new · End follow", model.unseen_lines)
    } else {
        text.to_owned()
    };
    frame.render_widget(
        Paragraph::new(text).style(semantic_style(model, Color::DarkGray)),
        area,
    );
}

fn composer_cursor(model: &AppModel, width: u16) -> (u16, u16) {
    let width = width.max(1) as usize;
    let before = model.draft.chars().take(model.cursor).collect::<String>();
    let mut row = 0usize;
    let mut column = 0usize;
    for character in before.chars() {
        if character == '\n' {
            row += 1;
            column = 0;
        } else {
            column += 1;
            if column >= width {
                row += 1;
                column = 0;
            }
        }
    }
    (
        u16::try_from(column).unwrap_or(u16::MAX),
        u16::try_from(row.min(5)).unwrap_or(5),
    )
}

fn wrapped_line_count(lines: &[Line<'_>], width: usize) -> usize {
    lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(width.max(1)))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Action, RuntimeEvent, dispatch};
    use heyfood_application::{PortError, RunTurnOutcome, TurnFailure};
    use heyfood_core::{
        AgentEvent, HouseholdLifecycleV1, HouseholdProfileStateV1, HouseholdRevision,
        HouseholdScope, HouseholdSubjectId, MemberId, ProfileRevision, RelationshipV1,
    };
    use ratatui::{Terminal, backend::TestBackend};

    fn snapshot(model: &AppModel, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, model)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn streaming_model() -> AppModel {
        let mut model = AppModel::default();
        model.draft = "Can you suggest another option?".into();
        model.cursor = 31;
        let original = std::mem::replace(&mut model.draft, "Pad thai for lunch".into());
        model.cursor = model.draft.chars().count();
        let _ = dispatch(&mut model, Action::Submit);
        model.draft = original;
        model.cursor = model.draft.chars().count();
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Partial {
                    text: "Pad thai can vary by preparation. Ask about fish sauce and peanut cross-contact before ordering.".into(),
                },
            }),
        );
        model
    }

    fn restaurant_recommendation_model() -> AppModel {
        let mut model = AppModel::default();
        model.draft = "What can I eat there?".into();
        model.cursor = 20;
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Result {
                    document: serde_json::json!({
                        "text": "I found several options that fit.",
                        "structured": {
                            "type": "household_menu",
                            "restaurant_name": "Harbor Cafe",
                            "menu_freshness": "Menu updated 2 hours ago",
                            "source_url": "https://example.test/menu",
                            "member_summaries": [{
                                "member_id": "_self",
                                "label": null
                            }],
                            "sections": [{
                                "name": "Dinner",
                                "items": [{
                                    "item_id": "item-1",
                                    "name": "Grilled Fish",
                                    "price_cents": 2400,
                                    "safety": {
                                        "_self": {
                                            "level": "safe",
                                            "reason": "No detected conflicts."
                                        }
                                    }
                                }]
                            }],
                            "agent_picks": {
                                "_self": [{
                                    "item_id": "item-1",
                                    "member_id": "_self",
                                    "reason": "A simple preparation with no detected conflicts.",
                                    "tag": "Top pick"
                                }]
                            }
                        }
                    }),
                    conversation_id: None,
                },
            }),
        );
        model
    }

    fn interrupted_response_model() -> AppModel {
        let mut model = AppModel::default();
        model.draft = "What do you know about me?".into();
        model.cursor = 26;
        let _ = dispatch(&mut model, Action::Submit);
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Partial {
                    text: "I can consider your saved dietary profile.".into(),
                },
            }),
        );
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnFailed {
                operation_id: 1,
                failure: TurnFailure::from_port_error(&PortError::new(
                    "sse_inactivity",
                    "event stream inactivity deadline expired",
                )),
            }),
        );
        model
    }

    fn allergy_onboarding_model() -> AppModel {
        let mut model = AppModel::default();
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::BeginOnboarding {
                message: "Complete your dietary profile.".into(),
            }),
        );
        model.draft = "none".into();
        model.cursor = model.draft.chars().count();
        assert!(dispatch(&mut model, Action::Submit).is_empty());
        model
    }

    fn restarted_local_only_scope_model(scope: HouseholdScope) -> AppModel {
        let mut model = AppModel::default();
        let generation = crate::HouseholdModeGenerationV1::new(1).unwrap();
        let digest = crate::HouseholdAccountBindingDigestV1::from_bytes([7; 32]);
        let bootstrap = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::HouseholdGenerationReadyV1 {
                session_mode_generation: generation,
                mode: crate::HouseholdPresentationModeV1::NativeEnabled,
                account_binding_digest: digest,
            }),
        );
        let [
            crate::Effect::LoadHouseholdManagementV1 {
                operation_id,
                reducer_correlation,
                ..
            },
        ] = bootstrap.as_slice()
        else {
            panic!("expected native bootstrap load")
        };
        let owner = HouseholdMemberPresentationV1::new(
            HouseholdSubjectId::self_(),
            "Me",
            RelationshipV1::Self_,
            HouseholdLifecycleV1::Active,
            HouseholdProfileStateV1::LocalOnly,
            Some(ProfileRevision::new(1).unwrap()),
        )
        .unwrap();
        let member = HouseholdMemberPresentationV1::new(
            HouseholdSubjectId::member(
                MemberId::parse_preserved("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            ),
            "Maya",
            RelationshipV1::Child,
            HouseholdLifecycleV1::Active,
            HouseholdProfileStateV1::LocalOnly,
            Some(ProfileRevision::new(1).unwrap()),
        )
        .unwrap();
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::HouseholdManagementLoadedV1 {
                operation_id: *operation_id,
                session_mode_generation: generation,
                reducer_correlation: *reducer_correlation,
                purpose: crate::HouseholdManagementLoadPurposeV1::Bootstrap,
                account_binding_digest: digest,
                household_revision: HouseholdRevision::new(1).unwrap(),
                active_scope: scope,
                members: vec![owner, member],
            }),
        );
        assert!(model.household_management_ready());
        model
    }

    #[test]
    fn reviewed_native_profile_copy_is_exact() {
        let version = heyfood_core::ConsentVersionV1::new(3).unwrap();
        assert_eq!(
            profile_copy(ProfileCopyStateV1::OnboardingSaveReview),
            "Save this profile on this device? Saving does not grant profile-sync consent. If you already granted consent, sync may continue after the local save."
        );
        assert_eq!(
            profile_copy(ProfileCopyStateV1::OnboardingSaveCancelled),
            "Profile not saved. Profile-sync consent was not changed."
        );
        assert_eq!(
            profile_copy(ProfileCopyStateV1::SavedWithAbsentConsent),
            "Saved on this device. Sync is off because profile-sync consent is not granted. Run /profile consent to review consent."
        );
        assert_eq!(
            profile_copy(ProfileCopyStateV1::ConsentReview),
            "Grant profile-sync consent for Me? This allows hello.food to store and update your owner dietary profile in your account. It does not sync household members, and granting consent does not upload a profile."
        );
        assert_eq!(
            profile_copy(ProfileCopyStateV1::ConsentReviewPrompt),
            "Press y to grant consent; n or Esc to cancel."
        );
        assert_eq!(
            profile_copy(ProfileCopyStateV1::ConsentCancelled),
            "Profile-sync consent was not changed. Nothing was uploaded."
        );
        assert_eq!(
            profile_copy(ProfileCopyStateV1::ConsentGranted {
                consent_version: version,
            }),
            "Profile-sync consent active for Me (version 3). No profile was uploaded."
        );
        assert_eq!(
            profile_copy(ProfileCopyStateV1::RetryOffered {
                consent_version: version,
            }),
            "Consent version 3 is active. Your owner profile is still saved only on this device. Run /profile retry-sync to retry."
        );
        assert_eq!(
            profile_copy(ProfileCopyStateV1::InterruptedRetry),
            "Sync was interrupted. Run /profile retry-sync to resume the exact saved owner profile."
        );
        assert_eq!(
            profile_copy(ProfileCopyStateV1::ConsentVersionChanged),
            "Profile-sync consent changed. Review and save your owner profile again before syncing."
        );
        assert_eq!(
            profile_copy(ProfileCopyStateV1::ConsentRevoked),
            "Profile-sync consent is no longer active. Run /profile consent, then review and save your owner profile again."
        );
        assert_eq!(
            profile_copy(ProfileCopyStateV1::SyncPending),
            "Saved on this device; sync pending."
        );
        assert_eq!(
            profile_copy(ProfileCopyStateV1::RetryUnavailable),
            "Owner profile sync retry is unavailable."
        );
    }

    fn long_full_menu_model(width: u16, height: u16, top_level: bool) -> AppModel {
        let tea = (1..=40)
            .map(|index| {
                serde_json::json!({
                    "item_id": format!("tea-{index}"),
                    "name": format!("Tea drink {index}"),
                    "price_cents": 450,
                    "safety": {
                        "_self": {
                            "level": "caution",
                            "reason": "Verify added sweeteners."
                        }
                    }
                })
            })
            .collect::<Vec<_>>();
        let structured = serde_json::json!({
            "type": "household_menu",
            "presentation": "full_menu",
            "restaurant_name": "Abby Jane Bakeshop",
            "is_stale": false,
            "freshness_hours": 1.0,
            "requested_max_age_seconds": 86400,
            "sections": [
                {
                    "name": "Bread",
                    "items": [
                        {"item_id": "bread-1", "name": "Crème brûlée 東京 loaf"},
                        {"item_id": "bread-2", "name": "Baguette"},
                        {"item_id": "bread-3", "name": "Sourdough"}
                    ]
                },
                {"name": "Tea", "items": tea}
            ]
        });
        let document = if top_level {
            structured
        } else {
            serde_json::json!({
                "text": "Here is the complete menu.",
                "structured": structured
            })
        };
        let mut model = AppModel::default();
        model.width = width;
        model.height = height;
        model.draft = "Can I see the full menu?".into();
        model.cursor = model.draft.chars().count();
        let _ = dispatch(&mut model, Action::Submit);
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
        model.draft = "Crème brûlée 東京\nsecond line\nthird line".into();
        model.cursor = model.draft.chars().count();
        assert!(
            dispatch(&mut model, Action::Submit).is_empty(),
            "a draft must not dispatch while the result is finishing"
        );
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnFinished {
                operation_id: 1,
                outcome: RunTurnOutcome::Completed,
            }),
        );
        model
    }

    #[test]
    fn responsive_snapshots_keep_stream_and_composer_visible() {
        let model = streaming_model();
        for width in [40, 80, 120] {
            let rendered = snapshot(&model, width, 18);
            insta::assert_snapshot!(format!("streaming_{width}"), rendered);
            assert!(rendered.contains("Pad thai"), "width {width}: {rendered}");
            assert!(
                rendered.contains("suggest another"),
                "width {width}: {rendered}"
            );
            assert!(rendered.contains("Responding"), "width {width}: {rendered}");
            assert!(!rendered.contains("██"));
        }
        assert_eq!(responsive_mode(40), ResponsiveMode::Compact);
        assert_eq!(responsive_mode(80), ResponsiveMode::Standard);
        assert_eq!(responsive_mode(120), ResponsiveMode::Wide);
    }

    #[test]
    fn onboarding_choices_are_responsive_paginated_and_keyboard_visible() {
        for width in [40, 80, 120] {
            let mut model = allergy_onboarding_model();
            for _ in 0..27 {
                let _ = dispatch(&mut model, Action::HistoryNext);
            }
            let _ = dispatch(&mut model, Action::Insert(' '));
            let rendered = snapshot(&model, width, 18);
            assert!(rendered.contains("Allergies & restrictions"), "{rendered}");
            assert!(rendered.contains("Red dye / food colorings"), "{rendered}");
            assert!(rendered.contains("[✓]"), "{rendered}");
            assert!(rendered.contains("Space"), "{rendered}");
            assert!(rendered.contains("Esc"), "{rendered}");
            assert!(rendered.contains("Page"), "{rendered}");
        }
    }

    #[test]
    fn no_color_preserves_semantics_without_emitting_terminal_colors() {
        let mut model = allergy_onboarding_model();
        model.set_color_enabled(false);
        let _ = dispatch(&mut model, Action::Insert(' '));
        let backend = TestBackend::new(80, 18);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &model)).unwrap();
        let buffer = terminal.backend().buffer();
        assert!(
            buffer
                .content()
                .iter()
                .all(|cell| { cell.fg == Color::Reset && cell.bg == Color::Reset })
        );
        let rendered = snapshot(&model, 80, 18);
        assert!(rendered.contains("[✓]"));
        assert!(rendered.contains("Allergies & restrictions"));
    }

    #[test]
    fn restarted_member_and_everyone_views_present_as_hosted_ready() {
        let member = HouseholdScope::Subject(HouseholdSubjectId::member(
            MemberId::parse_preserved("550e8400-e29b-41d4-a716-446655440000").unwrap(),
        ));
        for scope in [member, HouseholdScope::Everyone] {
            for width in [40, 80, 120] {
                let model = restarted_local_only_scope_model(scope.clone());
                let rendered = snapshot(&model, width, 18);
                let semantic = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
                assert!(
                    semantic.contains("ready"),
                    "width {width} omitted the ready state: {rendered}"
                );
                assert!(semantic.contains("Ask a question when you’re ready"));
                assert!(semantic.contains("Ask about food"));
                assert!(!semantic.contains("hosted unavailable"));
            }
        }
    }

    #[test]
    fn resize_reflows_without_mutating_semantic_content() {
        let model = streaming_model();
        let content = model.scrollback.clone();
        let narrow = snapshot(&model, 40, 16);
        let wide = snapshot(&model, 120, 16);
        assert_ne!(narrow, wide);
        assert_eq!(model.scrollback, content);
        assert!(narrow.contains("^C stop"));
        assert!(wide.contains("Tab complete"));
    }

    #[test]
    fn restaurant_recommendations_keep_semantics_at_supported_widths() {
        let model = restaurant_recommendation_model();
        for width in [40, 80, 120] {
            let rendered = snapshot(&model, width, 40);
            let semantic = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
            for expected in [
                "Top picks at Harbor Cafe",
                "Grilled Fish",
                "generally safer",
                "Top pick",
                "show me the full menu",
            ] {
                assert!(
                    semantic.contains(expected),
                    "width {width} is missing {expected:?}: {rendered}"
                );
            }
            assert!(!rendered.contains("_self"));
            assert!(!rendered.contains('\u{1b}'));
        }
    }

    #[test]
    fn long_full_menu_opens_on_heading_and_completeness_not_the_drink_tail() {
        for top_level in [false, true] {
            for width in [40, 80, 120] {
                let mut model = long_full_menu_model(width, 18, top_level);
                let rendered = snapshot(&model, width, 18);
                let semantic = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
                assert!(
                    semantic.contains("Current menu at Abby Jane Bakeshop"),
                    "top_level={top_level}, width {width} did not open at the menu heading: {rendered}"
                );
                assert!(
                    semantic.contains("2 sections · 43 items"),
                    "top_level={top_level}, width {width} omitted completeness: {rendered}"
                );
                assert!(semantic.contains("Bread"));
                assert!(semantic.contains("Page Up/Page Down to browse"));
                assert!(
                    !semantic.contains("Tea drink 40"),
                    "top_level={top_level}, width {width} still opened at the drink-heavy tail: {rendered}"
                );

                let _ = dispatch(&mut model, Action::ScrollDown(8));
                let first_page = snapshot(&model, width, 18);
                assert_ne!(first_page, rendered);
                assert!(
                    !first_page.contains("Tea drink 40"),
                    "top_level={top_level}, width {width} jumped to the tail on first Page Down: {first_page}"
                );
                let _ = dispatch(&mut model, Action::ScrollDown(8));
                let second_page = snapshot(&model, width, 18);
                assert_ne!(second_page, first_page);
                assert!(
                    !second_page.contains("Tea drink 40"),
                    "top_level={top_level}, width {width} jumped to the tail on repeated Page Down: {second_page}"
                );
                let _ = dispatch(&mut model, Action::ScrollUp(8));
                assert_eq!(snapshot(&model, width, 18), first_page);
                let _ = dispatch(&mut model, Action::ScrollUp(8));
                assert_eq!(snapshot(&model, width, 18), rendered);
                let _ = dispatch(&mut model, Action::FollowTail);
                assert!(snapshot(&model, width, 18).contains("Tea drink 40"));
            }
        }
    }

    #[test]
    fn interrupted_response_recovery_is_semantic_at_supported_widths() {
        let model = interrupted_response_model();
        for width in [40, 80, 120] {
            let rendered = snapshot(&model, width, 22);
            let semantic = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
            for expected in [
                "saved dietary profile",
                "response stopped before it finished",
                "did not retry",
                "ask a new question now",
            ] {
                assert!(
                    semantic.contains(expected),
                    "width {width} is missing {expected:?}: {rendered}"
                );
            }
            assert!(!rendered.contains("sse_inactivity"));
            assert!(!rendered.contains("inactivity deadline"));
            assert!(!rendered.contains('\u{1b}'));
        }
    }

    #[test]
    fn unseen_indicator_is_visible_until_following_tail() {
        let mut model = streaming_model();
        let _ = dispatch(&mut model, Action::ScrollUp(4));
        let _ = dispatch(
            &mut model,
            Action::Runtime(RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Partial {
                    text: "\nA newly streamed line.".into(),
                },
            }),
        );
        assert!(snapshot(&model, 80, 18).contains("new · End follow"));
        let _ = dispatch(&mut model, Action::FollowTail);
        assert!(!snapshot(&model, 80, 18).contains("new · End follow"));
    }

    #[test]
    fn slash_registry_is_visible_while_composing_a_command() {
        let mut model = AppModel::default();
        model.draft = "/st".into();
        model.cursor = 3;
        let rendered = snapshot(&model, 80, 18);
        assert!(rendered.contains("/status"));
        assert!(rendered.contains("Show session readiness"));
    }

    #[test]
    fn exact_slash_command_palette_is_complete_at_supported_widths() {
        let mut model = AppModel::default();
        model.draft = "/".into();
        model.cursor = 1;

        for width in [40, 80, 120] {
            let rendered = snapshot(&model, width, 18);
            let tokens = rendered.split_whitespace().collect::<Vec<_>>();
            for spec in crate::SLASH_COMMAND_REGISTRY {
                assert!(
                    tokens.contains(&spec.name),
                    "width {width} omitted {} from slash discovery: {rendered}",
                    spec.name
                );
            }
        }

        assert_eq!(composer_height(&model, 40), 10);
        assert_eq!(composer_height(&model, 80), 6);
        assert_eq!(composer_height(&model, 120), 5);
    }
}
