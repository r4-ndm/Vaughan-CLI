//! Keys: export recovery phrase / private key, import a hex private key.
//!
//! Every reveal path re-checks the vault password. Secrets are shown once and
//! cleared when the user leaves the screen — never logged.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};
use secrecy::{ExposeSecret, SecretString};
use tokio::runtime::Handle;
use vaughan_core::core::WalletState;
use vaughan_provider::EventBus;
use zeroize::Zeroize;

use crate::app::{KeyOutcome, Screen};
use crate::brand;
use crate::input::{Input, InputAction};
use crate::views::{body_areas, render_labeled_input, status_paragraph};

#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuItem {
    ExportPhrase,
    ExportKey,
    ImportKey,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Menu,
    Password,
    Reveal,
    ImportForm,
}

pub struct KeysView {
    stage: Stage,
    menu: MenuItem,
    password: Input,
    /// Password confirmed for the current import flow (never logged).
    verified_password: Option<SecretString>,
    label: Input,
    private_key: Input,
    import_focus: usize,
    /// Revealed secret for display only; zeroized on leave.
    revealed: Option<String>,
    reveal_title: String,
    status: String,
}

impl Default for KeysView {
    fn default() -> Self {
        Self {
            stage: Stage::Menu,
            menu: MenuItem::ExportPhrase,
            password: Input::new(true, "vault password"),
            verified_password: None,
            label: Input::new(false, "optional — blank → Wn-HD k"),
            private_key: Input::new(true, "0x… private key"),
            import_focus: 0,
            revealed: None,
            reveal_title: String::new(),
            status: String::new(),
        }
    }
}

