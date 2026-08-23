//! AI Agent View: Multi-mode interactive terminal interface.
//!
//! Handles:
//! - Human-Only mode cold-storage isolation notice.
//! - AI Assisted chat REPL with streaming LLM replies and tool activity.
//! - In-chat `/model` picker (OpenCode-style) to swap models without leaving chat.
//! - Session portfolio overlay (`p` / `/portfolio`) for native + imported ERC-20s.
//! - Ground-truth transaction proposal review modal with independent bytecode rendering.
//! - Degen mode autonomous trader status and emergency stop.
//! - Esc cancels an in-flight LLM turn.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, watch};
use vaughan_agent::proposal::TxProposal;
use vaughan_agent::tools::{
    default_assist_registry, default_degen_registry, ToolContext, ToolRegistry,
};
use vaughan_agent::{
    build_system_prompt, create_llm_client, load_file_config, models_for_provider,
    normalize_gemini_model, parse_model_ref, provider_id, run_assist_turn, save_file_config,
    skills_for_mode, AgentFileConfig, AgentSessionContext, ChatMessage, ChatUiEvent, DegenTrader,
    LlmClient, ModelConfig, ProviderType, SkillKind,
};
use vaughan_core::chains::Balance;
use vaughan_core::core::profile::OperatingMode;
use vaughan_core::error::WalletError;

use crate::app::{KeyOutcome, Screen};
use crate::brand;
use crate::input::Input;
use crate::jobs::{spinner_frame, UiJob};
use crate::views::{body_areas, render_labeled_input, status_paragraph};

/// OpenCode-inspired `/model` overlay: filter + ↑/↓ list for the active provider.
struct ModelPicker {
    filter: String,
    selected: usize,
}

/// In-session portfolio panel (native + imported ERC-20 balances).
struct PortfolioOverlay {
    assets: Vec<Balance>,
    selected: usize,
    loading: bool,
    error: Option<String>,
}

