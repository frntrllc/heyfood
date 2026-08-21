//! Semantic visual roles shared by every terminal surface.
//!
//! Omarchy and other terminal environments own their ANSI palette. Chrome,
//! focus, and accent roles therefore use named ANSI colors rather than fixed
//! RGB values. Dietary safety always retains its text or marker; color is an
//! enhancement and never the only signal.

use ratatui::{
    style::{Color, Modifier, Style},
    symbols::border,
};

/// Product accent, resolved through the terminal's cyan palette slot.
pub const ACCENT: Color = Color::Cyan;

/// Secondary chrome, resolved through the terminal's subdued palette slot.
pub const CHROME: Color = Color::DarkGray;

/// Keyboard focus and interactive discovery.
pub const FOCUS: Color = Color::Cyan;

/// Explicit selection. Selection markers remain visible without color.
pub const SELECTED: Color = Color::Green;

/// User-authored transcript labels.
pub const USER: Color = Color::Cyan;

/// Notices and bounded in-progress activity.
pub const NOTICE: Color = Color::Yellow;

/// Border set used for the window frame and the composer.
pub const BORDER: border::Set = border::ROUNDED;

#[must_use]
pub fn accent(color_enabled: bool) -> Style {
    if color_enabled {
        Style::default().fg(ACCENT)
    } else {
        Style::default()
    }
}

#[must_use]
pub fn accent_emphasis(color_enabled: bool) -> Style {
    accent(color_enabled).add_modifier(Modifier::BOLD)
}

/// Dim chrome: borders, hints, keybindings, and secondary text.
#[must_use]
pub fn dim(color_enabled: bool) -> Style {
    if color_enabled {
        Style::default().fg(CHROME)
    } else {
        Style::default()
    }
}

#[must_use]
pub fn emphasis() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_owned_roles_do_not_pin_rgb_values() {
        for role in [ACCENT, CHROME, FOCUS, SELECTED, USER, NOTICE] {
            assert!(
                !matches!(role, Color::Rgb(_, _, _)),
                "terminal-owned role unexpectedly pins RGB: {role:?}",
            );
        }
    }

    #[test]
    fn no_color_resets_every_semantic_role() {
        for style in [accent(false), accent_emphasis(false), dim(false)] {
            assert_eq!(style.fg, None);
            assert_eq!(style.bg, None);
        }
    }
}
