//! Application state and the main event loop.

use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::{DefaultTerminal, Frame};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};
use vaughan_agent::paths::profile_dir;
use vaughan_core::chains::evm::networks::{get_network_by_chain_id, resolve_switch_chain_id};
use vaughan_core::chains::Balance;
use vaughan_core::core::proposal::ProposalQueue;
use vaughan_core::core::{
    mark_replaced, operator_connect_allowed, push_recent, tui_mode_for_profile, AgentAutonomyTier,
    BroadcastEntry, McpSessionToken, OperatingMode, StateManager, WalletState,
};
use vaughan_core::error::WalletError;
use vaughan_provider::{EventBus, ProviderError, ProviderEvent};

use crate::jobs::{
    asset_index_for_address, chrome_assets_from_fetch, ChromeFocus, ChromeSnapshot, UiJob,
    UiJobResult, UnlockCompletion,
};
use crate::mcp::{McpHostRequest, McpService, McpSessionSnapshot};
use crate::provider::{self, ApprovalKind, BridgeStatusHandle, HostRequest};
use crate::views::{
    AaSendView, AgView, ApprovalsView, ApproveView, AssetsView, BridgeView, BrowserView, DappsView,
    DashboardView, DexView, HistoryView, KeysView, OnboardingView, PlaceholderView, ReceiveView,
    SettingsView, UnlockView, WrapView,
};

/// The active screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Onboarding,
    Unlock,
    Dashboard,
    AaSend,
    Receive,
    Settings,
    Keys,
    Dapps,
    Assets,
    Browser,
    Dex,
    Aggregator,
    SoonNft,
    Bridge,
    SoonStake,
    History,
    Approvals,
    Wrap,
    Approve,
}

impl Screen {
    pub fn title(self) -> &'static str {
        match self {
            Self::Onboarding => "Onboarding",
            Self::Unlock => "Unlock",
            Self::Dashboard => "Dashboard",
            Self::AaSend => "Batch Send",
            Self::Receive => "Receive",
            Self::Settings => "Settings",
            Self::Keys => "Keys",
            Self::Dapps => "Web (optional)",
            Self::Assets => "Assets",
            Self::Browser => "Contract Browser",
            Self::Dex => "DEX",
            Self::Aggregator => "Aggregator",
            Self::SoonNft => "NFT",
            Self::Bridge => "Bridge",
            Self::SoonStake => "Stake",
            Self::History => "History",
            Self::Approvals => "Approvals",
            Self::Wrap => "Wrap",
            Self::Approve => "Approve",
        }
    }
}

/// How the active view handled a key.
///
/// The app uses this to decide whether global shortcuts apply: a `Consumed`
/// key (e.g. `'q'` typed into a text field) must never trigger a
/// global action.
pub enum KeyOutcome {
    /// The view consumed the key; the app must not apply global shortcuts.
    Consumed,
    /// The view did not use the key; the app may apply global shortcuts.
    NotHandled,
    /// The view wants to navigate to a new screen.
    Navigate(Screen),
    /// Spawn a background RPC job; keep the current screen responsive.
    StartJob(crate::jobs::UiJob),
    /// Open Send prefilled for an Assets row.
    SendAsset(vaughan_core::chains::Balance),
    /// Intent macro → jump to an existing surface (optional prefills).
    Intent(crate::intent::IntentNav),
    /// Local EIP-712 sign (browser REPL) → Approve gate.
    SignTypedData(serde_json::Value),
    /// Show a short chrome toast (copy confirmations, etc.); key is consumed.
    Flash(String),
    /// Unlock picker: load a different profile vault and rebind MCP/grants.
    /// Carries the operating mode picked alongside the profile (FR-5.1).
    SwitchProfile(String, OperatingMode),
}

impl std::fmt::Debug for KeyOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Consumed => write!(f, "Consumed"),
            Self::NotHandled => write!(f, "NotHandled"),
            Self::Navigate(s) => f.debug_tuple("Navigate").field(s).finish(),
            Self::StartJob(_) => write!(f, "StartJob(..)"),
            Self::SendAsset(_) => write!(f, "SendAsset(..)"),
            Self::Intent(i) => f.debug_tuple("Intent").field(i).finish(),
            Self::SignTypedData(_) => write!(f, "SignTypedData(..)"),
            Self::Flash(s) => f.debug_tuple("Flash").field(s).finish(),
            Self::SwitchProfile(p, m) => f.debug_tuple("SwitchProfile").field(p).field(m).finish(),
        }
    }
}

/// The active view (screen + its state).
pub enum View {
    Onboarding(OnboardingView),
    Unlock(UnlockView),
    Dashboard(DashboardView),
    AaSend(AaSendView),
    Receive(ReceiveView),
    Settings(SettingsView),
    Keys(KeysView),
    Dapps(DappsView),
    Assets(AssetsView),
    Browser(BrowserView),
    Dex(DexView),
    Aggregator(AgView),
    Bridge(BridgeView),
    History(HistoryView),
    Approvals(ApprovalsView),
    Wrap(WrapView),
    Placeholder(PlaceholderView),
    Approve(ApproveView),
}

impl View {
    pub fn screen(&self) -> Screen {
        match self {
            Self::Onboarding(_) => Screen::Onboarding,
            Self::Unlock(_) => Screen::Unlock,
            Self::Dashboard(_) => Screen::Dashboard,
            Self::AaSend(_) => Screen::AaSend,
            Self::Receive(_) => Screen::Receive,
            Self::Settings(_) => Screen::Settings,
            Self::Keys(_) => Screen::Keys,
            Self::Dapps(_) => Screen::Dapps,
            Self::Assets(_) => Screen::Assets,
            Self::Browser(_) => Screen::Browser,
            Self::Dex(_) => Screen::Dex,
            Self::Aggregator(_) => Screen::Aggregator,
            Self::Bridge(_) => Screen::Bridge,
            Self::History(_) => Screen::History,
            Self::Approvals(_) => Screen::Approvals,
            Self::Wrap(_) => Screen::Wrap,
            Self::Placeholder(v) => v.screen(),
            Self::Approve(_) => Screen::Approve,
        }
    }

    pub fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        wallet: &WalletState,
        bridge_line: &str,
        assets: &[Balance],
    ) {
        match self {
            Self::Onboarding(v) => v.render(frame, area, wallet),
            Self::Unlock(v) => v.render(frame, area, wallet),
            Self::Dashboard(v) => v.render(frame, area, wallet),
            Self::AaSend(v) => v.render(frame, area, wallet),
            Self::Receive(v) => v.render(frame, area, wallet),
            Self::Settings(v) => v.render(frame, area, wallet),
            Self::Keys(v) => v.render(frame, area, wallet),
            Self::Dapps(v) => v.render(frame, area, wallet, bridge_line),
            Self::Assets(v) => v.render(frame, area, wallet),
            Self::Browser(v) => v.render(frame, area, wallet),
            Self::Dex(v) => v.render(frame, area, wallet, assets),
            Self::Aggregator(v) => v.render(frame, area, wallet, assets),
            Self::Bridge(v) => v.render(frame, area, wallet),
            Self::History(v) => v.render(frame, area, wallet),
            Self::Approvals(v) => v.render(frame, area, wallet),
            Self::Wrap(v) => v.render(frame, area, wallet),
            Self::Placeholder(v) => v.render(frame, area, wallet),
            Self::Approve(v) => v.render(frame, area, wallet),
        }
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        handle: &Handle,
        events: &EventBus,
    ) -> KeyOutcome {
        match self {
            Self::Onboarding(v) => v.handle_key(key, wallet, handle, events),
            Self::Unlock(v) => v.handle_key(key, wallet, handle, events),
            Self::Dashboard(v) => v.handle_key(key, wallet, handle, events),
            Self::AaSend(v) => v.handle_key(key, wallet, handle, events),
            Self::Receive(v) => v.handle_key(key, wallet, handle, events),
            Self::Settings(v) => v.handle_key(key, wallet, handle, events),
            Self::Keys(v) => v.handle_key(key, wallet, handle, events),
            Self::Dapps(v) => v.handle_key(key, wallet, handle, events),
            Self::Assets(v) => v.handle_key(key, wallet, handle, events),
            Self::Browser(v) => v.handle_key(key, wallet, handle, events),
            Self::Dex(v) => v.handle_key(key, wallet, handle, events),
            Self::Aggregator(v) => v.handle_key(key, wallet, handle, events),
            Self::Bridge(v) => v.handle_key(key, wallet, handle, events),
            Self::History(v) => v.handle_key(key, wallet, handle, events),
            Self::Approvals(v) => v.handle_key(key, wallet, handle, events),
            Self::Wrap(v) => v.handle_key(key, wallet, handle, events),
            Self::Placeholder(v) => v.handle_key(key, wallet, handle, events),
            Self::Approve(v) => v.handle_key(key, wallet, handle, events),
        }
    }

    /// Footer chip keys defer to global navigation when this returns true.
    pub fn allows_footer_shortcuts(&self) -> bool {
        match self {
            Self::Onboarding(v) => v.allows_footer_shortcuts(),
            Self::Unlock(v) => v.allows_footer_shortcuts(),
            Self::Dashboard(v) => v.allows_footer_shortcuts(),
            Self::AaSend(v) => v.allows_footer_shortcuts(),
            Self::Receive(v) => v.allows_footer_shortcuts(),
            Self::Settings(v) => v.allows_footer_shortcuts(),
            Self::Keys(v) => v.allows_footer_shortcuts(),
            Self::Dapps(v) => v.allows_footer_shortcuts(),
            Self::Assets(v) => v.allows_footer_shortcuts(),
            Self::Browser(v) => v.allows_footer_shortcuts(),
            Self::Dex(v) => v.allows_footer_shortcuts(),
            Self::Aggregator(v) => v.allows_footer_shortcuts(),
            Self::Bridge(v) => v.allows_footer_shortcuts(),
            Self::History(v) => v.allows_footer_shortcuts(),
            Self::Approvals(v) => v.allows_footer_shortcuts(),
            Self::Wrap(v) => v.allows_footer_shortcuts(),
            Self::Placeholder(v) => v.allows_footer_shortcuts(),
            Self::Approve(v) => v.allows_footer_shortcuts(),
        }
    }
}

/// A sign/send/connect request waiting on the user's approve/deny decision.
struct PendingApproval {
    kind: ApprovalKind,
    reply: PendingReply,
    /// When the prompt was shown (debounce + expiry).
    shown_at: std::time::Instant,
}

enum PendingReply {
    Sign(oneshot::Sender<Result<String, ProviderError>>),
    Accounts(oneshot::Sender<Result<Vec<String>, ProviderError>>),
    Switch(oneshot::Sender<Result<(), ProviderError>>),
    /// Browserless local EIP-712 — result shown as chrome flash on approve.
    LocalSign,
    /// File-queue MCP proposal (`reply: None`): no channel — approve/deny is
    /// written back to the queue files (`mark_approved` / `mark_rejected`).
    Queued,
}

/// Discard accidental keypresses for this long after an approve prompt appears.
const APPROVE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(400);
/// Auto-deny stale prompts (dApps typically time out around 30–60s).
const APPROVE_TTL: std::time::Duration = std::time::Duration::from_secs(60);
/// Bound on queues into the UI thread (provider requests, MCP requests, job
/// results) — a flooding client gets backpressure instead of unbounded growth.
const UI_QUEUE_CAPACITY: usize = 256;