impl KeysView {
    fn clear_secret(&mut self) {
        if let Some(ref mut s) = self.revealed {
            s.zeroize();
        }
        self.revealed = None;
        self.verified_password = None;
        self.password.set_value("");
        self.private_key.set_value("");
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, _wallet: &WalletState) {
        let [content, status_area] = body_areas(area);
        let text = match self.stage {
            Stage::Menu => vec![
                Line::from("Keys — export / import (password required)"),
                Line::from(""),
                menu_line(
                    self.menu == MenuItem::ExportPhrase,
                    "1  Export recovery phrase",
                ),
                menu_line(
                    self.menu == MenuItem::ExportKey,
                    "2  Export active private key",
                ),
                menu_line(self.menu == MenuItem::ImportKey, "3  Import private key"),
                Line::from(""),
                Line::from("Enter — continue   Esc — dashboard"),
            ],
            Stage::Password => vec![
                Line::from(match self.menu {
                    MenuItem::ExportPhrase => "Re-enter vault password to show recovery phrase",
                    MenuItem::ExportKey => "Re-enter vault password to show active private key",
                    MenuItem::ImportKey => "Re-enter vault password to import a key",
                }),
                Line::from(""),
                Line::from("Esc — cancel"),
            ],
            Stage::Reveal => vec![
                Line::from(Span::styled(
                    self.reveal_title.clone(),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    self.revealed.as_deref().unwrap_or(""),
                    Style::default().fg(Color::Green),
                )),
                Line::from(""),
                Line::from("Anyone with this can spend your funds. Esc clears and leaves."),
            ],
            Stage::ImportForm => vec![
                Line::from("Import a hex private key into this vault"),
                Line::from("Tab — next field   Enter — import   Esc — cancel"),
            ],
        };

        match self.stage {
            Stage::Password => {
                let [msg, pw] =
                    ratatui::layout::Layout::vertical([Constraint::Min(3), Constraint::Length(3)])
                        .areas(content);
                let msg_inner =
                    brand::render_faded_box(frame, msg, Some(brand::fade_line(" Keys ")));
                frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), msg_inner);
                render_labeled_input(frame, pw, "Password", &self.password, true);
            }
            Stage::ImportForm => {
                let [msg, label_a, key_a] = ratatui::layout::Layout::vertical([
                    Constraint::Min(2),
                    Constraint::Length(3),
                    Constraint::Length(3),
                ])
                .areas(content);
                let msg_inner =
                    brand::render_faded_box(frame, msg, Some(brand::fade_line(" Import key ")));
                frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), msg_inner);
                render_labeled_input(frame, label_a, "Label", &self.label, self.import_focus == 0);
                render_labeled_input(
                    frame,
                    key_a,
                    "Private key",
                    &self.private_key,
                    self.import_focus == 1,
                );
            }
            _ => {
                let inner =
                    brand::render_faded_box(frame, content, Some(brand::fade_line(" Keys ")));
                frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
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
            Stage::Menu => match key.code {
                KeyCode::Esc => KeyOutcome::Navigate(Screen::Dashboard),
                KeyCode::Char('1') => {
                    self.menu = MenuItem::ExportPhrase;
                    KeyOutcome::Consumed
                }
                KeyCode::Char('2') => {
                    self.menu = MenuItem::ExportKey;
                    KeyOutcome::Consumed
                }
                KeyCode::Char('3') => {
                    self.menu = MenuItem::ImportKey;
                    KeyOutcome::Consumed
                }
                KeyCode::Up | KeyCode::Down => {
                    self.menu = match (self.menu, key.code) {
                        (MenuItem::ExportPhrase, KeyCode::Down) => MenuItem::ExportKey,
                        (MenuItem::ExportKey, KeyCode::Down) => MenuItem::ImportKey,
                        (MenuItem::ImportKey, KeyCode::Up) => MenuItem::ExportKey,
                        (MenuItem::ExportKey, KeyCode::Up) => MenuItem::ExportPhrase,
                        (MenuItem::ImportKey, KeyCode::Down) => MenuItem::ExportPhrase,
                        (MenuItem::ExportPhrase, KeyCode::Up) => MenuItem::ImportKey,
                        (m, _) => m,
                    };
                    KeyOutcome::Consumed
                }
                KeyCode::Enter => {
                    self.status.clear();
                    self.password.set_value("");
                    self.stage = Stage::Password;
                    KeyOutcome::Consumed
                }
                _ => KeyOutcome::NotHandled,
            },
            Stage::Password => {
                if key.code == KeyCode::Esc {
                    self.clear_secret();
                    self.stage = Stage::Menu;
                    return KeyOutcome::Consumed;
                }
                match self.password.handle_key(key) {
                    InputAction::Ignored => KeyOutcome::NotHandled,
                    InputAction::Consumed => KeyOutcome::Consumed,
                    InputAction::Submitted => {
                        let pw = self.password.take_secret();
                        match self.menu {
                            MenuItem::ExportPhrase => match wallet.export_mnemonic(&pw) {
                                Ok(phrase) => {
                                    self.reveal_title = "Recovery phrase".into();
                                    self.revealed = Some(phrase.expose_secret().clone());
                                    self.stage = Stage::Reveal;
                                    self.status.clear();
                                }
                                Err(e) => self.status = e.user_message(),
                            },
                            MenuItem::ExportKey => match wallet.export_active_private_key(&pw) {
                                Ok(sk) => {
                                    self.reveal_title = "Active private key".into();
                                    self.revealed = Some(sk.expose_secret().clone());
                                    self.stage = Stage::Reveal;
                                    self.status.clear();
                                }
                                Err(e) => self.status = e.user_message(),
                            },
                            MenuItem::ImportKey => match wallet.verify_password(&pw) {
                                Ok(()) => {
                                    self.verified_password = Some(pw);
                                    self.stage = Stage::ImportForm;
                                    self.import_focus = 0;
                                    self.status.clear();
                                }
                                Err(e) => self.status = e.user_message(),
                            },
                        }
                        KeyOutcome::Consumed
                    }
                }
            }
            Stage::Reveal => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
                    self.clear_secret();
                    self.stage = Stage::Menu;
                    KeyOutcome::Consumed
                } else {
                    KeyOutcome::Consumed
                }
            }
            Stage::ImportForm => {
                if key.code == KeyCode::Esc {
                    self.clear_secret();
                    self.stage = Stage::Menu;
                    return KeyOutcome::Consumed;
                }
                if key.code == KeyCode::Tab {
                    self.import_focus = 1 - self.import_focus;
                    return KeyOutcome::Consumed;
                }
                let action = if self.import_focus == 0 {
                    self.label.handle_key(key)
                } else {
                    self.private_key.handle_key(key)
                };
                match action {
                    InputAction::Ignored => KeyOutcome::NotHandled,
                    InputAction::Consumed => KeyOutcome::Consumed,
                    InputAction::Submitted if self.import_focus == 0 => {
                        self.import_focus = 1;
                        KeyOutcome::Consumed
                    }
                    InputAction::Submitted => {
                        let Some(pw) = self.verified_password.take() else {
                            self.stage = Stage::Password;
                            self.status = "Password required again to import.".into();
                            return KeyOutcome::Consumed;
                        };
                        let sk = self.private_key.take_secret();
                        match wallet.import_private_key(&pw, self.label.value(), &sk) {
                            Ok(account) => {
                                self.clear_secret();
                                self.status =
                                    format!("Imported {} ({})", account.label, account.address);
                                self.stage = Stage::Menu;
                            }
                            Err(e) => {
                                self.verified_password = Some(pw);
                                self.status = e.user_message();
                            }
                        }
                        KeyOutcome::Consumed
                    }
                }
            }
        }
    }
}

use ratatui::layout::Constraint;

impl Drop for KeysView {
    fn drop(&mut self) {
        self.clear_secret();
    }
}

fn menu_line(selected: bool, text: &str) -> Line<'static> {
    let marker = if selected { ">" } else { " " };
    let style = if selected {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    Line::from(Span::styled(format!("{marker} {text}"), style))
}
