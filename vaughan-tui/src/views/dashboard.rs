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

use crate::app::Screen;
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
        let testnet = if net.is_testnet { " (testnet)" } else { "" };

        let text = vec![
            Line::from(vec![
                Span::raw("Address:  "),
                Span::styled(address, Style::default().fg(Color::Yellow)),
            ]),
            Line::from(format!("Network:  {}{testnet}", net.name)),
            Line::from(format!("Balance:  {balance_line}")),
            Line::from(""),
            Line::from("s send   v receive   n networks   r refresh   l lock"),
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
    ) -> Option<Screen> {
        match key.code {
            KeyCode::Char('r') => {
                self.refresh(wallet, handle);
                None
            }
            KeyCode::Char('l') => {
                wallet.lock();
                Some(Screen::Unlock)
            }
            KeyCode::Char('s') => Some(Screen::Send),
            KeyCode::Char('v') => Some(Screen::Receive),
            KeyCode::Char('n') => Some(Screen::Settings),
            _ => None,
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
