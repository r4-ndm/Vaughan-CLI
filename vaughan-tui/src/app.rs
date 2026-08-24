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
use vaughan_core::core::proposal::ProposalQueue;
use vaughan_core::core::{McpSessionToken, StateManager, WalletState};
use vaughan_core::error::WalletError;
use vaughan_provider::{EventBus, ProviderError, ProviderEvent};

use crate::jobs::{ChromeFocus, ChromeSnapshot, UiJob, UiJobResult};
use crate::mcp::{McpHostRequest, McpService, McpSessionSnapshot};
use crate::provider::{self, ApprovalKind, HostRequest};
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
            Self::Dapps => "Web (Freedom)",
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
}

impl std::fmt::Debug for KeyOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Consumed => write!(f, "Consumed"),
            Self::NotHandled => write!(f, "NotHandled"),
            Self::Navigate(s) => f.debug_tuple("Navigate").field(s).finish(),
            Self::StartJob(_) => write!(f, "StartJob(..)"),
            Self::SendAsset(_) => write!(f, "SendAsset(..)"),
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

    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        match self {
            Self::Onboarding(v) => v.render(frame, area, wallet),
            Self::Unlock(v) => v.render(frame, area, wallet),
            Self::Dashboard(v) => v.render(frame, area, wallet),
            Self::AaSend(v) => v.render(frame, area, wallet),
            Self::Receive(v) => v.render(frame, area, wallet),
            Self::Settings(v) => v.render(frame, area, wallet),
            Self::Keys(v) => v.render(frame, area, wallet),
            Self::Dapps(v) => v.render(frame, area, wallet),
            Self::Assets(v) => v.render(frame, area, wallet),
            Self::Browser(v) => v.render(frame, area, wallet),
            Self::Dex(v) => v.render(frame, area, wallet),
            Self::Aggregator(v) => v.render(frame, area, wallet),
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
}

/// A sign/send request waiting on the user's approve/deny decision.
struct PendingApproval {
    kind: ApprovalKind,
    reply: Option<oneshot::Sender<Result<String, ProviderError>>>,
}

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
    host_rx: mpsc::UnboundedReceiver<HostRequest>,
    /// Background job results (balance / fee / send).
    job_tx: mpsc::UnboundedSender<UiJobResult>,
    job_rx: mpsc::UnboundedReceiver<UiJobResult>,
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
    mcp_rx: mpsc::UnboundedReceiver<McpHostRequest>,
}

