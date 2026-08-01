use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::{AppModel, OperationState, Speaker, slash_suggestions};

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
    let suggestions = slash_suggestions(model, 3).len();
    u16::try_from(lines.saturating_add(suggestions))
        .unwrap_or(9)
        .saturating_add(2)
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
    let status = match model.operation {
        OperationState::Idle => "ready",
        OperationState::Running(_) => "working",
        OperationState::Cancelling(_) => "stopping",
        OperationState::Finishing(_) => "finishing",
        OperationState::Exiting(_) => "closing",
    };
    let line = match responsive_mode(area.width) {
        ResponsiveMode::Compact => Line::from(vec![
            Span::styled(" hey.food", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(format!(" · {status}"), Style::default().fg(Color::DarkGray)),
        ]),
        ResponsiveMode::Standard => Line::from(vec![
            Span::styled(" hey.food", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("  thoughtful food guidance · {status}"),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        ResponsiveMode::Wide => Line::from(vec![
            Span::styled(" hey.food", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("  Ask about food, meals, restaurants, or recipes · {status}"),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn render_transcript(frame: &mut Frame<'_>, area: Rect, model: &AppModel) {
    let mut lines = Vec::new();
    if model.scrollback.entries().is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Ask a question when you’re ready.",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for entry in model.scrollback.entries() {
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        let (label, color) = match entry.speaker {
            Speaker::User => ("You", Color::Cyan),
            Speaker::Assistant => ("hey.food", Color::Green),
            Speaker::Notice => ("Notice", Color::Yellow),
        };
        lines.push(Line::from(Span::styled(
            label,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )));
        if entry.text.is_empty() && entry.streaming {
            lines.push(Line::from(Span::styled(
                "…",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            lines.extend(entry.text.lines().map(|line| Line::from(line.to_owned())));
        }
    }
    if let Some(activity) = &model.activity {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            activity.clone(),
            Style::default().fg(Color::Yellow),
        )));
    }
    if model.unseen_lines > 0 {
        lines.push(Line::from(Span::styled(
            format!("{} new lines · End to follow", model.unseen_lines),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
    }

    let content_width = area.width.max(1) as usize;
    let total = wrapped_line_count(&lines, content_width);
    let visible = area.height as usize;
    let maximum_scroll = total.saturating_sub(visible);
    let scroll = if model.follow_tail {
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

fn render_composer(frame: &mut Frame<'_>, area: Rect, model: &AppModel) {
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let hint = if model.draft.is_empty() {
        "Ask about food, a meal, a restaurant, or a recipe…"
    } else {
        &model.draft
    };
    let style = if model.draft.is_empty() {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default()
    };
    let mut lines = vec![Line::from(vec![
        Span::raw("> "),
        Span::styled(hint.to_owned(), style),
    ])];
    for spec in slash_suggestions(model, 3) {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:<14}", spec.usage),
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(spec.description, Style::default().fg(Color::DarkGray)),
        ]));
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
    let text = match responsive_mode(area.width) {
        ResponsiveMode::Compact => " / commands · ? help · ^C stop · ^D exit",
        ResponsiveMode::Standard => " / commands · Enter send · Ctrl+C stop · End follow",
        ResponsiveMode::Wide => {
            " / commands · Tab complete · Shift+Enter newline · Ctrl+C stop · Ctrl+D exit"
        }
    };
    let text = if model.unseen_lines > 0 {
        format!(" {} new · End follow", model.unseen_lines)
    } else {
        text.to_owned()
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::DarkGray)),
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
    use heyfood_core::AgentEvent;
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

    fn long_full_menu_model(width: u16, height: u16) -> AppModel {
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
                    document: serde_json::json!({
                        "text": "Here is the complete menu.",
                        "structured": {
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
                                        {"item_id": "bread-1", "name": "Big Country"},
                                        {"item_id": "bread-2", "name": "Baguette"},
                                        {"item_id": "bread-3", "name": "Sourdough"}
                                    ]
                                },
                                {"name": "Tea", "items": tea}
                            ]
                        }
                    }),
                    conversation_id: None,
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
        for width in [40, 80, 120] {
            let model = long_full_menu_model(width, 18);
            let rendered = snapshot(&model, width, 18);
            let semantic = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                semantic.contains("Current menu at Abby Jane Bakeshop"),
                "width {width} did not open at the menu heading: {rendered}"
            );
            assert!(
                semantic.contains("2 sections · 43 items"),
                "width {width} omitted completeness: {rendered}"
            );
            assert!(semantic.contains("Bread"));
            assert!(semantic.contains("Page Up/Page Down to browse"));
            assert!(
                !semantic.contains("Tea drink 40"),
                "width {width} still opened at the drink-heavy tail: {rendered}"
            );
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
}