/// Root application state.
pub struct App {
    wallet: Arc<Mutex<WalletState>>,
    handle: Handle,
    view: View,
    quitting: bool,
    /// Soft quit prompt: `Some(true)` = Yes selected (default), `Some(false)` = No.
    quit_confirm: Option<bool>,
    /// Provider events published to connected dApps (chain/account changes).
    events: EventBus,
    /// Requests forwarded from the provider server; drained on the UI thread.
    host_rx: mpsc::Receiver<HostRequest>,
    /// Background job results (balance / fee / send).
    job_tx: mpsc::Sender<UiJobResult>,
    job_rx: mpsc::Receiver<UiJobResult>,
    /// Animation tick for spinners.
    tick: u64,
    /// The approval currently on screen, if any.
    pending_approval: Option<PendingApproval>,
    /// Screen to return to after the pending approval resolves.
    approve_return: Screen,
    /// Always-on status strip: native balance + gas price.
    chrome: ChromeSnapshot,
    /// MCP control plane (session + loopback listener).
    mcp: McpService,
    mcp_rx: mpsc::Receiver<McpHostRequest>,
    /// Session-scoped broadcasts for History cancel / speed-up.
    recent_broadcasts: Vec<BroadcastEntry>,
    /// Local EIP-1193 bridge listen state (VB / Freedom fallback when parked).
    bridge_status: BridgeStatusHandle,
    /// Sites granted `eth_requestAccounts` this unlock session (page/WS origin).
    /// Shared with the provider server so `accountsChanged` is relayed only to
    /// granted origins.
    connected_sites: std::sync::Arc<std::sync::RwLock<std::collections::HashSet<String>>>,
    /// Session-scoped circuit breaker for sentient auto-exec (created lazily,
    /// dropped on lock so tripwires reset with the session).
    sentient_breaker: Option<vaughan_agent::CircuitBreaker>,
    /// Live provider-bridge secret slots shared with the WS server; the token
    /// rotates on every lock/unlock edge so a stolen token dies at lock, and
    /// the origin-seal key tracks the running VB launch.
    provider_slots: provider::ProviderBridgeSlots,
    /// Tracks the last lock state the provider token was synced to (edge
    /// detection for rotation).
    provider_session_unlocked: bool,
}

