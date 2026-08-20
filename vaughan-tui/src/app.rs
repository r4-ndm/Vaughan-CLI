//! Application state and the main event loop.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::{DefaultTerminal, Frame};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};
use vaughan_agent::ModelConfig;
use vaughan_core::core::{StateManager, WalletState};
use vaughan_core::error::WalletError;
use vaughan_provider::{EventBus, ProviderError, ProviderEvent};

use std::str::FromStr;

use crate::jobs::{UiJob, UiJobResult};
use crate::provider::{self, ApprovalKind, HostRequest};
use crate::views::{
    AaSendView, AgentSetupView, AgentView, ApproveView, AssetsView, BrowserView, DappsView,
    DashboardView, KeysView, OnboardingView, ReceiveView, SendView, SettingsView, UnlockView,
};

/// The active screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Onboarding,
    Unlock,
    AgentSetup,
    Dashboard,
    Send,
    AaSend,
    Receive,
    Settings,
    Keys,
    Dapps,
    Assets,
    Browser,
    Agent,
    Approve,
}

impl Screen {
    pub fn title(self) -> &'static str {
        match self {
            Self::Onboarding => "Onboarding",
            Self::Unlock => "Unlock",
            Self::AgentSetup => "AI Setup",
            Self::Dashboard => "Dashboard",
            Self::Send => "Send",
            Self::AaSend => "Batch Send",
            Self::Receive => "Receive",
            Self::Settings => "Settings",
            Self::Keys => "Keys",
            Self::Dapps => "dApps",
            Self::Assets => "Assets",
            Self::Browser => "Contract Browser",
            Self::Agent => "AI Agent",
            Self::Approve => "Approve",
        }
    }
}

/// How the active view handled a key.
///
/// The app uses this to decide whether global shortcuts apply: a `Consumed`
/// key (e.g. `'q'` or `Tab` typed into a text field) must never trigger a
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
    AgentSetup(AgentSetupView),
    Dashboard(DashboardView),
    Send(SendView),
    AaSend(AaSendView),
    Receive(ReceiveView),
    Settings(SettingsView),
    Keys(KeysView),
    Dapps(DappsView),
    Assets(AssetsView),
    Browser(BrowserView),
    Agent(AgentView),
    Approve(ApproveView),
}

