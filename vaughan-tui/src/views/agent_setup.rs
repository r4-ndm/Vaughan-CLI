//! Standalone AI provider / API-key setup (post-unlock or first-run).
//!
//! Same flow as welcome onboarding: pick provider → optional key → model → done.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};
use secrecy::{ExposeSecret, SecretString};
use tokio::runtime::Handle;
use vaughan_agent::{
    looks_like_openrouter_key, normalize_openai_base_url, profile_dir,
    validate_cursor_chat_endpoint, AgentFileConfig, ModelConfig, PendingAgentSetup, ProviderType,
    DEFAULT_GEMINI_MODEL, GEMINI_PRO_MODEL,
};
use vaughan_core::core::WalletState;
use vaughan_provider::EventBus;
use zeroize::Zeroize;

use crate::app::{KeyOutcome, Screen};
use crate::brand;
use crate::input::{Input, InputAction};
use crate::views::{body_areas, render_labeled_input, status_paragraph};

enum Stage {
    ConfigureProvider,
    EnterApiKey,
    /// OpenAI-compatible gateway base URL (required for Cursor).
    EnterEndpoint,
    /// Pick Gemini Flash vs Pro.
    ChooseGeminiModel,
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
            status: "Pick a provider (API key is encrypted with your vault password and restored on next login), or s to keep the saved config."
                .into(),
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
            ProviderType::Gemini => "gemini-3.5-flash",
            ProviderType::OpenAi => "gpt-4o-mini",
            ProviderType::Cursor => "composer-2",
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        let [content, status_area] = body_areas(area);