impl App {
    /// Load the wallet for `profile` and pick the initial screen.
    ///
    /// The profile selects the vault (`default` = adviser; `sentient` = agent
    /// auto-exec) and is pre-selected on the unlock-screen profile picker.
    pub fn new(handle: Handle, profile: &str) -> Result<Self, WalletError> {
        let path = StateManager::profile_path(profile)?;
        // Seed the session mode from the profile name so a direct
        // `--profile sentient` launch is SentientTrader even through
        // onboarding; the unlock picker may override it pre-unlock.
        let wallet = WalletState::load_with_session(path, tui_mode_for_profile(profile), profile)?;
        let screen = if !wallet.is_initialized() {
            Screen::Onboarding
        } else if !wallet.is_unlocked() {
            Screen::Unlock
        } else {
            Screen::Dashboard
        };
        let events = EventBus::new();
        // Bounded queues into the UI thread: a flooding client gets
        // backpressure/an error instead of growing memory unboundedly.
        let (host_tx, host_rx) = mpsc::channel(UI_QUEUE_CAPACITY);
        let (mcp_tx, mcp_rx) = mpsc::channel(UI_QUEUE_CAPACITY);
        let (job_tx, job_rx) = mpsc::channel(UI_QUEUE_CAPACITY);
        let bridge_status = provider::new_bridge_status();
        let profile_dir = profile_dir(wallet.path());
        let dapp_origins = wallet.trusted_dapp_origins();
        // Grants persist across restarts (origins only, no secrets) and
        // are cleared on explicit lock — see core::site_grants.
        let connected_sites = std::sync::Arc::new(std::sync::RwLock::new(
            vaughan_core::core::site_grants::load(&profile_dir).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "site grants load failed; starting empty");
                std::collections::HashSet::new()
            }),
        ));
        let provider_slots = provider::spawn_provider_server(
            &handle,
            host_tx,
            events.clone(),
            dapp_origins,
            bridge_status.clone(),
            profile_dir.clone(),
            connected_sites.clone(),
        );
        let mcp = McpService::new(&profile_dir, mcp_tx);
        let mut app = Self {
            wallet: Arc::new(Mutex::new(wallet)),
            handle,
            view: View::Onboarding(OnboardingView::default()),
            quitting: false,
            quit_confirm: None,
            events,
            host_rx,
            job_tx,
            job_rx,
            tick: 0,
            pending_approval: None,
            approve_return: Screen::Dashboard,
            chrome: ChromeSnapshot {
                loading: false,
                ..ChromeSnapshot::default()
            },
            mcp,
            mcp_rx,
            recent_broadcasts: Vec::new(),
            bridge_status,
            connected_sites,
            sentient_breaker: None,
            provider_slots,
            provider_session_unlocked: false,
        };
        app.navigate(screen);
        Ok(app)
    }

    /// Lazily create the session circuit breaker for sentient auto-exec.
    fn sentient_breaker(&mut self) -> Result<vaughan_agent::CircuitBreaker, ProviderError> {
        if self.sentient_breaker.is_none() {
            let breaker = {
                let wallet = self.wallet();
                crate::sentient_mcp::new_session_breaker(&wallet)?
            };
            self.sentient_breaker = Some(breaker);
        }
        Ok(self.sentient_breaker.as_ref().unwrap().clone())
    }

    /// Ctrl+K kill switch: trip the session breaker so sentient auto-exec
    /// halts for the rest of the session. No-op on non-sentient profiles and
    /// while locked (auto-exec cannot run locked anyway).
    fn trip_sentient_breaker(&mut self) {
        if !self.wallet().is_unlocked()
            || !crate::sentient_mcp::mcp_auto_exec_enabled(self.wallet().profile_name())
        {
            return;
        }
        match self.sentient_breaker() {
            Ok(breaker) => {
                breaker.trip("manual kill switch (Ctrl+K)");
                tracing::warn!(target: "vaughan_tui::mcp", "sentient circuit breaker tripped manually");
                self.set_flash(
                    "Sentient breaker TRIPPED — auto-exec halted until restart".to_string(),
                );
            }
            Err(e) => self.set_flash(e.to_string()),
        }
    }

    pub fn wallet(&self) -> std::sync::MutexGuard<'_, WalletState> {
        self.wallet.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn try_wallet(&self) -> Option<std::sync::MutexGuard<'_, WalletState>> {
        self.wallet.try_lock().ok()
    }

    pub fn screen(&self) -> Screen {
        self.view.screen()
    }

    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Hide the splash tagline on the password prompt so the field stays clean.
    pub(crate) fn suppress_unlock_slogan(&self) -> bool {
        matches!(&self.view, View::Unlock(u) if u.is_password_stage())
    }

    /// Shared need-to-know strip (balance / gas) for every unlocked view.
    pub fn chrome(&self) -> &ChromeSnapshot {
        &self.chrome
    }

    /// Soft quit dialog: `Some(true)` = Yes highlighted, `Some(false)` = No.
    pub fn quit_confirm(&self) -> Option<bool> {
        self.quit_confirm
    }

    pub fn render_body(&self, frame: &mut Frame, area: Rect) {
        if let Some(wallet) = self.try_wallet() {
            let bridge_line = self
                .bridge_status
                .lock()
                .map(|s| s.summary_line())
                .unwrap_or_else(|_| "Bridge: status unavailable".into());
            self.view
                .render(frame, area, &wallet, &bridge_line, &self.chrome.assets);
        } else {
            // Job thread holds the wallet — keep painting a spinner body.
            use ratatui::widgets::Paragraph;
            frame.render_widget(
                Paragraph::new(format!(
                    "{} working…",
                    crate::jobs::spinner_frame(self.tick)
                )),
                area,
            );
        }
    }

    /// Run the terminal event loop until the user quits.
    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        while !self.quitting {
            self.tick = self.tick.wrapping_add(1);
            if self.chrome.flash_ticks_left > 0 {
                self.chrome.flash_ticks_left -= 1;
                if self.chrome.flash_ticks_left == 0 {
                    self.chrome.flash = None;
                }
            }
            self.poll_provider();
            self.poll_mcp();
            self.poll_jobs();
            if let View::Dex(v) = &mut self.view {
                v.set_tick(self.tick);
                if v.tick_quote_debounce() {
                    let job = {
                        let wallet = self.wallet.lock().unwrap_or_else(|e| e.into_inner());
                        v.start_quote_job(&wallet)
                    };
                    if let Some(job) = job {
                        self.spawn_job(job);
                    }
                }
            }
            if let View::Aggregator(v) = &mut self.view {
                v.set_tick(self.tick);
            }
            if let View::Bridge(v) = &mut self.view {
                v.set_tick(self.tick);
            }
            if let View::History(v) = &mut self.view {
                v.set_tick(self.tick);
            }
            if let View::Approvals(v) = &mut self.view {
                v.set_tick(self.tick);
            }
            if let View::Wrap(v) = &mut self.view {
                v.set_tick(self.tick);
            }
            if let View::Dashboard(v) = &mut self.view {
                v.set_tick(self.tick);
            }
            if let View::Assets(v) = &mut self.view {
                v.set_tick(self.tick);
            }
            if let View::Browser(v) = &mut self.view {
                v.set_tick(self.tick);
            }
            if let View::Unlock(v) = &mut self.view {
                v.set_tick(self.tick);
            }
            terminal.draw(|frame| crate::views::render(frame, self))?;
            if event::poll(Duration::from_millis(80))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        self.handle_key(key);
                    }
                }
            }
        }
        // Clean shutdown: leave no live session tokens on disk.
        let dir = profile_dir(self.wallet().path());
        let _ = vaughan_core::core::ProviderSessionToken::invalidate(&dir);
        let _ = McpSessionToken::invalidate(&dir);
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) {
        // Quit confirm owns the keyboard until Yes/No/Esc.
        if self.quit_confirm.is_some() {
            self.handle_quit_confirm_key(key);
            return;
        }

        // Sentient kill switch: Ctrl+K trips the session breaker from any
        // screen (including the approval prompt), halting auto-exec for the
        // rest of the session.
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('k') | KeyCode::Char('K'))
        {
            self.trip_sentient_breaker();
            return;
        }

        // The approval prompt owns its own key handling (approve/deny) and its
        // reply channel, so global shortcuts don't apply while it is shown.
        if self.screen() == Screen::Approve {
            self.handle_approval_key(key);
            return;
        }

        // Dex/Ag Token in/out: ↑/↓ pick from wallet assets; else F1–F3 chrome strip.
        if self.wallet().is_unlocked() {
            if matches!(key.code, KeyCode::Up | KeyCode::Down) {
                let forward = matches!(key.code, KeyCode::Down);
                if self.cycle_active_token_field(forward) {
                    return;
                }
            }
            if self.handle_chrome_hotkey(key) {
                return;
            }
        }

        let outcome = {
            let mut wallet = self.wallet.lock().unwrap_or_else(|e| e.into_inner());
            self.view
                .handle_key(key, &mut wallet, &self.handle, &self.events)
        };
        let outcome = defer_footer_shortcut(outcome, key, self.view.allows_footer_shortcuts());

        match global_action(key, &outcome) {
            GlobalAction::Quit => {
                self.quit_confirm = Some(true);
                return;
            }
            GlobalAction::CycleScreens => {
                let next = match self.screen() {
                    Screen::Dashboard => Screen::AaSend,
                    Screen::AaSend => Screen::Receive,
                    Screen::Receive => Screen::Settings,
                    Screen::Settings => Screen::Keys,
                    Screen::Keys => Screen::Dapps,
                    Screen::Dapps => Screen::Assets,
                    Screen::Assets => Screen::Browser,
                    Screen::Browser => Screen::Dex,
                    Screen::Dex => Screen::Aggregator,
                    Screen::Aggregator => Screen::Dashboard,
                    other => other,
                };
                if next != self.screen() {
                    self.navigate(next);
                }
                return;
            }
            GlobalAction::Navigate(screen) => {
                if self.wallet().is_unlocked() && screen != self.screen() {
                    self.navigate(screen);
                }
                return;
            }
            GlobalAction::RefreshChrome => {
                if self.wallet().is_unlocked() {
                    self.refresh_chrome();
                }
                return;
            }
            GlobalAction::Lock => {
                if self.wallet().is_unlocked() {
                    self.mcp.on_lock();
                    self.connected_sites.write().unwrap().clear();
                    self.sentient_breaker = None;
                    if let Some(dir) = self.wallet().path().parent() {
                        if let Err(e) = vaughan_core::core::site_grants::clear(dir) {
                            tracing::warn!(error = %e, "site grants clear failed");
                        }
                    }
                    {
                        let mut wallet = self.wallet.lock().unwrap_or_else(|e| e.into_inner());
                        wallet.lock();
                        self.events.publish(ProviderEvent::AccountsChanged(vec![]));
                    }
                    self.chrome.focus = ChromeFocus::None;
                    self.chrome.assets.clear();
                    self.chrome.asset_idx = 0;
                    self.navigate(Screen::Unlock);
                }
                return;
            }
            GlobalAction::CycleTheme => {
                let _ = crate::brand::cycle_theme();
                return;
            }
            GlobalAction::CopyAddress => {
                if self.wallet().is_unlocked() {
                    let addr = {
                        let w = self.wallet();
                        w.active_address().map(|a| a.to_string())
                    };
                    match addr {
                        Ok(addr) => match crate::clipboard::copy_text(&addr) {
                            Ok(()) => self.set_flash("F3 address copied"),
                            Err(e) => self.set_flash(e),
                        },
                        Err(e) => self.set_flash(e.user_message()),
                    }
                }
                return;
            }
            GlobalAction::None => {}
        }

        match outcome {
            KeyOutcome::Navigate(screen) => self.navigate(screen),
            KeyOutcome::StartJob(job) => self.spawn_job(job),
            KeyOutcome::SendAsset(balance) => {
                // Align F2 with the asset the user picked from Assets.
                if let Some(i) = self
                    .chrome
                    .assets
                    .iter()
                    .position(|b| same_f2_asset(b, &balance))
                {
                    self.chrome.asset_idx = i;
                } else {
                    self.chrome.assets_loading = true;
                    self.spawn_job(UiJob::RefreshAssets);
                }
                self.view = View::Dashboard(DashboardView::for_asset(balance));
            }
            KeyOutcome::Intent(nav) => self.apply_intent(nav),
            KeyOutcome::SignTypedData(data) => self.begin_local_typed_data_sign(data),
            KeyOutcome::Flash(msg) => self.set_flash(msg),
            KeyOutcome::SwitchProfile(profile, mode) => self.switch_profile(&profile, mode),
            KeyOutcome::Consumed | KeyOutcome::NotHandled => {}
        }
    }

    /// Unlock-screen profile switch: load the selected profile's vault and
    /// rebind the MCP control plane + site grants to its directory.
    ///
    /// Runs pre-unlock, so no session state is carried across — the `mode`
    /// picked at the next unlock stays immutable for the session (FR-5.1).
    /// Uninitialized profiles land on onboarding to create their vault.
    fn switch_profile(&mut self, profile: &str, mode: OperatingMode) {
        let path = match StateManager::profile_path(profile) {
            Ok(p) => p,
            Err(e) => return self.profile_switch_error(e.user_message()),
        };
        let wallet = match WalletState::load_with_session(path, OperatingMode::HumanOnly, profile) {
            Ok(w) => w,
            Err(e) => return self.profile_switch_error(e.user_message()),
        };
        let initialized = wallet.is_initialized();
        let old_dir = profile_dir(self.wallet().path());
        let dir = profile_dir(wallet.path());
        let mut wallet = wallet;
        // Carry the picked mode onto the fresh (locked) wallet so onboarding
        // and the password screen inherit it; unlock re-asserts it anyway.
        wallet.set_operating_mode(mode);
        *self.wallet() = wallet;
        self.mcp.set_profile_dir(&dir);
        // Provider bridge: the server keeps running on the shared token slot,
        // so re-pointing discovery means invalidating the old profile's
        // provider.session and clearing any stale file in the new one. The
        // next unlock edge publishes a fresh token into the new dir.
        let _ = vaughan_core::core::ProviderSessionToken::invalidate(&old_dir);
        let _ = vaughan_core::core::ProviderSessionToken::invalidate(&dir);
        self.provider_session_unlocked = false;
        // Site grants persist per profile; reload for the switched vault.
        if let Ok(mut grants) = self.connected_sites.write() {
            *grants = vaughan_core::core::site_grants::load(&dir).unwrap_or_default();
        }
        if !initialized {
            self.navigate(Screen::Onboarding);
        }
    }

    /// Chrome toast under the address (home + every unlocked screen).
    fn set_flash(&mut self, msg: impl Into<String>) {
        self.chrome.flash = Some(msg.into());
        self.chrome.flash_ticks_left = 45; // ~a few seconds at UI tick rate
    }

    /// Surface a profile-switch failure where the user can see it: the chrome
    /// flash only renders on unlocked screens, so on the unlock screen the
    /// error goes to the view's status line instead of vanishing — otherwise
    /// the password prompt would silently keep targeting the previous wallet.
    fn profile_switch_error(&mut self, msg: String) {
        if let View::Unlock(v) = &mut self.view {
            v.set_status(msg);
        } else {
            self.set_flash(msg);
        }
    }

    fn apply_intent(&mut self, nav: crate::intent::IntentNav) {
        use crate::intent::IntentNav;
        match nav {
            IntentNav::Aggregator { amount, token_out } => {
                let chain_id = self.wallet().networks().active().chain_id;
                self.view = View::Aggregator(AgView::for_chain_prefill(
                    chain_id,
                    amount.as_deref(),
                    token_out.as_deref(),
                ));
            }
            IntentNav::BrowserInspect { address } => {
                let mut browser = BrowserView::default();
                let handle = self.handle.clone();
                {
                    let wallet = self.wallet.lock().unwrap_or_else(|e| e.into_inner());
                    browser.browse_address(&address, &wallet, &handle);
                }
                self.view = View::Browser(browser);
            }
            IntentNav::Approvals => self.navigate(Screen::Approvals),
            IntentNav::Receive => self.navigate(Screen::Receive),
        }
    }

    /// Drain the provider request channel, answering read queries inline and
    /// surfacing sign/send/connect requests as an approval prompt.
    fn poll_provider(&mut self) {
        self.expire_stale_approval();
        self.sync_provider_session();
        while let Ok(request) = self.host_rx.try_recv() {
            match request {
                HostRequest::Accounts { site, reply } => {
                    let accounts = if self.connected_sites.read().unwrap().contains(&site) {
                        self.visible_accounts()
                    } else {
                        Vec::new()
                    };
                    let _ = reply.send(Ok(accounts));
                }
                HostRequest::RequestAccounts { site, reply } => {
                    if !self.wallet().is_unlocked() {
                        // 4100, not a silent `[]`: the dApp must be able to
                        // surface "unlock Vaughan" instead of hanging on a
                        // "Confirm connection…" modal forever.
                        let _ = reply.send(Err(ProviderError::Unauthorized(
                            "wallet is locked; unlock it first".into(),
                        )));
                        continue;
                    }
                    if self.connected_sites.read().unwrap().contains(&site) {
                        let _ = reply.send(Ok(self.visible_accounts()));
                        continue;
                    }
                    if self.wallet().agent_autonomy_tier() == AgentAutonomyTier::Operator {
                        let dapps = self.wallet().trusted_dapps();
                        if operator_connect_allowed(&site, &dapps) {
                            self.connected_sites.write().unwrap().insert(site.clone());
                            self.persist_connected_sites();
                            let _ = reply.send(Ok(self.visible_accounts()));
                            continue;
                        }
                    }
                    if self.pending_approval.is_some() {
                        let _ = reply.send(Err(ProviderError::Unauthorized(
                            "another approval is pending".into(),
                        )));
                        continue;
                    }
                    let kind = ApprovalKind::Connect { site: site.clone() };
                    let preview = provider::describe_approval(&kind, &self.wallet(), &self.handle);
                    let (title, details) = match preview {
                        Ok(p) => p,
                        Err(error) => {
                            let _ = reply.send(Err(error));
                            continue;
                        }
                    };
                    self.approve_return = self.screen();
                    self.view = View::Approve(ApproveView::new(title, Some(site.clone()), details));
                    self.pending_approval = Some(PendingApproval {
                        kind,
                        reply: PendingReply::Accounts(reply),
                        shown_at: std::time::Instant::now(),
                    });
                    break;
                }
                HostRequest::ChainId { reply } => {
                    let id = self.wallet().networks().active().chain_id;
                    let _ = reply.send(Ok(format!("0x{id:x}")));
                }
                HostRequest::SwitchChain {
                    chain_id,
                    origin,
                    reply,
                } => {
                    if !self.wallet().is_unlocked() {
                        let _ = reply.send(Err(ProviderError::Unauthorized(
                            "wallet is locked; unlock it first".into(),
                        )));
                        continue;
                    }
                    if self.pending_approval.is_some() {
                        let _ = reply.send(Err(ProviderError::Unauthorized(
                            "another approval is pending".into(),
                        )));
                        continue;
                    }
                    // Operator tier: auto-switch on allowlisted dApp origins
                    // (same suffix list as auto-connect). Signing still manual.
                    if self.wallet().agent_autonomy_tier() == AgentAutonomyTier::Operator {
                        if let Some(origin) = origin.as_deref().filter(|s| !s.is_empty()) {
                            if operator_connect_allowed(origin, &self.wallet().trusted_dapps()) {
                                let result = self.switch_chain(&chain_id);
                                let _ = reply.send(result);
                                continue;
                            }
                        }
                    }
                    let label = Self::network_label_for_chain_hex(&chain_id);
                    let kind = ApprovalKind::SwitchChain {
                        chain_id: chain_id.clone(),
                        label,
                    };
                    let preview = provider::describe_approval(&kind, &self.wallet(), &self.handle);
                    let (title, details) = match preview {
                        Ok(p) => p,
                        Err(error) => {
                            let _ = reply.send(Err(error));
                            continue;
                        }
                    };
                    self.approve_return = self.screen();
                    self.view = View::Approve(ApproveView::new(title, origin, details));
                    self.pending_approval = Some(PendingApproval {
                        kind,
                        reply: PendingReply::Switch(reply),
                        shown_at: std::time::Instant::now(),
                    });
                    break;
                }
                HostRequest::RpcRead {
                    method,
                    params,
                    reply,
                } => match self.wallet().network_rpc_snapshot() {
                    Ok(snap) => {
                        provider::spawn_rpc_read(&self.handle, snap, method, params, reply);
                    }
                    Err(e) => {
                        let _ = reply.send(Err(ProviderError::Internal(e.user_message())));
                    }
                },
                HostRequest::Approval {
                    kind,
                    origin,
                    site,
                    requires_grant,
                    reply,
                } => {
                    if !self.wallet().is_unlocked() {
                        let _ = reply.send(Err(ProviderError::Unauthorized(
                            "wallet is locked; unlock it first".to_string(),
                        )));
                        continue;
                    }
                    // Extension-path requests must hold a Connect grant first
                    // (MetaMask parity; stops prompt-spam from sites the user
                    // never connected). Freedom's transport is exempt.
                    if requires_grant && !self.connected_sites.read().unwrap().contains(&site) {
                        let _ = reply.send(Err(ProviderError::Unauthorized(
                            "site not connected; call eth_requestAccounts first".into(),
                        )));
                        continue;
                    }
                    let preview =
                        provider::describe_approval_with_fee(&kind, &self.wallet(), &self.handle);
                    let (title, details, fee) = match preview {
                        Ok(preview) => preview,
                        Err(error) => {
                            let _ = reply.send(Err(error));
                            continue;
                        }
                    };
                    self.approve_return = self.screen();
                    self.view = View::Approve(match fee {
                        Some(base_fee) => ApproveView::with_fee(title, origin, details, base_fee),
                        None => ApproveView::new(title, origin, details),
                    });
                    self.pending_approval = Some(PendingApproval {
                        kind: *kind,
                        reply: PendingReply::Sign(reply),
                        shown_at: std::time::Instant::now(),
                    });
                    break;
                }
            }
        }
    }

    /// Rotate the provider-bridge token on lock/unlock edges: a stolen
    /// `provider.session` dies at lock, and the file is only published while
    /// unlocked. Rotation never sets the slot to `None` (that would mean
    /// "no token required") — a locked bridge gets a fresh unwritten token.
    fn sync_provider_session(&mut self) {
        let unlocked = self.wallet().is_unlocked();
        if unlocked != self.provider_session_unlocked {
            self.provider_session_unlocked = unlocked;
            let fresh = vaughan_core::core::ProviderSessionToken::generate();
            *self.provider_slots.token.write().unwrap() = Some(fresh.as_str().to_string());
            let dir = profile_dir(self.wallet().path());
            if unlocked {
                if let Err(e) = fresh.write(&dir) {
                    tracing::warn!(error = %e, "provider session token write failed");
                }
            } else {
                let _ = vaughan_core::core::ProviderSessionToken::invalidate(&dir);
            }
        }
        self.sync_origin_seal_key();
    }

    /// Learn the per-launch VB extension seal key from `vb.session` (throttled
    /// — the file changes only when VB (re)launches). A stale session (PID
    /// gone / not VB) clears the slot so seals from a dead browser stop
    /// passing; no file at all leaves the slot untouched on a read error
    /// streak-free path (None → legacy issuer-only attestation).
    fn sync_origin_seal_key(&mut self) {
        // ~2s cadence: poll_provider runs every tick (~80ms).
        if !self.tick.is_multiple_of(25) {
            return;
        }
        let session = match vaughan_core::core::vb_browser::read_vb_session() {
            Ok(Some(s)) => s,
            _ => return,
        };
        if !vaughan_core::core::vb_browser::vb_session_pid_matches(&session) {
            let mut slot = self.provider_slots.origin_seal.write().unwrap();
            if slot.is_some() {
                *slot = None;
                tracing::info!("vb.session stale — cleared extension origin-seal key");
            }
            return;
        }
        if session.extension_secret.is_empty() {
            return;
        }
        let mut slot = self.provider_slots.origin_seal.write().unwrap();
        if slot.as_deref() != Some(session.extension_secret.as_str()) {
            *slot = Some(session.extension_secret.clone());
            tracing::info!("installed VB extension origin-seal key from vb.session");
        }
    }

    /// Write a denial back to the MCP file queue (no-op for live proposals
    /// that never entered the queue).
    fn reject_queued_proposal(&self, proposal_id: &str, reason: &str) {
        let wallet = self.wallet();
        if let Some(parent) = wallet.path().parent() {
            if let Ok(Some(token)) = McpSessionToken::read(parent) {
                let queue = ProposalQueue::new(parent);
                let _ = queue.mark_rejected(proposal_id, reason, token.as_bytes());
            }
        }
    }

    fn expire_stale_approval(&mut self) {
        let Some(pending) = self.pending_approval.as_ref() else {
            return;
        };
        if pending.shown_at.elapsed() < APPROVE_TTL {
            return;
        }
        let Some(pending) = self.pending_approval.take() else {
            return;
        };
        match pending.reply {
            PendingReply::Sign(reply) => {
                let _ = reply.send(Err(ProviderError::UserRejected));
            }
            PendingReply::Accounts(reply) => {
                let _ = reply.send(Err(ProviderError::UserRejected));
            }
            PendingReply::Switch(reply) => {
                let _ = reply.send(Err(ProviderError::UserRejected));
            }
            PendingReply::LocalSign => {}
            PendingReply::Queued => {
                // No channel to reject on — write the denial back to the
                // queue so the proposal does not resurface on the next poll.
                if let ApprovalKind::McpProposal { proposal_id, .. } = &pending.kind {
                    self.reject_queued_proposal(proposal_id, "expired without decision");
                }
            }
        }
        let back = self.approve_return;
        self.navigate(back);
    }

    fn poll_mcp(&mut self) {
        // HumanOnly runs no agent surface at all: no loopback control plane,
        // no session token, no file-queue surfacing (FR-5.1 mode teeth).
        let agent_surface =
            self.wallet().is_unlocked() && self.wallet().operating_mode().is_ai_enabled();
        let mcp_pending = if agent_surface {
            self.mcp.on_unlock(&self.handle);
            let wallet = self.wallet();
            let net = wallet.networks().active();
            self.mcp.update_session(McpSessionSnapshot {
                address: wallet.active_address().ok().map(str::to_string),
                chain_id: Some(net.chain_id),
                network_id: Some(net.id.clone()),
            });
            let profile_dir = vaughan_agent::paths::profile_dir(wallet.path());
            ProposalQueue::new(&profile_dir)
                .list_pending()
                .map(|p| p.len())
                .unwrap_or(0)
        } else {
            0
        };
        self.chrome.mcp_pending = mcp_pending;
        self.chrome.mcp_listener = self.mcp.listener_state();
        let pending_on_screen = self.pending_approval.is_some();
        if agent_surface {
            self.mcp.poll_file_queue(pending_on_screen);
        }

        while let Ok(request) = self.mcp_rx.try_recv() {
            match request {
                McpHostRequest::Propose {
                    proposal,
                    source,
                    reply,
                } => {
                    if !self.wallet().is_unlocked() {
                        if let Some(r) = reply {
                            let _ =
                                r.send(Err(ProviderError::Unauthorized("wallet is locked".into())));
                        }
                        continue;
                    }
                    let proposal_id = proposal.proposal_id.clone();
                    let kind = ApprovalKind::McpProposal {
                        proposal_id,
                        source: source.clone(),
                        proposal,
                    };

                    // Sentient profile: auto re-sim → policy → sign (no card).
                    if crate::sentient_mcp::mcp_auto_exec_enabled(self.wallet().profile_name()) {
                        tracing::info!(
                            target: "vaughan_tui::mcp",
                            source = %source,
                            "sentient auto-exec MCP proposal"
                        );
                        let result = match self.sentient_breaker() {
                            Ok(breaker) => crate::sentient_mcp::auto_exec_mcp_proposal(
                                &self.wallet(),
                                &self.handle,
                                &breaker,
                                &kind,
                            ),
                            Err(e) => Err(e),
                        };
                        if let Some(r) = reply {
                            let _ = r.send(result);
                        }
                        continue;
                    }

                    if self.pending_approval.is_some() {
                        if let Some(r) = reply {
                            let _ = r.send(Err(ProviderError::Internal(
                                "another approval is pending".into(),
                            )));
                        }
                        continue;
                    }
                    let preview = provider::describe_approval(&kind, &self.wallet(), &self.handle);
                    let (title, details) = match preview {
                        Ok(preview) => preview,
                        Err(error) => {
                            if let Some(r) = reply {
                                let _ = r.send(Err(error));
                            }
                            continue;
                        }
                    };
                    self.approve_return = self.screen();
                    self.view = View::Approve(ApproveView::new(
                        title,
                        Some(format!("MCP ({source})")),
                        details,
                    ));
                    // File-queue proposals have no reply channel; they still
                    // get a real pending approval so the card is answerable —
                    // approve/deny is written back to the queue files.
                    let pending_reply = match reply {
                        Some(reply) => PendingReply::Sign(reply),
                        None => PendingReply::Queued,
                    };
                    self.pending_approval = Some(PendingApproval {
                        kind,
                        reply: pending_reply,
                        shown_at: std::time::Instant::now(),
                    });
                    break;
                }
                McpHostRequest::StealthUri { reply } => {
                    if !self.wallet().is_unlocked() {
                        let _ =
                            reply.send(Err(ProviderError::Unauthorized("wallet is locked".into())));
                        continue;
                    }
                    let result = self
                        .wallet()
                        .stealth_uri()
                        .map_err(|e| ProviderError::Internal(e.user_message()));
                    let _ = reply.send(result);
                }
                McpHostRequest::StealthScan { reply } => {
                    if !self.wallet().is_unlocked() {
                        let _ =
                            reply.send(Err(ProviderError::Unauthorized("wallet is locked".into())));
                        continue;
                    }
                    let notes = match self.handle.block_on(self.wallet().scan_stealth_notes()) {
                        Ok(n) => n,
                        Err(e) => {
                            let _ = reply.send(Err(ProviderError::Internal(e.user_message())));
                            continue;
                        }
                    };
                    let rows: Vec<_> = notes
                        .iter()
                        .map(|n| {
                            serde_json::json!({
                                "stealth_address": format!("{:#x}", n.announcement.stealth_address),
                                "balance_wei": n.balance_wei.to_string(),
                                "balance": n.balance_formatted,
                                "view_tag": n.announcement.view_tag,
                            })
                        })
                        .collect();
                    let _ = reply.send(Ok(serde_json::json!({
                        "notes": rows,
                        "count": rows.len()
                    })));
                }
                McpHostRequest::StealthSweep {
                    stealth_address,
                    reply,
                } => {
                    if !self.wallet().is_unlocked() {
                        let _ =
                            reply.send(Err(ProviderError::Unauthorized("wallet is locked".into())));
                        continue;
                    }
                    let notes = match self.handle.block_on(self.wallet().scan_stealth_notes()) {
                        Ok(n) => n,
                        Err(e) => {
                            let _ = reply.send(Err(ProviderError::Internal(e.user_message())));
                            continue;
                        }
                    };
                    let Some(note) = notes.iter().find(|n| {
                        format!("{:#x}", n.announcement.stealth_address)
                            .eq_ignore_ascii_case(&stealth_address)
                    }) else {
                        let _ = reply.send(Err(ProviderError::Internal(format!(
                            "no unswept stealth note for {stealth_address}"
                        ))));
                        continue;
                    };
                    let kind = ApprovalKind::StealthSweep {
                        stealth_address: stealth_address.clone(),
                        balance_display: note.balance_formatted.clone(),
                    };
                    if crate::sentient_mcp::mcp_auto_exec_enabled(self.wallet().profile_name()) {
                        let result = match self.sentient_breaker() {
                            Ok(breaker) => crate::sentient_mcp::auto_exec_stealth_sweep(
                                &self.wallet(),
                                &self.handle,
                                &breaker,
                                &kind,
                            ),
                            Err(e) => Err(e),
                        };
                        let _ = reply.send(result);
                        continue;
                    }
                    if self.pending_approval.is_some() {
                        let _ = reply.send(Err(ProviderError::Internal(
                            "another approval is pending".into(),
                        )));
                        continue;
                    }
                    let preview = provider::describe_approval(&kind, &self.wallet(), &self.handle);
                    let (title, details) = match preview {
                        Ok(preview) => preview,
                        Err(error) => {
                            let _ = reply.send(Err(error));
                            continue;
                        }
                    };
                    self.approve_return = self.screen();
                    self.view =
                        View::Approve(ApproveView::new(title, Some("MCP stealth".into()), details));
                    self.pending_approval = Some(PendingApproval {
                        kind,
                        reply: PendingReply::Sign(reply),
                        shown_at: std::time::Instant::now(),
                    });
                    break;
                }
            }
        }
    }

    /// The account list visible to dApps: the active account, or `[]` when the
    /// wallet is locked/uninitialized.
    fn visible_accounts(&self) -> Vec<String> {
        let wallet = self.wallet();
        if !wallet.is_unlocked() {
            return Vec::new();
        }
        wallet
            .active_address()
            .map(|a| vec![a.to_string()])
            .unwrap_or_default()
    }

    /// Write the current site grants beside the vault (best-effort: a failed
    /// write only means the next restart asks dApps to reconnect once more).
    fn persist_connected_sites(&self) {
        if let Some(dir) = self.wallet().path().parent() {
            if let Err(e) =
                vaughan_core::core::site_grants::save(dir, &self.connected_sites.read().unwrap())
            {
                tracing::warn!(error = %e, "site grants save failed");
            }
        }
    }

    /// `wallet_switchEthereumChain`: switch to any configured network by chain id.
    fn switch_chain(&mut self, chain_id: &str) -> Result<(), ProviderError> {
        let decimal_id: u64 = chain_id
            .parse()
            .map_err(|_| ProviderError::UnrecognizedChain(chain_id.to_string()))?;
        let net = resolve_switch_chain_id(decimal_id)
            .ok_or_else(|| ProviderError::UnrecognizedChain(chain_id.to_string()))?;
        self.wallet()
            .set_active_network(&net.id)
            .map_err(|e| ProviderError::Internal(e.user_message()))?;
        self.events
            .publish(ProviderEvent::ChainChanged(format!("0x{:x}", net.chain_id)));
        Ok(())
    }

    /// Soft quit: Enter confirms Yes (default), Esc / No cancels.
    fn handle_quit_confirm_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Hard abort still available mid-dialog.
        if ctrl && matches!(key.code, KeyCode::Char('c')) {
            self.quitting = true;
            self.quit_confirm = None;
            return;
        }
        match key.code {
            KeyCode::Enter => {
                if self.quit_confirm == Some(true) {
                    self.quitting = true;
                }
                self.quit_confirm = None;
            }
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.quitting = true;
                self.quit_confirm = None;
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                self.quit_confirm = None;
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                if let Some(yes) = self.quit_confirm.as_mut() {
                    *yes = !*yes;
                }
            }
            _ => {}
        }
    }

    /// Resolve the on-screen approval: deny on `n`/Esc, approve on `y` (not bare
    /// Enter alone after debounce — Enter still works after the debounce window).
    /// Ctrl+C/Ctrl+Q still quit; dropping the reply channel rejects the request.
    fn handle_approval_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('q')) {
            self.quit_confirm = Some(true);
            return;
        }
        let Some(pending) = self.pending_approval.as_ref() else {
            return;
        };
        if pending.shown_at.elapsed() < APPROVE_DEBOUNCE {
            return;
        }
        // Fee editor gets first crack: speed presets, custom gwei input, and
        // vetoes on decision keys while the custom input is focused/invalid.
        if let View::Approve(view) = &mut self.view {
            match view.handle_fee_key(key) {
                crate::views::approve::FeeKeyOutcome::Consumed
                | crate::views::approve::FeeKeyOutcome::Blocked => return,
                crate::views::approve::FeeKeyOutcome::NotHandled => {}
            }
        }
        let approve = matches!(
            key.code,
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter
        );
        let deny = matches!(
            key.code,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc
        );
        if !approve && !deny {
            return;
        }
        let Some(pending) = self.pending_approval.take() else {
            return;
        };
        if deny {
            if let ApprovalKind::McpProposal { proposal_id, .. } = &pending.kind {
                self.reject_queued_proposal(proposal_id, "user rejected");
            }
            match pending.reply {
                PendingReply::Sign(reply) => {
                    let _ = reply.send(Err(ProviderError::UserRejected));
                }
                PendingReply::LocalSign | PendingReply::Queued => {}
                PendingReply::Accounts(reply) => {
                    let _ = reply.send(Err(ProviderError::UserRejected));
                }
                PendingReply::Switch(reply) => {
                    let _ = reply.send(Err(ProviderError::UserRejected));
                }
            }
            let back = self.approve_return;
            self.navigate(back);
            return;
        }

        match pending.reply {
            PendingReply::Accounts(reply) => {
                if let ApprovalKind::Connect { site } = &pending.kind {
                    self.connected_sites.write().unwrap().insert(site.clone());
                    self.persist_connected_sites();
                }
                let _ = reply.send(Ok(self.visible_accounts()));
            }
            PendingReply::Switch(reply) => {
                let result = if let ApprovalKind::SwitchChain { chain_id, .. } = &pending.kind {
                    self.switch_chain(chain_id)
                } else {
                    Err(ProviderError::Internal("switch reply mismatch".into()))
                };
                let _ = reply.send(result);
            }
            PendingReply::Sign(reply) => {
                let mut kind = pending.kind;
                // Apply the fee the user adjusted in the prompt (if any) so
                // what they saw is exactly what gets signed.
                if let View::Approve(view) = &self.view {
                    if let Some(fee) = view.adjusted_fee() {
                        match &mut kind {
                            ApprovalKind::SendTransaction(tx)
                            | ApprovalKind::SignTransaction(tx) => {
                                provider::apply_fee_override(tx, &fee)
                            }
                            _ => {}
                        }
                    }
                }
                let result = provider::execute_approval_sync(&kind, &self.wallet(), &self.handle);
                let _ = reply.send(result);
            }
            PendingReply::LocalSign => {
                let kind = pending.kind;
                let result = provider::execute_approval_sync(&kind, &self.wallet(), &self.handle);
                match result {
                    Ok(sig) => self.set_flash(format!("EIP-712 signature: {sig}")),
                    Err(e) => self.set_flash(format!("Sign failed: {e}")),
                }
            }
            PendingReply::Queued => {
                // File-queue proposal: execute writes mark_approved back to
                // the queue; surface the outcome as a flash.
                let kind = pending.kind;
                let result = provider::execute_approval_sync(&kind, &self.wallet(), &self.handle);
                match result {
                    Ok(hash) => self.set_flash(format!("Queued proposal executed: {hash}")),
                    Err(e) => self.set_flash(format!("Queued proposal failed: {e}")),
                }
            }
        }
        let back = self.approve_return;
        self.navigate(back);
    }

    /// Browserless EIP-712: show the standard Approve card for pasted typed data.
    fn begin_local_typed_data_sign(&mut self, typed_data: serde_json::Value) {
        if self.pending_approval.is_some() {
            self.set_flash("another approval is pending");
            return;
        }
        if !self.wallet().is_unlocked() {
            self.set_flash("unlock wallet first");
            return;
        }
        let address = {
            let wallet = self.wallet();
            wallet.active_address().map(|a| a.to_string())
        };
        let address = match address {
            Ok(a) => a,
            Err(e) => {
                self.set_flash(e.user_message());
                return;
            }
        };
        let kind = ApprovalKind::SignTypedData {
            address,
            typed_data,
        };
        let preview = {
            let wallet = self.wallet();
            provider::describe_approval(&kind, &wallet, &self.handle)
        };
        let (title, details) = match preview {
            Ok(p) => p,
            Err(e) => {
                self.set_flash(format!("{e}"));
                return;
            }
        };
        self.approve_return = self.screen();
        self.view = View::Approve(ApproveView::new(
            title,
            Some("Local (browserless)".into()),
            details,
        ));
        self.pending_approval = Some(PendingApproval {
            kind,
            reply: PendingReply::LocalSign,
            shown_at: std::time::Instant::now(),
        });
    }

    fn network_label_for_chain_hex(chain_id: &str) -> String {
        if let Ok(id) = chain_id.trim().parse::<u64>() {
            if let Some(net) = resolve_switch_chain_id(id) {
                return net.name.to_string();
            }
        }
        let Ok(id) = u64::from_str_radix(chain_id.trim_start_matches("0x"), 16) else {
            return chain_id.to_string();
        };
        resolve_switch_chain_id(id)
            .or_else(|| get_network_by_chain_id(id))
            .map(|n| n.name.to_string())
            .unwrap_or_else(|| format!("chain {id}"))
    }

    /// Build the default view for `screen`. Chrome/asset RPC refresh is limited
    /// to screens that need fresh data — other views reuse cached chrome.
    fn navigate(&mut self, screen: Screen) {
        self.settle_chrome_before_navigate();

        let view = match screen {
            Screen::Onboarding => View::Onboarding(OnboardingView::default()),
            Screen::Unlock => {
                let profile = self.wallet().profile_name().to_string();
                View::Unlock(UnlockView::new(&profile))
            }
            Screen::Dashboard => {
                let mut v = DashboardView::loading();
                v.sync_from_chrome(&self.chrome);
                View::Dashboard(v)
            }
            Screen::AaSend => View::AaSend(AaSendView::default()),
            Screen::Receive => View::Receive(ReceiveView::default()),
            Screen::Settings => {
                let wallet = self.wallet();
                let active_id = wallet.networks().active_id().to_string();
                let selected = wallet
                    .networks()
                    .networks()
                    .iter()
                    .position(|n| n.id == active_id)
                    .unwrap_or(0);
                View::Settings(SettingsView::new(selected))
            }
            Screen::Keys => View::Keys(KeysView::default()),
            Screen::Dapps => View::Dapps(DappsView::default()),
            Screen::Assets => View::Assets(AssetsView::loading()),
            Screen::Browser => View::Browser(BrowserView::default()),
            Screen::Dex => {
                let chain_id = self.wallet().networks().active().chain_id;
                View::Dex(DexView::for_chain(chain_id))
            }
            Screen::Aggregator => {
                let chain_id = self.wallet().networks().active().chain_id;
                View::Aggregator(AgView::for_chain(chain_id))
            }
            Screen::SoonNft => View::Placeholder(PlaceholderView::new(
                Screen::SoonNft,
                "NFT",
                "NFT gallery / transfers will land here.",
            )),
            Screen::Bridge => {
                let chain_id = self.wallet().networks().active().chain_id;
                View::Bridge(BridgeView::for_wallet_chain(chain_id))
            }
            Screen::SoonStake => View::Placeholder(PlaceholderView::new(
                Screen::SoonStake,
                "Stake",
                "Staking / liquid stake UI will land here.",
            )),
            Screen::History => {
                View::History(HistoryView::with_broadcasts(self.recent_broadcasts.clone()))
            }
            Screen::Approvals => View::Approvals(ApprovalsView::loading()),
            Screen::Wrap => {
                let chain_id = self.wallet().networks().active().chain_id;
                View::Wrap(WrapView::for_chain(chain_id))
            }
            // Approve is entered directly from `poll_provider`
            // pending request + reply channel), never via navigation; this arm
            // is only here to keep the match exhaustive.
            Screen::Approve => View::Approve(ApproveView::new(
                "Approve request".to_string(),
                None,
                Vec::new(),
            )),
        };
        self.view = view;
        match screen {
            Screen::Onboarding | Screen::Unlock | Screen::Approve => {}
            Screen::Assets => {
                self.refresh_chrome();
                self.spawn_refresh_assets();
            }
            Screen::Dex | Screen::Aggregator => {
                self.refresh_chrome();
                if self.chrome.assets.is_empty() {
                    self.spawn_refresh_assets();
                }
            }
            Screen::History => {
                self.refresh_chrome();
                if let View::History(v) = &self.view {
                    self.spawn_job(v.initial_job());
                }
            }
            Screen::Approvals => {
                self.refresh_chrome();
                self.spawn_job(ApprovalsView::initial_job());
            }
            Screen::Dashboard => {
                self.refresh_chrome();
                if self.chrome.assets.is_empty() {
                    self.spawn_refresh_assets();
                }
            }
            _ => {}
        }
    }

    /// Refresh always-on network / balance / gas chrome (unlocked screens).
    fn refresh_chrome(&mut self) {
        if !self.wallet().is_unlocked() || self.chrome.loading {
            return;
        }
        self.chrome.loading = true;
        self.chrome.error = None;
        self.spawn_job(UiJob::RefreshChrome);
    }

    /// Re-fetch the F2 asset list (native + ERC-20). Safe to call often; coalesces
    /// when a refresh is already in flight.
    fn spawn_refresh_assets(&mut self) {
        if !self.wallet().is_unlocked() || self.chrome.assets_loading {
            return;
        }
        self.chrome.assets_loading = true;
        self.spawn_job(UiJob::RefreshAssets);
    }

    /// Dex / Ag: ↑/↓ on Token in or Token out cycles the wallet asset list.
    fn cycle_active_token_field(&mut self, forward: bool) -> bool {
        if !matches!(self.screen(), Screen::Dex | Screen::Aggregator) {
            return false;
        }
        if self.chrome.assets.is_empty() {
            let on_token_field = match &mut self.view {
                View::Dex(v) => v.cycle_focused_token_picker(&[], forward),
                View::Aggregator(v) => v.cycle_focused_token_picker(&[], forward),
                _ => false,
            };
            if !on_token_field {
                return false;
            }
            if !self.chrome.assets_loading {
                self.chrome.assets_loading = true;
                self.spawn_job(UiJob::RefreshAssets);
            }
            return true;
        }
        let assets = self.chrome.assets.clone();
        match &mut self.view {
            View::Dex(v) => v.cycle_focused_token_picker(&assets, forward),
            View::Aggregator(v) => v.cycle_focused_token_picker(&assets, forward),
            _ => false,
        }
    }

    /// F1 / F2 / F3 focus + ↑/↓ preview + Enter set / Esc cancel.
    /// Returns true if the key was consumed.
    fn handle_chrome_hotkey(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::F(1) => {
                self.begin_chrome_focus(ChromeFocus::Network);
                true
            }
            KeyCode::F(2) => {
                self.begin_chrome_focus(ChromeFocus::Asset);
                if self.chrome.assets.is_empty() && !self.chrome.assets_loading {
                    self.chrome.assets_loading = true;
                    self.spawn_job(UiJob::RefreshAssets);
                }
                true
            }
            KeyCode::F(3) => {
                self.begin_chrome_focus(ChromeFocus::Account);
                true
            }
            // F4/F5 are home send fields — clear F1–F3 so ↑/↓ reach the form.
            KeyCode::F(4) | KeyCode::F(5) => {
                if self.chrome.focus != ChromeFocus::None {
                    self.cancel_chrome_focus();
                }
                false
            }
            KeyCode::Enter if self.chrome.focus != ChromeFocus::None => {
                self.commit_chrome_focus();
                true
            }
            KeyCode::Esc if self.chrome.focus != ChromeFocus::None => {
                self.cancel_chrome_focus();
                true
            }
            KeyCode::Up | KeyCode::Down if self.chrome.focus != ChromeFocus::None => {
                let forward = matches!(key.code, KeyCode::Down);
                self.preview_chrome_cycle(forward);
                true
            }
            _ => false,
        }
    }

    /// When leaving a screen, bind F3's visible account as active (no re-entry)
    /// so Keys / signing match the status strip. Other chrome focus is cancelled.
    fn settle_chrome_before_navigate(&mut self) {
        match self.chrome.focus {
            ChromeFocus::Account => {
                if let Some(index) = self.chrome.pending_account_index {
                    let ok = {
                        let mut w = self.wallet.lock().unwrap_or_else(|e| e.into_inner());
                        w.set_active_account(index).is_ok()
                    };
                    if ok {
                        let accounts = self.visible_accounts();
                        self.events
                            .publish(ProviderEvent::AccountsChanged(accounts));
                        self.chrome.assets.clear();
                        self.chrome.asset_idx = 0;
                    }
                }
                self.cancel_chrome_focus();
            }
            ChromeFocus::None => {}
            _ => self.cancel_chrome_focus(),
        }
    }

    fn clear_chrome_pending(&mut self) {
        self.chrome.pending_network_idx = None;
        self.chrome.pending_asset_idx = None;
        self.chrome.pending_account_index = None;
    }

    fn cancel_chrome_focus(&mut self) {
        self.clear_chrome_pending();
        self.chrome.focus = ChromeFocus::None;
    }

    fn begin_chrome_focus(&mut self, focus: ChromeFocus) {
        self.clear_chrome_pending();
        self.chrome.focus = focus;
        match focus {
            ChromeFocus::Network => {
                let active_id = self.wallet().networks().active_id().to_string();
                let idx = self
                    .wallet()
                    .networks()
                    .networks()
                    .iter()
                    .position(|n| n.id == active_id);
                self.chrome.pending_network_idx = idx.or(Some(0));
            }
            ChromeFocus::Asset => {
                self.chrome.pending_asset_idx = Some(self.chrome.asset_idx);
            }
            ChromeFocus::Account => {
                let idx = self.wallet().active_account_index().ok();
                self.chrome.pending_account_index = idx;
            }
            ChromeFocus::None => {}
        }
    }

    fn preview_chrome_cycle(&mut self, forward: bool) {
        match self.chrome.focus {
            ChromeFocus::Network => {
                let n = self.wallet().networks().networks().len();
                if n == 0 {
                    return;
                }
                let cur = self.chrome.pending_network_idx.unwrap_or(0);
                self.chrome.pending_network_idx = Some(if forward {
                    (cur + 1) % n
                } else {
                    (cur + n - 1) % n
                });
            }
            ChromeFocus::Asset => {
                let n = self.chrome.assets.len();
                if n == 0 {
                    if !self.chrome.assets_loading {
                        self.chrome.assets_loading = true;
                        self.spawn_job(UiJob::RefreshAssets);
                    }
                    return;
                }
                let cur = self
                    .chrome
                    .pending_asset_idx
                    .unwrap_or(self.chrome.asset_idx);
                self.chrome.pending_asset_idx = Some(if forward {
                    (cur + 1) % n
                } else {
                    (cur + n - 1) % n
                });
            }
            ChromeFocus::Account => {
                let Ok(choices) = self.wallet().account_choices() else {
                    return;
                };
                if choices.is_empty() {
                    return;
                }
                let cur_idx = self.chrome.pending_account_index.unwrap_or(choices[0].0);
                let pos = choices.iter().position(|(i, _)| *i == cur_idx).unwrap_or(0);
                let next = if forward {
                    (pos + 1) % choices.len()
                } else {
                    (pos + choices.len() - 1) % choices.len()
                };
                self.chrome.pending_account_index = Some(choices[next].0);
            }
            ChromeFocus::None => {}
        }
    }

    fn commit_chrome_focus(&mut self) {
        match self.chrome.focus {
            ChromeFocus::Network => {
                if let Some(idx) = self.chrome.pending_network_idx {
                    self.apply_chrome_network(idx);
                }
            }
            ChromeFocus::Asset => {
                if let Some(idx) = self.chrome.pending_asset_idx {
                    if idx < self.chrome.assets.len() {
                        self.chrome.asset_idx = idx;
                        if let View::Dashboard(v) = &mut self.view {
                            v.sync_from_chrome(&self.chrome);
                        }
                    }
                }
            }
            ChromeFocus::Account => {
                if let Some(index) = self.chrome.pending_account_index {
                    self.apply_chrome_account(index);
                }
            }
            ChromeFocus::None => {}
        }
        self.cancel_chrome_focus();
    }

    fn apply_chrome_network(&mut self, list_idx: usize) {
        let (id, name, chain_id) = {
            let mut w = self.wallet.lock().unwrap_or_else(|e| e.into_inner());
            let nets = w.networks().networks();
            let Some(net) = nets.get(list_idx) else {
                return;
            };
            let id = net.id.clone();
            let name = net.name.clone();
            let chain_id = net.chain_id;
            if w.set_active_network(&id).is_err() {
                return;
            }
            (id, name, chain_id)
        };
        let _ = id;
        self.events
            .publish(ProviderEvent::ChainChanged(format!("0x{chain_id:x}")));
        self.chrome.assets.clear();
        self.chrome.asset_idx = 0;
        self.chrome.pending_asset_address = None;
        self.chrome.error = None;
        match self.screen() {
            Screen::Dex | Screen::Aggregator | Screen::Settings => {
                self.navigate(self.screen());
            }
            _ => {}
        }
        self.refresh_chrome();
        self.spawn_refresh_assets();
        if let View::Dashboard(v) = &mut self.view {
            v.sync_from_chrome(&self.chrome);
        }
        tracing::debug!(%name, "chrome F1 set network");
    }

    fn apply_chrome_account(&mut self, index: u32) {
        let ok = {
            let mut w = self.wallet.lock().unwrap_or_else(|e| e.into_inner());
            w.set_active_account(index).is_ok()
        };
        if !ok {
            return;
        }
        let accounts = self.visible_accounts();
        self.events
            .publish(ProviderEvent::AccountsChanged(accounts));
        self.chrome.assets.clear();
        self.chrome.asset_idx = 0;
        self.chrome.pending_asset_address = None;
        self.refresh_chrome();
        self.spawn_refresh_assets();
        if matches!(
            self.screen(),
            Screen::Dex | Screen::Aggregator | Screen::Receive
        ) {
            self.navigate(self.screen());
        }
    }

    fn spawn_job(&self, job: UiJob) {
        let tx = self.job_tx.clone();
        let handle = self.handle.clone();
        let wallet = self.wallet.clone();
        std::thread::spawn(move || {
            // Never hold the wallet mutex across RPC / signing awaits — that freezes the
            // TUI on `[busy]` / `(busy)` (seen after unlock → Sentient → Dashboard chrome refresh).
            let result = match job {
                UiJob::Unlock { password, mode } => {
                    // Clone the vault under a brief lock, then run the Argon2id
                    // KDF unlocked — holding the mutex across it would freeze
                    // the render loop (and the spinner with it).
                    let payload = {
                        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                        w.unlock_payload()
                    };
                    UiJobResult::Unlock(match payload {
                        Ok(p) => p
                            .decrypt(&password)
                            .map(|accounts| UnlockCompletion { accounts, mode }),
                        Err(e) => Err(e),
                    })
                }
                UiJob::RefreshChrome => {
                    let snap = {
                        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                        w.chrome_rpc_snapshot()
                    };
                    UiJobResult::Chrome(match snap {
                        Ok(s) => handle.block_on(s.fetch_chrome()),
                        Err(e) => Err(e),
                    })
                }
                UiJob::RefreshBalance => {
                    let snap = {
                        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                        w.chrome_rpc_snapshot()
                    };
                    UiJobResult::Balance(match snap {
                        Ok(s) => handle.block_on(s.balance()),
                        Err(e) => Err(e),
                    })
                }
                UiJob::RefreshAssets => {
                    let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                    UiJobResult::Assets(handle.block_on(w.assets()))
                }
                UiJob::EstimateFee { to, value_wei } => {
                    let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                    UiJobResult::Fee(handle.block_on(w.estimate_fee(&to, &value_wei)))
                }
                UiJob::EstimateTokenFee { token, to, amount } => {
                    let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                    UiJobResult::Fee(handle.block_on(w.estimate_token_fee(&token, &to, &amount)))
                }
                UiJob::SendWithFee { to, value_wei, fee } => {
                    let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                    UiJobResult::Send(handle.block_on(w.send_with_fee(&to, &value_wei, &fee)))
                }
                UiJob::Send { to, value_wei } => {
                    let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                    UiJobResult::Send(handle.block_on(w.send(&to, &value_wei)))
                }
                UiJob::SendToken { token, to, amount } => {
                    let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                    UiJobResult::Send(handle.block_on(w.send_token(&token, &to, &amount)))
                }
                UiJob::SendTokenWithFee {
                    token,
                    to,
                    amount,
                    fee,
                } => {
                    let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                    UiJobResult::Send(
                        handle.block_on(w.send_token_with_fee(&token, &to, &amount, &fee)),
                    )
                }
                UiJob::SendStealth {
                    announcement,
                    value_wei,
                } => {
                    let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                    UiJobResult::SendStealth(
                        handle.block_on(w.send_stealth(&announcement, &value_wei)),
                    )
                }
                UiJob::SendEvm { tx } => {
                    let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                    UiJobResult::Send(handle.block_on(w.broadcast(tx, "Contract")))
                }
                UiJob::EstimateEvmFee { tx } => {
                    let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                    UiJobResult::Fee(handle.block_on(w.estimate_transaction_fee(tx)))
                }
                UiJob::SendEvmWithFee { tx, fee } => {
                    let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                    UiJobResult::Send(handle.block_on(w.send_evm_with_fee(tx, &fee)))
                }
                UiJob::AggQuote {
                    venue,
                    token_in,
                    token_out,
                    amount,
                    slippage,
                    native_in,
                    native_out,
                    account,
                } => {
                    use vaughan_core::core::quote_aggregator;

                    let parsed = parse_agg_quote_request(
                        &token_in,
                        &token_out,
                        &amount,
                        slippage,
                        native_in,
                        native_out,
                        account.as_deref(),
                    );
                    let dir = StateManager::default_path()
                        .ok()
                        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
                    let chain_id = wallet
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .networks()
                        .active()
                        .chain_id;
                    UiJobResult::AggQuote(match parsed {
                        Ok(req) => handle.block_on(quote_aggregator(
                            venue,
                            &req,
                            chain_id,
                            dir.as_deref(),
                            None,
                        )),
                        Err(e) => Err(e),
                    })
                }
                UiJob::AggCompareQuote {
                    token_in,
                    token_out,
                    amount,
                    slippage,
                    native_in,
                    native_out,
                    account,
                } => {
                    use vaughan_core::core::quote_live_aggregators;

                    let parsed = parse_agg_quote_request(
                        &token_in,
                        &token_out,
                        &amount,
                        slippage,
                        native_in,
                        native_out,
                        account.as_deref(),
                    );
                    let dir = StateManager::default_path()
                        .ok()
                        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
                    let chain_id = wallet
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .networks()
                        .active()
                        .chain_id;
                    UiJobResult::AggCompareQuote(match parsed {
                        Ok(req) => handle.block_on(quote_live_aggregators(
                            &req,
                            chain_id,
                            dir.as_deref(),
                            None,
                        )),
                        Err(e) => vec![vaughan_core::core::AggQuoteOutcome {
                            venue: vaughan_core::core::AggVenue::SquirrelSwap,
                            result: Err(e),
                        }],
                    })
                }
                UiJob::DexQuote {
                    quote_gen,
                    chain_id,
                    rpc_url,
                    protocol_v2,
                    router,
                    amount_in,
                    fee,
                    path,
                } => {
                    use alloy::primitives::{Address, U256};
                    use std::str::FromStr;
                    use vaughan_core::core::{quote_v2_exact_in, quote_v3_exact_in};

                    let parsed = (|| -> Result<vaughan_core::core::DexQuote, WalletError> {
                        let amount_in = U256::from_str(&amount_in)
                            .map_err(|_| WalletError::InvalidAmount("dex amount".into()))?;
                        let hops: Result<Vec<Address>, _> = path
                            .iter()
                            .map(|s| {
                                Address::from_str(s.trim()).map_err(|_| {
                                    WalletError::InvalidTransaction("dex path token".into())
                                })
                            })
                            .collect();
                        let hops = hops?;
                        if protocol_v2 {
                            let router = Address::from_str(&router).map_err(|_| {
                                WalletError::InvalidTransaction("dex router".into())
                            })?;
                            handle.block_on(quote_v2_exact_in(&rpc_url, router, amount_in, &hops))
                        } else {
                            if hops.len() != 2 {
                                return Err(WalletError::Other(
                                    "V3 quote requires a single-hop path".into(),
                                ));
                            }
                            handle.block_on(quote_v3_exact_in(
                                &rpc_url, chain_id, hops[0], hops[1], amount_in, fee,
                            ))
                        }
                    })();
                    UiJobResult::DexQuote {
                        quote_gen,
                        result: parsed,
                    }
                }
                UiJob::BridgeQuote {
                    src_token,
                    dst_token,
                    amount,
                    src_chain,
                    dst_chain,
                    recipient,
                } => {
                    use alloy::primitives::{Address, U256};
                    use vaughan_core::core::{BridgeAsset, BridgeQuoteRequest, LibertySwapClient};

                    let parsed = (|| -> Result<BridgeQuoteRequest, WalletError> {
                        let amount = U256::from_str(&amount)
                            .map_err(|_| WalletError::InvalidAmount("bridge amount".into()))?;
                        let recipient = Address::from_str(&recipient).map_err(|_| {
                            WalletError::InvalidTransaction("bridge recipient".into())
                        })?;
                        let src_token = if src_token.eq_ignore_ascii_case("USDC") {
                            BridgeAsset::Symbol("USDC")
                        } else {
                            BridgeAsset::Address(Address::from_str(&src_token).map_err(|_| {
                                WalletError::InvalidTransaction("bridge src_token".into())
                            })?)
                        };
                        let dst_token = if dst_token.eq_ignore_ascii_case("USDC") {
                            BridgeAsset::Symbol("USDC")
                        } else {
                            BridgeAsset::Address(Address::from_str(&dst_token).map_err(|_| {
                                WalletError::InvalidTransaction("bridge dst_token".into())
                            })?)
                        };
                        Ok(BridgeQuoteRequest {
                            src_token,
                            dst_token,
                            amount,
                            src_chain,
                            dst_chain,
                            recipient,
                        })
                    })();
                    UiJobResult::BridgeQuote(Box::new(match parsed {
                        Ok(req) => match LibertySwapClient::public() {
                            Ok(client) => handle.block_on(client.quote(&req)),
                            Err(e) => Err(e),
                        },
                        Err(e) => Err(e),
                    }))
                }
                UiJob::RefreshActivity { limit } => {
                    let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                    UiJobResult::Activity(handle.block_on(w.activity(limit)))
                }
                UiJob::RefreshAllowances => {
                    let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                    UiJobResult::Allowances(handle.block_on(w.list_allowances()))
                }
                UiJob::PollTxStatus { tx_hash } => {
                    let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                    UiJobResult::TxStatus(handle.block_on(w.get_tx_status(&tx_hash)))
                }
                UiJob::RefreshBroadcastStatuses { hashes } => {
                    let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                    let mut out = Vec::with_capacity(hashes.len());
                    let mut err = None;
                    for h in hashes {
                        match handle.block_on(w.get_tx_status(&h)) {
                            Ok(s) => out.push((h, s)),
                            Err(e) => {
                                err = Some(e);
                                break;
                            }
                        }
                    }
                    UiJobResult::BroadcastStatuses(match err {
                        Some(e) => Err(e),
                        None => Ok(out),
                    })
                }
                UiJob::ReplaceBroadcast { entry, kind } => {
                    let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                    UiJobResult::Send(handle.block_on(w.replace_broadcast(&entry, kind)))
                }
            };
            // Plain std thread: blocking_send applies backpressure if the UI
            // queue is full; Err means the UI is gone, result is droppable.
            let _ = tx.blocking_send(result);
        });
    }

    fn poll_jobs(&mut self) {
        while let Ok(result) = self.job_rx.try_recv() {
            match result {
                UiJobResult::Unlock(r) => {
                    match r {
                        Ok(done) => {
                            let address = {
                                let mut wallet =
                                    self.wallet.lock().unwrap_or_else(|e| e.into_inner());
                                wallet.apply_unlocked_accounts(done.accounts);
                                // Mode locks for the session (FR-5.1).
                                wallet.set_operating_mode(done.mode);
                                wallet.active_address().map(|a| a.to_string()).ok()
                            };
                            if let Some(addr) = address {
                                self.events
                                    .publish(ProviderEvent::AccountsChanged(vec![addr]));
                            }
                            self.navigate(Screen::Dashboard);
                        }
                        Err(e) => {
                            // Back to the password prompt with the error shown.
                            if let View::Unlock(v) = &mut self.view {
                                v.unlock_failed(e.user_message());
                            }
                        }
                    }
                }
                UiJobResult::Chrome(r) => {
                    self.chrome.loading = false;
                    match r {
                        Ok((bal, gas)) => {
                            if let View::Dashboard(v) = &mut self.view {
                                v.apply_balance(Ok(bal.clone()));
                            }
                            self.chrome.balance = Some(bal);
                            self.chrome.gas_gwei = Some(gas);
                            self.chrome.error = None;
                        }
                        Err(e) => {
                            self.chrome.error = Some(e.user_message());
                            if let View::Dashboard(v) = &mut self.view {
                                v.apply_balance(Err(e));
                            }
                        }
                    }
                }
                UiJobResult::Balance(r) => {
                    if let View::Dashboard(v) = &mut self.view {
                        v.apply_balance(r);
                    }
                }
                UiJobResult::Assets(r) => {
                    self.chrome.assets_loading = false;
                    match &r {
                        Ok(assets) => {
                            self.chrome.assets = chrome_assets_from_fetch(assets.clone());
                            if self.chrome.asset_idx >= self.chrome.assets.len() {
                                self.chrome.asset_idx = self.chrome.assets.len().saturating_sub(1);
                            }
                            if let Some(pending) = self.chrome.pending_asset_idx {
                                if pending >= self.chrome.assets.len() {
                                    self.chrome.pending_asset_idx =
                                        Some(self.chrome.assets.len().saturating_sub(1));
                                }
                            }
                            if let Some(addr) = self.chrome.pending_asset_address.take() {
                                if let Some(i) = asset_index_for_address(&self.chrome.assets, &addr)
                                {
                                    self.chrome.asset_idx = i;
                                }
                            }
                        }
                        Err(e) => {
                            self.chrome.error = Some(e.user_message());
                        }
                    }
                    match &mut self.view {
                        View::Assets(v) => v.apply_assets(r),
                        View::Dashboard(v) => v.sync_from_chrome(&self.chrome),
                        _ => {}
                    }
                }
                other => {
                    let send_ok = matches!(
                        &other,
                        UiJobResult::Send(Ok(_)) | UiJobResult::SendStealth(Ok(_))
                    );
                    let dex_swap_token = if send_ok {
                        if let View::Dex(v) = &self.view {
                            if v.is_completing_swap() {
                                v.token_out_address()
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    if let UiJobResult::Send(Ok(receipt)) = &other {
                        let old = receipt.entry.replaces.clone();
                        push_recent(&mut self.recent_broadcasts, receipt.entry.clone());
                        if let Some(old_hash) = old.as_deref() {
                            mark_replaced(&mut self.recent_broadcasts, old_hash, &receipt.hash);
                        }
                    }
                    if let UiJobResult::BroadcastStatuses(Ok(pairs)) = &other {
                        for (hash, status) in pairs {
                            if let Some(e) =
                                self.recent_broadcasts.iter_mut().find(|b| b.hash == *hash)
                            {
                                e.status = *status;
                            }
                        }
                    }
                    let reload = match &mut self.view {
                        View::Dashboard(v) => {
                            v.apply_job_result(other);
                            v.followup_job()
                        }
                        View::Dex(v) => {
                            v.apply_job_result(other);
                            None
                        }
                        View::Aggregator(v) => {
                            v.apply_job_result(other);
                            None
                        }
                        View::Bridge(v) => {
                            v.apply_job_result(other);
                            None
                        }
                        View::History(v) => {
                            v.apply_job_result(other);
                            None
                        }
                        View::Approvals(v) => {
                            v.apply_job_result(other);
                            v.reload_job()
                        }
                        View::Wrap(v) => {
                            v.apply_job_result(other);
                            None
                        }
                        View::Browser(v) => {
                            v.apply_job_result(other);
                            None
                        }
                        _ => None,
                    };
                    let dex_followup = {
                        let wallet = self.wallet.lock().unwrap_or_else(|e| e.into_inner());
                        if let View::Dex(v) = &mut self.view {
                            v.followup_job(&wallet)
                        } else {
                            None
                        }
                    };
                    if let Some(job) = reload.or(dex_followup) {
                        self.spawn_job(job);
                    }
                    if send_ok {
                        self.refresh_chrome();
                        if let Some(addr) = dex_swap_token {
                            {
                                let mut w = self.wallet.lock().unwrap_or_else(|e| e.into_inner());
                                if let Err(e) = self.handle.block_on(w.import_custom_token(&addr)) {
                                    tracing::warn!(error = %e, "import swap token-out failed");
                                }
                            }
                            self.chrome.pending_asset_address = Some(addr);
                            self.spawn_refresh_assets();
                        } else if !matches!(self.view, View::Dex(_)) {
                            self.spawn_refresh_assets();
                        }
                    }
                }
            }
        }
    }
}

/// What the app should do with a key after the active view handled it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GlobalAction {
    /// Nothing beyond the view's own handling.
    None,
    /// Quit the app.
    Quit,
    /// Cycle to the next screen (Tab navigation).
    CycleScreens,
    /// Jump to a common task screen (when the view did not consume the key).
    Navigate(Screen),
    /// Refresh status chrome (balance + gas).
    RefreshChrome,
    /// Lock the vault and return to the unlock screen.
    Lock,
    /// Cycle stock UI theme (boxes / footer; banner + address unchanged).
    CycleTheme,
    /// Copy the F3-active address to the clipboard.
    CopyAddress,
}

/// When a view swallows a footer chip key without acting on it, retry globally.
fn defer_footer_shortcut(outcome: KeyOutcome, key: KeyEvent, allows: bool) -> KeyOutcome {
    if allows && crate::views::is_footer_shortcut(key) && matches!(outcome, KeyOutcome::Consumed) {
        KeyOutcome::NotHandled
    } else {
        outcome
    }
}

/// Decide global shortcuts for `key` given how the active view handled it.
///
/// Pure and unit-tested: this is where the "typing into a field must not
/// navigate" and "Tab inside a form must not switch screens" rules live.
fn global_action(key: KeyEvent, outcome: &KeyOutcome) -> GlobalAction {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    // Ctrl+C / Ctrl+Q request quit (confirm dialog), even mid-typing.
    if ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('q')) {
        return GlobalAction::Quit;
    }

    // Ctrl+Y copies the chrome (F3) address from any unlocked screen.
    if ctrl && matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
        return GlobalAction::CopyAddress;
    }

    // Only when the active view did not consume the key (forms keep typing safe).
    if !matches!(outcome, KeyOutcome::NotHandled) {
        return GlobalAction::None;
    }

    if matches!(key.code, KeyCode::Char('x') | KeyCode::Char('X')) && !ctrl {
        return GlobalAction::Quit;
    }

    if key.code == KeyCode::Tab {
        return GlobalAction::CycleScreens;
    }

    if key.code == KeyCode::Esc {
        return GlobalAction::Navigate(Screen::Dashboard);
    }

    // Footer shortcuts — available from every unlocked view when idle.
    match key.code {
        KeyCode::Char(c) => match c.to_ascii_lowercase() {
            's' => GlobalAction::Navigate(Screen::Dashboard),
            'v' => GlobalAction::Navigate(Screen::Receive),
            'a' => GlobalAction::Navigate(Screen::Assets),
            'b' => GlobalAction::Navigate(Screen::AaSend),
            'w' => GlobalAction::Navigate(Screen::Dapps),
            'c' => GlobalAction::Navigate(Screen::Browser),
            'd' => GlobalAction::Navigate(Screen::Dex),
            'g' => GlobalAction::Navigate(Screen::Aggregator),
            'n' | 'i' => GlobalAction::Navigate(Screen::Settings),
            'k' => GlobalAction::Navigate(Screen::Keys),
            'e' => GlobalAction::Navigate(Screen::Wrap),
            'f' => GlobalAction::Navigate(Screen::Bridge),
            'j' => GlobalAction::Navigate(Screen::Approvals),
            'm' => GlobalAction::Navigate(Screen::History),
            'o' => GlobalAction::Navigate(Screen::SoonNft),
            'r' => GlobalAction::RefreshChrome,
            'h' => GlobalAction::Navigate(Screen::Dashboard),
            'l' => GlobalAction::Lock,
            't' => GlobalAction::CycleTheme,
            _ => GlobalAction::None,
        },
        _ => GlobalAction::None,
    }
}

