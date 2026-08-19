//! Application state and the main event loop.

use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::{DefaultTerminal, Frame};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};
use vaughan_core::core::{StateManager, WalletState};
use vaughan_core::error::WalletError;
use vaughan_provider::{EventBus, ProviderError, ProviderEvent};

use crate::provider::{self, ApprovalKind, HostRequest};
use crate::views::{
    AaSendView, ApproveView, AssetsView, BrowserView, DashboardView, OnboardingView, ReceiveView,
    SendView, SettingsView, UnlockView,
};

/// The active screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Onboarding,
    Unlock,
    Dashboard,
    Send,
    AaSend,
    Receive,
    Settings,
    Assets,
    Browser,
    Approve,
}

impl Screen {
    pub fn title(self) -> &'static str {
        match self {
            Self::Onboarding => "Onboarding",
            Self::Unlock => "Unlock",
            Self::Dashboard => "Dashboard",
            Self::Send => "Send",
            Self::AaSend => "Batch Send",
            Self::Receive => "Receive",
            Self::Settings => "Settings",
            Self::Assets => "Assets",
            Self::Browser => "Contract Browser",
            Self::Approve => "Approve",
        }
    }
}

/// How the active view handled a key.
///
/// The app uses this to decide whether global shortcuts apply: a `Consumed`
/// key (e.g. `'q'` or `Tab` typed into a text field) must never trigger a
/// global action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOutcome {
    /// The view consumed the key; the app must not apply global shortcuts.
    Consumed,
    /// The view did not use the key; the app may apply global shortcuts.
    NotHandled,
    /// The view wants to navigate to a new screen.
    Navigate(Screen),
}

/// The active view (screen + its state).
pub enum View {
    Onboarding(OnboardingView),
    Unlock(UnlockView),
    Dashboard(DashboardView),
    Send(SendView),
    AaSend(AaSendView),
    Receive(ReceiveView),
    Settings(SettingsView),
    Assets(AssetsView),
    Browser(BrowserView),
    Approve(ApproveView),
}

