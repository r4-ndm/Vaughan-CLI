//! Send: recipient + amount -> fee estimate -> confirm -> broadcast -> tx hash.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use tokio::runtime::Handle;
use vaughan_core::chains::{Fee, FeeSpeed};
use vaughan_core::core::{parse_native_amount, WalletState};
use vaughan_core::security::stealth::{StealthAnnouncement, StealthMetaAddress};
use vaughan_provider::EventBus;

use crate::app::{KeyOutcome, Screen};
use crate::input::{Input, InputAction};
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
    /// Unscaled Alloy/network fee estimate.
    base_fee: Option<Fee>,
    speed: FeeSpeed,
    tx_hash: Option<String>,
    stealth: Option<StealthAnnouncement>,
    status: String,
}

impl Default for SendView {
    fn default() -> Self {
        Self {
            stage: Stage::Input,
            focus: Focus::Recipient,
            recipient: Input::new(false, "0x..."),
            amount: Input::new(false, "0.0"),
            base_fee: None,
            speed: FeeSpeed::Normal,
            tx_hash: None,
            stealth: None,
            status: String::new(),
        }
    }
}

impl SendView {
    fn selected_fee(&self) -> Option<Fee> {
        self.base_fee.as_ref().map(|fee| fee.with_speed(self.speed))
    }

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
                let fee = self.selected_fee();
                let fee_ref = fee.as_ref();
                let fee_total = fee_ref.map(|f| f.total.clone()).unwrap_or_default();
                let fee_detail = fee_ref
                    .and_then(|f| match &f.details {
                        vaughan_core::chains::FeeDetails::Evm {
                            gas_limit,
                            max_fee_per_gas,
                            ..
                        } => Some((*gas_limit, max_fee_per_gas.as_deref())),
                        _ => None,
                    })
                    .map(|(gas_limit, max_fee)| {
                        let gwei = max_fee
                            .and_then(|mf| mf.parse::<u128>().ok())
                            .map(|wei| wei as f64 / 1e9)
                            .map(|g| format!("{g:.2} gwei"))
                            .unwrap_or_else(|| "—".to_string());
                        format!("max {gwei}/gas · limit {gas_limit}")
                    });
                let stealth_hint = self.stealth.as_ref().map(|s| {
                    format!(
                        "one-time stealth {} (sender/amount stay public)",
                        s.stealth_address
                    )
                });

                let speed_line = |digit: char, speed: FeeSpeed| {
                    let selected = self.speed == speed;
                    let marker = if selected { ">" } else { " " };
                    let style = if selected {
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    Line::from(Span::styled(
                        format!("{marker} {digit} {label}", label = speed.label()),
                        style,
                    ))
                };

                let text = vec![
                    Line::from(format!(
                        "Send {} {} to:",
                        self.amount.value(),
                        net.native_symbol
                    )),
                    Line::from(Span::styled(
                        stealth_hint.as_deref().unwrap_or(self.recipient.value()),
                        Style::default().fg(Color::Yellow),
                    )),
                    Line::from(""),
                    Line::from(format!("Network: {}{testnet}", net.name)),
                    Line::from(format!("Fee:      {fee_total}  [{}]", self.speed.label())),
                    Line::from(format!(
                        "          {}",
                        fee_detail.as_deref().unwrap_or("—")
                    )),
                    Line::from(""),
                    Line::from("Gas speed (Alloy estimate × preset):"),
                    speed_line('1', FeeSpeed::Slow),
                    speed_line('2', FeeSpeed::Normal),
                    speed_line('3', FeeSpeed::Fast),
                    speed_line('4', FeeSpeed::Ape),
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
                let label = if self.stealth.is_some() {
                    "Stealth payment broadcast (pay + announce)"
                } else {
                    "Transaction broadcast"
                };
                let text = vec![
                    Line::from(label),
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
        _events: &EventBus,
    ) -> KeyOutcome {
        match self.stage {
            Stage::Input => match self.focus {
                Focus::Recipient => {
                    if key.code == KeyCode::Esc {
                        return KeyOutcome::Navigate(Screen::Dashboard);
                    }
                    // Tab moves between fields; the view consumes it so the
                    // app-level Tab screen-cycle never fires mid-form.
                    if key.code == KeyCode::Tab {
                        self.focus = Focus::Amount;
                        return KeyOutcome::Consumed;
                    }
                    match self.recipient.handle_key(key) {
                        InputAction::Ignored => KeyOutcome::NotHandled,
                        InputAction::Submitted => {
                            self.focus = Focus::Amount;
                            KeyOutcome::Consumed
                        }
                        InputAction::Consumed => KeyOutcome::Consumed,
                    }
                }
                Focus::Amount => {
                    if key.code == KeyCode::Esc {
                        self.focus = Focus::Recipient;
                        return KeyOutcome::Consumed;
                    }
                    if key.code == KeyCode::Tab {
                        self.focus = Focus::Recipient;
                        return KeyOutcome::Consumed;
                    }
                    match self.amount.handle_key(key) {
                        InputAction::Ignored => KeyOutcome::NotHandled,
                        InputAction::Submitted => {
                            self.estimate(wallet, handle);
                            KeyOutcome::Consumed
                        }
                        InputAction::Consumed => KeyOutcome::Consumed,
                    }
                }
            },
            Stage::Confirm => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::Input;
                    KeyOutcome::Consumed
                }
                KeyCode::Char(c) if FeeSpeed::from_digit(c).is_some() => {
                    self.speed = FeeSpeed::from_digit(c).unwrap();
                    KeyOutcome::Consumed
                }
                KeyCode::Enter => {
                    self.send(wallet, handle);
                    KeyOutcome::Consumed
                }
                _ => KeyOutcome::NotHandled,
            },
            Stage::Done => match key.code {
                KeyCode::Enter | KeyCode::Esc => KeyOutcome::Navigate(Screen::Dashboard),
                _ => KeyOutcome::NotHandled,
            },
        }
    }

