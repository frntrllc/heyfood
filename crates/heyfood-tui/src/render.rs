use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::{AppModel, OperationState, Speaker};

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
    u16::try_from(lines).unwrap_or(6).saturating_add(2)
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
    frame.render_widget(
        Paragraph::new(format!("> {hint}"))
            .style(style)
            .wrap(Wrap { trim: false }),
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
        ResponsiveMode::Compact => " Enter send · ^C stop · ^D exit",
        ResponsiveMode::Standard => " Enter send · Shift+Enter newline · Ctrl+C stop · End follow",
        ResponsiveMode::Wide => {
            " Enter send · Shift+Enter newline · PageUp/PageDown scroll · Ctrl+C stop · Ctrl+D exit"
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
        assert!(wide.contains("PageUp/PageDown"));
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
}