impl PortfolioOverlay {
    fn loading() -> Self {
        Self {
            assets: Vec::new(),
            selected: 0,
            loading: true,
            error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentMessage {
    User(String),
    Assistant(String),
    ToolCall { name: String, args: String },
    ToolResult { name: String, result: String },
    System(String),
}

struct StreamingJob {
    cancel_tx: watch::Sender<bool>,
    event_rx: mpsc::UnboundedReceiver<ChatUiEvent>,
    /// Index in `history` of the live assistant bubble being filled by deltas.
    assistant_idx: Option<usize>,
}

impl Drop for StreamingJob {
    fn drop(&mut self) {
        let _ = self.cancel_tx.send(true);
    }
}

/// Agent View state.
pub struct AgentView {
    input: Input,
    history: Vec<AgentMessage>,
    active_proposal: Option<TxProposal>,
    status: String,
    scroll_offset: usize,
    /// When true (default), the chat view sticks to the latest lines.
    follow_tail: bool,
    registry: ToolRegistry,
    config: ModelConfig,
    llm: Arc<dyn LlmClient>,
    llm_history: Vec<ChatMessage>,
    job: Option<StreamingJob>,
    operating_mode: OperatingMode,
    profile_dir: Option<std::path::PathBuf>,
    /// Status chrome: `ollama/llama3.2 · skills: 2 must`.
    agent_badge: String,
    /// Burner trader when running Degen Bot (None in Assist).
    degen: Option<Arc<DegenTrader>>,
    /// Connected wallet + network facts for the system prompt.
    session: AgentSessionContext,
    /// In-chat model picker (`/model`).
    model_picker: Option<ModelPicker>,
    /// In-session portfolio (`p` / `/portfolio`).
    portfolio: Option<PortfolioOverlay>,
    /// Spinner tick while portfolio is refreshing.
    tick: u64,
}

impl Default for AgentView {
    fn default() -> Self {
        Self::with_session(
            ModelConfig::from_env(),
            OperatingMode::AiAssisted,
            None,
            None,
            AgentSessionContext {
                active_address: None,
                chain_id: 943,
                network_id: "pulsechain-testnet-v4".into(),
                network_name: "PulseChain Testnet v4".into(),
                native_symbol: "tPLS".into(),
                is_testnet: true,
                max_position_pct: None,
                max_slippage_bps: None,
            },
        )
    }
}

impl AgentView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build the agent view against an explicit model + mode (loads skills).
    pub fn with_config(config: ModelConfig) -> Self {
        Self::with_session(
            config,
            OperatingMode::AiAssisted,
            None,
            None,
            AgentSessionContext {
                active_address: None,
                chain_id: 369,
                network_id: "pulsechain".into(),
                network_name: "PulseChain".into(),
                native_symbol: "PLS".into(),
                is_testnet: false,
                max_position_pct: None,
                max_slippage_bps: None,
            },
        )
    }

    /// Full session wiring: LLM config, operating mode, optional profile skills dir,
    /// optional Degen burner trader, and live wallet/network context for the prompt.
    pub fn with_session(
        config: ModelConfig,
        mode: OperatingMode,
        profile_dir: Option<&std::path::Path>,
        degen: Option<Arc<DegenTrader>>,
        session: AgentSessionContext,
    ) -> Self {
        let must_count = skills_for_mode(mode, profile_dir)
            .iter()
            .filter(|s| s.kind == SkillKind::Must)
            .count();
        let agent_badge = format!(
            "{}/{} · skills: {must_count} must",
            provider_id(config.provider),
            config.model_name
        );
        let llm = create_llm_client(config.clone()).expect("LLM client construction is infallible");
        let system = build_system_prompt(mode, profile_dir, Some(&session));

        let registry = match (mode, degen.as_ref()) {
            (OperatingMode::DegenTrader, Some(trader)) => {
                default_degen_registry(Arc::clone(trader))
            }
            _ => default_assist_registry(),
        };

        let mut history = vec![AgentMessage::System(format!(
            "Vaughan Agent ready ({agent_badge}, mode: {}). Esc cancels streams. Type /model to switch models.",
            mode.badge()
        ))];
        if let Some(ref addr) = session.active_address {
            history.push(AgentMessage::System(format!(
                "Connected wallet: {addr} · {} (chain {}) · native {}.",
                session.network_name, session.chain_id, session.native_symbol
            )));
        }
        if mode == OperatingMode::DegenTrader {
            history.push(AgentMessage::System(
                "⚠ WARNING: Isolated wallet only — a fraction of your funds. Never your main vault."
                    .into(),
            ));
            if let Some(ref t) = degen {
                let dry = if t.is_dry_run() { " · dry-run" } else { "" };
                history.push(AgentMessage::System(format!(
                    "Degen Bot: execute_degen_swap on {:#x}{dry} (max {}% balance / {} bps slippage).",
                    t.address(),
                    t.circuit_breaker().config().max_position_pct,
                    t.circuit_breaker().config().max_slippage_bps,
                )));
            } else {
                history.push(AgentMessage::System(
                    "Degen Bot: signer unavailable — unlock required to execute.".into(),
                ));
            }
        }

        let input_placeholder = if mode == OperatingMode::DegenTrader {
            "⚠ Isolated funds only — ask the bot…"
        } else {
            "Ask the advisor…"
        };

        Self {
            input: Input::new(false, input_placeholder),
            history,
            active_proposal: None,
            status: String::new(),
            scroll_offset: 0,
            follow_tail: true,
            registry,
            config,
            llm,
            llm_history: vec![system],
            job: None,
            operating_mode: mode,
            profile_dir: profile_dir.map(|p| p.to_path_buf()),
            agent_badge,
            degen,
            session,
            model_picker: None,
            portfolio: None,
            tick: 0,
        }
    }

    /// Drive the portfolio loading spinner.
    pub fn set_tick(&mut self, tick: u64) {
        self.tick = tick;
    }

    /// Apply a background [`UiJob::RefreshAssets`] result to the open portfolio.
    pub fn apply_portfolio(&mut self, result: Result<Vec<Balance>, WalletError>) {
        let Some(panel) = self.portfolio.as_mut() else {
            return;
        };
        panel.loading = false;
        match result {
            Ok(assets) => {
                panel.assets = assets;
                if panel.selected >= panel.assets.len() {
                    panel.selected = panel.assets.len().saturating_sub(1);
                }
                panel.error = None;
            }
            Err(e) => {
                panel.error = Some(e.user_message());
            }
        }
    }

    fn open_portfolio(&mut self) -> KeyOutcome {
        self.portfolio = Some(PortfolioOverlay::loading());
        self.status.clear();
        KeyOutcome::StartJob(UiJob::RefreshAssets)
    }

    /// Status line: live job status, or overlay hints; empty when idle.
    fn footer_text(&self) -> String {
        if !self.status.is_empty() {
            return self.status.clone();
        }
        if self.portfolio.is_some() {
            return "Portfolio — ↑↓ · r refresh · Esc close".into();
        }
        if self.model_picker.is_some() {
            return "Model picker — ↑↓ · type to filter · Enter · Esc cancel".into();
        }
        String::new()
    }

    /// Active session LLM settings (for App to keep in sync after `/model`).
    pub fn model_config(&self) -> &ModelConfig {
        &self.config
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
    }

    fn refresh_badge(&mut self) {
        let must_count = skills_for_mode(self.operating_mode, self.profile_dir.as_deref())
            .iter()
            .filter(|s| s.kind == SkillKind::Must)
            .count();
        self.agent_badge = format!(
            "{}/{} · skills: {must_count} must",
            provider_id(self.config.provider),
            self.config.model_name
        );
    }

    fn open_model_picker(&mut self) {
        let selected = models_for_provider(self.config.provider)
            .iter()
            .position(|m| m.id == self.config.model_name)
            .unwrap_or(0);
        self.model_picker = Some(ModelPicker {
            filter: String::new(),
            selected,
        });
        self.status =
            "Select model (↑/↓, type to filter, Enter) — Esc cancels. Custom ids allowed.".into();
    }

    /// Visible picker rows: `(model_id, label)`.
    fn picker_rows(&self) -> Vec<(String, String)> {
        let Some(picker) = &self.model_picker else {
            return Vec::new();
        };
        let filter = picker.filter.trim().to_ascii_lowercase();
        let mut rows: Vec<(String, String)> = models_for_provider(self.config.provider)
            .iter()
            .filter(|m| {
                if filter.is_empty() {
                    return true;
                }
                m.id.to_ascii_lowercase().contains(&filter)
                    || m.label.to_ascii_lowercase().contains(&filter)
            })
            .map(|m| (m.id.to_string(), m.label.to_string()))
            .collect();

        let raw = picker.filter.trim();
        if !raw.is_empty() && !rows.iter().any(|(id, _)| id.eq_ignore_ascii_case(raw)) {
            rows.insert(0, (raw.to_string(), "custom".into()));
        }
        if rows.is_empty() && !raw.is_empty() {
            rows.push((raw.to_string(), "custom".into()));
        }
        rows
    }

    fn apply_model(&mut self, model_id: &str) {
        let model = if self.config.provider == ProviderType::Gemini {
            normalize_gemini_model(model_id)
        } else {
            model_id.trim().to_string()
        };
        if model.is_empty() {
            self.history
                .push(AgentMessage::System("Model id cannot be empty.".into()));
            return;
        }
        self.config.model_name = model;
        self.llm =
            create_llm_client(self.config.clone()).expect("LLM client construction is infallible");
        self.refresh_badge();
        self.persist_model();
        self.model_picker = None;
        self.status.clear();
        self.history.push(AgentMessage::System(format!(
            "Model set to {}/{}. Conversation history kept.",
            provider_id(self.config.provider),
            self.config.model_name
        )));
    }

    fn persist_model(&self) {
        let Some(dir) = &self.profile_dir else {
            return;
        };
        let file = match load_file_config(dir) {
            Ok(Some(mut existing)) => {
                existing.model = self.config.model_name.clone();
                existing
            }
            _ => match self.config.provider {
                ProviderType::Ollama => AgentFileConfig::ollama(&self.config.model_name),
                ProviderType::Gemini => AgentFileConfig::gemini(&self.config.model_name),
                ProviderType::OpenAi => AgentFileConfig::openai(
                    &self.config.model_name,
                    Some(self.config.endpoint_url.clone()),
                ),
                ProviderType::Cursor => {
                    let mut cfg = AgentFileConfig::cursor(&self.config.model_name);
                    cfg.endpoint_url = Some(self.config.endpoint_url.clone());
                    cfg
                }
            },
        };
        if let Err(e) = save_file_config(dir, &file) {
            // Non-fatal — session switch still applies.
            tracing::warn!("failed to persist model to agent.toml: {e}");
        }
    }

    fn handle_model_command(&mut self, rest: &str) -> KeyOutcome {
        let rest = rest.trim();
        if rest.is_empty() {
            self.open_model_picker();
            return KeyOutcome::Consumed;
        }
        let Some((provider_override, model_id)) = parse_model_ref(rest) else {
            self.history.push(AgentMessage::System(
                "Usage: /model  |  /model <id>  |  /model provider/id".into(),
            ));
            return KeyOutcome::Consumed;
        };
        if let Some(requested) = provider_override {
            if requested != self.config.provider {
                self.history.push(AgentMessage::System(format!(
                    "Provider switch to `{}` needs API key / endpoint setup — type /provider. \
                     Same-provider: /model <id> (current: {}).",
                    provider_id(requested),
                    provider_id(self.config.provider)
                )));
                return KeyOutcome::Consumed;
            }
        }
        self.apply_model(&model_id);
        KeyOutcome::Consumed
    }

    pub fn active_proposal(&self) -> Option<&TxProposal> {
        self.active_proposal.as_ref()
    }

    pub fn take_active_proposal(&mut self) -> Option<TxProposal> {
        self.active_proposal.take()
    }

    pub fn add_message(&mut self, msg: AgentMessage) {
        self.history.push(msg);
    }

    /// True while an LLM turn is running (streaming or tool loop).
    pub fn is_busy(&self) -> bool {
        self.job.is_some()
    }

    /// Drain async chat events. Call once per UI tick.
    pub fn poll(&mut self) {
        let Some(job) = self.job.as_mut() else {
            return;
        };

        let mut finished = false;
        while let Ok(event) = job.event_rx.try_recv() {
            match event {
                ChatUiEvent::Status(s) => {
                    self.status = s;
                }
                ChatUiEvent::Delta(delta) => {
                    if let Some(idx) = job.assistant_idx {
                        if let Some(AgentMessage::Assistant(buf)) = self.history.get_mut(idx) {
                            buf.push_str(&delta);
                        }
                    } else {
                        self.history.push(AgentMessage::Assistant(delta));
                        job.assistant_idx = Some(self.history.len() - 1);
                    }
                }
                ChatUiEvent::ToolCall { name, args } => {
                    job.assistant_idx = None;
                    self.history.push(AgentMessage::ToolCall { name, args });
                }
                ChatUiEvent::ToolResult { name, result } => {
                    self.history.push(AgentMessage::ToolResult { name, result });
                }
                ChatUiEvent::Proposal(prop) => {
                    self.active_proposal = Some(*prop);
                }
                ChatUiEvent::Finished { history } => {
                    self.llm_history = history;
                    self.status.clear();
                    finished = true;
                }
                ChatUiEvent::Cancelled { history } => {
                    self.llm_history = history;
                    self.history
                        .push(AgentMessage::System("Turn cancelled.".to_string()));
                    self.status.clear();
                    finished = true;
                }
                ChatUiEvent::Error { message, history } => {
                    self.llm_history = history;
                    self.history
                        .push(AgentMessage::Assistant(format!("Error: {message}")));
                    self.status.clear();
                    finished = true;
                }
            }
        }

        if finished {
            self.job = None;
        }
    }

    fn cancel_stream(&mut self) {
        if let Some(job) = &self.job {
            let _ = job.cancel_tx.send(true);
            self.status = "Cancelling…".to_string();
        }
        if let Some(ref trader) = self.degen {
            trader.emergency_stop("user Esc / kill switch");
        }
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        mode: OperatingMode,
        context: &ToolContext,
        handle: &Handle,
    ) -> KeyOutcome {
        if mode == OperatingMode::HumanOnly {
            if key.code == KeyCode::Esc {
                return KeyOutcome::NotHandled;
            }
            return KeyOutcome::Consumed;
        }

        // Esc cancels an in-flight turn instead of leaving the screen.
        if key.code == KeyCode::Esc && self.job.is_some() {
            self.cancel_stream();
            return KeyOutcome::Consumed;
        }

        // If there is an active proposal pending approval:
        if let Some(proposal) = &self.active_proposal {
            match key.code {
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    self.status =
                        format!("Proposal {} approved for broadcast.", proposal.proposal_id);
                    return KeyOutcome::Consumed;
                }
                KeyCode::Char('d') | KeyCode::Char('D') | KeyCode::Esc => {
                    self.status = format!("Proposal {} denied.", proposal.proposal_id);
                    self.active_proposal = None;
                    return KeyOutcome::Consumed;
                }
                _ => return KeyOutcome::Consumed,
            }
        }

        // Ignore input while a turn is streaming (except Esc handled above).
        if self.job.is_some() {
            return KeyOutcome::Consumed;
        }

        // `/model` picker overlay takes over keys until confirmed or Esc.
        if self.model_picker.is_some() {
            return self.handle_model_picker_key(key);
        }

        // Portfolio overlay: Esc closes; r refreshes; ↑↓ select.
        if self.portfolio.is_some() {
            return self.handle_portfolio_key(key);
        }

        match key.code {
            KeyCode::Esc => KeyOutcome::NotHandled,
            KeyCode::Up => {
                self.follow_tail = false;
                self.scroll_offset = self.scroll_offset.saturating_add(1);
                KeyOutcome::Consumed
            }
            KeyCode::Down => {
                if self.scroll_offset == 0 {
                    self.follow_tail = true;
                } else {
                    self.scroll_offset = self.scroll_offset.saturating_sub(1);
                    if self.scroll_offset == 0 {
                        self.follow_tail = true;
                    }
                }
                KeyOutcome::Consumed
            }
            KeyCode::Enter => {
                let prompt = self.input.value().to_string();
                self.input.set_value("");
                if prompt.trim().is_empty() {
                    KeyOutcome::Consumed
                } else {
                    self.execute_prompt(prompt.trim(), context, handle)
                }
            }
            // Ctrl+letter: chrome nav / portfolio even while typing in the prompt.
            KeyCode::Char('p') | KeyCode::Char('P') if is_ctrl_only(key) => self.open_portfolio(),
            _ if is_ctrl_chrome_hotkey(key) => KeyOutcome::NotHandled,
            // Empty prompt: bare `p` opens portfolio; other chrome keys bubble up.
            KeyCode::Char('p') | KeyCode::Char('P') if self.input.value().is_empty() => {
                self.open_portfolio()
            }
            _ if self.input.value().is_empty() && is_chrome_hotkey(key) => KeyOutcome::NotHandled,
            _ => {
                self.input.handle_key(key);
                KeyOutcome::Consumed
            }
        }
    }

    fn handle_portfolio_key(&mut self, key: KeyEvent) -> KeyOutcome {
        match key.code {
            KeyCode::Esc => {
                self.portfolio = None;
                self.status.clear();
                KeyOutcome::Consumed
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                if let Some(panel) = self.portfolio.as_mut() {
                    panel.loading = true;
                    panel.error = None;
                }
                KeyOutcome::StartJob(UiJob::RefreshAssets)
            }
            KeyCode::Up => {
                if let Some(panel) = self.portfolio.as_mut() {
                    if panel.selected > 0 {
                        panel.selected -= 1;
                    }
                }
                KeyOutcome::Consumed
            }
            KeyCode::Down => {
                if let Some(panel) = self.portfolio.as_mut() {
                    if !panel.assets.is_empty() && panel.selected + 1 < panel.assets.len() {
                        panel.selected += 1;
                    }
                }
                KeyOutcome::Consumed
            }
            _ => KeyOutcome::Consumed,
        }
    }

    fn handle_model_picker_key(&mut self, key: KeyEvent) -> KeyOutcome {
        let rows = self.picker_rows();
        let row_count = rows.len();

        match key.code {
            KeyCode::Esc => {
                self.model_picker = None;
                self.status.clear();
                KeyOutcome::Consumed
            }
            KeyCode::Up => {
                if let Some(p) = self.model_picker.as_mut() {
                    if p.selected > 0 {
                        p.selected -= 1;
                    }
                }
                KeyOutcome::Consumed
            }
            KeyCode::Down => {
                if let Some(p) = self.model_picker.as_mut() {
                    if row_count > 0 && p.selected + 1 < row_count {
                        p.selected += 1;
                    }
                }
                KeyOutcome::Consumed
            }
            KeyCode::Enter => {
                if row_count == 0 {
                    self.history.push(AgentMessage::System(
                        "No models match — type a custom id or Esc to cancel.".into(),
                    ));
                    return KeyOutcome::Consumed;
                }
                let selected = self
                    .model_picker
                    .as_ref()
                    .map(|p| p.selected.min(row_count - 1))
                    .unwrap_or(0);
                let model_id = rows[selected].0.clone();
                self.apply_model(&model_id);
                KeyOutcome::Consumed
            }
            KeyCode::Backspace => {
                if let Some(p) = self.model_picker.as_mut() {
                    p.filter.pop();
                    p.selected = 0;
                }
                KeyOutcome::Consumed
            }
            KeyCode::Char(c) if !c.is_control() => {
                if let Some(p) = self.model_picker.as_mut() {
                    p.filter.push(c);
                    p.selected = 0;
                }
                KeyOutcome::Consumed
            }
            _ => KeyOutcome::Consumed,
        }
    }

    fn execute_prompt(
        &mut self,
        prompt: &str,
        context: &ToolContext,
        handle: &Handle,
    ) -> KeyOutcome {
        self.history.push(AgentMessage::User(prompt.to_string()));
        self.follow_tail = true;
        self.scroll_offset = 0;

        let tokens: Vec<&str> = prompt.split_whitespace().collect();
        if tokens.is_empty() {
            return KeyOutcome::Consumed;
        }

        let cmd_owned = tokens[0].to_lowercase();
        let cmd = cmd_owned.strip_prefix('/').unwrap_or(cmd_owned.as_str());
        match cmd {
            "help" => {
                self.history.push(AgentMessage::Assistant(
                    "Commands:\n\
                     - p / /portfolio: Session portfolio (native + imported tokens)\n\
                     - /model [id]: Open model picker or set model (OpenCode-style)\n\
                     - /provider: Reconfigure LLM provider / API key\n\
                     - balance [0x...]: Check balance\n\
                     - inspect <0x...>: Fingerprint contract ABI & selectors\n\
                     - transfer <0x...> <amount_wei>: Propose native transfer (Assist)\n\
                     - clear: Clear history\n\
                     Or type a free-form question — in Degen Bot the model can call \
                     execute_degen_swap (breaker-gated); Assist uses propose_* + [a]/[d]."
                        .to_string(),
                ));
                KeyOutcome::Consumed
            }
            "portfolio" | "assets" => self.open_portfolio(),
            "clear" => {
                self.history.clear();
                self.llm_history = vec![build_system_prompt(
                    self.operating_mode,
                    self.profile_dir.as_deref(),
                    Some(&self.session),
                )];
                self.history
                    .push(AgentMessage::System("History cleared.".to_string()));
                self.follow_tail = true;
                self.scroll_offset = 0;
                KeyOutcome::Consumed
            }
            "model" | "models" => {
                let rest = prompt
                    .split_whitespace()
                    .skip(1)
                    .collect::<Vec<_>>()
                    .join(" ");
                self.handle_model_command(&rest)
            }
            "provider" | "connect" => {
                self.history
                    .push(AgentMessage::System("Opening provider setup…".into()));
                KeyOutcome::Navigate(Screen::AgentSetup)
            }
            "balance" => {
                self.cmd_balance(tokens, context, handle);
                KeyOutcome::Consumed
            }
            "inspect" => {
                self.cmd_inspect(tokens, context, handle);
                KeyOutcome::Consumed
            }
            "transfer" => {
                self.cmd_transfer(tokens, context, handle);
                KeyOutcome::Consumed
            }
            _ => {
                self.start_chat_turn(prompt, context, handle);
                KeyOutcome::Consumed
            }
        }
    }

    fn start_chat_turn(&mut self, prompt: &str, context: &ToolContext, handle: &Handle) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = watch::channel(false);

        let client = Arc::clone(&self.llm);
        let registry = self.registry.clone();
        let ctx = context.clone();
        let mut history = self.llm_history.clone();
        let user_text = prompt.to_string();

        self.job = Some(StreamingJob {
            cancel_tx,
            event_rx,
            assistant_idx: None,
        });
        self.status = format!("thinking ({})…", self.llm.name());

        // The TUI thread is outside the runtime — spawn on the app Handle.
        handle.spawn(async move {
            let _ = run_assist_turn(
                &mut history,
                client,
                &registry,
                &ctx,
                user_text,
                event_tx,
                cancel_rx,
            )
            .await;
        });
    }