    fn estimate(&mut self, wallet: &WalletState, handle: &Handle) {
        let net = wallet.networks().active();
        match parse_native_amount(self.amount.value(), net.decimals) {
            Ok(wei) => {
                let to = match self.resolve_recipient(wallet) {
                    Ok(to) => to,
                    Err(e) => {
                        self.status = e;
                        return;
                    }
                };
                match handle.block_on(wallet.estimate_fee(&to, &wei)) {
                    Ok(fee) => {
                        self.base_fee = Some(fee);
                        self.speed = FeeSpeed::Normal;
                        self.status.clear();
                        self.stage = Stage::Confirm;
                    }
                    Err(e) => self.status = e.user_message(),
                }
            }
            Err(e) => self.status = e.user_message(),
        }
    }

    fn send(&mut self, wallet: &WalletState, handle: &Handle) {
        let net = wallet.networks().active();
        match parse_native_amount(self.amount.value(), net.decimals) {
            Ok(wei) => {
                let result = if let Some(announcement) = self.stealth.as_ref() {
                    handle
                        .block_on(wallet.send_stealth(announcement, &wei))
                        .map(|r| format!("{}/{}", r.pay_tx, r.announce_tx))
                } else if let Some(fee) = self.selected_fee() {
                    let to = self.recipient.value();
                    handle
                        .block_on(wallet.send_with_fee(to, &wei, &fee))
                        .map(|h| h.to_string())
                } else {
                    handle
                        .block_on(wallet.send(self.recipient.value(), &wei))
                        .map(|h| h.to_string())
                };
                match result {
                    Ok(hash) => {
                        self.tx_hash = Some(hash);
                        self.status.clear();
                        self.stage = Stage::Done;
                    }
                    Err(e) => {
                        self.status = e.user_message();
                        self.stage = Stage::Input;
                    }
                }
            }
            Err(e) => self.status = e.user_message(),
        }
    }

    fn resolve_recipient(&mut self, wallet: &WalletState) -> Result<String, String> {
        let raw = self.recipient.value().trim();
        if StealthMetaAddress::looks_like_uri(raw) {
            match wallet.prepare_stealth_payment(raw) {
                Ok(announcement) => {
                    let to = format!("{:#x}", announcement.stealth_address);
                    self.stealth = Some(announcement);
                    Ok(to)
                }
                Err(e) => {
                    self.stealth = None;
                    Err(e.user_message())
                }
            }
        } else {
            self.stealth = None;
            Ok(raw.to_string())
        }
    }
}
