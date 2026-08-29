//! Send: recipient + amount -> fee estimate -> confirm -> broadcast -> tx hash.
//!
//! Powers **Home** (`h`) via [`SendView::home`] inside the dashboard view.
//! Integration tests exercise this type directly (non-home `Default`); the live
//! app never mounts a separate Send screen.
//!
//! Network / coin / from-account come from the F1 / F2 / F3 chrome boxes.
//! F4 focuses recipient ("Send to"); F5 focuses amount.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};
use tokio::runtime::Handle;
use vaughan_core::chains::{Balance, Fee, FeeSpeed};
use vaughan_core::core::{format_base_units, parse_native_amount, WalletState};
use vaughan_core::security::stealth::{StealthAnnouncement, StealthMetaAddress};
use vaughan_provider::EventBus;

use crate::app::{KeyOutcome, Screen};
use crate::brand;
use crate::input::{Input, InputAction};
use crate::jobs::{spinner_frame, ChromeSnapshot, UiJob, UiJobResult};
use crate::views::{body_areas, render_fkey_labeled_input, status_paragraph};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Input,
    Confirm,
    Done,
}

#[derive(PartialEq, Eq)]
enum Focus {
    /// Home only: form idle so footer shortcuts (h/d/…) still work.
    Idle,
    Recipient,
    Amount,
}

/// Confirm-stage focus: speed list vs custom gwei field.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ConfirmFocus {
    Speed,
    CustomGas,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Busy {
    Idle,
    Estimating,
    Sending,
    PollingStatus,
}

pub struct SendView {
    stage: Stage,
    focus: Focus,
    confirm_focus: ConfirmFocus,
    recipient: Input,
    amount: Input,
    /// Custom max fee in gwei (only when [`FeeSpeed::Custom`] is selected).
    custom_gas: Input,
    /// When set, send ERC-20 `transfer` instead of native.
    token: Option<TokenCtx>,
    /// Unscaled Alloy/network fee estimate.
    base_fee: Option<Fee>,
    speed: FeeSpeed,
    tx_hash: Option<String>,
    /// Inclusion status after broadcast (polled via RPC).
    receipt_status: Option<vaughan_core::chains::TxStatus>,
    stealth: Option<StealthAnnouncement>,
    busy: Busy,
    /// Animation tick mirrored from the app loop while busy.
    tick: u64,
    pub(crate) status: String,
    /// Home (`h`) mode: "Send to" label; coin follows F2 chrome.
    home_mode: bool,
}

struct TokenCtx {
    address: String,
    symbol: String,
    decimals: u8,
}

impl Default for SendView {
    fn default() -> Self {
        Self {
            stage: Stage::Input,
            focus: Focus::Recipient,
            confirm_focus: ConfirmFocus::Speed,
            recipient: Input::new(false, "0x… or st:…"),
            amount: Input::new(false, "0.0"),
            custom_gas: Input::new(false, "gwei"),
            token: None,
            base_fee: None,
            speed: FeeSpeed::Normal,
            tx_hash: None,
            receipt_status: None,
            stealth: None,
            busy: Busy::Idle,
            tick: 0,
            status: String::new(),
            home_mode: false,
        }
    }
}

impl SendView {
    /// Home screen send form (F1 net · F2 coin · F3 from).
    pub fn home() -> Self {
        Self {
            home_mode: true,
            focus: Focus::Idle,
            recipient: Input::new(false, ""),
            amount: Input::new(false, ""),
            ..Self::default()
        }
    }

    /// Prefill a send for a selected Assets row (native or ERC-20).
    pub fn for_asset(balance: Balance) -> Self {
        let mut view = Self::home();
        view.apply_balance_coin(&balance);
        view
    }

    /// Sync the send coin from F2 chrome (or native when empty).
    pub fn sync_from_chrome(&mut self, chrome: &ChromeSnapshot) {
        if !self.home_mode || !matches!(self.stage, Stage::Input) {
            return;
        }
        if let Some(b) = chrome.assets.get(chrome.asset_idx) {
            self.apply_balance_coin(b);
        } else {
            self.token = None;
        }
    }