    fn cmd_balance(&mut self, tokens: Vec<&str>, context: &ToolContext, handle: &Handle) {
        let account = if tokens.len() > 1 {
            tokens[1].to_string()
        } else if let Some(addr) = context.active_address {
            format!("{addr:#x}")
        } else {
            self.history.push(AgentMessage::Assistant(
                "Error: No account address provided".to_string(),
            ));
            return;
        };

        let reg = self.registry.clone();
        let ctx = context.clone();
        let acc = account.clone();

        let res = handle.block_on(async move {
            reg.execute(
                "get_balance",
                serde_json::json!({ "account_address": acc }),
                &ctx,
            )
            .await
        });

        match res {
            Ok(val) => {
                let bal = val["balance_wei"].as_str().unwrap_or("0");
                self.history.push(AgentMessage::Assistant(format!(
                    "Account: {account}\nBalance: {bal} wei"
                )));
            }
            Err(e) => {
                self.history.push(AgentMessage::Assistant(format!(
                    "Error querying balance: {e}"
                )));
            }
        }
    }

    fn cmd_inspect(&mut self, tokens: Vec<&str>, context: &ToolContext, handle: &Handle) {
        if tokens.len() < 2 {
            self.history.push(AgentMessage::Assistant(
                "Usage: inspect <0xAddress>".to_string(),
            ));
            return;
        }
        let target = tokens[1].to_string();
        let reg = self.registry.clone();
        let ctx = context.clone();
        let target_addr = target.clone();
        let res = handle.block_on(async move {
            reg.execute(
                "inspect_contract",
                serde_json::json!({ "address": target_addr }),
                &ctx,
            )
            .await
        });

        match res {
            Ok(val) => {
                let summary = vaughan_agent::summarize_tool_json("inspect_contract", &val);
                self.history.push(AgentMessage::Assistant(format!(
                    "Contract Inspection for {target}: {summary}"
                )));
            }
            Err(e) => {
                self.history.push(AgentMessage::Assistant(format!(
                    "Error inspecting contract: {e}"
                )));
            }
        }
    }

