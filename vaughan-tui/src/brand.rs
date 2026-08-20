//! Vaughan wordmark / spinner helpers for the TUI chrome (pure ratatui, no extra crates).

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Compact ASCII wordmark for the title bar.
pub fn logo_lines() -> Vec<Line<'static>> {
    let style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    vec![
        Line::from(Span::styled(
            r"  _   __                __            ",
            style,
        )),
        Line::from(Span::styled(
            r" | | / /__ ___ ___ ____/ /  ___ ____  ",
            style,
        )),
        Line::from(Span::styled(
            r" | |/ / _ `/ _ `/ _ `/ _ \/ _ `/ _ \ ",
            style,
        )),
        Line::from(Span::styled(
            r" |___/\_,_/\_,_/\_,_/_//_/\_,_/_//_/ ",
            style,
        )),
    ]
}
