//! A small text-input widget shared by the forms (password, mnemonic, address,
//! amount). The raw value is held in a `String`; secret inputs are masked on
//! display and moved into a [`secrecy::SecretString`] via [`Input::take_secret`].
//!
//! Editing is cursor-aware: `Left`/`Right` move the cursor, `Backspace` deletes
//! before it, `Delete` after it, and `Home`/`End` jump to the start/end. The
//! cursor is a byte index into `buffer` (always on a char boundary).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use secrecy::SecretString;

/// What the input did with a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputAction {
    /// The key was consumed (typed, cursor moved, deleted, …).
    Consumed,
    /// The user submitted the field (Enter).
    Submitted,
    /// The key is not handled by the input.
    Ignored,
}

/// A single-line text input.
pub struct Input {
    buffer: String,
    /// Byte index of the cursor; always on a char boundary.
    cursor: usize,
    masked: bool,
    placeholder: String,
}

impl Input {
    pub fn new(masked: bool, placeholder: impl Into<String>) -> Self {
        Self {
            buffer: String::new(),
            cursor: 0,
            masked,
            placeholder: placeholder.into(),
        }
    }

    pub fn value(&self) -> &str {
        &self.buffer
    }

    /// Handle a key; returns what the input did with it.
    pub fn handle_key(&mut self, key: KeyEvent) -> InputAction {
        match key.code {
            KeyCode::Enter => InputAction::Submitted,
            KeyCode::Char(c) => {
                self.buffer.insert(self.cursor, c);
                self.cursor += c.len_utf8();
                InputAction::Consumed
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let prev = self.buffer[..self.cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.buffer.remove(prev);
                    self.cursor = prev;
                }
                InputAction::Consumed
            }
            KeyCode::Delete => {
                if self.cursor < self.buffer.len() {
                    self.buffer.remove(self.cursor);
                }
                InputAction::Consumed
            }
            KeyCode::Left => {
                if self.cursor > 0 {
                    self.cursor = self.buffer[..self.cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                }
                InputAction::Consumed
            }
            KeyCode::Right => {
                if self.cursor < self.buffer.len() {
                    self.cursor += self.buffer[self.cursor..]
                        .chars()
                        .next()
                        .map(|c| c.len_utf8())
                        .unwrap_or(0);
                }
                InputAction::Consumed
            }
            KeyCode::Home => {
                self.cursor = 0;
                InputAction::Consumed
            }
            KeyCode::End => {
                self.cursor = self.buffer.len();
                InputAction::Consumed
            }
            _ => InputAction::Ignored,
        }
    }

    /// Move the buffer out as a [`SecretString`] (zeroized on drop).
    pub fn take_secret(&mut self) -> SecretString {
        self.cursor = 0;
        SecretString::from(std::mem::take(&mut self.buffer))
    }

    /// Move the buffer out as a plain `String` (caller zeroizes if sensitive).
    pub fn take_string(&mut self) -> String {
        self.cursor = 0;
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

    /// A renderable line: value (or dimmed placeholder) plus a cursor at the
    /// current edit position.
    pub fn line(&self) -> Line<'static> {
        if self.buffer.is_empty() {
            let mut line = Line::from(Span::styled(
                self.placeholder.clone(),
                Style::default().fg(Color::DarkGray),
            ));
            line.push_span(Span::styled("▌", Style::default().fg(Color::Yellow)));
            return line;
        }

        let display = self.display();
        // Convert the byte cursor into a position within the *displayed* text
        // (char count maps 1:1 for both masked and raw display).
        let cursor_chars = self.buffer[..self.cursor].chars().count();
        let cursor_bytes = display
            .char_indices()
            .nth(cursor_chars)
            .map(|(i, _)| i)
            .unwrap_or(display.len());

        // Owned segments so the returned Line is `'static`.
        let before = display[..cursor_bytes].to_string();
        let after = display[cursor_bytes..].to_string();
        let mut line = Line::default();
        line.push_span(Span::styled(before, Style::default()));
        line.push_span(Span::styled("▌", Style::default().fg(Color::Yellow)));
        line.push_span(Span::styled(after, Style::default()));
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

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn typed(input: &mut Input, text: &str) {
        for c in text.chars() {
            assert_eq!(input.handle_key(press(c)), InputAction::Consumed);
        }
    }

    #[test]
    fn typing_and_backspace() {
        let mut input = Input::new(false, "placeholder");
        typed(&mut input, "ab");
        assert_eq!(input.value(), "ab");
        assert_eq!(
            input.handle_key(key(KeyCode::Backspace)),
            InputAction::Consumed
        );
        assert_eq!(input.value(), "a");
    }

    #[test]
    fn submit_on_enter() {
        let mut input = Input::new(false, "placeholder");
        assert_eq!(input.handle_key(press('x')), InputAction::Consumed);
        assert_eq!(
            input.handle_key(key(KeyCode::Enter)),
            InputAction::Submitted
        );
    }

    #[test]
    fn masking_hides_value() {
        let mut input = Input::new(true, "pw");
        typed(&mut input, "se");
        assert_eq!(input.display(), "**");
        assert_eq!(input.value(), "se");
    }

    #[test]
    fn take_secret_moves_and_clears() {
        let mut input = Input::new(true, "pw");
        typed(&mut input, "x");
        let secret = input.take_secret();
        assert_eq!(secret.expose_secret(), "x");
        assert!(input.value().is_empty());
    }

    #[test]
    fn ignored_keys_are_reported() {
        let mut input = Input::new(false, "ph");
        assert_eq!(input.handle_key(key(KeyCode::Esc)), InputAction::Ignored);
        assert_eq!(input.handle_key(key(KeyCode::F(1))), InputAction::Ignored);
    }
    #[test]
    fn cursor_navigation_and_editing() {
        let mut input = Input::new(false, "ph");
        typed(&mut input, "abc"); // "abc", cursor at end

        // Home, insert at the front: "Xabc", cursor after X.
        assert_eq!(input.handle_key(key(KeyCode::Home)), InputAction::Consumed);
        typed(&mut input, "X");
        assert_eq!(input.value(), "Xabc");

        // Right twice (cursor after 'a'), Delete removes the char after it.
        assert_eq!(input.handle_key(key(KeyCode::Right)), InputAction::Consumed);
        assert_eq!(input.handle_key(key(KeyCode::Right)), InputAction::Consumed);
        assert_eq!(
            input.handle_key(key(KeyCode::Delete)),
            InputAction::Consumed
        );
        assert_eq!(input.value(), "Xab");

        // Backspace removes the char before the cursor (cursor is still at
        // the old position after Delete).
        assert_eq!(
            input.handle_key(key(KeyCode::Backspace)),
            InputAction::Consumed
        );
        assert_eq!(input.value(), "Xa");

        // End then Backspace removes the last char.
        assert_eq!(input.handle_key(key(KeyCode::End)), InputAction::Consumed);
        assert_eq!(
            input.handle_key(key(KeyCode::Backspace)),
            InputAction::Consumed
        );
        assert_eq!(input.value(), "X");

        // Cursor at Home/End boundaries is a no-op, not a panic.
        assert_eq!(input.handle_key(key(KeyCode::Home)), InputAction::Consumed);
        assert_eq!(input.handle_key(key(KeyCode::Left)), InputAction::Consumed);
        assert_eq!(input.handle_key(key(KeyCode::End)), InputAction::Consumed);
        assert_eq!(input.handle_key(key(KeyCode::Right)), InputAction::Consumed);
        assert_eq!(
            input.handle_key(key(KeyCode::Delete)),
            InputAction::Consumed
        );
        assert_eq!(input.value(), "X");
    }

    #[test]
    fn cursor_renders_in_middle() {
        let mut input = Input::new(false, "ph");
        typed(&mut input, "abc");
        input.handle_key(key(KeyCode::Home));
        let line = input.line();
        let spans = line.spans;
        assert_eq!(spans.len(), 3, "text + cursor + text");
        assert_eq!(spans[0].content.as_ref(), "");
        assert_eq!(spans[2].content.as_ref(), "abc");
    }
}
