//! Dashboard: active address + native balance.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use tokio::runtime::Handle;
use vaughan_core::chains::Balance;
use vaughan_core::core::WalletState;
use vaughan_core::error::WalletError;
use vaughan_provider::{EventBus, ProviderEvent};

use crate::app::{KeyOutcome, Screen};
use crate::views::{body_areas, status_paragraph};

#[derive(Default)]
pub struct DashboardView {
    balance: Option<Balance>,
    status: String,
}

impl DashboardView {
    pub fn with_balance(result: Result<Balance, WalletError>) -> Self {
        match result {
            Ok(balance) => Self {
                balance: Some(balance),
                status: String::new(),
            },
            Err(e) => Self {
                balance: None,
                status: e.user_message(),
            },
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        let [content, status_area] = body_areas(area);
        let net = wallet.networks().active();

        let address = wallet.active_address().unwrap_or("(locked)");
        let balance_line = match &self.balance {
            Some(balance) => format!("{} {}", balance.formatted, balance.token.symbol),
            None => "—".to_string(),
        };
        let mode_color = match wallet.operating_mode() {
            vaughan_core::core::OperatingMode::HumanOnly => Color::Cyan,
            vaughan_core::core::OperatingMode::AiAssisted => Color::Green,
            vaughan_core::core::OperatingMode::DegenTrader => Color::Magenta,
        };
        let mode_badge = wallet.operating_mode().badge();
        let testnet = if net.is_testnet { " (testnet)" } else { "" };
        let profile_info = if wallet.profile_name() != "default" {
            format!("  (profile: {})", wallet.profile_name())
        } else {
            String::new()
        };

        let shortcut_line = if wallet.operating_mode().is_ai_enabled() {
            "s send   b batch   v receive   n networks   a assets   c browse   g agent   r refresh   l lock"
        } else {
            "s send   b batch   v receive   n networks   a assets   c browse   r refresh   l lock"
        };

        let text = vec![
            Line::from(vec![
                Span::raw("Address:  "),
                Span::styled(address, Style::default().fg(Color::Yellow)),
                Span::raw("  ["),
                Span::styled(mode_badge, Style::default().fg(mode_color)),
                Span::raw(format!("]{profile_info}")),
            ]),
            Line::from(format!("Network:  {}{testnet}", net.name)),
            Line::from(format!("Balance:  {balance_line}")),
            Line::from(""),
            Line::from(shortcut_line),
        ];
        frame.render_widget(
            Paragraph::new(text)
                .block(Block::default().borders(Borders::ALL))
                .wrap(Wrap { trim: false }),
            content,
        );
        frame.render_widget(status_paragraph(&self.status), status_area);
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        handle: &Handle,
        events: &EventBus,
    ) -> KeyOutcome {
        match key.code {
            KeyCode::Char('r') => {
                self.refresh(wallet, handle);
                KeyOutcome::Consumed
            }
            KeyCode::Char('l') => {
                wallet.lock();
                // Connected dApps must learn the account list went empty.
                events.publish(ProviderEvent::AccountsChanged(vec![]));
                KeyOutcome::Navigate(Screen::Unlock)
            }
            KeyCode::Char('s') => KeyOutcome::Navigate(Screen::Send),
            KeyCode::Char('b') => KeyOutcome::Navigate(Screen::AaSend),
            KeyCode::Char('v') => KeyOutcome::Navigate(Screen::Receive),
            KeyCode::Char('n') => KeyOutcome::Navigate(Screen::Settings),
            KeyCode::Char('a') => KeyOutcome::Navigate(Screen::Assets),
            KeyCode::Char('c') => KeyOutcome::Navigate(Screen::Browser),
            _ => KeyOutcome::NotHandled,
        }
    }

    fn refresh(&mut self, wallet: &WalletState, handle: &Handle) {
        match handle.block_on(wallet.balance()) {
            Ok(balance) => {
                self.balance = Some(balance);
                self.status.clear();
            }
            Err(e) => self.status = e.user_message(),
        }
    }
}
