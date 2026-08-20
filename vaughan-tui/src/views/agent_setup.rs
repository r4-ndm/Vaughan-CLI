//! Standalone AI provider / API-key setup (post-unlock or first-run).
//!
//! Same flow as welcome onboarding: pick provider → optional key → model → done.

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
use vaughan_core::core::WalletState;
use vaughan_provider::EventBus;
use zeroize::Zeroize;

use crate::app::{KeyOutcome, Screen};
use crate::input::{Input, InputAction};
use crate::views::{body_areas, labeled_input, status_paragraph};

enum Stage {
    ConfigureProvider,
    EnterApiKey,
    EnterModel,
}

/// Shown after unlock when AI mode needs provider/key configuration.
pub struct AgentSetupView {
    stage: Stage,
    input: Input,
    status: String,
    pending_provider: Option<ProviderType>,
    pending_endpoint: Option<String>,
    pending_model: String,
    pending_api_key: Option<SecretString>,
    /// Vault password borrowed from unlock so keys can be encrypted at rest.
    vault_password: Option<SecretString>,
    session_agent_config: Option<ModelConfig>,
}

impl AgentSetupView {
    /// Start setup; `vault_password` encrypts API keys into `agent.key.json`.
    pub fn new(vault_password: Option<SecretString>) -> Self {
        Self {
            stage: Stage::ConfigureProvider,
            input: Input::new(false, ""),
            status: "AI mode needs a provider. Pick one, or press s for Ollama defaults.".into(),
            pending_provider: None,
            pending_endpoint: None,
            pending_model: String::new(),
            pending_api_key: None,
            vault_password,
            session_agent_config: None,
        }
    }

    pub fn take_session_agent_config(&mut self) -> Option<ModelConfig> {
        self.session_agent_config.take()
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
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" AI Provider Setup ");

        match self.stage {
            Stage::ConfigureProvider => {
                let mode = wallet.operating_mode().display_label();
                let text = vec![
                    Line::from(format!("Mode: {mode}")),
                    Line::from("No usable agent API key / config found for this profile."),
                    Line::from(""),
                    Line::from("Choose your AI provider:"),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled("1 — Ollama (local)  ", Style::default().fg(Color::Green)),
                        Span::raw("No API key"),
                    ]),
                    Line::from(vec![
                        Span::styled("2 — Google Gemini  ", Style::default().fg(Color::Cyan)),
                        Span::raw("Paste API key next"),
                    ]),
                    Line::from(vec![
                        Span::styled(
                            "3 — OpenAI / compatible ",
                            Style::default().fg(Color::Yellow),
                        ),
                        Span::raw("OpenAI, OpenRouter, DeepSeek, …"),
                    ]),
                    Line::from(""),
                    Line::from("s — skip (Ollama / env defaults)"),
                ];
                frame.render_widget(
                    Paragraph::new(text).block(block).wrap(Wrap { trim: false }),
                    content,
                );
            }
            Stage::EnterApiKey => {
                let provider = self
                    .pending_provider
                    .map(provider_label)
                    .unwrap_or("provider");
                let label = format!("Paste your {provider} API key (masked)");
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
            Stage::ConfigureProvider => match key.code {
                KeyCode::Char('1') => {
                    self.pending_provider = Some(ProviderType::Ollama);
                    self.pending_api_key = None;
                    let default = Self::default_model_for(ProviderType::Ollama);
                    self.input = Input::new(false, default);
                    self.input.set_value(default);
                    self.status.clear();
                    self.stage = Stage::EnterModel;
                    KeyOutcome::Consumed
                }
                KeyCode::Char('2') => {
                    self.pending_provider = Some(ProviderType::Gemini);
                    self.input = Input::new(true, "AIza…");
                    self.status.clear();
                    self.stage = Stage::EnterApiKey;
                    KeyOutcome::Consumed
                }
                KeyCode::Char('3') => {
                    self.pending_provider = Some(ProviderType::OpenAi);
                    self.pending_endpoint = std::env::var("OPENAI_BASE_URL").ok().and_then(|u| {
                        let t = u.trim_end_matches('/').trim_end_matches("/v1");
                        if t.is_empty() {
                            None
                        } else {
                            Some(t.to_string())
                        }
                    });
                    self.input = Input::new(true, "sk-…");
                    self.status.clear();
                    self.stage = Stage::EnterApiKey;
                    KeyOutcome::Consumed
                }
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    let pending = PendingAgentSetup {
                        file: AgentFileConfig::ollama("llama3.2"),
                        api_key: None,
                    };
                    let dir = profile_dir(wallet.path());
                    let _ = pending.persist(&dir, self.vault_password.as_ref());
                    self.session_agent_config = pending.to_model_config().ok();
                    self.vault_password = None;
                    KeyOutcome::Navigate(Screen::Dashboard)
                }
                _ => KeyOutcome::NotHandled,
            },
            Stage::EnterApiKey => match self.input.handle_key(key) {
                InputAction::Ignored => {
                    if key.code == KeyCode::Esc {
                        self.stage = Stage::ConfigureProvider;
                        self.status.clear();
                        KeyOutcome::Consumed
                    } else {
                        KeyOutcome::NotHandled
                    }
                }
                InputAction::Submitted => {
                    let key_secret = self.input.take_secret();
                    if key_secret.expose_secret().trim().is_empty() {
                        self.status = "API key cannot be empty.".into();
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
                    KeyOutcome::Consumed
                }
                InputAction::Consumed => KeyOutcome::Consumed,
            },
            Stage::EnterModel => match self.input.handle_key(key) {
                InputAction::Ignored => {
                    if key.code == KeyCode::Esc {
                        self.stage = Stage::ConfigureProvider;
                        KeyOutcome::Consumed
                    } else {
                        KeyOutcome::NotHandled
                    }
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
                    self.finish(wallet)
                }
                InputAction::Consumed => KeyOutcome::Consumed,
            },
        }
    }

    fn finish(&mut self, wallet: &WalletState) -> KeyOutcome {
        let Some(provider) = self.pending_provider else {
            return KeyOutcome::Navigate(Screen::Dashboard);
        };
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
        if let Err(e) = pending.persist(&dir, self.vault_password.as_ref()) {
            self.status = format!("Save failed: {e}");
            return KeyOutcome::Consumed;
        }
        self.session_agent_config = pending.to_model_config().ok();
        self.vault_password = None;
        KeyOutcome::Navigate(Screen::Dashboard)
    }
}

fn provider_label(provider: ProviderType) -> &'static str {
    match provider {
        ProviderType::Ollama => "Ollama",
        ProviderType::Gemini => "Gemini",
        ProviderType::OpenAi => "OpenAI",
    }
}
