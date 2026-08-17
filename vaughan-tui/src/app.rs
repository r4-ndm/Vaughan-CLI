//! Application state and the main event loop.

use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::{DefaultTerminal, Frame};
use tokio::runtime::Handle;
use vaughan_core::core::{StateManager, WalletState};
use vaughan_core::error::WalletError;

use crate::views::{
    DashboardView, OnboardingView, ReceiveView, SendView, SettingsView, UnlockView,
};

/// The active screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Onboarding,
    Unlock,
    Dashboard,
    Send,
    Receive,
    Settings,
}

impl Screen {
    pub fn title(self) -> &'static str {
        match self {
            Self::Onboarding => "Onboarding",
            Self::Unlock => "Unlock",
            Self::Dashboard => "Dashboard",
            Self::Send => "Send",
            Self::Receive => "Receive",
            Self::Settings => "Settings",
        }
    }
}

/// The active view (screen + its state).
pub enum View {
    Onboarding(OnboardingView),
    Unlock(UnlockView),
    Dashboard(DashboardView),
    Send(SendView),
    Receive(ReceiveView),
    Settings(SettingsView),
}

impl View {
    pub fn screen(&self) -> Screen {
        match self {
            Self::Onboarding(_) => Screen::Onboarding,
            Self::Unlock(_) => Screen::Unlock,
            Self::Dashboard(_) => Screen::Dashboard,
            Self::Send(_) => Screen::Send,
            Self::Receive(_) => Screen::Receive,
            Self::Settings(_) => Screen::Settings,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        match self {
            Self::Onboarding(v) => v.render(frame, area, wallet),
            Self::Unlock(v) => v.render(frame, area, wallet),
            Self::Dashboard(v) => v.render(frame, area, wallet),
            Self::Send(v) => v.render(frame, area, wallet),
            Self::Receive(v) => v.render(frame, area, wallet),
            Self::Settings(v) => v.render(frame, area, wallet),
        }
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        handle: &Handle,
    ) -> Option<Screen> {
        match self {
            Self::Onboarding(v) => v.handle_key(key, wallet, handle),
            Self::Unlock(v) => v.handle_key(key, wallet, handle),
            Self::Dashboard(v) => v.handle_key(key, wallet, handle),
            Self::Send(v) => v.handle_key(key, wallet, handle),
            Self::Receive(v) => v.handle_key(key, wallet, handle),
            Self::Settings(v) => v.handle_key(key, wallet, handle),
        }
    }
}

/// Root application state.
pub struct App {
    wallet: WalletState,
    handle: Handle,
    view: View,
    quitting: bool,
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
        let mut app = Self {
            wallet,
            handle,
            view: View::Onboarding(OnboardingView::default()),
            quitting: false,
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
        // Global quit keys.
        let quit = match key.code {
            KeyCode::Char('q') => true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => true,
            _ => false,
        };
        if quit {
            self.quitting = true;
            return;
        }

        // Global Tab navigation (cycles the unlocked screens).
        if key.code == KeyCode::Tab {
            let next = match self.screen() {
                Screen::Dashboard => Screen::Send,
                Screen::Send => Screen::Receive,
                Screen::Receive => Screen::Settings,
                Screen::Settings => Screen::Dashboard,
                other => other,
            };
            if next != self.screen() {
                self.navigate(next);
            }
            return;
        }

        let handle = self.handle.clone();
        if let Some(screen) = self.view.handle_key(key, &mut self.wallet, &handle) {
            self.navigate(screen);
        }
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
        };
        self.view = view;
    }
}
