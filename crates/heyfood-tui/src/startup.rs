//! The centered welcome surface shown before the first turn.

use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::theme;

const BANNER: &str = include_str!("../../../assets/brand/banner.txt");

/// Accent spans mirrored from `assets/brand/banner.palette.json`:
/// `(line, start, length)` in characters.
const BANNER_ACCENT_SPANS: &[(usize, usize, usize)] = &[(3, 18, 2)];

/// Left column is the action, right column the keybinding.
const MENU: &[(&str, &str)] = &[
    ("New question", "enter"),
    ("New line", "shift+enter"),
    ("Scroll history", "pgup/pgdn"),
    ("Quit", "ctrl+d"),
];

const MENU_WIDTH: usize = 34;

pub fn render_startup(frame: &mut Frame<'_>, area: Rect, color_enabled: bool) {
    let announcement = format!("heyfood {} is here!", crate::VERSION);
    let subtitle = if area.width < 52 {
        "Ask about food and meals."
    } else {
        "Ask about food, meals, restaurants, or recipes."
    };
    let banner_width = BANNER
        .lines()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    let block_width = banner_width
        .max(MENU_WIDTH)
        .max(announcement.chars().count())
        .max(subtitle.chars().count());
    let show_banner = (area.width as usize) >= banner_width + 2
        && (area.height as usize) >= BANNER.lines().count() + MENU.len() + 6;

    // Every line is padded to `block_width` so center alignment moves the
    // whole block as one unit instead of centering each line independently.
    let mut lines: Vec<Line<'_>> = Vec::new();
    if show_banner {
        lines.extend(banner_lines(block_width, color_enabled));
        lines.push(Line::from(""));
    }
    for (action, key) in MENU {
        lines.push(pad_to(menu_line(action, key, color_enabled), block_width));
    }
    lines.push(Line::from(""));
    lines.push(pad_to(
        Line::from(Span::styled(
            announcement,
            theme::accent_emphasis(color_enabled),
        )),
        block_width,
    ));
    lines.push(pad_to(
        Line::from(Span::styled(subtitle, theme::dim(color_enabled))),
        block_width,
    ));

    let top = (area.height as usize).saturating_sub(lines.len()) / 2;
    let mut padded = vec![Line::from(""); top];
    padded.extend(lines);
    frame.render_widget(Paragraph::new(padded).alignment(Alignment::Center), area);
}

fn banner_lines(block_width: usize, color_enabled: bool) -> Vec<Line<'static>> {
    let banner_width = BANNER
        .lines()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    let indent = block_width.saturating_sub(banner_width) / 2;
    BANNER
        .lines()
        .enumerate()
        .map(|(row, text)| {
            let accent = BANNER_ACCENT_SPANS.iter().find(|(line, _, _)| *line == row);
            let line = match accent {
                Some(&(_, start, length)) => {
                    let characters: Vec<char> = text.chars().collect();
                    let end = (start + length).min(characters.len());
                    let start = start.min(characters.len());
                    let before: String = characters[..start].iter().collect();
                    let middle: String = characters[start..end].iter().collect();
                    let after: String = characters[end..].iter().collect();
                    Line::from(vec![
                        Span::raw(" ".repeat(indent)),
                        Span::raw(before),
                        Span::styled(middle, theme::accent(color_enabled)),
                        Span::raw(after),
                    ])
                }
                None => Line::from(vec![
                    Span::raw(" ".repeat(indent)),
                    Span::raw(text.to_owned()),
                ]),
            };
            pad_to(line, block_width)
        })
        .collect()
}

fn menu_line(action: &'static str, key: &'static str, color_enabled: bool) -> Line<'static> {
    let padding = MENU_WIDTH
        .saturating_sub(action.chars().count())
        .saturating_sub(key.chars().count())
        .max(2);
    Line::from(vec![
        Span::styled(action, theme::emphasis()),
        Span::raw(" ".repeat(padding)),
        Span::styled(key, theme::dim(color_enabled)),
    ])
}

fn pad_to(mut line: Line<'static>, width: usize) -> Line<'static> {
    let missing = width.saturating_sub(line.width());
    if missing > 0 {
        line.spans.push(Span::raw(" ".repeat(missing)));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_spans_stay_in_sync_with_the_palette_asset() {
        let palette = include_str!("../../../assets/brand/banner.palette.json");
        for (line, start, length) in BANNER_ACCENT_SPANS {
            let expected =
                format!("{{\"line\": {line}, \"start\": {start}, \"length\": {length}}}");
            assert!(
                palette
                    .replace(char::is_whitespace, "")
                    .contains(&expected.replace(char::is_whitespace, "")),
                "palette asset no longer declares span {expected}",
            );
        }
    }

    #[test]
    fn banner_spans_never_split_outside_character_boundaries() {
        for line in banner_lines(60, true) {
            assert!(!line.to_string().contains('\u{fffd}'));
        }
    }
}