impl App {
    /// Load the wallet from the default location and pick the initial screen.
    pub fn new(handle: Handle) -> Result<Self, WalletError> {
        let path = StateManager::default_path()?;
        let wallet = WalletState::load(path)?;
        let screen = if !wallet.is_initialized() {
            Screen::Onboarding
        } else if !wallet.is_unlocked() {
            Screen::Unlock
        } else {
            Screen::Dashboard
        };
        let events = EventBus::new();
        let (host_tx, host_rx) = mpsc::unbounded_channel();
        let (mcp_tx, mcp_rx) = mpsc::unbounded_channel();
        let (job_tx, job_rx) = mpsc::unbounded_channel();
        let dapp_origins = wallet.trusted_dapp_origins();
        provider::spawn_provider_server(&handle, host_tx, events.clone(), dapp_origins);
        let profile_dir = profile_dir(wallet.path());
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
        };
        app.navigate(screen);
        Ok(app)
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
            self.view.render(frame, area, &wallet);
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
            self.poll_provider();
            self.poll_mcp();
            self.poll_jobs();
            if let View::Dex(v) = &mut self.view {
                v.set_tick(self.tick);
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
            terminal.draw(|frame| crate::views::render(frame, self))?;
            if event::poll(Duration::from_millis(80))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        self.handle_key(key);
                    }
                }
            }
        }
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) {
        // Quit confirm owns the keyboard until Yes/No/Esc.
        if self.quit_confirm.is_some() {
            self.handle_quit_confirm_key(key);
            return;
        }

        // The approval prompt owns its own key handling (approve/deny) and its
        // reply channel, so global shortcuts don't apply while it is shown.
        if self.screen() == Screen::Approve {
            self.handle_approval_key(key);
            return;
        }

        // F1/F2/F3 status strip — takes priority over view ↑/↓ when unlocked.
        if self.wallet().is_unlocked() && self.handle_chrome_hotkey(key) {
            return;
        }

        let outcome = {
            let mut wallet = self.wallet.lock().unwrap_or_else(|e| e.into_inner());
            self.view
                .handle_key(key, &mut wallet, &self.handle, &self.events)
        };

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
            GlobalAction::None => {}
        }

        match outcome {
            KeyOutcome::Navigate(screen) => self.navigate(screen),
            KeyOutcome::StartJob(job) => self.spawn_job(job),
            KeyOutcome::SendAsset(balance) => {
                // Align F2 with the asset the user picked from Assets.
                if let Some(i) = self.chrome.assets.iter().position(|b| {
                    b.token.contract_address == balance.token.contract_address
                        && b.token.symbol == balance.token.symbol
                }) {
                    self.chrome.asset_idx = i;
                }
                self.view = View::Dashboard(DashboardView::for_asset(balance));
            }
            KeyOutcome::Consumed | KeyOutcome::NotHandled => {}
        }
    }

    /// Drain the provider request channel, answering read queries inline and
    /// surfacing sign/send requests as an approval prompt.
    fn poll_provider(&mut self) {
        while let Ok(request) = self.host_rx.try_recv() {
            match request {
                HostRequest::Accounts { reply } | HostRequest::RequestAccounts { reply } => {
                    let _ = reply.send(Ok(self.visible_accounts()));
                }
                HostRequest::ChainId { reply } => {
                    let id = self.wallet().networks().active().chain_id;
                    let _ = reply.send(Ok(format!("0x{id:x}")));
                }
                HostRequest::SwitchChain { chain_id, reply } => {
                    let _ = reply.send(self.switch_chain(&chain_id));
                }
                HostRequest::Approval {
                    kind,
                    origin,
                    reply,
                } => {
                    // Locked wallet: reject without prompting (the execution
                    // path guards too, but no prompt should ever appear).
                    if !self.wallet().is_unlocked() {
                        let _ = reply.send(Err(ProviderError::Unauthorized(
                            "wallet is locked; unlock it first".to_string(),
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
                    self.view = View::Approve(ApproveView::new(title, origin, details));
                    self.pending_approval = Some(PendingApproval {
                        kind: *kind,
                        reply: Some(reply),
                    });
                    // One approval on screen at a time; remaining queued
                    // requests are served once this one resolves.
                    break;
                }
            }
        }
    }

    fn poll_mcp(&mut self) {
        let mcp_pending = if self.wallet().is_unlocked() {
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
        let pending_on_screen = self.pending_approval.is_some();
        self.mcp.poll_file_queue(pending_on_screen);

        while let Ok(request) = self.mcp_rx.try_recv() {
            match request {
                McpHostRequest::Propose {
                    proposal,
                    source,
                    reply,
                } => {
                    if !self.wallet().is_unlocked() {
                        if let Some(r) = reply {
                            let _ = r.send(Err(ProviderError::Unauthorized(
                                "wallet is locked".into(),
                            )));
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
                    let proposal_id = proposal.proposal_id.clone();
                    let kind = ApprovalKind::McpProposal {
                        proposal_id,
                        source: source.clone(),
                        proposal,
                    };
                    let preview =
                        provider::describe_approval(&kind, &self.wallet(), &self.handle);
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
                    self.pending_approval = Some(PendingApproval {
                        kind,
                        reply,
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

    /// `wallet_switchEthereumChain`: switch to a built-in network by chain id.
    fn switch_chain(&mut self, chain_id: &str) -> Result<(), ProviderError> {
        use vaughan_core::chains::evm::networks::get_network_by_chain_id;
        let id: u64 = chain_id
            .parse()
            .map_err(|_| ProviderError::UnrecognizedChain(chain_id.to_string()))?;
        let net = get_network_by_chain_id(id)
            .ok_or_else(|| ProviderError::UnrecognizedChain(chain_id.to_string()))?;
        self.wallet()
            .set_active_network(&net.id)
            .map_err(|e| ProviderError::Internal(e.user_message()))?;
        self.events
            .publish(ProviderEvent::ChainChanged(format!("0x{id:x}")));
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

    /// Resolve the on-screen approval: deny on `n`/Esc, approve on `y`/Enter.
    /// Ctrl+C/Ctrl+Q still quit; dropping `pending_approval`'s reply channel
    /// makes the waiting handler future observe a closed channel.
    fn handle_approval_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('q')) {
            self.quit_confirm = Some(true);
            return;
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
                let wallet = self.wallet();
                if let Some(parent) = wallet.path().parent() {
                    if let Ok(Some(token)) = McpSessionToken::read(parent) {
                        let queue = ProposalQueue::new(parent);
                        let _ = queue.mark_rejected(proposal_id, "user rejected", token.as_bytes());
                    }
                }
            }
        }
        let result = if deny {
            Err(ProviderError::UserRejected)
        } else {
            provider::execute_approval_sync(&pending.kind, &self.wallet(), &self.handle)
        };
        let _ = pending.reply.map(|reply| reply.send(result));
        let back = self.approve_return;
        self.navigate(back);
    }

    /// Build the default view for `screen` (refreshing balance on Dashboard).
    fn navigate(&mut self, screen: Screen) {
        let view = match screen {
            Screen::Onboarding => View::Onboarding(OnboardingView::default()),
            Screen::Unlock => View::Unlock(UnlockView::default()),
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
            Screen::History => View::History(HistoryView::loading()),
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
                self.spawn_job(UiJob::RefreshAssets);
            }
            Screen::History => {
                self.refresh_chrome();
                self.spawn_job(HistoryView::initial_job());
            }
            Screen::Approvals => {
                self.refresh_chrome();
                self.spawn_job(ApprovalsView::initial_job());
            }
            _ => self.refresh_chrome(),
        }
    }

    /// Refresh always-on network / balance / gas chrome (unlocked screens).
    fn refresh_chrome(&mut self) {
        if !self.wallet().is_unlocked() {
            return;
        }
        self.chrome.loading = true;
        self.chrome.error = None;
        self.spawn_job(UiJob::RefreshChrome);
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
        self.chrome.error = None;
        match self.screen() {
            Screen::Dex | Screen::Aggregator | Screen::Settings => {
                self.navigate(self.screen());
            }
            _ => {}
        }
        self.refresh_chrome();
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
        self.refresh_chrome();
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
            // TUI on `[busy]` / `(busy)` (seen after unlock → Degen → Dashboard chrome refresh).
            let result = match job {
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
                    UiJobResult::Send(
                        handle
                            .block_on(w.send_with_fee(&to, &value_wei, &fee))
                            .map(|h| h.to_string()),
                    )
                }
                UiJob::Send { to, value_wei } => {
                    let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                    UiJobResult::Send(
                        handle
                            .block_on(w.send(&to, &value_wei))
                            .map(|h| h.to_string()),
                    )
                }
                UiJob::SendToken { token, to, amount } => {
                    let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                    UiJobResult::Send(
                        handle
                            .block_on(w.send_token(&token, &to, &amount))
                            .map(|h| h.to_string()),
                    )
                }
                UiJob::SendTokenWithFee {
                    token,
                    to,
                    amount,
                    fee,
                } => {
                    let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
                    UiJobResult::Send(
                        handle
                            .block_on(w.send_token_with_fee(&token, &to, &amount, &fee))
                            .map(|h| h.to_string()),
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
                    UiJobResult::Send(
                        handle
                            .block_on(w.send_transaction(tx))
                            .map(|h| h.to_string()),
                    )
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
                    use alloy::primitives::{Address, U256};
                    use vaughan_core::core::{quote_aggregator, AggQuoteRequest};

                    let parsed = (|| -> Result<AggQuoteRequest, WalletError> {
                        let token_in = Address::from_str(&token_in)
                            .map_err(|_| WalletError::InvalidTransaction("agg token_in".into()))?;
                        let token_out = Address::from_str(&token_out)
                            .map_err(|_| WalletError::InvalidTransaction("agg token_out".into()))?;
                        let amount_in = U256::from_str(&amount)
                            .map_err(|_| WalletError::InvalidAmount("agg amount".into()))?;
                        let account = account
                            .as_deref()
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
                    })();
                    let dir = StateManager::default_path()
                        .ok()
                        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
                    UiJobResult::AggQuote(match parsed {
                        Ok(req) => {
                            handle.block_on(quote_aggregator(venue, &req, dir.as_deref(), None))
                        }
                        Err(e) => Err(e),
                    })
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
            };
            let _ = tx.send(result);
        });
    }

    fn poll_jobs(&mut self) {
        while let Ok(result) = self.job_rx.try_recv() {
            match result {
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
                            self.chrome.assets = assets
                                .iter()
                                .filter(|b| {
                                    let raw = b.raw.trim();
                                    !raw.is_empty() && raw != "0" && raw != "0x0"
                                })
                                .cloned()
                                .collect();
                            if self.chrome.asset_idx >= self.chrome.assets.len() {
                                self.chrome.asset_idx = self.chrome.assets.len().saturating_sub(1);
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
                    let reload = match &mut self.view {
                        View::Dashboard(v) => {
                            v.apply_job_result(other);
                            None
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
                        _ => None,
                    };
                    if let Some(job) = reload {
                        self.spawn_job(job);
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
            'r' => GlobalAction::RefreshChrome,
            'h' => GlobalAction::Navigate(Screen::Dashboard),
            'l' => GlobalAction::Lock,
            't' => GlobalAction::CycleTheme,
            _ => GlobalAction::None,
        },
        _ => GlobalAction::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
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
            global_action(press('o'), &KeyOutcome::NotHandled),
            GlobalAction::None
        );
        assert_eq!(
            global_action(press('q'), &KeyOutcome::Navigate(Screen::Dashboard)),
            GlobalAction::None
        );
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
            global_action(press('r'), &KeyOutcome::NotHandled),
            GlobalAction::RefreshChrome
        );
        assert_eq!(
            global_action(press('l'), &KeyOutcome::NotHandled),
            GlobalAction::Lock
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
    }
}
