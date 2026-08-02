//! Visual constants shared by every terminal surface.
//!
//! The accent color mirrors `assets/brand/banner.palette.json`; keep the two
//! in sync when the brand palette changes.

use ratatui::{
    style::{Color, Modifier, Style},
    symbols::border,
};

/// Brand accent (`#9BC53D`).
pub const ACCENT: Color = Color::Rgb(0x9B, 0xC5, 0x3D);

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
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default()
    }
}

#[must_use]
pub fn emphasis() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}