impl View {
    pub fn screen(&self) -> Screen {
        match self {
            Self::Onboarding(_) => Screen::Onboarding,
            Self::Unlock(_) => Screen::Unlock,
            Self::AgentSetup(_) => Screen::AgentSetup,
            Self::Dashboard(_) => Screen::Dashboard,
            Self::Send(_) => Screen::Send,
            Self::AaSend(_) => Screen::AaSend,
            Self::Receive(_) => Screen::Receive,
            Self::Settings(_) => Screen::Settings,
            Self::Keys(_) => Screen::Keys,
            Self::Dapps(_) => Screen::Dapps,
            Self::Assets(_) => Screen::Assets,
            Self::Browser(_) => Screen::Browser,
            Self::Agent(_) => Screen::Agent,
            Self::Approve(_) => Screen::Approve,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        match self {
            Self::Onboarding(v) => v.render(frame, area, wallet),
            Self::Unlock(v) => v.render(frame, area, wallet),
            Self::AgentSetup(v) => v.render(frame, area, wallet),
            Self::Dashboard(v) => v.render(frame, area, wallet),
            Self::Send(v) => v.render(frame, area, wallet),
            Self::AaSend(v) => v.render(frame, area, wallet),
            Self::Receive(v) => v.render(frame, area, wallet),
            Self::Settings(v) => v.render(frame, area, wallet),
            Self::Keys(v) => v.render(frame, area, wallet),
            Self::Dapps(v) => v.render(frame, area, wallet),
            Self::Assets(v) => v.render(frame, area, wallet),
            Self::Browser(v) => v.render(frame, area, wallet),
            Self::Agent(v) => v.render(frame, area, wallet.operating_mode()),
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
            Self::AgentSetup(v) => v.handle_key(key, wallet, handle, events),
            Self::Dashboard(v) => v.handle_key(key, wallet, handle, events),
            Self::Send(v) => v.handle_key(key, wallet, handle, events),
            Self::AaSend(v) => v.handle_key(key, wallet, handle, events),
            Self::Receive(v) => v.handle_key(key, wallet, handle, events),
            Self::Settings(v) => v.handle_key(key, wallet, handle, events),
            Self::Keys(v) => v.handle_key(key, wallet, handle, events),
            Self::Dapps(v) => v.handle_key(key, wallet, handle, events),
            Self::Assets(v) => v.handle_key(key, wallet, handle, events),
            Self::Browser(v) => v.handle_key(key, wallet, handle, events),
            Self::Agent(v) => {
                let context = vaughan_agent::tools::ToolContext {
                    rpc_url: wallet.networks().active().rpc_url.clone(),
                    chain_id: wallet.networks().active().chain_id,
                    active_address: wallet
                        .active_address()
                        .ok()
                        .and_then(|a| alloy::primitives::Address::from_str(a).ok()),
                };
                v.handle_key(key, wallet.operating_mode(), &context)
            }
            Self::Approve(v) => v.handle_key(key, wallet, handle, events),
        }
    }
}

/// A sign/send request waiting on the user's approve/deny decision.
struct PendingApproval {
    kind: ApprovalKind,
    reply: oneshot::Sender<Result<String, ProviderError>>,
}

/// Root application state.
pub struct App {
    wallet: Arc<Mutex<WalletState>>,
    handle: Handle,
    view: View,
    quitting: bool,
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
    /// Active LLM settings for this session (welcome setup / unlock load / env).
    agent_config: ModelConfig,
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
        let (job_tx, job_rx) = mpsc::unbounded_channel();
        let dapp_origins = wallet.trusted_dapp_origins();
        provider::spawn_provider_server(&handle, host_tx, events.clone(), dapp_origins);
        let mut app = Self {
            wallet: Arc::new(Mutex::new(wallet)),
            handle,
            view: View::Onboarding(OnboardingView::default()),
            quitting: false,
            events,
            host_rx,
            job_tx,
            job_rx,
            tick: 0,
            pending_approval: None,
            approve_return: Screen::Dashboard,
            agent_config: ModelConfig::from_env(),
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
            self.poll_agent();
            self.poll_jobs();
            if let View::Send(v) = &mut self.view {
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
        // The approval prompt owns its own key handling (approve/deny) and its
        // reply channel, so global shortcuts don't apply while it is shown.
        if self.screen() == Screen::Approve {
            self.handle_approval_key(key);
            return;
        }

        let outcome = {
            let mut wallet = self.wallet.lock().unwrap_or_else(|e| e.into_inner());
            self.view
                .handle_key(key, &mut wallet, &self.handle, &self.events)
        };

        match global_action(key, &outcome) {
            GlobalAction::Quit => {
                self.quitting = true;
                return;
            }
            GlobalAction::CycleScreens => {
                let next = match self.screen() {
                    Screen::Dashboard => Screen::Send,
                    Screen::Send => Screen::AaSend,
                    Screen::AaSend => Screen::Receive,
                    Screen::Receive => Screen::Settings,
                    Screen::Settings => Screen::Keys,
                    Screen::Keys => Screen::Dapps,
                    Screen::Dapps => Screen::Assets,
                    Screen::Assets => Screen::Browser,
                    Screen::Browser => Screen::Agent,
                    Screen::Agent => Screen::Dashboard,
                    other => other,
                };
                if next != self.screen() {
                    self.navigate(next);
                }
                return;
            }
            GlobalAction::None => {}
        }

        match outcome {
            KeyOutcome::Navigate(screen) => {
                self.capture_session_agent_config();
                self.navigate(screen);
            }
            KeyOutcome::StartJob(job) => self.spawn_job(job),
            KeyOutcome::SendAsset(balance) => {
                self.view = View::Send(SendView::for_asset(balance));
            }
            _ => {}
        }
    }

    /// Pull agent config out of onboarding / unlock / setup before those views are dropped.
    fn capture_session_agent_config(&mut self) {
        match &mut self.view {
            View::Onboarding(v) => {
                if let Some(cfg) = v.take_session_agent_config() {
                    self.agent_config = cfg;
                }
            }
            View::Unlock(v) => {
                if let Some(cfg) = v.take_session_agent_config() {
                    self.agent_config = cfg;
                }
            }
            View::AgentSetup(v) => {
                if let Some(cfg) = v.take_session_agent_config() {
                    self.agent_config = cfg;
                }
            }
            _ => {}
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
                    self.pending_approval = Some(PendingApproval { kind: *kind, reply });
                    // One approval on screen at a time; remaining queued
                    // requests are served once this one resolves.
                    break;
                }
            }
        }
    }

