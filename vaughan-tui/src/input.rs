//! A small text-input widget shared by the forms (password, mnemonic, address,
//! amount). The raw value is held in a `String`; secret inputs are masked on
//! display and moved into a [`secrecy::SecretString`] via [`Input::take_secret`].

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use secrecy::SecretString;

/// A single-line text input.
pub struct Input {
    buffer: String,
    masked: bool,
    placeholder: String,
}

impl Input {
    pub fn new(masked: bool, placeholder: impl Into<String>) -> Self {
        Self {
            buffer: String::new(),
            masked,
            placeholder: placeholder.into(),
        }
    }

    pub fn value(&self) -> &str {
        &self.buffer
    }

    /// Handle a key; returns `true` when the user submitted (Enter).
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Enter => true,
            KeyCode::Char(c) => {
                self.buffer.push(c);
                false
            }
            KeyCode::Backspace => {
                self.buffer.pop();
                false
            }
            _ => false,
        }
    }

    /// Move the buffer out as a [`SecretString`] (zeroized on drop).
    pub fn take_secret(&mut self) -> SecretString {
        SecretString::from(std::mem::take(&mut self.buffer))
    }

    /// Move the buffer out as a plain `String` (caller zeroizes if sensitive).
    pub fn take_string(&mut self) -> String {
        std::mem::take(&mut self.buffer)
    }

    /// The value as displayed (masked if secret).
    pub fn display(&self) -> String {
        if self.masked {
            "*".repeat(self.buffer.chars().count())
        } else {
            self.buffer.clone()
        }
    }

    /// A renderable line: value (or dimmed placeholder) plus a cursor.
    pub fn line(&self) -> Line<'static> {
        let (text, style) = if self.buffer.is_empty() {
            (
                self.placeholder.clone(),
                Style::default().fg(Color::DarkGray),
            )
        } else {
            (self.display(), Style::default())
        };
        let mut line = Line::from(Span::styled(text, style));
        line.push_span(Span::styled("▌", Style::default().fg(Color::Yellow)));
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use secrecy::ExposeSecret;

    fn press(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn backspace() -> KeyEvent {
        KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)
    }

    #[test]
    fn typing_and_backspace() {
        let mut input = Input::new(false, "placeholder");
        input.handle_key(press('a'));
        input.handle_key(press('b'));
        assert_eq!(input.value(), "ab");
        input.handle_key(backspace());
        assert_eq!(input.value(), "a");
    }

    #[test]
    fn submit_on_enter() {
        let mut input = Input::new(false, "placeholder");
        assert!(!input.handle_key(press('x')));
        assert!(input.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    }

    #[test]
    fn masking_hides_value() {
        let mut input = Input::new(true, "pw");
        input.handle_key(press('s'));
        input.handle_key(press('e'));
        assert_eq!(input.display(), "**");
        assert_eq!(input.value(), "se");
    }

    #[test]
    fn take_secret_moves_and_clears() {
        let mut input = Input::new(true, "pw");
        input.handle_key(press('x'));
        let secret = input.take_secret();
        assert_eq!(secret.expose_secret(), "x");
        assert!(input.value().is_empty());
    }
}
