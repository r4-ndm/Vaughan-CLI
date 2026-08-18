//! Onboarding: create a new wallet (show mnemonic) or restore from a phrase.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use secrecy::{ExposeSecret, SecretString};
use tokio::runtime::Handle;
use vaughan_core::core::WalletState;
use vaughan_core::security::encryption::validate_password_policy;
use vaughan_core::security::hd_wallet::{generate_mnemonic, validate_mnemonic};
use vaughan_core::security::Mnemonic;
use vaughan_provider::EventBus;
use zeroize::Zeroize;

use crate::app::{KeyOutcome, Screen};
use crate::input::{Input, InputAction};
use crate::views::{body_areas, labeled_input, status_paragraph};

/// Which step of onboarding the user is on.
enum Stage {
    /// Choose create vs restore.
    Choose,
    /// Display the freshly generated mnemonic (create flow).
    ShowMnemonic,
    /// Enter the recovery phrase (restore flow).
    EnterMnemonic,
    /// Set the vault password.
    SetPassword,
    /// Confirm the vault password.
    ConfirmPassword,
}

pub struct OnboardingView {
    stage: Stage,
    input: Input,
    mnemonic: Option<Mnemonic>,
    pending_password: Option<SecretString>,
    status: String,
}

impl Default for OnboardingView {
    fn default() -> Self {
        Self {
            stage: Stage::Choose,
            input: Input::new(false, ""),
            mnemonic: None,
            pending_password: None,
            status: String::new(),
        }
    }
}

impl OnboardingView {
    pub fn render(&self, frame: &mut Frame, area: Rect, _wallet: &WalletState) {
        let [content, status_area] = body_areas(area);
        let block = Block::default().borders(Borders::ALL);

        match self.stage {
            Stage::Choose => {
                let text = vec![
                    Line::from("Welcome to Vaughan."),
                    Line::from(""),
                    Line::from("c — create a new wallet (generates a 12-word recovery phrase)"),
                    Line::from("r — restore a wallet from an existing phrase"),
                ];
                frame.render_widget(
                    Paragraph::new(text).block(block).wrap(Wrap { trim: false }),
                    content,
                );
            }
            Stage::ShowMnemonic => {
                let phrase = self
                    .mnemonic
                    .as_ref()
                    .map(|m| m.to_string())
                    .unwrap_or_default();
                let text = vec![
                    Line::from("Your recovery phrase. Write it down and keep it offline:"),
                    Line::from(""),
                    Line::from(Span::styled(phrase, Style::default().fg(Color::Yellow))),
                    Line::from(""),
                    Line::from("Press Enter to continue."),
                ];
                frame.render_widget(
                    Paragraph::new(text).block(block).wrap(Wrap { trim: false }),
                    content,
                );
            }
            Stage::EnterMnemonic => {
                frame.render_widget(
                    labeled_input("Enter your 12-word recovery phrase", &self.input, true),
                    content,
                );
            }
            Stage::SetPassword => {
                frame.render_widget(
                    labeled_input(
                        "Choose a password (>= 12 chars: upper, lower, digit, symbol)",
                        &self.input,
                        true,
                    ),
                    content,
                );
            }
            Stage::ConfirmPassword => {
                frame.render_widget(
                    labeled_input("Confirm password", &self.input, true),
                    content,
                );
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
            Stage::Choose => match key.code {
                KeyCode::Char('c') => match generate_mnemonic() {
                    Ok(mnemonic) => {
                        self.mnemonic = Some(mnemonic);
                        self.status.clear();
                        self.stage = Stage::ShowMnemonic;
                    }
                    Err(e) => self.status = e.user_message(),
                },
                KeyCode::Char('r') => {
                    self.input = Input::new(false, "abandon abandon ... about");
                    self.status.clear();
                    self.stage = Stage::EnterMnemonic;
                }
                _ => return KeyOutcome::NotHandled,
            },
            Stage::ShowMnemonic => {
                if key.code == KeyCode::Enter {
                    self.input = Input::new(true, "");
                    self.stage = Stage::SetPassword;
                } else {
                    return KeyOutcome::NotHandled;
                }
            }
            Stage::EnterMnemonic => match self.input.handle_key(key) {
                InputAction::Ignored => return KeyOutcome::NotHandled,
                InputAction::Submitted => {
                    let mut phrase = self.input.take_string();
                    match validate_mnemonic(&phrase) {
                        Ok(mnemonic) => {
                            phrase.zeroize();
                            self.mnemonic = Some(mnemonic);
                            self.input = Input::new(true, "");
                            self.status.clear();
                            self.stage = Stage::SetPassword;
                        }
                        Err(e) => {
                            phrase.zeroize();
                            self.status = e.user_message();
                        }
                    }
                }
                InputAction::Consumed => {}
            },
            Stage::SetPassword => match self.input.handle_key(key) {
                InputAction::Ignored => return KeyOutcome::NotHandled,
                InputAction::Submitted => {
                    let password = self.input.take_secret();
                    match validate_password_policy(&password) {
                        Ok(()) => {
                            self.pending_password = Some(password);
                            self.input = Input::new(true, "");
                            self.status.clear();
                            self.stage = Stage::ConfirmPassword;
                        }
                        Err(e) => self.status = e.user_message(),
                    }
                }
                InputAction::Consumed => {}
            },
            Stage::ConfirmPassword => {
                let action = self.input.handle_key(key);
                if action == InputAction::Ignored {
                    return KeyOutcome::NotHandled;
                }
                if action == InputAction::Submitted {
                    let confirm = self.input.take_secret();
                    let matches = match &self.pending_password {
                        Some(pending) => pending.expose_secret() == confirm.expose_secret(),
                        None => false,
                    };
                    if matches {
                        let password = self.pending_password.take().unwrap();
                        let mnemonic = self.mnemonic.as_ref().unwrap().clone();
                        match wallet.create(&password, mnemonic) {
                            Ok(()) => {
                                self.mnemonic = None;
                                return KeyOutcome::Navigate(Screen::Dashboard);
                            }
                            Err(e) => {
                                self.pending_password = Some(password);
                                self.status = e.user_message();
                                self.stage = Stage::SetPassword;
                            }
                        }
                    } else {
                        self.pending_password = None;
                        self.status = "Passwords do not match.".to_string();
                        self.stage = Stage::SetPassword;
                    }
                }
            }
        }
        KeyOutcome::Consumed
    }
}