    fn cmd_transfer(&mut self, tokens: Vec<&str>, context: &ToolContext, handle: &Handle) {
        if tokens.len() < 3 {
            self.history.push(AgentMessage::Assistant(
                "Usage: transfer <0xRecipient> <amount_wei>".to_string(),
            ));
            return;
        }
        let recipient = tokens[1].to_string();
        let amount = tokens[2].to_string();
        let reg = self.registry.clone();
        let ctx = context.clone();

        let res = handle.block_on(async move {
            reg.execute(
                "propose_transfer",
                serde_json::json!({
                    "recipient": recipient,
                    "amount": amount,
                    "explanation": format!("User requested transfer of {amount} wei to {recipient}")
                }),
                &ctx,
            )
            .await
        });

        match res {
            Ok(val) => match serde_json::from_value::<TxProposal>(val) {
                Ok(prop) => {
                    self.history.push(AgentMessage::Assistant(format!(
                        "Drafted proposal: {}\nReview details in the confirmation modal below.",
                        prop.proposal_id
                    )));
                    self.active_proposal = Some(prop);
                }
                Err(e) => {
                    self.history.push(AgentMessage::Assistant(format!(
                        "Error decoding proposal: {e}"
                    )));
                }
            },
            Err(e) => {
                self.history.push(AgentMessage::Assistant(format!(
                    "Error creating proposal: {e}"
                )));
            }
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, mode: OperatingMode) {
        let [body, status_rect] = body_areas(area);

        if mode == OperatingMode::HumanOnly {
            let p = Paragraph::new(vec![
                Line::from(Span::styled(
                    "HUMAN PURIST MODE (Cold Storage)",
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from("AI agent subsystem is completely deactivated and barred from running."),
                Line::from("Zero network requests to LLMs are made, and no autonomous tooling is registered."),
                Line::from(""),
                Line::from(Span::styled(
                    "Press Esc to return to the Dashboard.",
                    Style::default().fg(Color::DarkGray),
                )),
            ]);
            let inner =
                brand::render_faded_box(frame, body, Some(brand::fade_line(" Agent Subsystem ")));
            frame.render_widget(p, inner);
            frame.render_widget(status_paragraph(&self.footer_text()), status_rect);
            return;
        }

        let chunks = if self.active_proposal.is_some() {
            Layout::vertical([
                Constraint::Min(4),
                Constraint::Length(10),
                Constraint::Length(3),
            ])
            .split(body)
        } else if self.model_picker.is_some() || self.portfolio.is_some() {
            Layout::vertical([
                Constraint::Min(3),
                Constraint::Length(12),
                Constraint::Length(3),
            ])
            .split(body)
        } else {
            Layout::vertical([Constraint::Min(4), Constraint::Length(3)]).split(body)
        };

        let chat_area = chunks[0];
        let inner_width = chat_area.width.saturating_sub(2) as usize;
        let inner_height = chat_area.height.saturating_sub(2) as usize;
        let chat_lines = build_chat_lines(&self.history, inner_width);
        let max_scroll = chat_lines.len().saturating_sub(inner_height.max(1));
        let scroll_y = if self.follow_tail {
            max_scroll
        } else {
            max_scroll.saturating_sub(self.scroll_offset.min(max_scroll))
        };

        let title = if self.job.is_some() {
            format!(" Agent · {} · streaming (Esc) ", self.agent_badge)
        } else {
            format!(" Agent · {} ", self.agent_badge)
        };
        let chat_inner = brand::render_faded_box(frame, chat_area, Some(brand::fade_line(&title)));
        let chat = Paragraph::new(chat_lines).scroll((scroll_y as u16, 0));
        frame.render_widget(chat, chat_inner);
        if let Some(prop) = &self.active_proposal {
            let sim_tag = if prop.simulation_success {
                Span::styled(" [SIMULATION: SUCCESS] ", Style::default().fg(Color::Green))
            } else {
                Span::styled(" [SIMULATION: REVERTED] ", Style::default().fg(Color::Red))
            };

            let prop_lines = vec![
                Line::from(vec![
                    Span::styled("Action: ", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(format!("{:?}", prop.proposal_type)),
                    sim_tag,
                ]),
                Line::from(vec![
                    Span::styled("Target: ", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(format!("{:#x}", prop.to)),
                    Span::raw(" | "),
                    Span::styled("Value: ", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(format!("{} wei", prop.value_wei)),
                ]),
                Line::from(vec![
                    Span::styled(
                        "Raw Calldata: ",
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{:#x}", prop.calldata),
                        Style::default().fg(Color::Yellow),
                    ),
                ]),
                Line::from(vec![
                    Span::styled(
                        "AI Rationale (Untrusted): ",
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(&prop.llm_explanation, Style::default().fg(Color::DarkGray)),
                ]),
                Line::from(vec![
                    Span::styled(
                        " [a] Approve & Sign ",
                        Style::default()
                            .bg(Color::Green)
                            .fg(Color::Black)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        " [d] Deny & Discard ",
                        Style::default()
                            .bg(Color::Red)
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
            ];

            let prop_title = format!(" Transaction Proposal Review — {} ", prop.proposal_id);
            let prop_inner =
                brand::render_faded_box(frame, chunks[1], Some(brand::focus_title(&prop_title)));
            let prop_block = Paragraph::new(prop_lines).wrap(Wrap { trim: true });
            frame.render_widget(prop_block, prop_inner);
        }

        let input_chunk = if self.active_proposal.is_some()
            || self.model_picker.is_some()
            || self.portfolio.is_some()
        {
            chunks[2]
        } else {
            chunks[1]
        };

        if let Some(picker) = &self.model_picker {
            self.render_model_picker(frame, chunks[1], picker);
        }
        if let Some(panel) = &self.portfolio {
            self.render_portfolio(frame, chunks[1], panel);
        }

        render_labeled_input(
            frame,
            input_chunk,
            "Prompt",
            &self.input,
            self.active_proposal.is_none()
                && self.job.is_none()
                && self.model_picker.is_none()
                && self.portfolio.is_none(),
        );

        frame.render_widget(status_paragraph(&self.footer_text()), status_rect);
    }

    fn render_portfolio(&self, frame: &mut Frame, area: Rect, panel: &PortfolioOverlay) {
        let net = &self.session.network_name;
        let testnet = if self.session.is_testnet {
            " (testnet)"
        } else {
            ""
        };
        let title = format!(" Portfolio — {net}{testnet} · Esc close · r refresh ");

        let items: Vec<ListItem> = if panel.loading {
            vec![ListItem::new(Line::from(format!(
                "  {} loading balances…",
                spinner_frame(self.tick)
            )))]
        } else if let Some(err) = &panel.error {
            vec![ListItem::new(Line::from(Span::styled(
                format!("  {err}"),
                Style::default().fg(Color::Red),
            )))]
        } else if panel.assets.is_empty() {
            vec![ListItem::new(Line::from(
                "  No balances — import tokens from Assets (dashboard), then r to refresh.",
            ))]
        } else {
            panel
                .assets
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    let mark = if i == panel.selected { ">" } else { " " };
                    let kind = if b.token.contract_address.is_none() {
                        "native"
                    } else {
                        "ERC-20"
                    };
                    let label = format!(
                        "{mark} {:<12} {:>22}  ({kind})",
                        b.token.symbol, b.formatted
                    );
                    let style = if i == panel.selected {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else if b.token.contract_address.is_none() {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default()
                    };
                    ListItem::new(Line::from(Span::styled(label, style)))
                })
                .collect()
        };

        frame.render_widget(Clear, area);
        let list = List::new(items);
        let inner = brand::render_faded_box(frame, area, Some(brand::fade_line(&title)));
        frame.render_widget(list, inner);
    }

    fn render_model_picker(&self, frame: &mut Frame, area: Rect, picker: &ModelPicker) {
        let rows = self.picker_rows();
        let selected = if rows.is_empty() {
            0
        } else {
            picker.selected.min(rows.len() - 1)
        };
        let filter_hint = if picker.filter.is_empty() {
            "(type to filter)".to_string()
        } else {
            format!("filter: {}", picker.filter)
        };
        let title = format!(
            " Select model · {} · {filter_hint} ",
            provider_id(self.config.provider)
        );

        let items: Vec<ListItem> = if rows.is_empty() {
            vec![ListItem::new(Line::from(Span::styled(
                "No matches — keep typing a custom model id",
                Style::default().fg(Color::DarkGray),
            )))]
        } else {
            rows.iter()
                .enumerate()
                .map(|(i, (id, label))| {
                    let current = id == &self.config.model_name;
                    let marker = if current { "★ " } else { "  " };
                    let line = format!("{marker}{id}  —  {label}");
                    let style = if i == selected {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else if current {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default()
                    };
                    ListItem::new(Line::from(Span::styled(line, style)))
                })
                .collect()
        };

        // Clear underlay so the list doesn't blend into chat history.
        frame.render_widget(Clear, area);
        let list = List::new(items);
        let inner = brand::render_faded_box(frame, area, Some(brand::fade_line(&title)));
        frame.render_widget(list, inner);
    }
}

/// Prefixed chat bubbles, hard-wrapped to `width` so text never runs off-screen.
fn build_chat_lines(history: &[AgentMessage], width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for msg in history {
        match msg {
            AgentMessage::User(u) => append_wrapped(
                &mut lines,
                "[You]: ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
                u,
                Style::default(),
                width,
            ),
            AgentMessage::Assistant(a) => append_wrapped(
                &mut lines,
                "[Advisor]: ",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
                a,
                Style::default(),
                width,
            ),
            AgentMessage::ToolCall { name, args } => append_wrapped(
                &mut lines,
                &format!("[Tool Call: {name}]: "),
                Style::default().fg(Color::Yellow),
                &truncate_display(args, 160),
                Style::default().fg(Color::DarkGray),
                width,
            ),
            AgentMessage::ToolResult { name, result } => append_wrapped(
                &mut lines,
                &format!("[Tool Result: {name}]: "),
                Style::default().fg(Color::Magenta),
                &truncate_display(result, 320),
                Style::default().fg(Color::DarkGray),
                width,
            ),
            AgentMessage::System(s) => append_wrapped(
                &mut lines,
                "[System]: ",
                Style::default().fg(Color::DarkGray),
                s,
                Style::default().fg(Color::DarkGray),
                width,
            ),
        }
    }
    lines
}

fn append_wrapped(
    out: &mut Vec<Line<'static>>,
    prefix: &str,
    prefix_style: Style,
    body: &str,
    body_style: Style,
    width: usize,
) {
    let width = width.max(8);
    let prefix_cols = prefix.chars().count();
    let indent = " ".repeat(prefix_cols.min(width.saturating_sub(1)));
    let body_width = width.saturating_sub(prefix_cols).max(8);

    let paragraphs: Vec<&str> = if body.is_empty() {
        vec![""]
    } else {
        body.split('\n').collect()
    };

    let mut first = true;
    for para in paragraphs {
        let chunks = wrap_words(para, body_width);
        if chunks.is_empty() {
            if first {
                out.push(Line::from(Span::styled(prefix.to_string(), prefix_style)));
                first = false;
            } else {
                out.push(Line::from(""));
            }
            continue;
        }
        for chunk in chunks {
            if first {
                out.push(Line::from(vec![
                    Span::styled(prefix.to_string(), prefix_style),
                    Span::styled(chunk, body_style),
                ]));
                first = false;
            } else {
                out.push(Line::from(vec![
                    Span::raw(indent.clone()),
                    Span::styled(chunk, body_style),
                ]));
            }
        }
    }
}

/// Cap tool call/result bodies shown in the chat pane.
fn truncate_display(s: &str, max: usize) -> String {
    let collapsed: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max {
        collapsed
    } else {
        let trimmed: String = collapsed.chars().take(max.saturating_sub(1)).collect();
        format!("{trimmed}…")
    }
}

/// True when only Control is held (no Alt).
fn is_ctrl_only(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT)
}

/// Bare footer chrome keys — used when the agent prompt is empty.
fn is_chrome_hotkey(key: KeyEvent) -> bool {
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return false;
    }
    match key.code {
        KeyCode::Tab => true,
        KeyCode::Char(c) => is_chrome_char(c),
        _ => false,
    }
}

/// Ctrl+footer key — works mid-typing in agent chat (`Ctrl+C` / `Ctrl+Q` still request quit).
fn is_ctrl_chrome_hotkey(key: KeyEvent) -> bool {
    if !is_ctrl_only(key) {
        return false;
    }
    match key.code {
        // Quit is handled in `global_action` before the view outcome matters.
        KeyCode::Char('c' | 'C' | 'q' | 'Q') => false,
        KeyCode::Char(c) => is_chrome_char(c),
        _ => false,
    }
}

fn is_chrome_char(c: char) -> bool {
    matches!(
        c.to_ascii_lowercase(),
        's' | 'v'
            | 'a'
            | 'b'
            | 'w'
            | 'c'
            | 'd'
            | 'g'
            | 'h'
            | 'n'
            | 'i'
            | 'k'
            | 'e'
            | 'f'
            | 'j'
            | 'm'
            | 'q'
            | 'r'
            | 'l'
            | 't'
            | 'x'
    )
}

/// Soft-wrap `text` to `width` columns, preferring breaks at whitespace.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    if text.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        let word_len = word.chars().count();
        if word_len > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            let chars: Vec<char> = word.chars().collect();
            for chunk in chars.chunks(width) {
                lines.push(chunk.iter().collect());
            }
            continue;
        }

        let next_len = if current.is_empty() {
            word_len
        } else {
            current.chars().count() + 1 + word_len
        };
        if next_len > width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }

    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod wrap_tests {
    use super::wrap_words;

    #[test]
    fn wraps_long_sentence() {
        let lines = wrap_words(
            "I'd be happy to help you buy meme coins on PulseChain testnet!",
            40,
        );
        assert!(lines.len() >= 2);
        assert!(lines.iter().all(|l| l.chars().count() <= 40));
    }

    #[test]
    fn hard_breaks_long_token() {
        let lines = wrap_words(&"x".repeat(25), 10);
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|l| l.chars().count() <= 10));
    }
}
