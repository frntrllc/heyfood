use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Position, Rect},
    style::Style,
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::{AppModel, OperationState, Speaker, startup, theme};

/// Release channel stamped by CI; local builds present as stable.
const CHANNEL: &str = match option_env!("HEYFOOD_BUILD_CHANNEL") {
    Some(channel) => channel,
    None => "stable",
};

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

/// Rows reserved for the composer, borders included. `width` is the full
/// terminal width; frame, border, and prompt chrome are subtracted here.
#[must_use]
pub fn composer_height(model: &AppModel, width: u16) -> u16 {
    let available = width.saturating_sub(10).max(1) as usize;
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
    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);
    render_window(frame, regions[0], model);
    render_footer(frame, regions[1]);
}

fn render_window(frame: &mut Frame<'_>, area: Rect, model: &AppModel) {
    let window = Block::default()
        .borders(Borders::ALL)
        .border_set(theme::BORDER)
        .border_style(theme::dim());
    let inner = window.inner(area);
    frame.render_widget(window, area);

    let composer = composer_height(model, area.width);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(composer),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Span::styled(format!(" {}", model.location), theme::dim())),
        rows[0],
    );
    if show_startup(model) {
        startup::render_startup(frame, inset_horizontal(rows[1], 1));
    } else {
        render_transcript(frame, inset_horizontal(rows[1], 1), model);
    }
    render_tip(frame, inset_horizontal(rows[2], 1), model);
    render_composer(frame, inset_horizontal(rows[3], 1), model);
}

const fn inset_horizontal(area: Rect, margin: u16) -> Rect {
    Rect {
        x: area.x.saturating_add(margin),
        y: area.y,
        width: area.width.saturating_sub(margin.saturating_mul(2)),
        height: area.height,
    }
}

fn show_startup(model: &AppModel) -> bool {
    model.scrollback.entries().is_empty()
        && model.activity.is_none()
        && matches!(model.operation, OperationState::Idle)
}

fn render_transcript(frame: &mut Frame<'_>, area: Rect, model: &AppModel) {
    let mut lines = Vec::new();
    for entry in model.scrollback.entries() {
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        let (label, style) = match entry.speaker {
            Speaker::User => ("You", theme::user_label()),
            Speaker::Assistant => ("hey.food", theme::accent_emphasis()),
            Speaker::Notice => ("Notice", theme::notice_emphasis()),
        };
        lines.push(Line::from(Span::styled(label, style)));
        if entry.text.is_empty() && entry.streaming {
            lines.push(Line::from(Span::styled("…", theme::dim())));
        } else {
            lines.extend(entry.text.lines().map(|line| Line::from(line.to_owned())));
        }
    }
    if let Some(activity) = &model.activity {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(activity.clone(), theme::notice())));
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

fn render_tip(frame: &mut Frame<'_>, area: Rect, model: &AppModel) {
    let line = if model.unseen_lines > 0 {
        Line::from(Span::styled(
            format!("{} new lines · End to follow", model.unseen_lines),
            theme::notice_emphasis(),
        ))
    } else {
        let tip = match responsive_mode(area.width) {
            ResponsiveMode::Compact => "Tip: Shift+Enter adds a new line.",
            ResponsiveMode::Standard | ResponsiveMode::Wide => {
                "Tip: Shift+Enter adds a new line. PageUp/PageDown scroll history."
            }
        };
        Line::from(Span::styled(tip, theme::dim()))
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn render_composer(frame: &mut Frame<'_>, area: Rect, model: &AppModel) {
    let status = match model.operation {
        OperationState::Idle => "ready",
        OperationState::Running(_) => "working",
        OperationState::Cancelling(_) => "stopping",
        OperationState::Finishing(_) => "finishing",
        OperationState::Exiting(_) => "closing",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(theme::BORDER)
        .border_style(theme::dim())
        .title_bottom(
            Line::from(Span::styled(format!(" hey.food ({status}) "), theme::dim()))
                .right_aligned(),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (content, style) = if model.draft.is_empty() {
        (
            "Ask about food, a meal, a restaurant, or a recipe…",
            theme::dim(),
        )
    } else {
        (model.draft.as_str(), Style::default())
    };
    let text = Text::from(
        content
            .split('\n')
            .enumerate()
            .map(|(row, line)| {
                let prefix = if row == 0 {
                    Span::styled("› ", theme::accent())
                } else {
                    Span::raw("  ")
                };
                Line::from(vec![prefix, Span::styled(line.to_owned(), style)])
            })
            .collect::<Vec<_>>(),
    );
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);

    let (cursor_x, cursor_y) = composer_cursor(model, inner.width.saturating_sub(2).max(1));
    frame.set_cursor_position(Position::new(
        inner.x.saturating_add(2).saturating_add(cursor_x),
        inner.y.saturating_add(cursor_y),
    ));
}

fn render_footer(frame: &mut Frame<'_>, area: Rect) {
    let line = Line::from(vec![
        Span::styled("heyfood", theme::emphasis()),
        Span::styled(format!("  {} ", crate::VERSION), theme::dim()),
        Span::styled(format!("[{CHANNEL}] "), theme::dim()),
    ]);
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Right), area);
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
            assert!(rendered.contains("(working)"), "width {width}: {rendered}");
            assert!(!rendered.contains("██"));
        }
        assert_eq!(responsive_mode(40), ResponsiveMode::Compact);
        assert_eq!(responsive_mode(80), ResponsiveMode::Standard);
        assert_eq!(responsive_mode(120), ResponsiveMode::Wide);
    }

    #[test]
    fn startup_screen_shows_logo_menu_and_version() {
        let model = AppModel::default();
        for width in [40, 80, 120] {
            let rendered = snapshot(&model, width, 24);
            insta::assert_snapshot!(format!("startup_{width}"), rendered);
            assert!(rendered.contains("New question"), "width {width}");
            assert!(rendered.contains("ctrl+d"), "width {width}");
            assert!(
                rendered.contains(&format!("heyfood {} is here!", crate::VERSION)),
                "width {width}",
            );
            assert!(
                rendered.contains(&format!("heyfood  {}", crate::VERSION)),
                "footer missing at width {width}",
            );
        }
        let narrow = snapshot(&model, 40, 24);
        assert!(!narrow.contains('█'), "logo must hide on narrow terminals");
        let wide = snapshot(&model, 120, 24);
        assert!(wide.contains('█'), "logo must show on wide terminals");
    }

    #[test]
    fn startup_screen_is_replaced_once_conversation_starts() {
        let mut model = AppModel::default();
        assert!(snapshot(&model, 80, 24).contains("New question"));
        model.draft = "lunch".into();
        model.cursor = 5;
        let _ = dispatch(&mut model, Action::Submit);
        let rendered = snapshot(&model, 80, 24);
        assert!(!rendered.contains("New question"));
        assert!(rendered.contains("You"));
    }

    #[test]
    fn resize_reflows_without_mutating_semantic_content() {
        let model = streaming_model();
        let content = model.scrollback.clone();
        let narrow = snapshot(&model, 40, 16);
        let wide = snapshot(&model, 120, 16);
        assert_ne!(narrow, wide);
        assert_eq!(model.scrollback, content);
        assert!(narrow.contains("heyfood"));
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
        assert!(snapshot(&model, 80, 18).contains("new lines · End to follow"));
        let _ = dispatch(&mut model, Action::FollowTail);
        assert!(!snapshot(&model, 80, 18).contains("new lines · End to follow"));
    }
}