    fn apply_balance_coin(&mut self, balance: &Balance) {
        if let Some(addr) = balance.token.contract_address.clone() {
            self.token = Some(TokenCtx {
                address: addr,
                symbol: balance.token.symbol.clone(),
                decimals: balance.token.decimals,
            });
        } else {
            self.token = None;
        }
    }

    fn amount_decimals(&self, wallet: &WalletState) -> u8 {
        self.token
            .as_ref()
            .map(|t| t.decimals)
            .unwrap_or_else(|| wallet.networks().active().decimals)
    }

    fn selected_fee(&self) -> Option<Fee> {
        let base = self.base_fee.as_ref()?;
        match self.speed {
            FeeSpeed::Custom => base.with_custom_max_fee_gwei(self.custom_gas.value()).ok(),
            speed => Some(base.with_speed(speed)),
        }
    }

    /// Prefill custom gwei from the base estimate when entering Custom.
    fn select_speed(&mut self, speed: FeeSpeed) {
        self.speed = speed;
        if speed == FeeSpeed::Custom {
            if self.custom_gas.value().is_empty() {
                if let Some(gwei) = self.base_fee.as_ref().and_then(max_fee_gwei_display) {
                    self.custom_gas.set_value(gwei);
                }
            }
            self.confirm_focus = ConfirmFocus::CustomGas;
        } else {
            self.confirm_focus = ConfirmFocus::Speed;
        }
    }

