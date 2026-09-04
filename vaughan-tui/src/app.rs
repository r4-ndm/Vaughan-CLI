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
use vaughan_core::core::proposal::{ProposalQueue, ProposalType};
use vaughan_core::core::token_launch::TokenLaunchOutcome;
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
    DashboardView, DexView, HexView, HistoryView, KeysView, LpView, OnboardingView, PlaceholderView,
    ReceiveView, SettingsView, TokenLaunchView, UnlockView, WrapView,
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
    Lp,
    History,
    Approvals,
    Wrap,
    Hex,
    TokenLaunch,
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
            Self::Lp => "LP",
            Self::History => "History",
            Self::Approvals => "Approvals",
            Self::Wrap => "Wrap",
            Self::Hex => "HEX",
            Self::TokenLaunch => "Launch",
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
    /// Pop the navigation stack (Esc); falls back to Dashboard when empty.
    Back,
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
    /// Settings: open Home send prefilled for ≥13 WZRD burn to the dead sink.
    AssistBurn,
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
            Self::Back => write!(f, "Back"),
            Self::StartJob(_) => write!(f, "StartJob(..)"),
            Self::SendAsset(_) => write!(f, "SendAsset(..)"),
            Self::Intent(i) => f.debug_tuple("Intent").field(i).finish(),
            Self::SignTypedData(_) => write!(f, "SignTypedData(..)"),
            Self::Flash(s) => f.debug_tuple("Flash").field(s).finish(),
            Self::AssistBurn => write!(f, "AssistBurn"),
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
    Hex(HexView),
    Lp(LpView),
    TokenLaunch(TokenLaunchView),
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
            Self::Hex(_) => Screen::Hex,
            Self::Lp(_) => Screen::Lp,
            Self::TokenLaunch(_) => Screen::TokenLaunch,
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
        mcp_signing: bool,
        tick: u64,
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
            Self::Hex(v) => v.render(frame, area, wallet),
            Self::Lp(v) => v.render(frame, area, wallet, assets),
            Self::TokenLaunch(v) => v.render(frame, area, wallet),
            Self::Placeholder(v) => v.render(frame, area, wallet),
            Self::Approve(v) => v.render(frame, area, wallet, mcp_signing, tick),
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
            Self::Hex(v) => v.handle_key(key, wallet, handle, events),
            Self::Lp(v) => v.handle_key(key, wallet, handle, events),
            Self::TokenLaunch(v) => v.handle_key(key, wallet, handle, events),
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
            Self::Hex(v) => v.allows_footer_shortcuts(),
            Self::Lp(v) => v.allows_footer_shortcuts(),
            Self::TokenLaunch(v) => v.allows_footer_shortcuts(),
            Self::Placeholder(v) => v.allows_footer_shortcuts(),
            Self::Approve(v) => v.allows_footer_shortcuts(),
        }
    }

    /// Swap / Add LP: ↑/↓ on the venue row before the global token asset picker.
    fn try_cycle_venue_selector(&mut self, forward: bool) -> bool {
        match self {
            Self::Dex(v) => v.cycle_venue_selector(forward),
            Self::Lp(v) => v.cycle_venue_selector(forward),
            _ => false,
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

/// MCP token launch approved on-card but deploy runs off-thread (receipt wait).
struct PendingMcpTokenLaunch {
    proposal_id: String,
    reply: Option<oneshot::Sender<Result<String, ProviderError>>>,
}

/// Discard accidental keypresses for this long after an approve prompt appears.
const APPROVE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(400);
/// Auto-deny stale live provider / loopback MCP prompts (dApps ~30–60s).
const APPROVE_TTL: std::time::Duration = std::time::Duration::from_secs(60);
/// File-queue MCP cards may stay on screen longer; on expiry dismiss UI only
/// (proposal stays pending and [`McpService::poll_file_queue`] resurfaces it).
const QUEUED_APPROVE_SCREEN_TTL: std::time::Duration =
    std::time::Duration::from_secs(vaughan_core::core::proposal::PROPOSAL_TTL_SECS);
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
    /// When set, the next [`UiJobResult::DeployToken`] finalizes MCP queue / IPC reply.
    pending_mcp_token_launch: Option<PendingMcpTokenLaunch>,
    /// Screen to return to after the pending approval resolves.
    approve_return: Screen,
    /// File-queue MCP approve running off-thread (keeps UI responsive).
    mcp_approve_inflight: bool,
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
    /// Esc / Back stack — footer jumps push; Esc pops (Dashboard clears it).
    nav_back: Vec<Screen>,
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
            pending_mcp_token_launch: None,
            approve_return: Screen::Dashboard,
            mcp_approve_inflight: false,
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
            nav_back: Vec::new(),
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

    /// Where to return after an approval card. Never chain Approve → Approve (ghost trap).
    fn approve_return_screen(&self) -> Screen {
        match self.screen() {
            Screen::Approve => Screen::Dashboard,
            other => other,
        }
    }

    fn normalize_approve_return(screen: Screen) -> Screen {
        if screen == Screen::Approve {
            Screen::Dashboard
        } else {
            screen
        }
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
            self.view.render(
                frame,
                area,
                &wallet,
                &bridge_line,
                &self.chrome.assets,
                self.mcp_approve_inflight,
                self.tick,
            );
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
            if self.chrome.flash_ticks_left > 0 && !self.chrome.flash_dismiss_on_enter {
                self.chrome.flash_ticks_left -= 1;
                if self.chrome.flash_ticks_left == 0 {
                    self.clear_chrome_flash();
                }
            }
            self.poll_provider();
            self.poll_mcp();
            self.poll_jobs();
            if self.screen() == Screen::Approve
                && self.pending_approval.is_none()
                && !self.mcp_approve_inflight
            {
                self.navigate(Self::normalize_approve_return(self.approve_return));
            }
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
            if let View::Hex(v) = &mut self.view {
                v.set_tick(self.tick);
            }
            if let View::Lp(v) = &mut self.view {
                v.set_tick(self.tick);
            }
            if let View::TokenLaunch(v) = &mut self.view {
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

        // LP Brew success (and similar) stays until Enter.
        if self.chrome.flash_dismiss_on_enter
            && (self.chrome.flash_table.is_some() || self.chrome.flash_title.is_some())
            && matches!(key.code, KeyCode::Enter)
        {
            self.clear_chrome_flash();
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

        // F1–F3 chrome strip (focus · ↑/↓ preview · Enter set · Esc cancel) runs
        // before view handlers so Dex/Ag/LP token ↑/↓ and Browser REPL history
        // cannot steal arrows while a chrome box is focused.
        if self.wallet().is_unlocked() && self.handle_chrome_hotkey(key) {
            return;
        }

        // Approval prompt: fee editor + y/n/Enter/Esc; chrome hotkeys above still apply.
        if self.screen() == Screen::Approve {
            self.handle_approval_key(key);
            return;
        }

        // Dex/Ag/LP: ↑/↓ on venue row or token fields when chrome is idle.
        if self.wallet().is_unlocked() && matches!(key.code, KeyCode::Up | KeyCode::Down) {
            let forward = matches!(key.code, KeyCode::Down);
            if self.view.try_cycle_venue_selector(forward) {
                return;
            }
            if self.cycle_active_token_field(forward) {
                return;
            }
        }

        let before_addr = self
            .wallet()
            .active_address()
            .ok()
            .map(|a| a.to_string());
        let outcome = {
            let mut wallet = self.wallet.lock().unwrap_or_else(|e| e.into_inner());
            self.view
                .handle_key(key, &mut wallet, &self.handle, &self.events)
        };
        let after_addr = self
            .wallet()
            .active_address()
            .ok()
            .map(|a| a.to_string());
        // Keys import (and any path that flips the active account) must refresh F2.
        if before_addr.is_some()
            && after_addr.is_some()
            && before_addr != after_addr
        {
            if let Some(owner) = after_addr {
                self.events
                    .publish(ProviderEvent::AccountsChanged(vec![owner.clone()]));
                self.sync_f2_to_owner(&owner);
            }
        }
        let outcome = defer_footer_shortcut(outcome, key, self.view.allows_footer_shortcuts());

        match global_action(key, &outcome) {
            GlobalAction::Quit => {
                self.quit_confirm = Some(true);
                return;
            }
            GlobalAction::Navigate(screen) => {
                if self.wallet().is_unlocked() && screen != self.screen() {
                    self.navigate(screen);
                }
                return;
            }
            GlobalAction::Back => {
                if self.wallet().is_unlocked() {
                    self.navigate_back();
                }
                return;
            }
            GlobalAction::RefreshChrome => {
                if self.wallet().is_unlocked() {
                    self.sync_f2_to_displayed();
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
                    self.copy_chrome_f3_address();
                }
                return;
            }
            GlobalAction::AssistBurn => {
                if self.wallet().is_unlocked() {
                    self.begin_assist_burn();
                }
                return;
            }
            GlobalAction::None => {}
        }

        match outcome {
            KeyOutcome::Navigate(screen) => self.navigate(screen),
            KeyOutcome::Back => self.navigate_back(),
            KeyOutcome::StartJob(job) => {
                if matches!(job, crate::jobs::UiJob::SendStealth { .. }) && !self.power_features_ok()
                {
                    self.flash_tools_locked();
                } else if matches!(
                    job,
                    UiJob::RefreshChrome { .. } | UiJob::RefreshAssets { .. }
                ) {
                    // Views request a reload; always bind to the F3-displayed owner.
                    self.sync_f2_to_displayed();
                } else {
                    self.spawn_job(job);
                }
            }
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
                    self.spawn_refresh_assets();
                }
                self.view = View::Dashboard(DashboardView::for_asset(balance));
            }
            KeyOutcome::Intent(nav) => self.apply_intent(nav),
            KeyOutcome::SignTypedData(data) => self.begin_local_typed_data_sign(data),
            KeyOutcome::Flash(msg) => self.set_flash(msg),
            KeyOutcome::AssistBurn => self.begin_assist_burn(),
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
        self.chrome.flash_title = None;
        self.chrome.flash_table = None;
        self.chrome.flash_dismiss_on_enter = false;
        self.chrome.flash_ticks_left = 45; // ~a few seconds at UI tick rate
    }

    fn clear_chrome_flash(&mut self) {
        self.chrome.flash = None;
        self.chrome.flash_title = None;
        self.chrome.flash_table = None;
        self.chrome.flash_dismiss_on_enter = false;
        self.chrome.flash_ticks_left = 0;
    }

    /// Post-mint LP Brew success: summary under the address until Enter.
    fn set_success_flash(&mut self, title: impl Into<String>, rows: Vec<(String, String)>) {
        self.chrome.flash_title = Some(title.into());
        self.chrome.flash_table = Some(rows);
        self.chrome.flash = None;
        self.chrome.flash_dismiss_on_enter = true;
        self.chrome.flash_ticks_left = 0;
    }

    /// Height of the chrome flash strip (single line or verification table).
    pub fn chrome_flash_height(&self, width: u16) -> u16 {
        use crate::views::approve::verify_table_compact_lines;
        if let Some(rows) = &self.chrome.flash_table {
            let title = u16::from(self.chrome.flash_title.is_some());
            let table = verify_table_compact_lines(rows, width.saturating_sub(4)).len() as u16;
            let dismiss = u16::from(self.chrome.flash_dismiss_on_enter);
            title
                .saturating_add(table)
                .saturating_add(dismiss)
                .clamp(1, 14)
        } else {
            1
        }
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
                    self.approve_return = self.approve_return_screen();
                    self.view =
                        View::Approve(ApproveView::new(title, Some(site.clone()), details, vec![]));
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
                    self.approve_return = self.approve_return_screen();
                    self.view = View::Approve(ApproveView::new(title, origin, details, vec![]));
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
                    let preview = match preview {
                        Ok(preview) => preview,
                        Err(error) => {
                            let _ = reply.send(Err(error));
                            continue;
                        }
                    };
                    self.approve_return = self.approve_return_screen();
                    self.view = View::Approve(match preview.base_fee {
                        Some(base_fee) => ApproveView::with_fee(
                            preview.title,
                            origin,
                            preview.details,
                            preview.verify_table,
                            base_fee,
                        ),
                        None => ApproveView::new(
                            preview.title,
                            origin,
                            preview.details,
                            preview.verify_table,
                        ),
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

    fn mcp_token_launch_fields(kind: &ApprovalKind) -> Option<(String, String, String, String)> {
        let ApprovalKind::McpProposal {
            proposal_id,
            proposal,
            ..
        } = kind
        else {
            return None;
        };
        let ProposalType::TokenLaunch {
            name,
            symbol,
            supply_human,
        } = &proposal.proposal_type
        else {
            return None;
        };
        Some((
            proposal_id.clone(),
            name.clone(),
            symbol.clone(),
            supply_human.clone(),
        ))
    }

    fn begin_mcp_token_launch_deploy(
        &mut self,
        proposal_id: String,
        reply: Option<oneshot::Sender<Result<String, ProviderError>>>,
        name: String,
        symbol: String,
        supply: String,
    ) {
        self.pending_mcp_token_launch = Some(PendingMcpTokenLaunch { proposal_id, reply });
        let chain_id = self.wallet().networks().active().chain_id;
        let mut view = TokenLaunchView::for_chain(chain_id);
        view.begin_deploying(name.clone(), symbol.clone(), supply.clone());
        self.view = View::TokenLaunch(view);
        self.spawn_job(UiJob::DeployToken {
            name,
            symbol,
            supply,
        });
    }

    fn finalize_mcp_token_launch(&mut self, result: &Result<TokenLaunchOutcome, WalletError>) {
        let Some(pending) = self.pending_mcp_token_launch.take() else {
            return;
        };
        let parent = self.wallet().path().parent().map(|p| p.to_path_buf());
        match result {
            Ok(outcome) => {
                if let Some(dir) = parent.as_deref() {
                    if let Ok(Some(token)) = McpSessionToken::read(dir) {
                        let queue = ProposalQueue::new(dir);
                        let _ = queue.mark_approved(
                            &pending.proposal_id,
                            &outcome.tx_hash,
                            token.as_bytes(),
                        );
                    }
                }
                if let Some(reply) = pending.reply {
                    let _ = reply.send(Ok(outcome.tx_hash.clone()));
                }
                let flash = format!(
                    "Launched {} at {}",
                    outcome.token.symbol, outcome.token.address
                );
                self.set_flash(flash);
            }
            Err(e) => {
                let msg = e.user_message();
                self.reject_queued_proposal(&pending.proposal_id, &msg);
                if let Some(reply) = pending.reply {
                    let _ = reply.send(Err(ProviderError::Internal(msg)));
                }
            }
        }
    }

    fn expire_stale_approval(&mut self) {
        let Some(pending) = self.pending_approval.as_ref() else {
            return;
        };
        let ttl = match pending.reply {
            PendingReply::Queued => QUEUED_APPROVE_SCREEN_TTL,
            _ => APPROVE_TTL,
        };
        if pending.shown_at.elapsed() < ttl {
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
                // Keep the file-queue entry pending — dismiss the card so the
                // next poll can surface it again (tab switches, reading time).
                if let ApprovalKind::McpProposal { proposal_id, .. } = &pending.kind {
                    self.mcp.clear_inflight_proposal(proposal_id);
                }
                self.set_flash("MCP approval still pending — card will return");
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
                account_index: wallet.active_account_index().ok(),
                account_label: wallet.active_account_label().ok().map(str::to_string),
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
                            Ok(breaker) => {
                                let mut wallet =
                                    self.wallet.lock().unwrap_or_else(|e| e.into_inner());
                                crate::sentient_mcp::auto_exec_mcp_proposal(
                                    &mut wallet,
                                    &self.handle,
                                    &breaker,
                                    &kind,
                                )
                            }
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
                    let preview =
                        provider::describe_approval_preview(&kind, &self.wallet(), &self.handle);
                    let preview = match preview {
                        Ok(preview) => preview,
                        Err(error) => {
                            if let Some(r) = reply {
                                let _ = r.send(Err(error));
                            } else if let ApprovalKind::McpProposal { proposal_id, .. } = &kind {
                                self.mcp.clear_inflight_proposal(proposal_id);
                                self.set_flash(format!("MCP proposal failed to open: {error}"));
                            }
                            continue;
                        }
                    };
                    self.approve_return = self.approve_return_screen();
                    self.view = View::Approve(match preview.base_fee {
                        Some(base_fee) => ApproveView::with_fee(
                            preview.title,
                            Some(format!("MCP ({source})")),
                            preview.details,
                            preview.verify_table,
                            base_fee,
                        ),
                        None => ApproveView::new(
                            preview.title,
                            Some(format!("MCP ({source})")),
                            preview.details,
                            preview.verify_table,
                        ),
                    });
                    // File-queue proposals have no reply channel; they still
                    // get a real pending approval so the card is answerable —
                    // approve/deny is written back to the queue files.
                    let pending_reply = match reply {
                        Some(reply) => PendingReply::Sign(reply),
                        None => PendingReply::Queued,
                    };
                    if matches!(pending_reply, PendingReply::Queued) {
                        if let ApprovalKind::McpProposal { proposal_id, .. } = &kind {
                            // Card is on screen — clear inflight only; do not mark
                            // surfaced until the user approves or denies.
                            self.mcp.clear_inflight_proposal(proposal_id);
                        }
                    }
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
                    if !self.power_features_ok() {
                        let _ = reply.send(Err(ProviderError::Unauthorized(
                            "assist_locked: burn ≥13 WZRD (Settings → Unlock tools)".into(),
                        )));
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
                    if !self.power_features_ok() {
                        let _ = reply.send(Err(ProviderError::Unauthorized(
                            "assist_locked: burn ≥13 WZRD (Settings → Unlock tools)".into(),
                        )));
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
                    if !self.power_features_ok() {
                        let _ = reply.send(Err(ProviderError::Unauthorized(
                            "assist_locked: burn ≥13 WZRD (Settings → Unlock tools)".into(),
                        )));
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
                            Ok(breaker) => {
                                let mut wallet =
                                    self.wallet.lock().unwrap_or_else(|e| e.into_inner());
                                crate::sentient_mcp::auto_exec_stealth_sweep(
                                    &mut wallet,
                                    &self.handle,
                                    &breaker,
                                    &kind,
                                )
                            }
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
                    let preview =
                        provider::describe_approval_preview(&kind, &self.wallet(), &self.handle);
                    let preview = match preview {
                        Ok(preview) => preview,
                        Err(error) => {
                            let _ = reply.send(Err(error));
                            continue;
                        }
                    };
                    self.approve_return = self.approve_return_screen();
                    self.view = View::Approve(ApproveView::new(
                        preview.title,
                        Some("MCP stealth".into()),
                        preview.details,
                        preview.verify_table,
                    ));
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
        if self.mcp_approve_inflight {
            return;
        }
        let approve = matches!(
            key.code,
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter
        );
        let Some(pending) = self.pending_approval.as_ref() else {
            if approve {
                self.set_flash("No approval pending — wait for signing or the next MCP card");
            } else if matches!(
                key.code,
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc
            ) {
                self.navigate(Self::normalize_approve_return(self.approve_return));
            }
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
                if matches!(pending.reply, PendingReply::Queued) {
                    self.mcp.mark_proposal_decided(proposal_id);
                }
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
                if let Some((proposal_id, name, symbol, supply)) =
                    Self::mcp_token_launch_fields(&kind)
                {
                    self.begin_mcp_token_launch_deploy(
                        proposal_id,
                        Some(reply),
                        name,
                        symbol,
                        supply,
                    );
                    return;
                }
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
                let mut wallet = self.wallet.lock().unwrap_or_else(|e| e.into_inner());
                let result = provider::execute_approval_sync(&kind, &mut wallet, &self.handle);
                let _ = reply.send(result);
            }
            PendingReply::LocalSign => {
                let kind = pending.kind;
                let result = {
                    let mut wallet = self.wallet.lock().unwrap_or_else(|e| e.into_inner());
                    provider::execute_approval_sync(&kind, &mut wallet, &self.handle)
                };
                match result {
                    Ok(sig) => self.set_flash(format!("EIP-712 signature: {sig}")),
                    Err(e) => self.set_flash(format!("Sign failed: {e}")),
                }
            }
            PendingReply::Queued => {
                let kind = pending.kind;
                if let Some((proposal_id, name, symbol, supply)) =
                    Self::mcp_token_launch_fields(&kind)
                {
                    self.begin_mcp_token_launch_deploy(proposal_id, None, name, symbol, supply);
                    return;
                }
                if let ApprovalKind::McpProposal {
                    proposal_id,
                    source,
                    proposal,
                    ..
                } = kind
                {
                    self.mcp.mark_proposal_executing(&proposal_id);
                    let fee_override = if let View::Approve(view) = &self.view {
                        view.adjusted_fee()
                    } else {
                        None
                    };
                    self.mcp_approve_inflight = true;
                    self.spawn_job(UiJob::McpQueuedApprove {
                        proposal_id,
                        source,
                        proposal: (*proposal).clone(),
                        fee_override,
                    });
                    return;
                }
                // Non-MCP queued kinds (should not happen today).
                let result = {
                    let mut wallet = self.wallet.lock().unwrap_or_else(|e| e.into_inner());
                    provider::execute_approval_sync(&kind, &mut wallet, &self.handle)
                };
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
        self.approve_return = self.approve_return_screen();
        self.view = View::Approve(ApproveView::new(
            title,
            Some("Local (browserless)".into()),
            details,
            vec![],
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

    fn records_back_stack(screen: Screen) -> bool {
        !matches!(
            screen,
            Screen::Onboarding | Screen::Unlock | Screen::Approve
        )
    }

    /// Build the default view for `screen`. Chrome/asset RPC refresh is limited
    /// to screens that need fresh data — other views reuse cached chrome.
    fn navigate(&mut self, screen: Screen) {
        if Self::screen_requires_power_unlock(screen) && !self.power_features_ok() {
            self.flash_tools_locked();
            return;
        }
        let from = self.screen();
        if matches!(
            screen,
            Screen::Dashboard | Screen::Onboarding | Screen::Unlock
        ) {
            self.nav_back.clear();
        } else if from != screen
            && Self::records_back_stack(from)
            && Self::records_back_stack(screen)
            && self.nav_back.last() != Some(&from)
        {
            self.nav_back.push(from);
        }
        self.mount_screen(screen);
    }

    /// Esc: return to the screen we came from, or Dashboard when the stack is empty.
    fn navigate_back(&mut self) {
        let Some(dest) = self.nav_back.pop() else {
            if self.screen() != Screen::Dashboard && Self::records_back_stack(self.screen()) {
                self.navigate(Screen::Dashboard);
            }
            return;
        };
        if Self::screen_requires_power_unlock(dest) && !self.power_features_ok() {
            self.nav_back.clear();
            self.flash_tools_locked();
            self.mount_screen(Screen::Dashboard);
            return;
        }
        if dest == Screen::Dashboard {
            self.nav_back.clear();
        }
        self.mount_screen(dest);
    }

    /// Bridge / launch / browser / AA batch / NFT placeholder.
    /// Ag, Dex, and LP stay free so users can buy WZRD (or exit LP for tPLS) before burning.
    fn screen_requires_power_unlock(screen: Screen) -> bool {
        matches!(
            screen,
            Screen::Bridge
                | Screen::TokenLaunch
                | Screen::Browser
                | Screen::AaSend
                | Screen::SoonNft
        )
    }

    /// True when the burn gate is off or this vault has unlocked tools (any F3 burner).
    fn power_features_ok(&self) -> bool {
        use vaughan_core::core::{
            assist_burn_gate_enabled, entitlement_chain_id, power_features_unlocked_blocking,
        };
        if !assist_burn_gate_enabled() {
            return true;
        }
        let Some(chain_id) = entitlement_chain_id() else {
            return false;
        };
        let (dir, addrs) = {
            let w = self.wallet();
            let dir = profile_dir(w.path());
            let addrs = w.account_addresses().unwrap_or_default();
            (dir, addrs)
        };
        if addrs.is_empty() {
            return false;
        }
        power_features_unlocked_blocking(&self.handle, Some(&dir), chain_id, &addrs)
    }

    fn flash_tools_locked(&mut self) {
        self.set_flash("Tools locked: burn ≥13 WZRD from any account — press w");
    }

    fn mount_screen(&mut self, screen: Screen) {
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
            Screen::Lp => {
                let chain_id = self.wallet().networks().active().chain_id;
                View::Lp(LpView::for_chain(chain_id))
            }
            Screen::History => {
                View::History(HistoryView::with_broadcasts(self.recent_broadcasts.clone()))
            }
            Screen::Approvals => View::Approvals(ApprovalsView::loading()),
            Screen::Wrap => {
                let chain_id = self.wallet().networks().active().chain_id;
                View::Wrap(WrapView::for_chain(chain_id))
            }
            Screen::Hex => {
                let w = self.wallet();
                let chain_id = w.networks().active().chain_id;
                let owner = w.active_address().ok().unwrap_or_default();
                View::Hex(HexView::for_chain(chain_id, &owner))
            }
            Screen::TokenLaunch => {
                let chain_id = self.wallet().networks().active().chain_id;
                View::TokenLaunch(TokenLaunchView::for_chain(chain_id))
            }
            // Approve is entered directly from `poll_provider`
            // pending request + reply channel), never via navigation; this arm
            // is only here to keep the match exhaustive.
            Screen::Approve => View::Approve(ApproveView::new(
                "Approve request".to_string(),
                None,
                Vec::new(),
                Vec::new(),
            )),
        };
        self.view = view;
        match screen {
            Screen::Onboarding | Screen::Unlock | Screen::Approve => {}
            Screen::Assets => {
                self.sync_f2_to_displayed();
            }
            Screen::Dex | Screen::Aggregator => {
                if self.chrome.assets.is_empty() {
                    self.sync_f2_to_displayed();
                } else {
                    self.refresh_chrome();
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
            Screen::Hex => {
                self.refresh_chrome();
                if let View::Hex(v) = &self.view {
                    if let Some(job) = v.initial_job() {
                        self.spawn_job(job);
                    }
                }
            }
            Screen::Lp => {
                self.refresh_chrome();
                let job = if matches!(self.view, View::Lp(_)) {
                    let w = self.wallet();
                    let owner = w.active_address().ok().map(|s| s.to_string());
                    let rpc = w.active_rpc_url();
                    drop(w);
                    if let (Some(owner), View::Lp(v)) = (owner, &mut self.view) {
                        v.list_job_for(owner, rpc)
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some(job) = job {
                    self.spawn_job(job);
                }
            }
            Screen::Dashboard => {
                if self.chrome.assets.is_empty() {
                    self.sync_f2_to_displayed();
                } else {
                    self.refresh_chrome();
                }
            }
            _ => {}
        }
    }

    /// Refresh always-on network / balance / gas chrome (unlocked screens).
    ///
    /// Clears the previous F2 balance immediately and stamps the job so a late
    /// response from another F3 account cannot paint over the strip.
    fn refresh_chrome(&mut self) {
        if !self.wallet().is_unlocked() {
            return;
        }
        let Some(owner) = self.chrome_display_owner() else {
            return;
        };
        self.kick_f2_chrome(&owner);
    }

    /// Re-fetch the F2 asset list (native + ERC-20). Safe to call often; stamped
    /// jobs discard stale results after F3 switch / import.
    fn spawn_refresh_assets(&mut self) {
        if !self.wallet().is_unlocked() {
            return;
        }
        let Some(owner) = self.chrome_display_owner() else {
            return;
        };
        self.kick_f2_assets(&owner);
    }

    /// Address shown under the wordmark / driving F2 (F3 preview when focused).
    fn chrome_display_owner(&self) -> Option<String> {
        let w = self.wallet();
        if !w.is_unlocked() {
            return None;
        }
        if self.chrome.focus == ChromeFocus::Account {
            if let Some(idx) = self.chrome.pending_account_index {
                if let Ok(addr) = w.account_address(idx) {
                    return Some(addr);
                }
            }
        }
        w.active_address().ok().map(str::to_string)
    }

    fn kick_f2_chrome(&mut self, owner: &str) {
        self.chrome.f2_gen = self.chrome.f2_gen.wrapping_add(1);
        let gen = self.chrome.f2_gen;
        self.chrome.balance = None;
        self.chrome.loading = true;
        self.chrome.error = None;
        self.spawn_job(UiJob::RefreshChrome {
            owner: owner.to_string(),
            gen,
        });
    }

    fn kick_f2_assets(&mut self, owner: &str) {
        self.chrome.f2_gen = self.chrome.f2_gen.wrapping_add(1);
        let gen = self.chrome.f2_gen;
        self.chrome.assets.clear();
        self.chrome.asset_idx = 0;
        self.chrome.assets_loading = true;
        self.spawn_job(UiJob::RefreshAssets {
            owner: owner.to_string(),
            gen,
        });
    }

    /// Clear F2 and refetch native + assets for `owner` (F3 must match F2).
    fn sync_f2_to_owner(&mut self, owner: &str) {
        self.chrome.f2_gen = self.chrome.f2_gen.wrapping_add(1);
        let gen = self.chrome.f2_gen;
        self.chrome.balance = None;
        self.chrome.assets.clear();
        self.chrome.asset_idx = 0;
        self.chrome.pending_asset_address = None;
        self.chrome.loading = true;
        self.chrome.assets_loading = true;
        self.chrome.error = None;
        self.spawn_job(UiJob::RefreshChrome {
            owner: owner.to_string(),
            gen,
        });
        self.spawn_job(UiJob::RefreshAssets {
            owner: owner.to_string(),
            gen,
        });
    }

    fn f2_result_current(&self, owner: &str, gen: u64) -> bool {
        if gen != self.chrome.f2_gen {
            return false;
        }
        self.chrome_display_owner()
            .is_some_and(|want| want.eq_ignore_ascii_case(owner))
    }

    /// Clear F2 and refetch native + assets for the chrome-displayed account.
    fn sync_f2_to_displayed(&mut self) {
        if let Some(owner) = self.chrome_display_owner() {
            self.sync_f2_to_owner(&owner);
        }
    }

    /// Dex / Ag: ↑/↓ on Token in or Token out cycles the wallet asset list.
    fn cycle_active_token_field(&mut self, forward: bool) -> bool {
        if !matches!(self.screen(), Screen::Dex | Screen::Aggregator | Screen::Lp) {
            return false;
        }
        if self.chrome.assets.is_empty() {
            let on_token_field = match &mut self.view {
                View::Dex(v) => v.cycle_focused_token_picker(&[], forward),
                View::Aggregator(v) => v.cycle_focused_token_picker(&[], forward),
                View::Lp(v) => v.cycle_focused_token_picker(&[], forward),
                _ => false,
            };
            if !on_token_field {
                return false;
            }
            self.spawn_refresh_assets();
            return true;
        }
        let assets = self.chrome.assets.clone();
        match &mut self.view {
            View::Dex(v) => v.cycle_focused_token_picker(&assets, forward),
            View::Aggregator(v) => v.cycle_focused_token_picker(&assets, forward),
            View::Lp(v) => v.cycle_focused_token_picker(&assets, forward),
            _ => false,
        }
    }

    /// F1 / F2 / F3 focus + ↑/↓ preview + Enter set / Esc cancel.
    /// Returns true if the key was consumed.
    fn handle_chrome_hotkey(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Ctrl+Y while F3 (or any chrome) focused: copy the address under the
        // wordmark — for F3 that is the ↑/↓ preview wallet, not only the committed one.
        if ctrl && matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
            self.copy_chrome_f3_address();
            return true;
        }
        match key.code {
            KeyCode::F(1) => {
                self.begin_chrome_focus(ChromeFocus::Network);
                true
            }
            KeyCode::F(2) => {
                self.begin_chrome_focus(ChromeFocus::Asset);
                if self.chrome.assets.is_empty() {
                    self.spawn_refresh_assets();
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
                let was_account = self.chrome.focus == ChromeFocus::Account;
                self.cancel_chrome_focus();
                if was_account {
                    // Restore F2 to the committed (active) account after abandoning preview.
                    self.sync_f2_to_displayed();
                }
                true
            }
            KeyCode::Up | KeyCode::Down if self.chrome.focus != ChromeFocus::None => {
                let forward = matches!(key.code, KeyCode::Down);
                self.preview_chrome_cycle(forward);
                true
            }
            KeyCode::Left | KeyCode::Right if self.chrome.focus == ChromeFocus::Asset => {
                self.chrome.f2_show_contract = !self.chrome.f2_show_contract;
                if self.chrome.f2_show_contract {
                    let idx = self
                        .chrome
                        .pending_asset_idx
                        .unwrap_or(self.chrome.asset_idx);
                    if let Some(b) = self.chrome.assets.get(idx) {
                        if let Some(addr) = b.token.contract_address.as_deref() {
                            self.set_flash(format!("{} · {}", b.token.symbol, addr));
                        } else {
                            self.set_flash(format!("{} · native (no contract)", b.token.symbol));
                        }
                    }
                }
                true
            }
            // Plain `y` while F3 select mode: same as Ctrl+Y (preview address).
            KeyCode::Char('y') | KeyCode::Char('Y')
                if !ctrl && self.chrome.focus == ChromeFocus::Account =>
            {
                self.copy_chrome_f3_address();
                true
            }
            _ => false,
        }
    }

    /// Copy the address shown under the wordmark (F3 ↑/↓ preview when Account focused).
    fn copy_chrome_f3_address(&mut self) {
        let Some(addr) = self.chrome_display_owner().or_else(|| {
            self.wallet()
                .active_address()
                .ok()
                .map(|a| a.to_string())
        }) else {
            self.set_flash("No account address to copy");
            return;
        };
        match crate::clipboard::copy_text(&addr) {
            Ok(()) => {
                let short = if addr.len() > 12 {
                    format!("{}…{}", &addr[..6], &addr[addr.len() - 4..])
                } else {
                    addr.clone()
                };
                let preview = self.chrome.focus == ChromeFocus::Account
                    && self.chrome.pending_account_index.is_some_and(|idx| {
                        self.wallet()
                            .active_account_index()
                            .ok()
                            .is_some_and(|active| active != idx)
                    });
                if preview {
                    self.set_flash(format!("Copied preview {short}"));
                } else {
                    self.set_flash(format!("F3 address copied · {short}"));
                }
            }
            Err(e) => self.set_flash(e),
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
                        if let Some(owner) = self.chrome_display_owner() {
                            self.sync_f2_to_owner(&owner);
                        }
                        self.enforce_assist_entitlement("F3 account");
                        self.kick_if_power_screen_locked();
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
        self.chrome.f2_show_contract = false;
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
                    self.spawn_refresh_assets();
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
                // F2 must track the F3-preview address immediately (not only after Enter).
                let owner = self.wallet().account_address(choices[next].0).ok();
                if let Some(owner) = owner {
                    self.sync_f2_to_owner(&owner);
                }
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
        self.sync_f2_to_displayed();
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
        if let Some(owner) = {
            let w = self.wallet();
            w.active_address().ok().map(|a| a.to_string())
        } {
            self.sync_f2_to_owner(&owner);
        }
        if matches!(
            self.screen(),
            Screen::Dex
                | Screen::Aggregator
                | Screen::Receive
                | Screen::Lp
                | Screen::Assets
                | Screen::Approvals
                | Screen::Wrap
                | Screen::Hex
                | Screen::Bridge
        ) {
            self.navigate(self.screen());
        }
        self.enforce_assist_entitlement("F3 account");
        self.kick_if_power_screen_locked();
    }

    /// Leave Dex/LP/etc when the new F3 account is not entitled.
    fn kick_if_power_screen_locked(&mut self) {
        if Self::screen_requires_power_unlock(self.screen()) && !self.power_features_ok() {
            self.flash_tools_locked();
            self.nav_back.clear();
            self.mount_screen(Screen::Dashboard);
        }
    }

    /// After unlock / F3: if AI mode is on but the vault has no burn, force HumanOnly.
    fn enforce_assist_entitlement(&mut self, reason: &str) {
        use vaughan_core::core::{
            assist_burn_gate_enabled, entitlement_chain_id, vault_has_assist_burn,
        };

        if !assist_burn_gate_enabled() {
            return;
        }
        let needs_ai = self.wallet().operating_mode().is_ai_enabled();
        if !needs_ai {
            return;
        }
        let Some(chain_id) = entitlement_chain_id() else {
            self.force_human_only_assist_locked(reason);
            return;
        };
        let (dir, addrs) = {
            let w = self.wallet();
            let dir = profile_dir(w.path());
            let addrs = w.account_addresses().unwrap_or_default();
            (dir, addrs)
        };
        if addrs.is_empty() {
            self.force_human_only_assist_locked(reason);
            return;
        }
        let entitled = self
            .handle
            .block_on(vault_has_assist_burn(Some(&dir), chain_id, &addrs))
            .unwrap_or(false);
        if !entitled {
            self.force_human_only_assist_locked(reason);
        }
    }

    fn force_human_only_assist_locked(&mut self, reason: &str) {
        {
            let mut w = self.wallet.lock().unwrap_or_else(|e| e.into_inner());
            w.set_operating_mode(OperatingMode::HumanOnly);
        }
        self.set_flash(format!(
            "AI locked ({reason}): burn ≥13 WZRD from any account — press w"
        ));
    }

    /// Settings `w`: switch to Pulse testnet if needed, prefill Home burn send.
    fn begin_assist_burn(&mut self) {
        use vaughan_core::core::{
            assist_burn_gate_enabled, burn_sink_hex, entitlement_chain_id, wzrd_token_hex,
            ASSIST_BURN_AMOUNT_HUMAN,
        };

        if !assist_burn_gate_enabled() {
            self.set_flash("Tools burn gate disabled (VAUGHAN_ASSIST_BURN_GATE=0)");
            return;
        }
        let Some(chain_id) = entitlement_chain_id() else {
            self.set_flash("WZRD unlock burn not available on any chain yet");
            return;
        };
        let Some(token) = wzrd_token_hex(chain_id) else {
            self.set_flash("WZRD token not configured for entitlement chain");
            return;
        };

        let switched = {
            let mut w = self.wallet.lock().unwrap_or_else(|e| e.into_inner());
            if w.networks().active().chain_id == chain_id {
                Ok(false)
            } else if let Some(net) =
                vaughan_core::chains::evm::networks::get_network_by_chain_id(chain_id)
            {
                w.set_active_network(&net.id).map(|_| true)
            } else {
                Err(WalletError::Other(format!(
                    "no network config for chain {chain_id}"
                )))
            }
        };
        match switched {
            Ok(true) => {
                self.events
                    .publish(ProviderEvent::ChainChanged(format!("0x{chain_id:x}")));
                self.sync_f2_to_displayed();
            }
            Ok(false) => {}
            Err(e) => {
                self.set_flash(e.user_message());
                return;
            }
        }

        self.nav_back.clear();
        self.view = View::Dashboard(DashboardView::for_assist_burn(
            &token,
            &burn_sink_hex(),
            ASSIST_BURN_AMOUNT_HUMAN,
        ));
        self.set_flash("Unlock tools: confirm ≥13 WZRD to the dead address (one tx, no drip)");
    }

    fn lp_job_rpc_urls(wallet: &WalletState, job_primary: &str) -> Vec<String> {
        wallet
            .active_rpc_url_list()
            .unwrap_or_else(|_| vaughan_core::core::merge_rpc_urls(job_primary, &[]))
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
                UiJob::RefreshChrome { owner, gen } => {
                    let owner_for_result = owner.clone();
                    let snap = {
                        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                        w.chrome_rpc_snapshot_for(&owner)
                    };
                    UiJobResult::Chrome {
                        owner: owner_for_result,
                        gen,
                        result: match snap {
                            Ok(s) => handle.block_on(s.fetch_chrome()),
                            Err(e) => Err(e),
                        },
                    }
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
                UiJob::RefreshAssets { owner, gen } => {
                    let owner_for_result = owner.clone();
                    let (snap, extras) = {
                        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                        (
                            w.chrome_rpc_snapshot_for(&owner),
                            w.custom_token_addresses_for_active_chain(),
                        )
                    };
                    UiJobResult::Assets {
                        owner: owner_for_result,
                        gen,
                        result: match snap {
                            Ok(s) => handle.block_on(s.fetch_assets(&extras)),
                            Err(e) => Err(e),
                        },
                    }
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
                UiJob::AssistBurnVerify => {
                    use vaughan_core::core::{
                        entitlement_chain_id, vault_has_assist_burn_with_retry,
                    };
                    let (dir, addrs) = {
                        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                        let dir = profile_dir(w.path());
                        let addrs = w.account_addresses().unwrap_or_default();
                        (dir, addrs)
                    };
                    UiJobResult::AssistBurnVerify(match entitlement_chain_id() {
                        Some(chain_id) if !addrs.is_empty() => handle.block_on(
                            vault_has_assist_burn_with_retry(Some(&dir), chain_id, &addrs),
                        ),
                        _ => Ok(false),
                    })
                }
                UiJob::EstimateEvmFee { tx } => {
                    let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                    UiJobResult::Fee(handle.block_on(w.estimate_transaction_fee(tx)))
                }
                UiJob::DexSwapEstimateAfterApprove {
                    rpc_url,
                    token_in,
                    owner,
                    router,
                    amount_in,
                    tx,
                } => {
                    use alloy::primitives::{Address, U256};
                    use std::str::FromStr;
                    use vaughan_core::chains::Fee;
                    use vaughan_core::core::wait_erc20_allowance;

                    let parsed = (|| -> Result<Fee, WalletError> {
                        let token = Address::from_str(token_in.trim())
                            .map_err(|_| WalletError::InvalidTransaction("dex token in".into()))?;
                        let owner = Address::from_str(owner.trim())
                            .map_err(|_| WalletError::InvalidTransaction("dex owner".into()))?;
                        let router = Address::from_str(router.trim())
                            .map_err(|_| WalletError::InvalidTransaction("dex router".into()))?;
                        let need = U256::from_str(&amount_in)
                            .map_err(|_| WalletError::InvalidAmount("dex amount".into()))?;
                        handle
                            .block_on(wait_erc20_allowance(&rpc_url, token, owner, router, need))?;
                        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                        handle.block_on(w.estimate_transaction_fee(tx))
                    })();
                    UiJobResult::Fee(parsed)
                }
                UiJob::DexAllowanceCheck {
                    rpc_url,
                    token_in,
                    owner,
                    router,
                    amount_in,
                } => {
                    use alloy::primitives::{Address, U256};
                    use std::str::FromStr;
                    use vaughan_core::core::erc20_allowance_covers;

                    let parsed = (|| -> Result<bool, WalletError> {
                        let token = Address::from_str(token_in.trim())
                            .map_err(|_| WalletError::InvalidTransaction("dex token in".into()))?;
                        let owner = Address::from_str(owner.trim())
                            .map_err(|_| WalletError::InvalidTransaction("dex owner".into()))?;
                        let router = Address::from_str(router.trim())
                            .map_err(|_| WalletError::InvalidTransaction("dex router".into()))?;
                        let need = U256::from_str(&amount_in)
                            .map_err(|_| WalletError::InvalidAmount("dex amount".into()))?;
                        handle
                            .block_on(erc20_allowance_covers(&rpc_url, token, owner, router, need))
                    })();
                    UiJobResult::DexAllowanceCheck(parsed)
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
                    quoter,
                    amount_in,
                    fee,
                    path,
                    token_in,
                    token_out,
                    wpls,
                    native_in,
                } => {
                    use alloy::primitives::{Address, U256};
                    use std::str::FromStr;
                    use vaughan_core::core::{
                        discover_v3_swap_route, quote_v2_exact_in, quote_v3_path_exact_in,
                        resolve_v3_swap_path,
                    };

                    let parsed = (|| -> Result<vaughan_core::core::DexQuote, WalletError> {
                        let amount_in = U256::from_str(&amount_in)
                            .map_err(|_| WalletError::InvalidAmount("dex amount".into()))?;
                        if protocol_v2 {
                            let hops: Result<Vec<Address>, _> = path
                                .iter()
                                .map(|s| {
                                    Address::from_str(s.trim()).map_err(|_| {
                                        WalletError::InvalidTransaction("dex path token".into())
                                    })
                                })
                                .collect();
                            let hops = hops?;
                            let router = Address::from_str(&router).map_err(|_| {
                                WalletError::InvalidTransaction("dex router".into())
                            })?;
                            handle.block_on(quote_v2_exact_in(&rpc_url, router, amount_in, &hops))
                        } else {
                            let token_in = Address::from_str(token_in.trim()).map_err(|_| {
                                WalletError::InvalidTransaction("dex token in".into())
                            })?;
                            let token_out = Address::from_str(token_out.trim()).map_err(|_| {
                                WalletError::InvalidTransaction("dex token out".into())
                            })?;
                            let wpls = match wpls {
                                Some(s) if !s.trim().is_empty() => {
                                    Some(Address::from_str(s.trim()).map_err(|_| {
                                        WalletError::InvalidTransaction("dex WPLS".into())
                                    })?)
                                }
                                _ => None,
                            };
                            let quoter = match quoter {
                                Some(s) if !s.trim().is_empty() => {
                                    Some(Address::from_str(s.trim()).map_err(|_| {
                                        WalletError::InvalidTransaction("dex quoter".into())
                                    })?)
                                }
                                _ => None,
                            };
                            let (hops, hop_fees, amount_out) =
                                if vaughan_core::core::deployment_for_chain(chain_id).is_some() {
                                    let route = handle.block_on(discover_v3_swap_route(
                                        &rpc_url, chain_id, token_in, token_out, amount_in, wpls,
                                        native_in,
                                    ))?;
                                    (route.path, route.hop_fees, route.amount_out)
                                } else {
                                    let hops = handle.block_on(resolve_v3_swap_path(
                                        &rpc_url, chain_id, token_in, token_out, fee, wpls,
                                        native_in,
                                    ))?;
                                    let quote = handle.block_on(quote_v3_path_exact_in(
                                        &rpc_url, chain_id, &hops, amount_in, fee, quoter,
                                    ))?;
                                    (
                                        hops,
                                        vec![fee; quote.path.len().saturating_sub(1)],
                                        quote.amount_out,
                                    )
                                };
                            Ok(vaughan_core::core::DexQuote {
                                amount_out,
                                path: hops,
                                fee_tier: hop_fees.first().copied().unwrap_or(0),
                                hop_fees,
                            })
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
                UiJob::RefreshHexStakes { owner, gen } => {
                    let rpc = {
                        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                        w.active_rpc_url()
                    };
                    let (stakes, globals) =
                        handle.block_on(crate::views::hex::load_hex_stakes(&rpc, &owner));
                    UiJobResult::HexStakes {
                        gen,
                        owner,
                        stakes,
                        globals,
                    }
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
                UiJob::LpListPositions {
                    venue,
                    chain_id,
                    rpc_url,
                    owner,
                    list_gen,
                } => {
                    use alloy::primitives::Address;
                    use std::str::FromStr;
                    use vaughan_core::core::{list_v3_lp_position_views, with_lp_rpc_urls};
                    let owner_for_result = owner.clone();
                    let parsed_owner = Address::from_str(&owner).map_err(|_| {
                        WalletError::InvalidTransaction("invalid owner address".into())
                    });
                    let rpc_urls = {
                        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                        Self::lp_job_rpc_urls(&w, &rpc_url)
                    };
                    UiJobResult::LpPositions {
                        list_gen,
                        owner: owner_for_result,
                        result: match parsed_owner {
                            Err(e) => Err(e),
                            Ok(addr) => {
                                handle.block_on(with_lp_rpc_urls(&rpc_urls, |url| async move {
                                    list_v3_lp_position_views(
                                        &url, venue, chain_id, addr, None, None,
                                    )
                                    .await
                                }))
                            }
                        },
                    }
                }
                UiJob::LpListV2Positions {
                    venue,
                    chain_id,
                    rpc_url,
                    owner,
                    list_gen,
                } => {
                    use alloy::primitives::Address;
                    use std::str::FromStr;
                    use vaughan_core::core::{default_v2_watch_pairs, list_v2_lp_positions};
                    let owner_for_result = owner.clone();
                    let parsed = (|| -> Result<_, WalletError> {
                        let addr = Address::from_str(&owner).map_err(|_| {
                            WalletError::InvalidTransaction("invalid owner address".into())
                        })?;
                        let watch = default_v2_watch_pairs(chain_id, venue);
                        handle.block_on(list_v2_lp_positions(
                            &rpc_url, venue, chain_id, addr, &watch,
                        ))
                    })();
                    UiJobResult::LpV2Positions {
                        list_gen,
                        owner: owner_for_result,
                        result: parsed,
                    }
                }
                UiJob::LpV3PoolDeployStep {
                    venue,
                    chain_id,
                    rpc_url,
                    from,
                    token0,
                    token1,
                    fee,
                    dec0,
                    dec1,
                    pool_initial_price,
                    pool_min_price,
                    pool_max_price,
                    amount0,
                    amount1,
                    deploy_wait,
                    after_step_label,
                } => {
                    use alloy::primitives::Address;
                    use std::str::FromStr;
                    use vaughan_core::core::{
                        v3_lp_prepare_deploy_step, v3_lp_run_deploy_wait, with_lp_rpc_urls,
                        V3LpDeployContext, V3LpDeployParams,
                    };
                    let parse_addr = |s: &str, label: &str| {
                        Address::from_str(s.trim()).map_err(|_| {
                            WalletError::InvalidTransaction(format!("invalid {label}"))
                        })
                    };
                    let rpc_urls = {
                        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                        Self::lp_job_rpc_urls(&w, &rpc_url)
                    };
                    UiJobResult::LpV3PoolDeployStep(handle.block_on(async {
                        use tokio::time::{timeout, Duration};
                        use vaughan_core::core::V3LpDeployWait;
                        const DEPLOY_PREP_TIMEOUT: Duration = Duration::from_secs(30);
                        const DEPLOY_ONCHAIN_WAIT: Duration = Duration::from_secs(60);
                        let job_timeout = match deploy_wait {
                            V3LpDeployWait::None => DEPLOY_PREP_TIMEOUT,
                            _ => DEPLOY_ONCHAIN_WAIT + DEPLOY_PREP_TIMEOUT,
                        };
                        let t0 = parse_addr(&token0, "token0")?;
                        let t1 = parse_addr(&token1, "token1")?;
                        let params = V3LpDeployParams {
                            from,
                            venue,
                            chain_id,
                            rpc_url: String::new(),
                            token0: t0,
                            token1: t1,
                            fee,
                            dec0,
                            dec1,
                            pool_initial_price,
                            pool_min_price,
                            pool_max_price,
                            amount0,
                            amount1,
                            deposit_on_token0: true,
                        };
                        with_lp_rpc_urls(&rpc_urls, |url| {
                            let mut p = params.clone();
                            p.rpc_url = url;
                            let ctx = after_step_label.clone().map(|label| V3LpDeployContext {
                                last_step_label: Some(label),
                            });
                            async move {
                                timeout(job_timeout, async {
                                    v3_lp_run_deploy_wait(deploy_wait, &p, ctx.as_ref()).await?;
                                    v3_lp_prepare_deploy_step(&p).await
                                })
                                .await
                                .map_err(|_| {
                                    WalletError::NetworkError(format!(
                                        "LP deploy step timed out ({}s)",
                                        job_timeout.as_secs()
                                    ))
                                })?
                            }
                        })
                        .await
                    }))
                }
                UiJob::LpEnableCheck {
                    venue,
                    chain_id,
                    rpc_url,
                    from,
                    token0,
                    token1,
                    fee,
                    dec0,
                    dec1,
                    pool_initial_price,
                    pool_min_price,
                    pool_max_price,
                    amount0,
                    amount1,
                } => {
                    use alloy::primitives::Address;
                    use std::str::FromStr;
                    use vaughan_core::core::{
                        v3_lp_token_enable_status, with_lp_rpc_urls, V3LpDeployParams,
                    };
                    let rpc_urls = {
                        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                        Self::lp_job_rpc_urls(&w, &rpc_url)
                    };
                    UiJobResult::LpEnableCheck((|| {
                        let parse_addr = |s: &str, label: &str| {
                            Address::from_str(s.trim()).map_err(|_| {
                                WalletError::InvalidTransaction(format!("invalid {label}"))
                            })
                        };
                        let params = V3LpDeployParams {
                            from,
                            venue,
                            chain_id,
                            rpc_url: String::new(),
                            token0: parse_addr(&token0, "token0")?,
                            token1: parse_addr(&token1, "token1")?,
                            fee,
                            dec0,
                            dec1,
                            pool_initial_price,
                            pool_min_price,
                            pool_max_price,
                            amount0,
                            amount1,
                            deposit_on_token0: true,
                        };
                        handle.block_on(async {
                            use tokio::time::{timeout, Duration};
                            const ENABLE_CHECK_TIMEOUT: Duration = Duration::from_secs(30);
                            with_lp_rpc_urls(&rpc_urls, |url| {
                                let mut p = params.clone();
                                p.rpc_url = url.to_string();
                                async move {
                                    timeout(ENABLE_CHECK_TIMEOUT, v3_lp_token_enable_status(&p))
                                        .await
                                        .map_err(|_| {
                                            WalletError::NetworkError(
                                                "enable check timed out (30s)".into(),
                                            )
                                        })?
                                }
                            })
                            .await
                        })
                    })())
                }
                UiJob::LpEnablePrepare {
                    venue,
                    chain_id,
                    rpc_url,
                    from,
                    token0,
                    token1,
                    fee,
                    dec0,
                    dec1,
                    pool_initial_price,
                    pool_min_price,
                    pool_max_price,
                    amount0,
                    amount1,
                    symbol,
                } => {
                    use alloy::primitives::Address;
                    use std::str::FromStr;
                    use vaughan_core::core::{
                        v3_lp_build_next_enable_tx, with_lp_rpc_urls, V3LpDeployParams,
                    };
                    let rpc_urls = {
                        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                        Self::lp_job_rpc_urls(&w, &rpc_url)
                    };
                    UiJobResult::LpEnablePrepare(handle.block_on(async {
                        let parse_addr = |s: &str, label: &str| {
                            Address::from_str(s.trim()).map_err(|_| {
                                WalletError::InvalidTransaction(format!("invalid {label}"))
                            })
                        };
                        let params = V3LpDeployParams {
                            from,
                            venue,
                            chain_id,
                            rpc_url: String::new(),
                            token0: parse_addr(&token0, "token0")?,
                            token1: parse_addr(&token1, "token1")?,
                            fee,
                            dec0,
                            dec1,
                            pool_initial_price,
                            pool_min_price,
                            pool_max_price,
                            amount0,
                            amount1,
                            deposit_on_token0: true,
                        };
                        use tokio::time::{timeout, Duration};
                        const ENABLE_PREP_TIMEOUT: Duration = Duration::from_secs(30);
                        let out = with_lp_rpc_urls(&rpc_urls, |url| {
                            let mut p = params.clone();
                            p.rpc_url = url.to_string();
                            async move {
                                timeout(ENABLE_PREP_TIMEOUT, v3_lp_build_next_enable_tx(&p))
                                    .await
                                    .map_err(|_| {
                                        WalletError::NetworkError(
                                            "enable prepare timed out (30s)".into(),
                                        )
                                    })?
                            }
                        })
                        .await?;
                        match out {
                            Some((tx, label)) => Ok((tx, label, symbol)),
                            None => Err(WalletError::InvalidTransaction(
                                "pool not ready for Enable yet — finish pool setup first".into(),
                            )),
                        }
                    }))
                }
                UiJob::LpEnableWait {
                    venue,
                    chain_id,
                    rpc_url,
                    from,
                    token0,
                    token1,
                    fee,
                    dec0,
                    dec1,
                    pool_initial_price,
                    pool_min_price,
                    pool_max_price,
                    amount0,
                    amount1,
                    after_step_label,
                } => {
                    use alloy::primitives::Address;
                    use std::str::FromStr;
                    use vaughan_core::core::{
                        v3_lp_run_deploy_wait, with_lp_rpc_urls, V3LpDeployContext,
                        V3LpDeployParams, V3LpDeployWait,
                    };
                    let rpc_urls = {
                        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                        Self::lp_job_rpc_urls(&w, &rpc_url)
                    };
                    UiJobResult::LpEnableWait(handle.block_on(async {
                        let parse_addr = |s: &str, label: &str| {
                            Address::from_str(s.trim()).map_err(|_| {
                                WalletError::InvalidTransaction(format!("invalid {label}"))
                            })
                        };
                        let params = V3LpDeployParams {
                            from,
                            venue,
                            chain_id,
                            rpc_url: String::new(),
                            token0: parse_addr(&token0, "token0")?,
                            token1: parse_addr(&token1, "token1")?,
                            fee,
                            dec0,
                            dec1,
                            pool_initial_price,
                            pool_min_price,
                            pool_max_price,
                            amount0,
                            amount1,
                            deposit_on_token0: true,
                        };
                        use tokio::time::{timeout, Duration};
                        const ENABLE_WAIT: Duration = Duration::from_secs(75);
                        let ctx = V3LpDeployContext {
                            last_step_label: Some(after_step_label),
                        };
                        with_lp_rpc_urls(&rpc_urls, |url| {
                            let mut p = params.clone();
                            p.rpc_url = url;
                            let ctx = ctx.clone();
                            async move {
                                timeout(
                                    ENABLE_WAIT,
                                    v3_lp_run_deploy_wait(
                                        V3LpDeployWait::AfterApprove,
                                        &p,
                                        Some(&ctx),
                                    ),
                                )
                                .await
                                .map_err(|_| {
                                    WalletError::NetworkError("Enable wait timed out (75s)".into())
                                })?
                            }
                        })
                        .await
                    }))
                }
                UiJob::LpV3PoolQuote {
                    venue,
                    chain_id,
                    rpc_url,
                    token0,
                    token1,
                    fee,
                    dec0,
                    dec1,
                } => {
                    use alloy::primitives::Address;
                    use std::str::FromStr;
                    use vaughan_core::core::{fetch_v3_lp_pool_quote, with_lp_rpc_urls};
                    let parse_addr = |s: &str, label: &str| {
                        Address::from_str(s.trim()).map_err(|_| {
                            WalletError::InvalidTransaction(format!("invalid {label}"))
                        })
                    };
                    let rpc_urls = {
                        let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                        Self::lp_job_rpc_urls(&w, &rpc_url)
                    };
                    UiJobResult::LpV3PoolQuote(handle.block_on(async {
                        use tokio::time::{timeout, Duration};
                        const POOL_QUOTE_TIMEOUT: Duration = Duration::from_secs(45);
                        let t0 = parse_addr(&token0, "token0")?;
                        let t1 = parse_addr(&token1, "token1")?;
                        if t0 >= t1 {
                            return Err(WalletError::InvalidTransaction(
                                "internal: token0 must be sorted".into(),
                            ));
                        }
                        with_lp_rpc_urls(&rpc_urls, |url| async move {
                            timeout(
                                POOL_QUOTE_TIMEOUT,
                                fetch_v3_lp_pool_quote(
                                    &url, venue, chain_id, t0, t1, dec0, dec1, fee,
                                ),
                            )
                            .await
                            .map_err(|_| {
                                WalletError::NetworkError("pool lookup timed out (45s)".into())
                            })?
                        })
                        .await
                    }))
                }
                UiJob::DeployToken {
                    name,
                    symbol,
                    supply,
                } => UiJobResult::DeployToken(handle.block_on(
                    WalletState::deploy_fixed_supply_token_background(
                        &wallet, &name, &symbol, &supply,
                    ),
                )),
                UiJob::McpQueuedApprove {
                    proposal_id,
                    source,
                    proposal,
                    fee_override,
                } => {
                    use crate::provider::{execute_approval_with_fee, ApprovalKind};
                    let proposal_for_flash = proposal.clone();
                    let kind = ApprovalKind::McpProposal {
                        proposal_id: proposal_id.clone(),
                        source,
                        proposal: Box::new(proposal),
                    };
                    let result = handle.block_on(async {
                        let mut w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                        execute_approval_with_fee(&kind, &mut w, fee_override.as_ref())
                            .await
                            .map_err(|e| WalletError::Other(e.to_string()))
                    });
                    UiJobResult::McpQueuedApprove {
                        result,
                        proposal: proposal_for_flash,
                        proposal_id,
                    }
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
                UiJobResult::McpQueuedApprove {
                    result,
                    proposal,
                    proposal_id,
                } => {
                    self.mcp_approve_inflight = false;
                    match result {
                        Ok(hash) => {
                            self.mcp.mark_proposal_decided(&proposal_id);
                            let flash = {
                                let wallet = self.wallet.lock().unwrap_or_else(|e| e.into_inner());
                                self.handle.block_on(provider::lp_brew_mint_success_flash(
                                    &wallet, &proposal, &hash,
                                ))
                            };
                            if let Some((title, rows)) = flash {
                                self.set_success_flash(title, rows);
                            } else {
                                self.set_flash(format!("Queued proposal executed: {hash}"));
                            }
                        }
                        Err(e) => {
                            self.mcp.clear_inflight_proposal(&proposal_id);
                            self.set_flash(format!("Queued proposal failed: {}", e.user_message()));
                        }
                    }
                    let back = self.approve_return;
                    self.navigate(back);
                }
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
                            self.enforce_assist_entitlement("unlock");
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
                UiJobResult::Chrome {
                    owner,
                    gen,
                    result: r,
                } => {
                    if !self.f2_result_current(&owner, gen) {
                        // Stale F3 race — keep waiting for the current owner fetch.
                    } else {
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
                }
                UiJobResult::Balance(r) => {
                    if let View::Dashboard(v) = &mut self.view {
                        v.apply_balance(r);
                    }
                }
                UiJobResult::Assets {
                    owner,
                    gen,
                    result: r,
                } => {
                    if !self.f2_result_current(&owner, gen) {
                        // Stale F3 race.
                    } else {
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
                }
                other => {
                    if let UiJobResult::AssistBurnVerify(ref r) = other {
                        match r {
                            Ok(true) => self.set_flash(
                                "Tools unlocked for this wallet — AI/MCP on every account",
                            ),
                            Ok(false) => self.set_flash(
                                "Burn not seen yet — wait a block, or burn ≥13 WZRD in one tx",
                            ),
                            Err(e) => {
                                self.set_flash(format!("Unlock check failed: {}", e.user_message()))
                            }
                        }
                        continue;
                    }
                    if let UiJobResult::DeployToken(ref deploy_result) = other {
                        self.finalize_mcp_token_launch(deploy_result);
                    }
                    if let UiJobResult::DeployToken(Ok(outcome)) = &other {
                        self.chrome.pending_asset_address = Some(outcome.token.address.clone());
                        self.sync_f2_to_displayed();
                    }
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
                        if matches!(&self.view, View::Dashboard(v) if v.is_assist_burn()) {
                            self.spawn_job(UiJob::AssistBurnVerify);
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
                        View::Hex(v) => {
                            v.apply_job_result(other);
                            v.reload_job()
                        }
                        View::Lp(v) => {
                            v.apply_job_result(other);
                            None
                        }
                        View::TokenLaunch(v) => {
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
                    let lp_followup = {
                        let wallet = self.wallet.lock().unwrap_or_else(|e| e.into_inner());
                        if let View::Lp(v) = &mut self.view {
                            v.followup_job(&wallet)
                        } else {
                            None
                        }
                    };
                    if let Some(job) = reload.or(dex_followup).or(lp_followup) {
                        self.spawn_job(job);
                    }
                    if send_ok {
                        if let Some(addr) = dex_swap_token {
                            {
                                let mut w = self.wallet.lock().unwrap_or_else(|e| e.into_inner());
                                if let Err(e) = self.handle.block_on(w.import_custom_token(&addr)) {
                                    tracing::warn!(error = %e, "import swap token-out failed");
                                }
                            }
                            self.chrome.pending_asset_address = Some(addr);
                        }
                        self.sync_f2_to_displayed();
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
    /// Jump to a common task screen (when the view did not consume the key).
    Navigate(Screen),
    /// Pop the navigation stack (Esc).
    Back,
    /// Refresh status chrome (balance + gas).
    RefreshChrome,
    /// Lock the vault and return to the unlock screen.
    Lock,
    /// Cycle stock UI theme (boxes / footer; banner + address unchanged).
    CycleTheme,
    /// Copy the F3-active address to the clipboard.
    CopyAddress,
    /// Prefill Home send for ≥13 WZRD burn unlock (footer `w` / Settings `w`).
    AssistBurn,
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
/// navigate" rule lives. Tab is reserved for per-view field/box focus.
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

    if key.code == KeyCode::Esc {
        return GlobalAction::Back;
    }

    // Footer shortcuts — available from every unlocked view when idle.
    match key.code {
        KeyCode::Char(c) => match c.to_ascii_lowercase() {
            's' => GlobalAction::Navigate(Screen::Dashboard),
            'v' => GlobalAction::Navigate(Screen::Receive),
            'a' => GlobalAction::Navigate(Screen::Assets),
            'b' => GlobalAction::Navigate(Screen::AaSend),
            'q' => GlobalAction::Navigate(Screen::Dapps),
            'd' => GlobalAction::Navigate(Screen::Dex),
            'g' => GlobalAction::Navigate(Screen::Aggregator),
            'n' | 'i' => GlobalAction::Navigate(Screen::Settings),
            'k' => GlobalAction::Navigate(Screen::Keys),
            'e' => GlobalAction::Navigate(Screen::Wrap),
            'u' => GlobalAction::Navigate(Screen::Hex),
            'p' => GlobalAction::Navigate(Screen::Lp),
            'f' => GlobalAction::Navigate(Screen::Bridge),
            'j' => GlobalAction::Navigate(Screen::Approvals),
            'm' => GlobalAction::Navigate(Screen::History),
            'o' => GlobalAction::Navigate(Screen::SoonNft),
            'z' => GlobalAction::Navigate(Screen::TokenLaunch),
            'w' => GlobalAction::AssistBurn,
            'r' => GlobalAction::RefreshChrome,
            'h' => GlobalAction::Navigate(Screen::Dashboard),
            'l' => GlobalAction::Lock,
            't' => GlobalAction::CycleTheme,
            'y' => GlobalAction::CopyAddress,
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
    fn tab_is_never_global() {
        assert_eq!(
            global_action(tab(), &KeyOutcome::Consumed),
            GlobalAction::None
        );
        assert_eq!(
            global_action(tab(), &KeyOutcome::NotHandled),
            GlobalAction::None
        );
    }

    #[test]
    fn other_keys_are_inert() {
        assert_eq!(
            global_action(press('c'), &KeyOutcome::NotHandled),
            GlobalAction::None
        );
        assert_eq!(
            global_action(press('1'), &KeyOutcome::NotHandled),
            GlobalAction::None
        );
        assert_eq!(
            global_action(press('u'), &KeyOutcome::Navigate(Screen::Dashboard)),
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
            global_action(press('w'), &KeyOutcome::NotHandled),
            GlobalAction::AssistBurn
        );
        assert_eq!(
            global_action(press('q'), &KeyOutcome::NotHandled),
            GlobalAction::Navigate(Screen::Dapps)
        );
        assert_eq!(
            global_action(press('z'), &KeyOutcome::NotHandled),
            GlobalAction::Navigate(Screen::TokenLaunch)
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
            GlobalAction::Back
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
