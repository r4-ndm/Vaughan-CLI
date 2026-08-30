//! Receive: public address, stealth URI, and scan/sweep of stealth notes.
//!
//! `y` copies the public address; `u` copies the stealth URI — both via
//! clipboard helpers so mouse-select never grabs box borders.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};
use tokio::runtime::Handle;
use vaughan_core::core::{StealthNote, WalletState};
use vaughan_provider::EventBus;

use crate::app::KeyOutcome;
use crate::brand;
use crate::clipboard;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Address,
    Notes,
}

pub struct ReceiveView {
    stage: Stage,
    notes: Vec<StealthNote>,
    selected: usize,
    status: String,
}

impl Default for ReceiveView {
    fn default() -> Self {
        Self {
            stage: Stage::Address,
            notes: Vec::new(),
            selected: 0,
            status: String::new(),
        }
    }
}

impl ReceiveView {
    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        let net = wallet.networks().active();
        match self.stage {
            Stage::Address => self.render_address(frame, area, wallet, net.name.as_str()),
            Stage::Notes => {
                let text = self.notes_lines(net.native_symbol.as_str());
                let inner =
                    brand::render_faded_box(frame, area, Some(brand::fade_line(" Receive ")));
                frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
            }
        }
    }

    /// Address / URI on unbordered rows so copy/select stays clean.
    fn render_address(&self, frame: &mut Frame, area: Rect, wallet: &WalletState, net_name: &str) {
        let address = wallet
            .active_address()
            .ok()
            .map(|a| a.to_string())
            .unwrap_or_else(|| "(locked)".into());
        let stealth = wallet.stealth_uri().ok();

        let [head, addr_row, mid, uri_row, foot] = Layout::vertical([
            Constraint::Length(4),
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Min(2),
            Constraint::Length(4),
        ])
        .areas(area);

        let head_inner = brand::render_faded_box(frame, head, Some(brand::fade_line(" Receive ")));
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(format!("Network: {net_name}")),
                Line::from("Public address (y copy) · stealth URI (u copy)"),
            ]),
            head_inner,
        );

        frame.render_widget(
            Paragraph::new(Line::from(brand::colored_address_spans(&address))),
            addr_row,
        );

        let mid_inner = brand::render_faded_box(frame, mid, None);
        frame.render_widget(
            Paragraph::new(Line::from("Stealth URI (one-time receive path):")),
            mid_inner,
        );

        let uri_line = match &stealth {
            Some(uri) => Line::from(Span::styled(uri.clone(), Style::default().fg(Color::Cyan))),
            None => Line::from("(unlock to show stealth URI)"),
        };
        frame.render_widget(Paragraph::new(uri_line).wrap(Wrap { trim: false }), uri_row);

        let mut foot_lines = vec![Line::from(
            "y address · u stealth URI · s scan notes · Esc dashboard",
        )];
        if !self.status.is_empty() {
            foot_lines.push(Line::from(Span::styled(
                self.status.clone(),
                Style::default().fg(Color::Yellow),
            )));
        }
        let foot_inner = brand::render_faded_box(frame, foot, None);
        frame.render_widget(Paragraph::new(foot_lines), foot_inner);
    }

    fn notes_lines(&self, symbol: &str) -> Vec<Line<'static>> {
        let mut lines = vec![
            Line::from(format!("Stealth notes ({symbol})")),
            Line::from("↑↓ select   Enter sweep to public address   Esc back"),
            Line::from(""),
        ];
        if self.notes.is_empty() {
            lines.push(Line::from("No funded stealth notes found."));
        } else {
            for (i, note) in self.notes.iter().enumerate() {
                let mark = if i == self.selected { ">" } else { " " };
                lines.push(Line::from(format!(
                    "{mark} {}  {}",
                    note.balance_formatted, note.announcement.stealth_address
                )));
            }
        }
        if !self.status.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                self.status.clone(),
                Style::default().fg(Color::Yellow),
            )));
        }
        lines
    }

    pub fn allows_footer_shortcuts(&self) -> bool {
        true
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        handle: &Handle,
        _events: &EventBus,
    ) -> KeyOutcome {
        match self.stage {
            Stage::Address => match key.code {
                KeyCode::Esc => KeyOutcome::Back,
                KeyCode::Char('s') => {
                    self.scan(wallet, handle);
                    KeyOutcome::Consumed
                }
                KeyCode::Char('y') | KeyCode::Char('Y') => match wallet.active_address() {
                    Ok(addr) => match clipboard::copy_text(addr) {
                        Ok(()) => KeyOutcome::Flash("F3 address copied".into()),
                        Err(e) => KeyOutcome::Flash(e),
                    },
                    Err(e) => {
                        self.status = e.user_message();
                        KeyOutcome::Consumed
                    }
                },
                KeyCode::Char('u') | KeyCode::Char('U') => match wallet.stealth_uri() {
                    Ok(uri) => match clipboard::copy_text(&uri) {
                        Ok(()) => KeyOutcome::Flash("Stealth URI copied".into()),
                        Err(e) => KeyOutcome::Flash(e),
                    },
                    Err(e) => {
                        self.status = e.user_message();
                        KeyOutcome::Consumed
                    }
                },
                _ => KeyOutcome::NotHandled,
            },
            Stage::Notes => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::Address;
                    KeyOutcome::Consumed
                }
                KeyCode::Up => {
                    self.selected = self.selected.saturating_sub(1);
                    KeyOutcome::Consumed
                }
                KeyCode::Down => {
                    if !self.notes.is_empty() {
                        self.selected = (self.selected + 1).min(self.notes.len() - 1);
                    }
                    KeyOutcome::Consumed
                }
                KeyCode::Enter => {
                    self.sweep(wallet, handle);
                    KeyOutcome::Consumed
                }
                _ => KeyOutcome::NotHandled,
            },
        }
    }

    fn scan(&mut self, wallet: &WalletState, handle: &Handle) {
        self.status = "Scanning announcer logs…".into();
        match handle.block_on(wallet.scan_stealth_notes()) {
            Ok(notes) => {
                self.notes = notes;
                self.selected = 0;
                self.stage = Stage::Notes;
                self.status = if self.notes.is_empty() {
                    "No funded notes.".into()
                } else {
                    format!("{} note(s)", self.notes.len())
                };
            }
            Err(e) => self.status = e.user_message(),
        }
    }

    fn sweep(&mut self, wallet: &WalletState, handle: &Handle) {
        let Some(note) = self.notes.get(self.selected).cloned() else {
            self.status = "No note selected.".into();
            return;
        };
        match handle.block_on(wallet.sweep_stealth_note(&note)) {
            Ok(hash) => {
                self.status = format!("Swept to public address: {hash}");
                self.scan(wallet, handle);
            }
            Err(e) => self.status = e.user_message(),
        }
    }
}
