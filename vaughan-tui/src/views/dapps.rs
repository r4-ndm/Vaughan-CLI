//! Whitelisted dApps: add URLs, open in Freedom (auto-connect later).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame,
};
use tokio::runtime::Handle;
use vaughan_core::core::{TrustedDapp, WalletState};
use vaughan_provider::EventBus;

use crate::app::{KeyOutcome, Screen};
use crate::freedom;
use crate::input::{Input, InputAction};
use crate::views::{body_areas, labeled_input, status_paragraph};

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
                let items: Vec<ListItem> = if dapps.is_empty() {
                    vec![ListItem::new(Line::from(
                        "  No dApps yet — press a to add one.",
                    ))]
                } else {
                    dapps
                        .iter()
                        .enumerate()
                        .map(|(i, d)| {
                            let mark = if i == self.selected { ">" } else { " " };
                            let style = if i == self.selected {
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(Color::Cyan)
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                Style::default()
                            };
                            ListItem::new(Line::from(Span::styled(
                                format!("{mark} {}  {}", d.name, d.url),
                                style,
                            )))
                        })
                        .collect()
                };
                let list =
                    List::new(items).block(Block::default().borders(Borders::ALL).title(
                        " Trusted dApps (↑↓, Enter open Freedom, a add, x remove, Esc back) ",
                    ));
                frame.render_widget(list, content);
            }
            Stage::Add => {
                let [msg, name_a, url_a] = Layout::vertical([
                    Constraint::Min(2),
                    Constraint::Length(3),
                    Constraint::Length(3),
                ])
                .areas(content);
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from("Add a whitelisted dApp"),
                        Line::from("Tab switches fields · Enter saves · Esc cancels"),
                        Line::from(
                            "Also add the origin to VAUGHAN_PROVIDER_TRUSTED_ORIGINS (or restart after save — origins merge on next launch).",
                        ),
                    ])
                    .block(Block::default().borders(Borders::ALL).title(" Add dApp "))
                    .wrap(Wrap { trim: false }),
                    msg,
                );
                frame.render_widget(labeled_input("Name", &self.name, self.focus == 0), name_a);
                frame.render_widget(labeled_input("URL", &self.url, self.focus == 1), url_a);
            }
        }
        frame.render_widget(status_paragraph(&self.status), status_area);
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
                KeyCode::Char('x') => {
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
