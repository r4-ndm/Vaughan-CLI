//! Send: recipient + amount -> fee estimate -> confirm -> broadcast -> tx hash.
//!
//! Powers **Home** (`h`) via [`SendView::home`] inside the dashboard view.
//! Integration tests exercise this type directly (non-home `Default`); the live
//! app never mounts a separate Send screen.
//!
//! Network / coin / from-account come from the F1 / F2 / F3 chrome boxes.

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
use vaughan_core::core::{parse_native_amount, WalletState};
use vaughan_core::security::stealth::{StealthAnnouncement, StealthMetaAddress};
use vaughan_provider::EventBus;

use crate::app::{KeyOutcome, Screen};
use crate::brand;
use crate::input::{Input, InputAction};
use crate::jobs::{spinner_frame, ChromeSnapshot, UiJob, UiJobResult};
use crate::views::{body_areas, render_labeled_input, status_paragraph};

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Busy {
    Idle,
    Estimating,
    Sending,
}

pub struct SendView {
    stage: Stage,
    focus: Focus,
    recipient: Input,
    amount: Input,
    /// When set, send ERC-20 `transfer` instead of native.
    token: Option<TokenCtx>,
    /// Unscaled Alloy/network fee estimate.
    base_fee: Option<Fee>,
    speed: FeeSpeed,
    tx_hash: Option<String>,
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
            recipient: Input::new(false, "0x… or st:…"),
            amount: Input::new(false, "0.0"),
            token: None,
            base_fee: None,
            speed: FeeSpeed::Normal,
            tx_hash: None,
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
        self.base_fee.as_ref().map(|fee| fee.with_speed(self.speed))
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

    pub fn apply_job_result(&mut self, result: UiJobResult) {
        match result {
            UiJobResult::Fee(Ok(fee)) => {
                self.base_fee = Some(fee);
                self.speed = FeeSpeed::Normal;
                self.status.clear();
                self.busy = Busy::Idle;
                self.stage = Stage::Confirm;
            }
            UiJobResult::Fee(Err(e)) => {
                self.busy = Busy::Idle;
                self.status = e.user_message();
            }
            UiJobResult::Send(Ok(hash)) => {
                self.tx_hash = Some(hash);
                self.status.clear();
                self.busy = Busy::Idle;
                self.stage = Stage::Done;
            }
            UiJobResult::SendStealth(Ok(r)) => {
                self.tx_hash = Some(format!("{}/{}", r.pay_tx, r.announce_tx));
                self.status.clear();
                self.busy = Busy::Idle;
                self.stage = Stage::Done;
            }
            UiJobResult::Send(Err(e)) | UiJobResult::SendStealth(Err(e)) => {
                self.busy = Busy::Idle;
                self.status = e.user_message();
                self.stage = Stage::Input;
            }
            _ => {}
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        let [content, status_area] = body_areas(area);
        let net = wallet.networks().active();
        let status = if self.busy != Busy::Idle {
            let label = match self.busy {
                Busy::Estimating => "estimating fee",
                Busy::Sending => "broadcasting",
                Busy::Idle => "",
            };
            format!("{} {label}…", spinner_frame(self.tick))
        } else {
            self.status.clone()
        };

        match self.stage {
            Stage::Input => {
                let [to_area, amount_area] =
                    Layout::vertical([Constraint::Length(3), Constraint::Length(3)]).areas(content);

                render_labeled_input(
                    frame,
                    to_area,
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
                render_labeled_input(
                    frame,
                    amount_area,
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
                let back = if self.home_mode {
                    "Enter — new send"
                } else {
                    "Enter — back to home"
                };
                let text = vec![
                    Line::from(label),
                    Line::from(""),
                    Line::from(Span::styled(hash, Style::default().fg(Color::Green))),
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
            Stage::Input => match self.focus {
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
            },
            Stage::Confirm => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::Input;
                    if self.home_mode {
                        self.focus = Focus::Idle;
                    }
                    KeyOutcome::Consumed
                }
                KeyCode::Char(c) if FeeSpeed::from_digit(c).is_some() => {
                    self.speed = FeeSpeed::from_digit(c).unwrap();
                    KeyOutcome::Consumed
                }
                KeyCode::Enter => self.begin_send(wallet),
                _ => KeyOutcome::NotHandled,
            },
            Stage::Done => match key.code {
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
