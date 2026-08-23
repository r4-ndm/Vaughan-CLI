//! Chat history line building and word-wrap helpers.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use super::AgentMessage;

/// Prefixed chat bubbles, hard-wrapped to `width` so text never runs off-screen.
pub(super) fn build_chat_lines(history: &[AgentMessage], width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for msg in history {
        match msg {
            AgentMessage::User(u) => append_wrapped(
                &mut lines,
                "[You]: ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
                u,
                Style::default(),
                width,
            ),
            AgentMessage::Assistant(a) => append_wrapped(
                &mut lines,
                "[Advisor]: ",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
                a,
                Style::default(),
                width,
            ),
            AgentMessage::ToolCall { name, args } => append_wrapped(
                &mut lines,
                &format!("[Tool Call: {name}]: "),
                Style::default().fg(Color::Yellow),
                &truncate_display(args, 160),
                Style::default().fg(Color::DarkGray),
                width,
            ),
            AgentMessage::ToolResult { name, result } => append_wrapped(
                &mut lines,
                &format!("[Tool Result: {name}]: "),
                Style::default().fg(Color::Magenta),
                &truncate_display(result, 320),
                Style::default().fg(Color::DarkGray),
                width,
            ),
            AgentMessage::System(s) => append_wrapped(
                &mut lines,
                "[System]: ",
                Style::default().fg(Color::DarkGray),
                s,
                Style::default().fg(Color::DarkGray),
                width,
            ),
        }
    }
    lines
}

fn append_wrapped(
    out: &mut Vec<Line<'static>>,
    prefix: &str,
    prefix_style: Style,
    body: &str,
    body_style: Style,
    width: usize,
) {
    let width = width.max(8);
    let prefix_cols = prefix.chars().count();
    let indent = " ".repeat(prefix_cols.min(width.saturating_sub(1)));
    let body_width = width.saturating_sub(prefix_cols).max(8);

    let paragraphs: Vec<&str> = if body.is_empty() {
        vec![""]
    } else {
        body.split('\n').collect()
    };

    let mut first = true;
    for para in paragraphs {
        let chunks = wrap_words(para, body_width);
        if chunks.is_empty() {
            if first {
                out.push(Line::from(Span::styled(prefix.to_string(), prefix_style)));
                first = false;
            } else {
                out.push(Line::from(""));
            }
            continue;
        }
        for chunk in chunks {
            if first {
                out.push(Line::from(vec![
                    Span::styled(prefix.to_string(), prefix_style),
                    Span::styled(chunk, body_style),
                ]));
                first = false;
            } else {
                out.push(Line::from(vec![
                    Span::raw(indent.clone()),
                    Span::styled(chunk, body_style),
                ]));
            }
        }
    }
}

/// Cap tool call/result bodies shown in the chat pane.
fn truncate_display(s: &str, max: usize) -> String {
    let collapsed: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max {
        collapsed
    } else {
        let trimmed: String = collapsed.chars().take(max.saturating_sub(1)).collect();
        format!("{trimmed}…")
    }
}

/// Soft-wrap `text` to `width` columns, preferring breaks at whitespace.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    if text.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        let word_len = word.chars().count();
        if word_len > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            let chars: Vec<char> = word.chars().collect();
            for chunk in chars.chunks(width) {
                lines.push(chunk.iter().collect());
            }
            continue;
        }

        let next_len = if current.is_empty() {
            word_len
        } else {
            current.chars().count() + 1 + word_len
        };
        if next_len > width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }

    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod wrap_tests {
    use super::wrap_words;

    #[test]
    fn wraps_long_sentence() {
        let lines = wrap_words(
            "I'd be happy to help you buy meme coins on PulseChain testnet!",
            40,
        );
        assert!(lines.len() >= 2);
        assert!(lines.iter().all(|l| l.chars().count() <= 40));
    }

    #[test]
    fn hard_breaks_long_token() {
        let lines = wrap_words(&"x".repeat(25), 10);
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|l| l.chars().count() <= 10));
    }
}
