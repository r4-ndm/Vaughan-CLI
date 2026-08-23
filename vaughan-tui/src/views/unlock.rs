//! Unlock: decrypt the vault, then choose this session's operating mode.
//!
//! FR-5.1 — mode is selected at session start and stays fixed for the process.
//! Returning users pick Human / Assist / Degen after each successful unlock
//! (not only during first-time onboarding).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;
use secrecy::SecretString;
use tokio::runtime::Handle;
use vaughan_agent::{needs_agent_setup, profile_dir, resolve_model_config, ModelConfig};
use vaughan_core::core::{OperatingMode, WalletState};
use vaughan_provider::{EventBus, ProviderEvent};

use crate::app::{KeyOutcome, Screen};
use crate::brand;
use crate::input::{Input, InputAction};
use crate::views::{body_areas, render_labeled_input, status_paragraph};

enum Stage {
    /// Masked password entry.
    EnterPassword,
    /// Post-unlock: pick Human / Assist / Degen for this process session.
    SelectMode,
}

pub struct UnlockView {
    stage: Stage,
    input: Input,
    status: String,
    session_agent_config: Option<ModelConfig>,
    /// Password held until mode is chosen (agent setup / key decrypt), then cleared.
    handoff_password: Option<SecretString>,
    needs_setup: bool,
}

impl Default for UnlockView {
    fn default() -> Self {
        Self {
            stage: Stage::EnterPassword,
            input: Input::new(true, "password"),
            status: String::new(),
            session_agent_config: None,
            handoff_password: None,
            needs_setup: false,
        }
    }
}

impl UnlockView {
    /// Agent config loaded after a successful unlock (file + decrypted key).
    pub fn take_session_agent_config(&mut self) -> Option<ModelConfig> {
        self.session_agent_config.take()
    }

    /// Vault password for post-unlock agent setup (cleared after take).
    pub fn take_handoff_password(&mut self) -> Option<SecretString> {
        self.handoff_password.take()
    }

    pub fn needs_agent_setup(&self) -> bool {
        self.needs_setup
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, _wallet: &WalletState) {
        let [content, status_area] = body_areas(area);
        match self.stage {
            Stage::EnterPassword => {
                render_labeled_input(frame, content, "Password", &self.input, true);
            }
            Stage::SelectMode => {
                let text = vec![
                    Line::from("Wallet unlocked. Choose operating mode for this session:"),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled("1 — Classic Human Mode: ", Style::default().fg(Color::Cyan)),
                        Span::raw("Zero AI, 100% manual sovereignty"),
                    ]),
                    Line::from(vec![
                        Span::styled("2 — AI Assisted Mode:  ", Style::default().fg(Color::Green)),
                        Span::raw("AI advisor with manual human confirmation"),
                    ]),
                    Line::from(vec![
                        Span::styled("3 — Degen Bot Mode: ", Style::default().fg(Color::Magenta)),
                        Span::styled("⚠ WARNING: ", Style::default().fg(Color::Yellow)),
                        Span::raw("Use an isolated wallet with only a fraction of your funds."),
                    ]),
                    Line::from(""),
                    Line::from(
                        "This choice lasts until you quit Vaughan (cannot switch mid-session).",
                    ),
                ];
                let inner = brand::render_faded_box(
                    frame,
                    content,
                    Some(brand::fade_line(" Session mode ")),
                );
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
        events: &EventBus,
    ) -> KeyOutcome {
        match self.stage {
            Stage::EnterPassword => self.handle_password(key, wallet, events),
            Stage::SelectMode => self.handle_mode_select(key, wallet),
        }
    }

    fn handle_password(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        events: &EventBus,
    ) -> KeyOutcome {
        match self.input.handle_key(key) {
            InputAction::Ignored => KeyOutcome::NotHandled,
            InputAction::Submitted => {
                let password = self.input.take_secret();
                match wallet.unlock(&password) {
                    Ok(()) => {
                        if let Ok(address) = wallet.active_address() {
                            events
                                .publish(ProviderEvent::AccountsChanged(vec![address.to_string()]));
                        }
                        // Keep password for agent config / setup after mode pick.
                        self.handoff_password = Some(password);
                        self.status.clear();
                        self.stage = Stage::SelectMode;
                        KeyOutcome::Consumed
                    }
                    Err(e) => {
                        self.status = e.user_message();
                        KeyOutcome::Consumed
                    }
                }
            }
            InputAction::Consumed => KeyOutcome::Consumed,
        }
    }

    fn handle_mode_select(&mut self, key: KeyEvent, wallet: &mut WalletState) -> KeyOutcome {
        let mode = match key.code {
            KeyCode::Char('1') => OperatingMode::HumanOnly,
            KeyCode::Char('2') => OperatingMode::AiAssisted,
            KeyCode::Char('3') => OperatingMode::DegenTrader,
            _ => return KeyOutcome::NotHandled,
        };

        wallet.set_operating_mode(mode);
        self.status.clear();

        if mode.is_ai_enabled() {
            let dir = profile_dir(wallet.path());
            if let Some(pw) = self.handoff_password.clone() {
                self.load_agent_config(wallet, &pw);
                // Only force setup when this profile has no usable key yet.
                // Saved agent.toml + agent.key.json restore on the next login.
                if needs_agent_setup(&dir, Some(&pw)) {
                    self.needs_setup = true;
                    return KeyOutcome::Navigate(Screen::AgentSetup);
                }
            } else if needs_agent_setup(&dir, None) {
                self.needs_setup = true;
                return KeyOutcome::Navigate(Screen::AgentSetup);
            }
            self.needs_setup = false;
            return KeyOutcome::Navigate(Screen::Dashboard);
        }

        self.handoff_password = None;
        KeyOutcome::Navigate(Screen::Dashboard)
    }

    fn load_agent_config(&mut self, wallet: &WalletState, password: &SecretString) {
        let dir = profile_dir(wallet.path());
        match resolve_model_config(&dir, Some(password)) {
            Ok(cfg) => self.session_agent_config = Some(cfg),
            Err(e) => {
                self.status = format!("Mode set (agent config: {e})");
            }
        }
    }
}