    fn recipient_label(&self) -> &'static str {
        if self.home_mode {
            "Send to"
        } else {
            "Recipient"
        }
    }

    pub fn set_tick(&mut self, tick: u64) {
        self.tick = tick;
    }

    pub fn allows_footer_shortcuts(&self) -> bool {
        if self.busy != Busy::Idle {
            return true;
        }
        match self.stage {
            Stage::Input => self.focus == Focus::Idle,
            Stage::Confirm => self.confirm_focus != ConfirmFocus::CustomGas,
            Stage::Done => true,
        }
    }

    pub fn apply_job_result(&mut self, result: UiJobResult) {
        match result {
            UiJobResult::Fee(Ok(fee)) => {
                self.base_fee = Some(fee);
                self.speed = FeeSpeed::Normal;
                self.confirm_focus = ConfirmFocus::Speed;
                self.custom_gas.set_value("");
                self.status.clear();
                self.busy = Busy::Idle;
                self.stage = Stage::Confirm;
            }
            UiJobResult::Fee(Err(e)) => {
                self.busy = Busy::Idle;
                self.status = e.user_message();
            }
            UiJobResult::Send(Ok(receipt)) => {
                self.tx_hash = Some(receipt.hash);
                self.receipt_status = None;
                self.status.clear();
                self.busy = Busy::Idle;
                self.stage = Stage::Done;
            }
            UiJobResult::SendStealth(Ok(r)) => {
                self.tx_hash = Some(format!("{}/{}", r.pay_tx, r.announce_tx));
                self.receipt_status = None;
                self.status.clear();
                self.busy = Busy::Idle;
                self.stage = Stage::Done;
            }
            UiJobResult::Send(Err(e)) | UiJobResult::SendStealth(Err(e)) => {
                self.busy = Busy::Idle;
                self.status = e.user_message();
                self.stage = Stage::Input;
            }
            UiJobResult::TxStatus(Ok(status)) => {
                self.receipt_status = Some(status);
                self.busy = Busy::Idle;
                self.status = match status {
                    vaughan_core::chains::TxStatus::Pending => {
                        "Still pending on RPC · r to re-check".into()
                    }
                    vaughan_core::chains::TxStatus::Confirmed => "Confirmed on-chain".into(),
                    vaughan_core::chains::TxStatus::Failed => {
                        "Failed on-chain (receipt status 0)".into()
                    }
                };
            }
            UiJobResult::TxStatus(Err(e)) => {
                self.busy = Busy::Idle;
                self.status = e.user_message();
            }
            _ => {}
        }
    }

    /// After a successful broadcast, return a job to poll inclusion (first hash
    /// only for stealth pay+announce pairs). Marks the view busy while polling.
    pub fn followup_poll_status(&mut self) -> Option<UiJob> {
        if self.stage != Stage::Done || self.busy != Busy::Idle {
            return None;
        }
        if self.receipt_status.is_some() {
            return None;
        }
        let hash = self.tx_hash.as_ref()?;
        let first = hash.split('/').next()?.trim();
        if first.is_empty() {
            return None;
        }
        self.busy = Busy::PollingStatus;
        Some(UiJob::PollTxStatus {
            tx_hash: first.to_string(),
        })
    }

    fn begin_poll_status(&mut self) -> KeyOutcome {
        let Some(hash) = self.tx_hash.as_ref() else {
            return KeyOutcome::Consumed;
        };
        let first = hash.split('/').next().unwrap_or(hash).trim().to_string();
        if first.is_empty() {
            return KeyOutcome::Consumed;
        }
        self.busy = Busy::PollingStatus;
        self.status.clear();
        KeyOutcome::StartJob(UiJob::PollTxStatus { tx_hash: first })
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        let [content, status_area] = body_areas(area);
        let net = wallet.networks().active();
        let status = if self.busy != Busy::Idle {
            let label = match self.busy {
                Busy::Estimating => "estimating fee",
                Busy::Sending => "broadcasting",
                Busy::PollingStatus => "checking receipt",
                Busy::Idle => "",
            };
            format!("{} {label}…", spinner_frame(self.tick))
        } else {
            self.status.clone()
        };

        match self.stage {
            Stage::Input => {
                let [to_area, _gap, amount_area] = Layout::vertical([
                    Constraint::Length(3),
                    Constraint::Length(1), // blank between F4 Send to and F5 Amount
                    Constraint::Length(3),
                ])
                .areas(content);

                render_fkey_labeled_input(
                    frame,
                    to_area,
                    "F4",
                    self.recipient_label(),
                    &self.recipient,
                    self.focus == Focus::Recipient,
                );
                let amount_label = format!(
                    "Amount ({})",
                    self.token
                        .as_ref()
                        .map(|t| t.symbol.as_str())
                        .unwrap_or(net.native_symbol.as_str())
                );
                render_fkey_labeled_input(
                    frame,
                    amount_area,
                    "F5",
                    &amount_label,
                    &self.amount,
                    self.focus == Focus::Amount,
                );
            }
            Stage::Confirm => {
                let testnet = if net.is_testnet { " (testnet)" } else { "" };
                let from_label = wallet.active_account_label().unwrap_or("—");
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

                let custom_editing =
                    self.speed == FeeSpeed::Custom && self.confirm_focus == ConfirmFocus::CustomGas;
                let custom_hint = if self.speed == FeeSpeed::Custom {
                    let mut spans = vec![Span::raw("    max fee (gwei): ")];
                    if custom_editing {
                        spans.extend(self.custom_gas.line().spans);
                    } else {
                        let shown = if self.custom_gas.value().is_empty() {
                            "—"
                        } else {
                            self.custom_gas.value()
                        };
                        spans.push(Span::styled(
                            shown.to_string(),
                            Style::default().fg(Color::DarkGray),
                        ));
                    }
                    Line::from(spans)
                } else {
                    Line::from("")
                };

                let text = vec![
                    Line::from(format!(
                        "Send {} {} to:",
                        self.amount.value(),
                        self.token
                            .as_ref()
                            .map(|t| t.symbol.as_str())
                            .unwrap_or(&net.native_symbol)
                    )),
                    if let Some(hint) = &stealth_hint {
                        Line::from(Span::styled(
                            hint.clone(),
                            Style::default().fg(Color::Yellow),
                        ))
                    } else {
                        Line::from(brand::colored_address_spans(self.recipient.value()))
                    },
                    Line::from(""),
                    Line::from(format!("From:     {from_label}")),
                    Line::from(format!("Network:  {}{testnet}", net.name)),
                    Line::from(format!(
                        "Fee:      {}  [{}]",
                        if fee_total.is_empty() {
                            "—"
                        } else {
                            fee_total.as_str()
                        },
                        self.speed.label()
                    )),
                    Line::from(format!(
                        "          {}",
                        fee_detail.as_deref().unwrap_or("—")
                    )),
                    Line::from(""),
                    Line::from("Gas speed (↑↓ or 1–5):"),
                    speed_line('1', FeeSpeed::Slow),
                    speed_line('2', FeeSpeed::Normal),
                    speed_line('3', FeeSpeed::Fast),
                    speed_line('4', FeeSpeed::Ape),
                    speed_line('5', FeeSpeed::Custom),
                    custom_hint,
                    Line::from(""),
                    Line::from("Enter — broadcast   Esc — cancel"),
                ];
                let inner = brand::render_faded_box(frame, content, None);
                frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
            }
            Stage::Done => {
                let hash = self.tx_hash.as_deref().unwrap_or("");
                let label = if self.stealth.is_some() {
                    "Stealth payment broadcast (pay + announce)"
                } else {
                    "Transaction broadcast"
                };
                let status_line = match self.receipt_status {
                    Some(vaughan_core::chains::TxStatus::Pending) => "Status:   Pending",
                    Some(vaughan_core::chains::TxStatus::Confirmed) => "Status:   Confirmed",
                    Some(vaughan_core::chains::TxStatus::Failed) => "Status:   Failed",
                    None => "Status:   checking…",
                };
                let back = if self.home_mode {
                    "Enter — new send · r — re-check receipt"
                } else {
                    "Enter — back to home · r — re-check receipt"
                };
                let text = vec![
                    Line::from(label),
                    Line::from(""),
                    Line::from(Span::styled(hash, Style::default().fg(Color::Green))),
                    Line::from(""),
                    Line::from(status_line),
                    Line::from(""),
                    Line::from(back),
                ];
                let inner = brand::render_faded_box(frame, content, None);
                frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
            }
        }

        frame.render_widget(status_paragraph(&status), status_area);
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &WalletState,
        _handle: &Handle,
        _events: &EventBus,
    ) -> KeyOutcome {
        if self.busy != Busy::Idle {
            return KeyOutcome::Consumed;
        }
        match self.stage {
            Stage::Input => {
                // F4 / F5 jump to recipient / amount from any input focus (incl. Idle).
                if let KeyCode::F(4) = key.code {
                    self.focus = Focus::Recipient;
                    return KeyOutcome::Consumed;
                }
                if let KeyCode::F(5) = key.code {
                    self.focus = Focus::Amount;
                    return KeyOutcome::Consumed;
                }
                match self.focus {
                    Focus::Idle => match key.code {
                        KeyCode::Enter | KeyCode::Char(' ') => {
                            self.focus = Focus::Recipient;
                            KeyOutcome::Consumed
                        }
                        _ => KeyOutcome::NotHandled,
                    },
                    Focus::Recipient => {
                        if key.code == KeyCode::Esc {
                            return if self.home_mode {
                                self.focus = Focus::Idle;
                                KeyOutcome::Consumed
                            } else {
                                KeyOutcome::Navigate(Screen::Dashboard)
                            };
                        }
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
                            self.focus = if self.home_mode {
                                Focus::Idle
                            } else {
                                Focus::Recipient
                            };
                            return KeyOutcome::Consumed;
                        }
                        if key.code == KeyCode::Tab {
                            self.focus = Focus::Recipient;
                            return KeyOutcome::Consumed;
                        }
                        match self.amount.handle_key(key) {
                            InputAction::Ignored => KeyOutcome::NotHandled,
                            InputAction::Submitted => self.begin_estimate(wallet),
                            InputAction::Consumed => KeyOutcome::Consumed,
                        }
                    }
                }
            }
            Stage::Confirm => {
                if let KeyCode::F(4) = key.code {
                    self.stage = Stage::Input;
                    self.focus = Focus::Recipient;
                    self.confirm_focus = ConfirmFocus::Speed;
                    return KeyOutcome::Consumed;
                }
                if let KeyCode::F(5) = key.code {
                    self.stage = Stage::Input;
                    self.focus = Focus::Amount;
                    self.confirm_focus = ConfirmFocus::Speed;
                    return KeyOutcome::Consumed;
                }
                match key.code {
                    KeyCode::Esc => {
                        if self.confirm_focus == ConfirmFocus::CustomGas {
                            self.confirm_focus = ConfirmFocus::Speed;
                            return KeyOutcome::Consumed;
                        }
                        self.stage = Stage::Input;
                        if self.home_mode {
                            self.focus = Focus::Idle;
                        }
                        KeyOutcome::Consumed
                    }
                    KeyCode::Up => {
                        self.select_speed(self.speed.prev());
                        KeyOutcome::Consumed
                    }
                    KeyCode::Down => {
                        self.select_speed(self.speed.next());
                        KeyOutcome::Consumed
                    }
                    KeyCode::Char(c)
                        if FeeSpeed::from_digit(c).is_some()
                            && self.confirm_focus != ConfirmFocus::CustomGas =>
                    {
                        self.select_speed(FeeSpeed::from_digit(c).unwrap());
                        KeyOutcome::Consumed
                    }
                    KeyCode::Tab if self.speed == FeeSpeed::Custom => {
                        self.confirm_focus = match self.confirm_focus {
                            ConfirmFocus::Speed => ConfirmFocus::CustomGas,
                            ConfirmFocus::CustomGas => ConfirmFocus::Speed,
                        };
                        KeyOutcome::Consumed
                    }
                    KeyCode::Enter => {
                        if self.speed == FeeSpeed::Custom {
                            match self
                                .base_fee
                                .as_ref()
                                .map(|f| f.with_custom_max_fee_gwei(self.custom_gas.value()))
                            {
                                Some(Ok(_)) => self.begin_send(wallet),
                                Some(Err(e)) => {
                                    self.status = e;
                                    self.confirm_focus = ConfirmFocus::CustomGas;
                                    KeyOutcome::Consumed
                                }
                                None => {
                                    self.status = "fee estimate missing".into();
                                    KeyOutcome::Consumed
                                }
                            }
                        } else {
                            self.begin_send(wallet)
                        }
                    }
                    _ if self.confirm_focus == ConfirmFocus::CustomGas => {
                        match self.custom_gas.handle_key(key) {
                            InputAction::Ignored => KeyOutcome::NotHandled,
                            InputAction::Submitted => {
                                // Enter already handled above; treat as broadcast attempt.
                                match self
                                    .base_fee
                                    .as_ref()
                                    .map(|f| f.with_custom_max_fee_gwei(self.custom_gas.value()))
                                {
                                    Some(Ok(_)) => self.begin_send(wallet),
                                    Some(Err(e)) => {
                                        self.status = e;
                                        KeyOutcome::Consumed
                                    }
                                    None => KeyOutcome::Consumed,
                                }
                            }
                            InputAction::Consumed => {
                                self.status.clear();
                                KeyOutcome::Consumed
                            }
                        }
                    }
                    _ => KeyOutcome::NotHandled,
                }
            }
            Stage::Done => match key.code {
                KeyCode::F(4) => {
                    if self.home_mode {
                        *self = Self::home();
                        self.focus = Focus::Recipient;
                    } else {
                        self.stage = Stage::Input;
                        self.focus = Focus::Recipient;
                    }
                    KeyOutcome::Consumed
                }
                KeyCode::F(5) => {
                    if self.home_mode {
                        *self = Self::home();
                        self.focus = Focus::Amount;
                    } else {
                        self.stage = Stage::Input;
                        self.focus = Focus::Amount;
                    }
                    KeyOutcome::Consumed
                }
                KeyCode::Char('r') | KeyCode::Char('R') => self.begin_poll_status(),
                KeyCode::Enter | KeyCode::Esc => {
                    if self.home_mode {
                        *self = Self::home();
                        KeyOutcome::Consumed
                    } else {
                        KeyOutcome::Navigate(Screen::Dashboard)
                    }
                }
                _ => KeyOutcome::NotHandled,
            },
        }
    }

    fn begin_estimate(&mut self, wallet: &WalletState) -> KeyOutcome {
        let decimals = self.amount_decimals(wallet);
        match parse_native_amount(self.amount.value(), decimals) {
            Ok(amount) => match self.resolve_recipient(wallet) {
                Ok(to) => {
                    self.busy = Busy::Estimating;
                    self.status.clear();
                    if let Some(token) = &self.token {
                        KeyOutcome::StartJob(UiJob::EstimateTokenFee {
                            token: token.address.clone(),
                            to,
                            amount,
                        })
                    } else {
                        KeyOutcome::StartJob(UiJob::EstimateFee {
                            to,
                            value_wei: amount,
                        })
                    }
                }
                Err(e) => {
                    self.status = e;
                    KeyOutcome::Consumed
                }
            },
            Err(e) => {
                self.status = e.user_message();
                KeyOutcome::Consumed
            }
        }
    }

    fn begin_send(&mut self, wallet: &WalletState) -> KeyOutcome {
        let decimals = self.amount_decimals(wallet);
        match parse_native_amount(self.amount.value(), decimals) {
            Ok(amount) => {
                self.busy = Busy::Sending;
                self.status.clear();
                if let Some(token) = &self.token {
                    if let Some(fee) = self.selected_fee() {
                        KeyOutcome::StartJob(UiJob::SendTokenWithFee {
                            token: token.address.clone(),
                            to: self.recipient.value().to_string(),
                            amount,
                            fee,
                        })
                    } else {
                        KeyOutcome::StartJob(UiJob::SendToken {
                            token: token.address.clone(),
                            to: self.recipient.value().to_string(),
                            amount,
                        })
                    }
                } else if let Some(announcement) = self.stealth.clone() {
                    KeyOutcome::StartJob(UiJob::SendStealth {
                        announcement,
                        value_wei: amount,
                    })
                } else if let Some(fee) = self.selected_fee() {
                    KeyOutcome::StartJob(UiJob::SendWithFee {
                        to: self.recipient.value().to_string(),
                        value_wei: amount,
                        fee,
                    })
                } else {
                    KeyOutcome::StartJob(UiJob::Send {
                        to: self.recipient.value().to_string(),
                        value_wei: amount,
                    })
                }
            }
            Err(e) => {
                self.status = e.user_message();
                KeyOutcome::Consumed
            }
        }
    }

    fn resolve_recipient(&mut self, wallet: &WalletState) -> Result<String, String> {
        let raw = self.recipient.value().trim();
        if self.token.is_some() && StealthMetaAddress::looks_like_uri(raw) {
            return Err("ERC-20 send does not support stealth URIs yet".into());
        }
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

/// Base estimate max fee formatted as gwei for the Custom field prefill.
fn max_fee_gwei_display(fee: &Fee) -> Option<String> {
    match &fee.details {
        vaughan_core::chains::FeeDetails::Evm {
            max_fee_per_gas: Some(wei),
            ..
        } => {
            let s = format_base_units(wei, 9);
            if s.is_empty() || s == "0" {
                None
            } else {
                Some(s)
            }
        }
        _ => None,
    }
}
