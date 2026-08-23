//! Footer chrome hotkey detection for agent chat (pass through to App).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// True when only Control is held (no Alt).
pub(super) fn is_ctrl_only(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT)
}

/// Bare footer chrome keys — used when the agent prompt is empty.
pub(super) fn is_chrome_hotkey(key: KeyEvent) -> bool {
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return false;
    }
    match key.code {
        KeyCode::Tab => true,
        KeyCode::Char(c) => is_chrome_char(c),
        _ => false,
    }
}

/// Ctrl+footer key — works mid-typing in agent chat (`Ctrl+C` / `Ctrl+Q` still request quit).
pub(super) fn is_ctrl_chrome_hotkey(key: KeyEvent) -> bool {
    if !is_ctrl_only(key) {
        return false;
    }
    match key.code {
        // Quit is handled in `global_action` before the view outcome matters.
        KeyCode::Char('c' | 'C' | 'q' | 'Q') => false,
        KeyCode::Char(c) => is_chrome_char(c),
        _ => false,
    }
}

fn is_chrome_char(c: char) -> bool {
    matches!(
        c.to_ascii_lowercase(),
        's' | 'v'
            | 'a'
            | 'b'
            | 'w'
            | 'c'
            | 'd'
            | 'g'
            | 'h'
            | 'n'
            | 'i'
            | 'k'
            | 'e'
            | 'f'
            | 'j'
            | 'm'
            | 'q'
            | 'r'
            | 'l'
            | 't'
            | 'x'
    )
}
