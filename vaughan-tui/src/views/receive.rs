//! Receive: display the active address.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use tokio::runtime::Handle;
use vaughan_core::core::WalletState;
use vaughan_provider::EventBus;

use crate::app::{KeyOutcome, Screen};

pub struct ReceiveView;

impl ReceiveView {
    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        let net = wallet.networks().active();
        let address = wallet.active_address().unwrap_or("(locked)");
        let text = vec![
            Line::from(format!("Network: {}", net.name)),
            Line::from(""),
            Line::from("Your address:"),
            Line::from(Span::styled(address, Style::default().fg(Color::Yellow))),
            Line::from(""),
            Line::from("Share this address to receive funds."),
        ];
        frame.render_widget(
            Paragraph::new(text)
                .block(Block::default().borders(Borders::ALL))
                .wrap(Wrap { trim: false }),
            area,
        );
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        _wallet: &mut WalletState,
        _handle: &Handle,
        _events: &EventBus,
    ) -> KeyOutcome {
        match key.code {
            KeyCode::Esc => KeyOutcome::Navigate(Screen::Dashboard),
            _ => KeyOutcome::NotHandled,
        }
    }
}