fn parse_agg_quote_request(
    token_in: &str,
    token_out: &str,
    amount: &str,
    slippage: f64,
    native_in: bool,
    native_out: bool,
    account: Option<&str>,
) -> Result<vaughan_core::core::AggQuoteRequest, WalletError> {
    use alloy::primitives::{Address, U256};
    use vaughan_core::core::AggQuoteRequest;

    let token_in = Address::from_str(token_in)
        .map_err(|_| WalletError::InvalidTransaction("agg token_in".into()))?;
    let token_out = Address::from_str(token_out)
        .map_err(|_| WalletError::InvalidTransaction("agg token_out".into()))?;
    let amount_in =
        U256::from_str(amount).map_err(|_| WalletError::InvalidAmount("agg amount".into()))?;
    let account = account
        .map(Address::from_str)
        .transpose()
        .map_err(|_| WalletError::InvalidTransaction("agg account".into()))?;
    Ok(AggQuoteRequest {
        token_in,
        token_out,
        token_in_is_native: native_in,
        token_out_is_native: native_out,
        amount_in,
        slippage_percent: slippage,
        account,
    })
}

/// Match native or ERC-20 rows for F2 chrome alignment (checksum-safe).
fn same_f2_asset(a: &vaughan_core::chains::Balance, b: &vaughan_core::chains::Balance) -> bool {
    match (
        a.token.contract_address.as_deref(),
        b.token.contract_address.as_deref(),
    ) {
        (None, None) => true,
        (Some(left), Some(right)) => left.eq_ignore_ascii_case(right),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn tab() -> KeyEvent {
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)
    }

    #[test]
    fn typing_q_never_navigates_when_consumed() {
        assert_eq!(
            global_action(press('q'), &KeyOutcome::Consumed),
            GlobalAction::None
        );
        assert_eq!(
            global_action(press('q'), &KeyOutcome::Navigate(Screen::Dashboard)),
            GlobalAction::None
        );
    }

    #[test]
    fn q_is_inert_when_unhandled() {
        assert_eq!(
            global_action(press('q'), &KeyOutcome::NotHandled),
            GlobalAction::None
        );
    }

    #[test]
    fn x_quits_only_when_unhandled() {
        assert_eq!(
            global_action(press('x'), &KeyOutcome::NotHandled),
            GlobalAction::Quit
        );
        assert_eq!(
            global_action(press('x'), &KeyOutcome::Consumed),
            GlobalAction::None
        );
    }

    #[test]
    fn ctrl_quit_always_wins() {
        assert_eq!(
            global_action(ctrl('c'), &KeyOutcome::Consumed),
            GlobalAction::Quit
        );
        assert_eq!(
            global_action(ctrl('q'), &KeyOutcome::Consumed),
            GlobalAction::Quit
        );
        assert_eq!(
            global_action(ctrl('q'), &KeyOutcome::NotHandled),
            GlobalAction::Quit
        );
    }

    #[test]
    fn tab_cycles_only_when_unhandled() {
        assert_eq!(
            global_action(tab(), &KeyOutcome::Consumed),
            GlobalAction::None
        );
        assert_eq!(
            global_action(tab(), &KeyOutcome::NotHandled),
            GlobalAction::CycleScreens
        );
    }

    #[test]
    fn other_keys_are_inert() {
        assert_eq!(
            global_action(press('p'), &KeyOutcome::NotHandled),
            GlobalAction::None
        );
        assert_eq!(
            global_action(press('q'), &KeyOutcome::Navigate(Screen::Dashboard)),
            GlobalAction::None
        );
    }

    #[test]
    fn defer_footer_shortcut_retries_consumed_chip_keys() {
        assert!(matches!(
            defer_footer_shortcut(KeyOutcome::Consumed, press('d'), true),
            KeyOutcome::NotHandled
        ));
        assert!(matches!(
            defer_footer_shortcut(KeyOutcome::Consumed, press('d'), false),
            KeyOutcome::Consumed
        ));
        assert!(matches!(
            defer_footer_shortcut(KeyOutcome::Navigate(Screen::Dex), press('d'), true),
            KeyOutcome::Navigate(Screen::Dex)
        ));
    }

    #[test]
    fn footer_shortcuts_when_unhandled() {
        assert_eq!(
            global_action(press('s'), &KeyOutcome::NotHandled),
            GlobalAction::Navigate(Screen::Dashboard)
        );
        assert_eq!(
            global_action(press('d'), &KeyOutcome::NotHandled),
            GlobalAction::Navigate(Screen::Dex)
        );
        assert_eq!(
            global_action(press('g'), &KeyOutcome::NotHandled),
            GlobalAction::Navigate(Screen::Aggregator)
        );
        assert_eq!(
            global_action(press('i'), &KeyOutcome::NotHandled),
            GlobalAction::Navigate(Screen::Settings)
        );
        assert_eq!(
            global_action(press('e'), &KeyOutcome::NotHandled),
            GlobalAction::Navigate(Screen::Wrap)
        );
        assert_eq!(
            global_action(press('j'), &KeyOutcome::NotHandled),
            GlobalAction::Navigate(Screen::Approvals)
        );
        assert_eq!(
            global_action(press('m'), &KeyOutcome::NotHandled),
            GlobalAction::Navigate(Screen::History)
        );
        assert_eq!(
            global_action(press('o'), &KeyOutcome::NotHandled),
            GlobalAction::Navigate(Screen::SoonNft)
        );
        assert_eq!(
            global_action(press('r'), &KeyOutcome::NotHandled),
            GlobalAction::RefreshChrome
        );
        assert_eq!(
            global_action(press('l'), &KeyOutcome::NotHandled),
            GlobalAction::Lock
        );
        assert_eq!(
            global_action(key(KeyCode::Esc), &KeyOutcome::NotHandled),
            GlobalAction::Navigate(Screen::Dashboard)
        );
        assert_eq!(
            global_action(press('s'), &KeyOutcome::Consumed),
            GlobalAction::None
        );
        // Ctrl+letter still navigates when the view deferred (agent mid-typing).
        assert_eq!(
            global_action(ctrl('h'), &KeyOutcome::NotHandled),
            GlobalAction::Navigate(Screen::Dashboard)
        );
        assert_eq!(
            global_action(ctrl('v'), &KeyOutcome::NotHandled),
            GlobalAction::Navigate(Screen::Receive)
        );
        assert_eq!(
            global_action(press('t'), &KeyOutcome::NotHandled),
            GlobalAction::CycleTheme
        );
        assert_eq!(
            global_action(ctrl('t'), &KeyOutcome::NotHandled),
            GlobalAction::CycleTheme
        );
        assert_eq!(
            global_action(ctrl('y'), &KeyOutcome::Consumed),
            GlobalAction::CopyAddress
        );
        assert_eq!(
            global_action(ctrl('y'), &KeyOutcome::NotHandled),
            GlobalAction::CopyAddress
        );
    }
}
