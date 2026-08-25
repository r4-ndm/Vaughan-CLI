//! Dashboard home: default **send** screen.
//!
//! Body: Send to + amount. Chrome F1 / F2 / F3 pick network, coin, and from-account.

use crossterm::event::KeyEvent;
use ratatui::{layout::Rect, Frame};
use tokio::runtime::Handle;
use vaughan_core::chains::Balance;
use vaughan_core::core::WalletState;
use vaughan_core::error::WalletError;
use vaughan_provider::EventBus;

use crate::app::KeyOutcome;
use crate::jobs::{ChromeSnapshot, UiJob, UiJobResult};
use crate::views::send::SendView;

pub struct DashboardView {
    send: SendView,
}

impl Default for DashboardView {
    fn default() -> Self {
        Self::loading()
    }
}

impl DashboardView {
    pub fn loading() -> Self {
        Self {
            send: SendView::home(),
        }
    }

    pub fn with_balance(result: Result<Balance, WalletError>) -> Self {
        let mut v = Self::loading();
        if let Err(e) = result {
            v.send.status = e.user_message();
        }
        v
    }

    pub fn for_asset(balance: Balance) -> Self {
        Self {
            send: SendView::for_asset(balance),
        }
    }

    pub fn set_tick(&mut self, tick: u64) {
        self.send.set_tick(tick);
    }

    pub fn apply_balance(&mut self, result: Result<Balance, WalletError>) {
        match result {
            Ok(_) => {}
            Err(e) => self.send.status = e.user_message(),
        }
    }

    pub fn apply_job_result(&mut self, result: UiJobResult) {
        self.send.apply_job_result(result);
    }

    /// Auto-poll receipt after a successful broadcast.
    pub fn followup_job(&mut self) -> Option<UiJob> {
        self.send.followup_poll_status()
    }

    /// Keep send coin in sync with F2 chrome selection.
    pub fn sync_from_chrome(&mut self, chrome: &ChromeSnapshot) {
        self.send.sync_from_chrome(chrome);
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        self.send.render(frame, area, wallet);
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        handle: &Handle,
        events: &EventBus,
    ) -> KeyOutcome {
        self.send.handle_key(key, wallet, handle, events)
    }
}
