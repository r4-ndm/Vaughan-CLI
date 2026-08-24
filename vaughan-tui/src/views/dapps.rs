//! Optional web (Freedom): whitelisted origins for the EIP-1193 bridge.
//!
//! Not the default Browserless Pulse path — use Ag / Dex / Browse / MCP first.
//! Enter launches Freedom only — no system-browser fallback.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Paragraph, Wrap},
    Frame,
};
use tokio::runtime::Handle;
use vaughan_core::core::{TrustedDapp, WalletState};
use vaughan_provider::EventBus;

use crate::app::{KeyOutcome, Screen};
use crate::brand;
use crate::freedom;
use crate::input::{Input, InputAction};
use crate::views::{body_areas, render_labeled_input, status_paragraph};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Stage {
    #[default]
    List,
    Add,
}

pub struct DappsView {
    stage: Stage,
    selected: usize,
    name: Input,
    url: Input,
    focus: usize,
    status: String,
}

impl Default for DappsView {
    fn default() -> Self {
        Self {
            stage: Stage::List,
            selected: 0,
            name: Input::new(false, "PulseX"),
            url: Input::new(false, "https://…"),
            focus: 0,
            status: String::new(),
        }
    }
}

impl DappsView {
    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        let [content, status_area] = body_areas(area);
        let dapps = wallet.trusted_dapps();

        match self.stage {
            Stage::List => {
                let inner = brand::render_faded_box(
                    frame,
                    content,
                    Some(brand::fade_line(
                        " Optional web / Freedom (↑↓ · Enter → Freedom · a add · d delete · Esc) ",
                    )),
                );
                if dapps.is_empty() {
                    frame.render_widget(
                        Paragraph::new("  No sites yet — press a to add one (optional Freedom path)."),
                        inner,
                    );
                } else {
                    self.render_dapp_list(frame, inner, &dapps);
                }
            }
            Stage::Add => {
                let [msg, name_a, url_a] = Layout::vertical([
                    Constraint::Min(2),
                    Constraint::Length(3),
                    Constraint::Length(3),
                ])
                .areas(content);
                let msg_inner =
                    brand::render_faded_box(frame, msg, Some(brand::fade_line(" Add dApp ")));
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from("Add a whitelisted dApp"),
                        Line::from("Tab switches fields · Enter saves · Esc cancels"),
                        Line::from(
                            "Origins feed the provider allowlist on next Vaughan launch. Open requires Freedom Browser.",
                        ),
                    ])
                    .wrap(Wrap { trim: false }),
                    msg_inner,
                );
                render_labeled_input(frame, name_a, "Name", &self.name, self.focus == 0);
                render_labeled_input(frame, url_a, "URL", &self.url, self.focus == 1);
            }
        }
        frame.render_widget(status_paragraph(&self.status), status_area);
    }

    fn render_dapp_list(&self, frame: &mut Frame, area: Rect, dapps: &[TrustedDapp]) {
        let buf = frame.buffer_mut();
        for (i, d) in dapps.iter().enumerate() {
            let y = area.y.saturating_add(i as u16);
            if y >= area.y.saturating_add(area.height) {
                break;
            }
            let selected = i == self.selected;
            let mark = if selected { ">" } else { " " };
            let row_style = if selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let url_style = if selected {
                row_style.add_modifier(Modifier::UNDERLINED)
            } else {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::UNDERLINED)
            };

            let prefix = format!("{mark} {}  ", d.name);
            let prefix_w = Line::from(prefix.as_str()).width() as u16;
            buf.set_stringn(area.x, y, &prefix, area.width as usize, row_style);

            if prefix_w < area.width {
                let url_x = area.x.saturating_add(prefix_w);
                let url_w = area.width.saturating_sub(prefix_w) as usize;
                // Defanged so terminal click ≠ system browser; Enter uses real URL.
                let shown = freedom::display_url(&d.url);
                buf.set_stringn(url_x, y, &shown, url_w, url_style);
            }
        }
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        _handle: &Handle,
        _events: &EventBus,
    ) -> KeyOutcome {
        match self.stage {
            Stage::List => match key.code {
                KeyCode::Esc => KeyOutcome::Navigate(Screen::Dashboard),
                KeyCode::Char('a') => {
                    self.stage = Stage::Add;
                    self.focus = 0;
                    self.status.clear();
                    KeyOutcome::Consumed
                }
                KeyCode::Up => {
                    self.selected = self.selected.saturating_sub(1);
                    KeyOutcome::Consumed
                }
                KeyCode::Down => {
                    let len = wallet.trusted_dapps().len();
                    if len > 0 {
                        self.selected = (self.selected + 1).min(len - 1);
                    }
                    KeyOutcome::Consumed
                }
                KeyCode::Enter => {
                    let dapps = wallet.trusted_dapps();
                    if let Some(TrustedDapp { url, .. }) = dapps.get(self.selected) {
                        match freedom::open_dapp_url(url) {
                            Ok(msg) => self.status = msg,
                            Err(e) => self.status = e,
                        }
                    }
                    KeyOutcome::Consumed
                }
                KeyCode::Char('d') => {
                    let dapps = wallet.trusted_dapps();
                    if let Some(TrustedDapp { url, .. }) = dapps.get(self.selected).cloned() {
                        match wallet.remove_trusted_dapp(&url) {
                            Ok(()) => {
                                self.status = "Removed dApp.".into();
                                self.selected = self.selected.saturating_sub(1);
                            }
                            Err(e) => self.status = e.user_message(),
                        }
                    }
                    KeyOutcome::Consumed
                }
                _ => KeyOutcome::NotHandled,
            },
            Stage::Add => {
                if key.code == KeyCode::Esc {
                    self.stage = Stage::List;
                    return KeyOutcome::Consumed;
                }
                if key.code == KeyCode::Tab {
                    self.focus = 1 - self.focus;
                    return KeyOutcome::Consumed;
                }
                let action = if self.focus == 0 {
                    self.name.handle_key(key)
                } else {
                    self.url.handle_key(key)
                };
                match action {
                    InputAction::Ignored => KeyOutcome::NotHandled,
                    InputAction::Consumed => KeyOutcome::Consumed,
                    InputAction::Submitted if self.focus == 0 => {
                        self.focus = 1;
                        KeyOutcome::Consumed
                    }
                    InputAction::Submitted => {
                        match wallet.add_trusted_dapp(self.name.value(), self.url.value()) {
                            Ok(d) => {
                                self.status = format!(
                                    "Added {} — restart Vaughan so the provider allowlist picks up the origin.",
                                    d.name
                                );
                                self.name.set_value("");
                                self.url.set_value("");
                                self.stage = Stage::List;
                            }
                            Err(e) => self.status = e.user_message(),
                        }
                        KeyOutcome::Consumed
                    }
                }
            }
        }
    }
}