        match self.stage {
            Stage::ConfigureProvider => {
                let mode = wallet.operating_mode().display_label();
                let text = vec![
                    Line::from(format!("Mode: {mode}")),
                    Line::from("Configure your LLM (API keys are encrypted at rest and restored on unlock)."),
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
                    Line::from(vec![
                        Span::styled("4 — Cursor gateway   ", Style::default().fg(Color::Magenta)),
                        Span::raw("Key + OpenAI-compatible chat URL (not api.cursor.com)"),
                    ]),
                    Line::from(""),
                    Line::from("s — keep saved config (or Ollama / env defaults)"),
                ];
                let inner = brand::render_faded_box(
                    frame,
                    content,
                    Some(brand::fade_line(" AI Provider Setup ")),
                );
                frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
            }
            Stage::EnterApiKey => {
                let provider = self
                    .pending_provider
                    .map(provider_label)
                    .unwrap_or("provider");
                let label = format!("Paste your {provider} API key (masked)");
                render_labeled_input(frame, content, &label, &self.input, true);
            }
            Stage::EnterEndpoint => {
                let label = "Chat gateway base URL (OpenAI-compatible; Enter keeps current)";
                render_labeled_input(frame, content, label, &self.input, true);
            }
            Stage::ChooseGeminiModel => {
                let text = vec![
                    Line::from("Choose a Gemini API model:"),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled("1 — Gemini 3.5 Flash", Style::default().fg(Color::Cyan)),
                        Span::raw(format!("  ({DEFAULT_GEMINI_MODEL})")),
                    ]),
                    Line::from(vec![
                        Span::styled("2 — Gemini 3.5 Pro", Style::default().fg(Color::Yellow)),
                        Span::raw(format!("  ({GEMINI_PRO_MODEL})")),
                    ]),
                    Line::from(""),
                    Line::from("Esc — back to API key"),
                ];
                let inner = brand::render_faded_box(
                    frame,
                    content,
                    Some(brand::fade_line(" AI Provider Setup ")),
                );
                frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
            }
            Stage::EnterModel => {
                let placeholder = self
                    .pending_provider
                    .map(Self::default_model_for)
                    .unwrap_or("model");
                let label = format!("Model name (Enter for default: {placeholder})");
                render_labeled_input(frame, content, &label, &self.input, true);
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
                KeyCode::Char('4') => {
                    self.pending_provider = Some(ProviderType::Cursor);
                    self.pending_endpoint = std::env::var("CURSOR_BASE_URL").ok().and_then(|u| {
                        let t = normalize_openai_base_url(&u);
                        if t.is_empty() {
                            None
                        } else {
                            Some(t)
                        }
                    });
                    self.input = Input::new(true, "crsr_…");
                    self.status.clear();
                    self.stage = Stage::EnterApiKey;
                    KeyOutcome::Consumed
                }
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    // Keep existing agent.toml / encrypted key; re-hydrate session config.
                    let dir = profile_dir(wallet.path());
                    match vaughan_agent::resolve_model_config(&dir, self.vault_password.as_ref()) {
                        Ok(cfg) => {
                            self.session_agent_config = Some(cfg);
                        }
                        Err(_) => {
                            if vaughan_agent::load_file_config(&dir)
                                .ok()
                                .flatten()
                                .is_none()
                            {
                                let pending = PendingAgentSetup {
                                    file: AgentFileConfig::ollama("llama3.2"),
                                    api_key: None,
                                };
                                let _ = pending.persist(&dir, self.vault_password.as_ref());
                                self.session_agent_config = pending.to_model_config().ok();
                            }
                        }
                    }
                    self.vault_password = None;
                    if wallet.is_unlocked() {
                        KeyOutcome::Navigate(Screen::Agent)
                    } else {
                        KeyOutcome::Navigate(Screen::Dashboard)
                    }
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
                        if provider == ProviderType::Cursor {
                            let default = self
                                .pending_endpoint
                                .clone()
                                .unwrap_or_else(|| "http://127.0.0.1:8765".into());
                            self.input = Input::new(false, "http://127.0.0.1:8765");
                            self.input.set_value(&default);
                            self.status =
                                "Cursor needs a chat gateway URL — not api.cursor.com.".into();
                            self.stage = Stage::EnterEndpoint;
                        } else if provider == ProviderType::Gemini {
                            self.status.clear();
                            self.stage = Stage::ChooseGeminiModel;
                        } else {
                            if let Some(ref key) = self.pending_api_key {
                                if looks_like_openrouter_key(key.expose_secret()) {
                                    self.pending_endpoint =
                                        Some(vaughan_agent::OPENROUTER_BASE_URL.to_string());
                                    self.status =
                                        "Detected OpenRouter key — using openrouter.ai/api.".into();
                                }
                            }
                            let default = if self
                                .pending_endpoint
                                .as_deref()
                                .is_some_and(|u| u.contains("openrouter"))
                            {
                                "openrouter/free"
                            } else {
                                Self::default_model_for(provider)
                            };
                            self.input = Input::new(false, default);
                            self.input.set_value(default);
                            self.stage = Stage::EnterModel;
                        }
                    }
                    KeyOutcome::Consumed
                }
                InputAction::Consumed => KeyOutcome::Consumed,
            },
            Stage::EnterEndpoint => match self.input.handle_key(key) {
                InputAction::Ignored => {
                    if key.code == KeyCode::Esc {
                        self.input = Input::new(true, "crsr_…");
                        self.status.clear();
                        self.stage = Stage::EnterApiKey;
                        KeyOutcome::Consumed
                    } else {
                        KeyOutcome::NotHandled
                    }
                }
                InputAction::Submitted => {
                    let mut raw = self.input.take_string();
                    let candidate = {
                        let trimmed = raw.trim();
                        let chosen = if trimmed.is_empty() {
                            self.pending_endpoint
                                .clone()
                                .unwrap_or_else(|| "http://127.0.0.1:8765".into())
                        } else {
                            trimmed.to_string()
                        };
                        raw.zeroize();
                        chosen
                    };
                    match validate_cursor_chat_endpoint(&candidate) {
                        Ok(url) => {
                            self.pending_endpoint = Some(url);
                            let default = Self::default_model_for(ProviderType::Cursor);
                            self.input = Input::new(false, default);
                            self.input.set_value(default);
                            self.status.clear();
                            self.stage = Stage::EnterModel;
                        }
                        Err(e) => {
                            self.status = e.to_string();
                            self.input = Input::new(false, "http://127.0.0.1:8765");
                            self.input.set_value(&candidate);
                        }
                    }
                    KeyOutcome::Consumed
                }
                InputAction::Consumed => KeyOutcome::Consumed,
            },
            Stage::ChooseGeminiModel => match key.code {
                KeyCode::Char('1') => {
                    self.pending_model = DEFAULT_GEMINI_MODEL.to_string();
                    self.finish(wallet)
                }
                KeyCode::Char('2') => {
                    self.pending_model = GEMINI_PRO_MODEL.to_string();
                    self.finish(wallet)
                }
                KeyCode::Esc => {
                    self.input = Input::new(true, "AIza…");
                    self.status.clear();
                    self.stage = Stage::EnterApiKey;
                    KeyOutcome::Consumed
                }
                _ => KeyOutcome::NotHandled,
            },
            Stage::EnterModel => match self.input.handle_key(key) {
                InputAction::Ignored => {
                    if key.code == KeyCode::Esc {
                        if self.pending_provider == Some(ProviderType::Cursor) {
                            let default = self
                                .pending_endpoint
                                .clone()
                                .unwrap_or_else(|| "http://127.0.0.1:8765".into());
                            self.input = Input::new(false, "http://127.0.0.1:8765");
                            self.input.set_value(&default);
                            self.stage = Stage::EnterEndpoint;
                        } else {
                            self.stage = Stage::ConfigureProvider;
                        }
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
            ProviderType::Cursor => {
                let mut cfg = AgentFileConfig::cursor(&self.pending_model);
                cfg.endpoint_url = self.pending_endpoint.clone();
                cfg
            }
        };
        let pending = PendingAgentSetup {
            file,
            api_key: self.pending_api_key.clone(),
        };
        let dir = profile_dir(wallet.path());
        if self.pending_api_key.is_some() && self.vault_password.is_none() {
            self.status =
                "API key kept for this session only — unlock again to save it encrypted.".into();
            self.session_agent_config = pending.to_model_config().ok();
            self.vault_password = None;
            return if wallet.is_unlocked() {
                KeyOutcome::Navigate(Screen::Agent)
            } else {
                KeyOutcome::Navigate(Screen::Dashboard)
            };
        }
        if let Err(e) = pending.persist(&dir, self.vault_password.as_ref()) {
            self.status = format!("Save failed: {e}");
            return KeyOutcome::Consumed;
        }
        self.session_agent_config = pending.to_model_config().ok();
        self.vault_password = None;
        // Prefer Agent chat when the vault is already unlocked (e.g. `/provider`).
        if wallet.is_unlocked() {
            KeyOutcome::Navigate(Screen::Agent)
        } else {
            KeyOutcome::Navigate(Screen::Dashboard)
        }
    }
}

fn provider_label(provider: ProviderType) -> &'static str {
    match provider {
        ProviderType::Ollama => "Ollama",
        ProviderType::Gemini => "Gemini",
        ProviderType::OpenAi => "OpenAI",
        ProviderType::Cursor => "Cursor",
    }
}