    /// Pump streaming agent chat events into the Agent view.
    fn poll_agent(&mut self) {
        if let View::Agent(view) = &mut self.view {
            view.poll();
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

    /// Resolve the on-screen approval: deny on `n`/Esc, approve on `y`/Enter.
    /// Ctrl+C/Ctrl+Q still quit; dropping `pending_approval`'s reply channel
    /// makes the waiting handler future observe a closed channel.
    fn handle_approval_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('q')) {
            self.quitting = true;
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
        let result = if deny {
            Err(ProviderError::UserRejected)
        } else {
            provider::execute_approval_sync(&pending.kind, &self.wallet(), &self.handle)
        };
        let _ = pending.reply.send(result);
        let back = self.approve_return;
        self.navigate(back);
    }

    /// Build the default view for `screen` (refreshing balance on Dashboard).
    fn navigate(&mut self, screen: Screen) {
        let setup_password = if screen == Screen::AgentSetup {
            if let View::Unlock(v) = &mut self.view {
                v.take_handoff_password()
            } else {
                None
            }
        } else {
            None
        };

        let view = match screen {
            Screen::Onboarding => View::Onboarding(OnboardingView::default()),
            Screen::Unlock => View::Unlock(UnlockView::default()),
            Screen::AgentSetup => View::AgentSetup(AgentSetupView::new(setup_password)),
            Screen::Dashboard => View::Dashboard(DashboardView::loading()),
            Screen::Send => View::Send(SendView::default()),
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
            Screen::Agent => {
                let dir = vaughan_agent::profile_dir(self.wallet().path());
                View::Agent(AgentView::with_session(
                    self.agent_config.clone(),
                    self.wallet().operating_mode(),
                    Some(dir.as_path()),
                ))
            }
            // Approve is entered directly from `poll_provider` (it needs the
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
            Screen::Dashboard => self.spawn_job(UiJob::RefreshBalance),
            Screen::Assets => self.spawn_job(UiJob::RefreshAssets),
            _ => {}
        }
    }

    fn spawn_job(&self, job: UiJob) {
        let tx = self.job_tx.clone();
        let handle = self.handle.clone();
        let wallet = self.wallet.clone();
        std::thread::spawn(move || {
            let w = wallet.lock().unwrap_or_else(|e| e.into_inner());
            let result = match job {
                UiJob::RefreshBalance => UiJobResult::Balance(handle.block_on(w.balance())),
                UiJob::RefreshAssets => UiJobResult::Assets(handle.block_on(w.assets())),
                UiJob::EstimateFee { to, value_wei } => {
                    UiJobResult::Fee(handle.block_on(w.estimate_fee(&to, &value_wei)))
                }
                UiJob::EstimateTokenFee { token, to, amount } => {
                    UiJobResult::Fee(handle.block_on(w.estimate_token_fee(&token, &to, &amount)))
                }
                UiJob::SendWithFee { to, value_wei, fee } => UiJobResult::Send(
                    handle
                        .block_on(w.send_with_fee(&to, &value_wei, &fee))
                        .map(|h| h.to_string()),
                ),
                UiJob::Send { to, value_wei } => UiJobResult::Send(
                    handle
                        .block_on(w.send(&to, &value_wei))
                        .map(|h| h.to_string()),
                ),
                UiJob::SendToken { token, to, amount } => UiJobResult::Send(
                    handle
                        .block_on(w.send_token(&token, &to, &amount))
                        .map(|h| h.to_string()),
                ),
                UiJob::SendTokenWithFee {
                    token,
                    to,
                    amount,
                    fee,
                } => UiJobResult::Send(
                    handle
                        .block_on(w.send_token_with_fee(&token, &to, &amount, &fee))
                        .map(|h| h.to_string()),
                ),
                UiJob::SendStealth {
                    announcement,
                    value_wei,
                } => UiJobResult::SendStealth(
                    handle.block_on(w.send_stealth(&announcement, &value_wei)),
                ),
            };
            drop(w);
            let _ = tx.send(result);
        });
    }

    fn poll_jobs(&mut self) {
        while let Ok(result) = self.job_rx.try_recv() {
            match (&mut self.view, result) {
                (View::Dashboard(v), UiJobResult::Balance(r)) => v.apply_balance(r),
                (View::Assets(v), UiJobResult::Assets(r)) => v.apply_assets(r),
                (View::Send(v), r) => v.apply_job_result(r),
                _ => {}
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
}

/// Decide global shortcuts for `key` given how the active view handled it.
///
/// Pure and unit-tested: this is where the "typing 'q' must not quit" and
/// "Tab inside a form must not switch screens" rules live.
fn global_action(key: KeyEvent, outcome: &KeyOutcome) -> GlobalAction {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    // Ctrl+C / Ctrl+Q always quit, even mid-typing.
    if ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('q')) {
        return GlobalAction::Quit;
    }

    // A bare 'q' quits only when the active view did not consume the key.
    if key.code == KeyCode::Char('q') && !ctrl && matches!(outcome, KeyOutcome::NotHandled) {
        return GlobalAction::Quit;
    }

    // Tab cycles screens only when the active view did not consume it.
    if key.code == KeyCode::Tab && matches!(outcome, KeyOutcome::NotHandled) {
        return GlobalAction::CycleScreens;
    }

    GlobalAction::None
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
    fn typing_q_never_quits() {
        // The exact audit 2.2 regression: 'q' typed into a text field.
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
    fn q_quits_only_when_unhandled() {
        assert_eq!(
            global_action(press('q'), &KeyOutcome::NotHandled),
            GlobalAction::Quit
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
            global_action(press('x'), &KeyOutcome::NotHandled),
            GlobalAction::None
        );
        assert_eq!(
            global_action(press('q'), &KeyOutcome::Navigate(Screen::Send)),
            GlobalAction::None
        );
    }
}
