//! Onboarding: create a new wallet (show mnemonic) or restore from a phrase.
//!
//! After an AI mode is chosen (Assist / Degen), the welcome flow asks which LLM
//! provider to use and collects an API key when needed. Keys are encrypted with
//! the vault password at create time and never written in the clear.

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
use vaughan_agent::{profile_dir, AgentFileConfig, ModelConfig, PendingAgentSetup, ProviderType};
use vaughan_core::core::{OperatingMode, WalletState};
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
    /// Select operating mode (Human, Assist, Degen) or action.
    SelectMode,
    /// Pick LLM provider after an AI mode was chosen.
    ConfigureProvider,
    /// Enter a cloud API key (masked).
    EnterApiKey,
    /// Optional model id override.
    EnterModel,
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
    /// Provider chosen on the welcome agent-setup screens.
    pending_provider: Option<ProviderType>,
    /// Optional custom endpoint for OpenAI-compatible gateways (set via env later).
    pending_endpoint: Option<String>,
    pending_model: String,
    pending_api_key: Option<SecretString>,
    /// Ready-to-use config handed to the App after vault create.
    session_agent_config: Option<ModelConfig>,
}

impl Default for OnboardingView {
    fn default() -> Self {
        Self {
            stage: Stage::SelectMode,
            input: Input::new(false, ""),
            mnemonic: None,
            pending_password: None,
            status: String::new(),
            pending_provider: None,
            pending_endpoint: None,
            pending_model: String::new(),
            pending_api_key: None,
            session_agent_config: None,
        }
    }
}

impl OnboardingView {
    /// Take the session agent config produced by welcome setup (if any).
    pub fn take_session_agent_config(&mut self) -> Option<ModelConfig> {
        self.session_agent_config.take()
    }

    fn begin_agent_setup(&mut self) {
        self.pending_provider = None;
        self.pending_endpoint = None;
        self.pending_model.clear();
        self.pending_api_key = None;
        self.status.clear();
        self.stage = Stage::ConfigureProvider;
    }

    fn finish_agent_setup(&mut self) {
        if let (Some(provider), true) = (self.pending_provider, !self.pending_model.is_empty()) {
            let file = match provider {
                ProviderType::Ollama => AgentFileConfig::ollama(&self.pending_model),
                ProviderType::Gemini => AgentFileConfig::gemini(&self.pending_model),
                ProviderType::OpenAi => {
                    AgentFileConfig::openai(&self.pending_model, self.pending_endpoint.clone())
                }
            };
            let pending = PendingAgentSetup {
                file,
                api_key: self.pending_api_key.clone(),
            };
            match pending.to_model_config() {
                Ok(cfg) => {
                    self.session_agent_config = Some(cfg);
                    self.status = format!(
                        "Agent ready: {} / {}",
                        provider_label(provider),
                        self.pending_model
                    );
                }
                Err(e) => {
                    self.status = e.to_string();
                    self.stage = Stage::ConfigureProvider;
                    return;
                }
            }
        }
        self.stage = Stage::Choose;
    }