impl View {
    pub fn screen(&self) -> Screen {
        match self {
            Self::Onboarding(_) => Screen::Onboarding,
            Self::Unlock(_) => Screen::Unlock,
            Self::Dashboard(_) => Screen::Dashboard,
            Self::Send(_) => Screen::Send,
            Self::AaSend(_) => Screen::AaSend,
            Self::Receive(_) => Screen::Receive,
            Self::Settings(_) => Screen::Settings,
            Self::Assets(_) => Screen::Assets,
            Self::Browser(_) => Screen::Browser,
            Self::Approve(_) => Screen::Approve,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        match self {
            Self::Onboarding(v) => v.render(frame, area, wallet),
            Self::Unlock(v) => v.render(frame, area, wallet),
            Self::Dashboard(v) => v.render(frame, area, wallet),
            Self::Send(v) => v.render(frame, area, wallet),
            Self::AaSend(v) => v.render(frame, area, wallet),
            Self::Receive(v) => v.render(frame, area, wallet),
            Self::Settings(v) => v.render(frame, area, wallet),
            Self::Assets(v) => v.render(frame, area, wallet),
            Self::Browser(v) => v.render(frame, area, wallet),
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
            Self::Send(v) => v.handle_key(key, wallet, handle, events),
            Self::AaSend(v) => v.handle_key(key, wallet, handle, events),
            Self::Receive(v) => v.handle_key(key, wallet, handle, events),
            Self::Settings(v) => v.handle_key(key, wallet, handle, events),
            Self::Assets(v) => v.handle_key(key, wallet, handle, events),
            Self::Browser(v) => v.handle_key(key, wallet, handle, events),
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
    wallet: WalletState,
    handle: Handle,
    view: View,
    quitting: bool,
    /// Provider events published to connected dApps (chain/account changes).
    events: EventBus,
    /// Requests forwarded from the provider server; drained on the UI thread.
    host_rx: mpsc::UnboundedReceiver<HostRequest>,
    /// The approval currently on screen, if any.
    pending_approval: Option<PendingApproval>,
    /// Screen to return to after the pending approval resolves.
    approve_return: Screen,
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
        provider::spawn_provider_server(&handle, host_tx, events.clone());
        let mut app = Self {
            wallet,
            handle,
            view: View::Onboarding(OnboardingView::default()),
            quitting: false,
            events,
            host_rx,
            pending_approval: None,
            approve_return: Screen::Dashboard,
        };
        app.navigate(screen);
        Ok(app)
    }

    pub fn wallet(&self) -> &WalletState {
        &self.wallet
    }

    pub fn screen(&self) -> Screen {
        self.view.screen()
    }

    pub fn render_body(&self, frame: &mut Frame, area: Rect) {
        self.view.render(frame, area, &self.wallet);
    }

    /// Run the terminal event loop until the user quits.
    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        while !self.quitting {
            // Service provider requests first so an incoming approval prompt
            // is on screen before the next draw.
            self.poll_provider();
            terminal.draw(|frame| crate::views::render(frame, self))?;
            if event::poll(Duration::from_millis(100))? {
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

        let outcome = self
            .view
            .handle_key(key, &mut self.wallet, &self.handle, &self.events);

        match global_action(key, outcome) {
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
                    Screen::Settings => Screen::Assets,
                    Screen::Assets => Screen::Browser,
                    Screen::Browser => Screen::Dashboard,
                    other => other,
                };
                if next != self.screen() {
                    self.navigate(next);
                }
                return;
            }
            GlobalAction::None => {}
        }

        if let KeyOutcome::Navigate(screen) = outcome {
            self.navigate(screen);
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
                    let id = self.wallet.networks().active().chain_id;
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
                    if !self.wallet.is_unlocked() {
                        let _ = reply.send(Err(ProviderError::Unauthorized(
                            "wallet is locked; unlock it first".to_string(),
                        )));
                        continue;
                    }
                    let preview = provider::describe_approval(&kind, &self.wallet, &self.handle);
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

    /// The account list visible to dApps: the active account, or `[]` when the
    /// wallet is locked/uninitialized.
    fn visible_accounts(&self) -> Vec<String> {
        if !self.wallet.is_unlocked() {
            return Vec::new();
        }
        self.wallet
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
        self.wallet
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
            provider::execute_approval_sync(&pending.kind, &self.wallet, &self.handle)
        };
        let _ = pending.reply.send(result);
        let back = self.approve_return;
        self.navigate(back);
    }

    /// Build the default view for `screen` (refreshing balance on Dashboard).
    fn navigate(&mut self, screen: Screen) {
        let view = match screen {
            Screen::Onboarding => View::Onboarding(OnboardingView::default()),
            Screen::Unlock => View::Unlock(UnlockView::default()),
            Screen::Dashboard => {
                let balance = self.handle.block_on(self.wallet.balance());
                View::Dashboard(DashboardView::with_balance(balance))
            }
            Screen::Send => View::Send(SendView::default()),
            Screen::AaSend => View::AaSend(AaSendView::default()),
            Screen::Receive => View::Receive(ReceiveView),
            Screen::Settings => {
                let active_id = self.wallet.networks().active_id().to_string();
                let selected = self
                    .wallet
                    .networks()
                    .networks()
                    .iter()
                    .position(|n| n.id == active_id)
                    .unwrap_or(0);
                View::Settings(SettingsView::new(selected))
            }
            Screen::Assets => {
                let assets = self.handle.block_on(self.wallet.assets());
                View::Assets(AssetsView::with_assets(assets))
            }
            Screen::Browser => View::Browser(BrowserView::default()),
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
fn global_action(key: KeyEvent, outcome: KeyOutcome) -> GlobalAction {
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
            global_action(press('q'), KeyOutcome::Consumed),
            GlobalAction::None
        );
        assert_eq!(
            global_action(press('q'), KeyOutcome::Navigate(Screen::Dashboard)),
            GlobalAction::None
        );
    }

    #[test]
    fn q_quits_only_when_unhandled() {
        assert_eq!(
            global_action(press('q'), KeyOutcome::NotHandled),
            GlobalAction::Quit
        );
    }

    #[test]
    fn ctrl_quit_always_wins() {
        assert_eq!(
            global_action(ctrl('c'), KeyOutcome::Consumed),
            GlobalAction::Quit
        );
        assert_eq!(
            global_action(ctrl('q'), KeyOutcome::Consumed),
            GlobalAction::Quit
        );
        assert_eq!(
            global_action(ctrl('q'), KeyOutcome::NotHandled),
            GlobalAction::Quit
        );
    }

    #[test]
    fn tab_cycles_only_when_unhandled() {
        assert_eq!(
            global_action(tab(), KeyOutcome::Consumed),
            GlobalAction::None
        );
        assert_eq!(
            global_action(tab(), KeyOutcome::NotHandled),
            GlobalAction::CycleScreens
        );
    }

    #[test]
    fn other_keys_are_inert() {
        assert_eq!(
            global_action(press('x'), KeyOutcome::NotHandled),
            GlobalAction::None
        );
        assert_eq!(
            global_action(press('q'), KeyOutcome::Navigate(Screen::Send)),
            GlobalAction::None
        );
    }
}
