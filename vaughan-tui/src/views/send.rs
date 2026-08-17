//! Send: recipient + amount -> fee estimate -> confirm -> broadcast -> tx hash.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use tokio::runtime::Handle;
use vaughan_core::chains::Fee;
use vaughan_core::core::{parse_native_amount, WalletState};

use crate::app::Screen;
use crate::input::Input;
use crate::views::{body_areas, labeled_input, status_paragraph};

enum Stage {
    Input,
    Confirm,
    Done,
}

#[derive(PartialEq, Eq)]
enum Focus {
    Recipient,
    Amount,
}

pub struct SendView {
    stage: Stage,
    focus: Focus,
    recipient: Input,
    amount: Input,
    fee: Option<Fee>,
    tx_hash: Option<String>,
    status: String,
}

impl Default for SendView {
    fn default() -> Self {
        Self {
            stage: Stage::Input,
            focus: Focus::Recipient,
            recipient: Input::new(false, "0x..."),
            amount: Input::new(false, "0.0"),
            fee: None,
            tx_hash: None,
            status: String::new(),
        }
    }
}

impl SendView {
    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        let [content, status_area] = body_areas(area);
        let net = wallet.networks().active();
        let testnet = if net.is_testnet { " (testnet)" } else { "" };

        match self.stage {
            Stage::Input => {
                let [recipient_area, amount_area] =
                    Layout::vertical([Constraint::Length(3), Constraint::Length(3)]).areas(content);
                frame.render_widget(
                    labeled_input("Recipient", &self.recipient, self.focus == Focus::Recipient),
                    recipient_area,
                );
                frame.render_widget(
                    labeled_input(
                        &format!("Amount ({})", net.native_symbol),
                        &self.amount,
                        self.focus == Focus::Amount,
                    ),
                    amount_area,
                );
            }
            Stage::Confirm => {
                let fee_total = self
                    .fee
                    .as_ref()
                    .map(|f| f.total.clone())
                    .unwrap_or_default();
                let text = vec![
                    Line::from(format!(
                        "Send {} {} to:",
                        self.amount.value(),
                        net.native_symbol
                    )),
                    Line::from(Span::styled(
                        self.recipient.value(),
                        Style::default().fg(Color::Yellow),
                    )),
                    Line::from(""),
                    Line::from(format!("Network: {}{testnet}", net.name)),
                    Line::from(format!("Fee:      {fee_total}")),
                    Line::from(""),
                    Line::from("Enter — broadcast   Esc — cancel"),
                ];
                frame.render_widget(
                    Paragraph::new(text)
                        .block(Block::default().borders(Borders::ALL))
                        .wrap(Wrap { trim: false }),
                    content,
                );
            }
            Stage::Done => {
                let hash = self.tx_hash.as_deref().unwrap_or("");
                let text = vec![
                    Line::from("Transaction broadcast"),
                    Line::from(""),
                    Line::from(Span::styled(hash, Style::default().fg(Color::Green))),
                    Line::from(""),
                    Line::from("Enter — back to dashboard"),
                ];
                frame.render_widget(
                    Paragraph::new(text)
                        .block(Block::default().borders(Borders::ALL))
                        .wrap(Wrap { trim: false }),
                    content,
                );
            }
        }

        frame.render_widget(status_paragraph(&self.status), status_area);
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &WalletState,
        handle: &Handle,
    ) -> Option<Screen> {
        match self.stage {
            Stage::Input => match self.focus {
                Focus::Recipient => {
                    if key.code == KeyCode::Esc {
                        return Some(Screen::Dashboard);
                    }
                    if self.recipient.handle_key(key) {
                        self.focus = Focus::Amount;
                    }
                    None
                }
                Focus::Amount => {
                    if key.code == KeyCode::Esc {
                        self.focus = Focus::Recipient;
                        return None;
                    }
                    if self.amount.handle_key(key) {
                        self.estimate(wallet, handle);
                    }
                    None
                }
            },
            Stage::Confirm => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::Input;
                    None
                }
                KeyCode::Enter => {
                    self.send(wallet, handle);
                    None
                }
                _ => None,
            },
            Stage::Done => match key.code {
                KeyCode::Enter | KeyCode::Esc => Some(Screen::Dashboard),
                _ => None,
            },
        }
    }

    fn estimate(&mut self, wallet: &WalletState, handle: &Handle) {
        let net = wallet.networks().active();
        match parse_native_amount(self.amount.value(), net.decimals) {
            Ok(wei) => match handle.block_on(wallet.estimate_fee(self.recipient.value(), &wei)) {
                Ok(fee) => {
                    self.fee = Some(fee);
                    self.status.clear();
                    self.stage = Stage::Confirm;
                }
                Err(e) => self.status = e.user_message(),
            },
            Err(e) => self.status = e.user_message(),
        }
    }

    fn send(&mut self, wallet: &WalletState, handle: &Handle) {
        let net = wallet.networks().active();
        match parse_native_amount(self.amount.value(), net.decimals) {
            Ok(wei) => match handle.block_on(wallet.send(self.recipient.value(), &wei)) {
                Ok(hash) => {
                    self.tx_hash = Some(hash.to_string());
                    self.status.clear();
                    self.stage = Stage::Done;
                }
                Err(e) => {
                    self.status = e.user_message();
                    self.stage = Stage::Input;
                }
            },
            Err(e) => self.status = e.user_message(),
        }
    }
}