    fn default_model_for(provider: ProviderType) -> &'static str {
        match provider {
            ProviderType::Ollama => "llama3.2",
            ProviderType::Gemini => "gemini-1.5-flash",
            ProviderType::OpenAi => "gpt-4o-mini",
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        let [content, status_area] = body_areas(area);
        let block = Block::default().borders(Borders::ALL);

        match self.stage {
            Stage::SelectMode => {
                let text = vec![
                    Line::from("Welcome to Vaughan."),
                    Line::from(""),
                    Line::from("Select operating mode for this session:"),
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
                        Span::styled(
                            "3 — Degen Bot Mode:    ",
                            Style::default().fg(Color::Magenta),
                        ),
                        Span::raw("Autonomous trading in isolated burner wallet"),
                    ]),
                    Line::from(""),
                    Line::from("Or press:"),
                    Line::from("c — create a new wallet (generates a 12-word recovery phrase)"),
                    Line::from("r — restore a wallet from an existing phrase"),
                ];
                frame.render_widget(
                    Paragraph::new(text).block(block).wrap(Wrap { trim: false }),
                    content,
                );
            }
            Stage::ConfigureProvider => {
                let mode = wallet.operating_mode().display_label();
                let text = vec![
                    Line::from(format!("Mode: {mode}")),
                    Line::from(""),
                    Line::from("Choose your AI provider:"),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled("1 — Ollama (local)  ", Style::default().fg(Color::Green)),
                        Span::raw("No API key — runs on this machine"),
                    ]),
                    Line::from(vec![
                        Span::styled("2 — Google Gemini  ", Style::default().fg(Color::Cyan)),
                        Span::raw("Paste a Gemini API key next"),
                    ]),
                    Line::from(vec![
                        Span::styled(
                            "3 — OpenAI / compatible ",
                            Style::default().fg(Color::Yellow),
                        ),
                        Span::raw("OpenAI, OpenRouter, DeepSeek, …"),
                    ]),
                    Line::from(""),
                    Line::from("s — skip (use environment variables / Ollama defaults)"),
                    Line::from("Esc — back to mode select"),
                ];
                frame.render_widget(
                    Paragraph::new(text)
                        .block(block.title(" AI Provider "))
                        .wrap(Wrap { trim: false }),
                    content,
                );
            }
            Stage::EnterApiKey => {
                let provider = self
                    .pending_provider
                    .map(provider_label)
                    .unwrap_or("provider");
                let label = format!("Paste your {provider} API key (masked, never logged)");
                frame.render_widget(labeled_input(&label, &self.input, true), content);
            }
            Stage::EnterModel => {
                let placeholder = self
                    .pending_provider
                    .map(Self::default_model_for)
                    .unwrap_or("model");
                let label = format!("Model name (Enter for default: {placeholder})");
                frame.render_widget(labeled_input(&label, &self.input, true), content);
            }
            Stage::Choose => {
                let mode_badge = wallet.operating_mode().display_label();
                let agent_line = self
                    .session_agent_config
                    .as_ref()
                    .map(|c| format!("Agent: {} ({})", c.model_name, provider_label(c.provider)))
                    .unwrap_or_else(|| "Agent: (env / defaults)".to_string());
                let text = vec![
                    Line::from(format!("Mode selected: {mode_badge}")),
                    Line::from(agent_line),
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
            Stage::SelectMode => match key.code {
                KeyCode::Char('1') => {
                    wallet.set_operating_mode(OperatingMode::HumanOnly);
                    self.session_agent_config = None;
                    self.status.clear();
                    self.stage = Stage::Choose;
                }
                KeyCode::Char('2') => {
                    wallet.set_operating_mode(OperatingMode::AiAssisted);
                    self.begin_agent_setup();
                }
                KeyCode::Char('3') => {
                    wallet.set_operating_mode(OperatingMode::DegenTrader);
                    self.begin_agent_setup();
                }
                KeyCode::Char('c') => match generate_mnemonic() {
                    Ok(mnemonic) => {
                        wallet.set_operating_mode(OperatingMode::HumanOnly);
                        self.mnemonic = Some(mnemonic);
                        self.status.clear();
                        self.stage = Stage::ShowMnemonic;
                    }
                    Err(e) => self.status = e.user_message(),
                },
                KeyCode::Char('r') => {
                    wallet.set_operating_mode(OperatingMode::HumanOnly);
                    self.input = Input::new(false, "abandon abandon ... about");
                    self.status.clear();
                    self.stage = Stage::EnterMnemonic;
                }
                _ => return KeyOutcome::NotHandled,
            },
            Stage::ConfigureProvider => match key.code {
                KeyCode::Char('1') => {
                    self.pending_provider = Some(ProviderType::Ollama);
                    self.pending_api_key = None;
                    self.pending_endpoint = None;
                    self.input = Input::new(false, Self::default_model_for(ProviderType::Ollama));
                    self.input
                        .set_value(Self::default_model_for(ProviderType::Ollama));
                    self.status.clear();
                    self.stage = Stage::EnterModel;
                }
                KeyCode::Char('2') => {
                    self.pending_provider = Some(ProviderType::Gemini);
                    self.pending_endpoint = None;
                    self.input = Input::new(true, "AIza…");
                    self.status.clear();
                    self.stage = Stage::EnterApiKey;
                }
                KeyCode::Char('3') => {
                    self.pending_provider = Some(ProviderType::OpenAi);
                    self.pending_endpoint = std::env::var("OPENAI_BASE_URL").ok().and_then(|u| {
                        let trimmed = u.trim_end_matches('/').trim_end_matches("/v1");
                        if trimmed.is_empty() {
                            None
                        } else {
                            Some(trimmed.to_string())
                        }
                    });
                    self.input = Input::new(true, "sk-…");
                    self.status.clear();
                    self.stage = Stage::EnterApiKey;
                }
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    self.pending_provider = None;
                    self.session_agent_config = Some(ModelConfig::from_env());
                    self.status = "Using environment / Ollama defaults.".to_string();
                    self.stage = Stage::Choose;
                }
                KeyCode::Esc => {
                    self.status.clear();
                    self.stage = Stage::SelectMode;
                }
                _ => return KeyOutcome::NotHandled,
            },
            Stage::EnterApiKey => match self.input.handle_key(key) {
                InputAction::Ignored => {
                    if key.code == KeyCode::Esc {
                        self.status.clear();
                        self.stage = Stage::ConfigureProvider;
                        return KeyOutcome::Consumed;
                    }
                    return KeyOutcome::NotHandled;
                }
                InputAction::Submitted => {
                    let key_secret = self.input.take_secret();
                    if key_secret.expose_secret().trim().is_empty() {
                        self.status = "API key cannot be empty (or press Esc to go back).".into();
                        self.input = Input::new(true, "sk-…");
                    } else {
                        self.pending_api_key = Some(key_secret);
                        let provider = self.pending_provider.unwrap_or(ProviderType::OpenAi);
                        let default = Self::default_model_for(provider);
                        self.input = Input::new(false, default);
                        self.input.set_value(default);
                        self.status.clear();
                        self.stage = Stage::EnterModel;
                    }
                }
                InputAction::Consumed => {}
            },
            Stage::EnterModel => match self.input.handle_key(key) {
                InputAction::Ignored => {
                    if key.code == KeyCode::Esc {
                        self.status.clear();
                        if matches!(
                            self.pending_provider,
                            Some(ProviderType::Gemini | ProviderType::OpenAi)
                        ) {
                            self.input = Input::new(true, "sk-…");
                            self.stage = Stage::EnterApiKey;
                        } else {
                            self.stage = Stage::ConfigureProvider;
                        }
                        return KeyOutcome::Consumed;
                    }
                    return KeyOutcome::NotHandled;
                }
                InputAction::Submitted => {
                    let mut model = self.input.take_string();
                    let model = {
                        let trimmed = model.trim();
                        let chosen = if trimmed.is_empty() {
                            self.pending_provider
                                .map(Self::default_model_for)
                                .unwrap_or("llama3.2")
                                .to_string()
                        } else {
                            trimmed.to_string()
                        };
                        model.zeroize();
                        chosen
                    };
                    self.pending_model = model;
                    self.finish_agent_setup();
                }
                InputAction::Consumed => {}
            },
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
                                self.persist_agent_setup(wallet, &password);
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

    fn persist_agent_setup(&mut self, wallet: &WalletState, password: &SecretString) {
        let Some(provider) = self.pending_provider else {
            // Skip / env defaults — still try to write nothing; session config may exist.
            return;
        };
        if self.pending_model.is_empty() {
            return;
        }
        let file = match provider {
            ProviderType::Ollama => AgentFileConfig::ollama(&self.pending_model),
            ProviderType::Gemini => AgentFileConfig::gemini(&self.pending_model),
            ProviderType::OpenAi => {
                AgentFileConfig::openai(&self.pending_model, self.pending_endpoint.clone())
            }
        };
        let pending = PendingAgentSetup {
            file,
            api_key: self.pending_api_key.clone(),
        };
        let dir = profile_dir(wallet.path());
        if let Err(e) = pending.persist(&dir, Some(password)) {
            self.status = format!("Wallet created, but agent config save failed: {e}");
        }
        if let Ok(cfg) = pending.to_model_config() {
            self.session_agent_config = Some(cfg);
        }
    }
}

fn provider_label(provider: ProviderType) -> &'static str {
    match provider {
        ProviderType::Ollama => "Ollama",
        ProviderType::Gemini => "Gemini",
        ProviderType::OpenAi => "OpenAI",
    }
}
