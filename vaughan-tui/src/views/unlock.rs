//! Unlock: decrypt the vault with the user's password.

use crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::Frame;
use secrecy::SecretString;
use tokio::runtime::Handle;
use vaughan_agent::{needs_agent_setup, profile_dir, resolve_model_config, ModelConfig};
use vaughan_core::core::WalletState;
use vaughan_provider::{EventBus, ProviderEvent};

use crate::app::{KeyOutcome, Screen};
use crate::input::{Input, InputAction};
use crate::views::{body_areas, labeled_input, status_paragraph};

pub struct UnlockView {
    input: Input,
    status: String,
    session_agent_config: Option<ModelConfig>,
    /// Password held only long enough to hand off to AgentSetup for key encryption.
    handoff_password: Option<SecretString>,
    needs_setup: bool,
}

impl Default for UnlockView {
    fn default() -> Self {
        Self {
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
        frame.render_widget(labeled_input("Password", &self.input, true), content);
        frame.render_widget(status_paragraph(&self.status), status_area);
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        _handle: &Handle,
        events: &EventBus,
    ) -> KeyOutcome {
        match self.input.handle_key(key) {
            InputAction::Ignored => KeyOutcome::NotHandled,
            InputAction::Submitted => {
                let password = self.input.take_secret();
                match wallet.unlock(&password) {
                    Ok(()) => {
                        self.load_agent_config(wallet, &password);
                        if let Ok(address) = wallet.active_address() {
                            events
                                .publish(ProviderEvent::AccountsChanged(vec![address.to_string()]));
                        }

                        let dir = profile_dir(wallet.path());
                        self.needs_setup = wallet.operating_mode().is_ai_enabled()
                            && needs_agent_setup(&dir, Some(&password));
                        if self.needs_setup {
                            self.handoff_password = Some(password);
                            KeyOutcome::Navigate(Screen::AgentSetup)
                        } else {
                            KeyOutcome::Navigate(Screen::Dashboard)
                        }
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

    fn load_agent_config(&mut self, wallet: &WalletState, password: &SecretString) {
        if !wallet.operating_mode().is_ai_enabled() {
            return;
        }
        let dir = profile_dir(wallet.path());
        match resolve_model_config(&dir, Some(password)) {
            Ok(cfg) => self.session_agent_config = Some(cfg),
            Err(e) => {
                self.status = format!("Unlocked (agent config: {e})");
            }
        }
    }
}
