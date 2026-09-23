//! Send: recipient + coin + amount -> fee estimate -> confirm -> broadcast -> tx hash.
//!
//! Powers **Home** (`h`) via [`SendView::home`] inside the dashboard view.
//! Integration tests exercise this type directly (non-home `Default`); the live
//! app never mounts a separate Send screen.
//!
//! Network / from-account come from F1 / F3 chrome. Coin defaults from F2 but
//! can be overridden by pasting an ERC-20 contract (or ↑↓ through F2 assets).
//! F4 focuses recipient (↑↓ cycles installed Vaughan wallets); F5 coin (↑↓
//! cycles F2 assets); F6 amount. Tab / BackTab cycle the three fields with
//! reverse-video focus (F-keys still jump).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Gauge, Paragraph, Wrap},
    Frame,
};
use tokio::runtime::Handle;
use vaughan_core::chains::{Balance, Fee, FeeSpeed};
use vaughan_core::core::{format_base_units, parse_native_amount, WalletState};
use vaughan_core::security::stealth::{StealthAnnouncement, StealthMetaAddress};
use vaughan_provider::EventBus;

use crate::app::KeyOutcome;
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
    Coin,
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
    /// Label of a vault account selected via ↑↓ on F4 (shown in the box title).
    recipient_pick_label: Option<String>,
    /// Empty = native; otherwise paste `0x…` or pick via ↑↓ from F2 assets.
    coin: Input,
    amount: Input,
    /// Custom max fee in gwei (only when [`FeeSpeed::Custom`] is selected).
    custom_gas: Input,
    /// When set, send ERC-20 `transfer` instead of native.
    token: Option<TokenCtx>,
    /// User pasted/edited coin (or ↑↓ picked) — ignore F2 chrome until cleared.
    coin_override: bool,
    /// F2 chrome asset list for ↑↓ on F5 (avoids a blocking re-fetch each step).
    asset_choices: Vec<Balance>,
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
    /// Home (`h`) mode: "Send to" label; coin defaults from F2 unless overridden.
    home_mode: bool,
    /// Locked WZRD→dead burn form (Settings Unlock tools); skip F2 chrome sync.
    assist_burn: bool,
}

#[derive(Clone)]
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
            recipient_pick_label: None,
            coin: Input::new(false, "native / paste 0x…"),
            amount: Input::new(false, "0.0"),
            custom_gas: Input::new(false, "gwei"),
            token: None,
            coin_override: false,
            asset_choices: Vec::new(),
            base_fee: None,
            speed: FeeSpeed::Normal,
            tx_hash: None,
            receipt_status: None,
            stealth: None,
            busy: Busy::Idle,
            tick: 0,
            status: String::new(),
            home_mode: false,
            assist_burn: false,
        }
    }
}

impl SendView {
    /// Home screen send form (F1 net · F2 coin default · F3 from).
    pub fn home() -> Self {
        Self {
            home_mode: true,
            focus: Focus::Idle,
            recipient: Input::new(false, ""),
            coin: Input::new(false, "native / paste 0x…"),
            amount: Input::new(false, ""),
            status: "Tab fields · ↑↓ wallets F4 · ↑↓ assets F5 · F4/F5/F6".into(),
            ..Self::default()
        }
    }

    /// Prefill a send for a selected Assets row (native or ERC-20).
    pub fn for_asset(balance: Balance) -> Self {
        let mut view = Self::home();
        view.apply_balance_coin(&balance);
        view.coin_override = false;
        view
    }

    /// Prefill ERC-20 burn to the assist sink (≥13 WZRD default).
    pub fn for_assist_burn(token: &str, sink: &str, amount: &str) -> Self {
        let mut view = Self::home();
        view.assist_burn = true;
        view.coin_override = true;
        view.focus = Focus::Amount;
        view.recipient.set_value(sink);
        view.amount.set_value(amount);
        view.coin.set_value(token);
        view.token = Some(TokenCtx {
            address: token.to_string(),
            symbol: "WZRD".into(),
            decimals: 18,
        });
        view.status =
            "Unlock tools: one transfer of at least 13 WZRD to the dead address (no drip)".into();
        view
    }

    pub fn is_assist_burn(&self) -> bool {
        self.assist_burn
    }

    /// Sync the send coin from F2 chrome unless the user overrode the coin field.
    pub fn sync_from_chrome(&mut self, chrome: &ChromeSnapshot) {
        if self.assist_burn || !self.home_mode || !matches!(self.stage, Stage::Input) {
            return;
        }
        // Always refresh the F5 picker list so ↑↓ matches chrome even after override.
        self.asset_choices = chrome.assets.clone();
        if self.coin_override {
            return;
        }
        if let Some(b) = chrome.assets.get(chrome.asset_idx) {
            self.apply_balance_coin(b);
        } else {
            self.clear_to_native();
        }
    }

    fn clear_to_native(&mut self) {
        self.token = None;
        self.coin.set_value("");
    }

    fn apply_balance_coin(&mut self, balance: &Balance) {
        if let Some(addr) = balance.token.contract_address.clone() {
            self.coin.set_value(addr.clone());
            self.token = Some(TokenCtx {
                address: addr,
                symbol: balance.token.symbol.clone(),
                decimals: balance.token.decimals,
            });
        } else {
            self.token = None;
            self.coin.set_value("");
        }
    }

    fn coin_label(&self, wallet: &WalletState) -> String {
        if let Some(t) = &self.token {
            format!("Coin ({})", t.symbol)
        } else if self.coin.value().trim().is_empty() {
            format!("Coin ({})", wallet.networks().active().native_symbol)
        } else {
            "Coin".into()
        }
    }

    /// Apply pasted/edited coin text: empty → native; `0x` → resolve metadata.
    fn apply_coin_field(&mut self, wallet: &WalletState, handle: &Handle) -> Result<(), String> {
        let raw = self.coin.value().trim();
        let native = wallet.networks().active().native_symbol.clone();
        if raw.is_empty() || raw.eq_ignore_ascii_case(&native) {
            self.token = None;
            self.coin.set_value("");
            return Ok(());
        }
        let addr = crate::views::parse_token_address(raw, "Coin")?;
        let checksum = format!("{addr:#x}");
        if self
            .token
            .as_ref()
            .is_some_and(|t| t.address.eq_ignore_ascii_case(&checksum))
        {
            self.coin.set_value(checksum);
            return Ok(());
        }
        let (symbol, _name, decimals) = handle
            .block_on(wallet.resolve_erc20_metadata(&checksum))
            .map_err(|e| match e {
            vaughan_core::error::WalletError::InvalidTransaction(m) => m,
            other => other.user_message(),
        })?;
        self.coin.set_value(checksum.clone());
        self.token = Some(TokenCtx {
            address: checksum,
            symbol,
            decimals,
        });
        Ok(())
    }

    /// ↑↓ on F5: cycle installed assets (chrome/F2 list, else live wallet fetch).
    fn cycle_coin_from_wallet(
        &mut self,
        wallet: &WalletState,
        handle: &Handle,
        down: bool,
    ) -> KeyOutcome {
        let assets = if !self.asset_choices.is_empty() {
            self.asset_choices.clone()
        } else {
            match handle.block_on(wallet.assets()) {
                Ok(a) => {
                    self.asset_choices = a.clone();
                    a
                }
                Err(e) => {
                    self.status = e.user_message();
                    return KeyOutcome::Consumed;
                }
            }
        };
        if assets.is_empty() {
            self.status = "No assets yet — wait for F2 load, press r, or paste 0x".into();
            return KeyOutcome::Consumed;
        }
        let cur = if let Some(t) = &self.token {
            assets
                .iter()
                .position(|b| {
                    b.token
                        .contract_address
                        .as_ref()
                        .is_some_and(|a| a.eq_ignore_ascii_case(&t.address))
                })
                .unwrap_or(usize::MAX)
        } else {
            assets
                .iter()
                .position(|b| b.token.contract_address.is_none())
                .unwrap_or(0)
        };
        let next = if down {
            if cur == usize::MAX {
                0
            } else {
                (cur + 1) % assets.len()
            }
        } else if cur == usize::MAX || cur == 0 {
            assets.len() - 1
        } else {
            cur - 1
        };
        if let Some(b) = assets.get(next) {
            self.coin_override = true;
            self.apply_balance_coin(b);
            let sym = b.token.symbol.as_str();
            self.status = if b.token.contract_address.is_some() {
                format!("F5: {sym}")
            } else {
                format!("F5: {sym} (native)")
            };
        }
        KeyOutcome::Consumed
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

    fn recipient_label(&self) -> String {
        let base = if self.home_mode {
            "Send to"
        } else {
            "Recipient"
        };
        match self.recipient_pick_label.as_deref() {
            Some(name) if !name.is_empty() => format!("{base} · {name}"),
            _ => base.to_string(),
        }
    }

    /// Vault display name for the recipient when known (F4 pick or address match).
    fn resolve_recipient_name(&self, wallet: &WalletState) -> Option<String> {
        if let Some(name) = self
            .recipient_pick_label
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            return Some(name.to_string());
        }
        let addr = self.recipient.value().trim();
        if addr.is_empty() {
            return None;
        }
        let Ok(choices) = wallet.account_choices() else {
            return None;
        };
        for (idx, label) in choices {
            if wallet
                .account_address(idx)
                .ok()
                .is_some_and(|a| a.eq_ignore_ascii_case(addr))
            {
                return Some(label);
            }
        }
        None
    }

    /// Cycle F4 through installed vault accounts (name in title, address in field).
    fn cycle_recipient_from_wallet(&mut self, wallet: &WalletState, down: bool) -> KeyOutcome {
        let choices = match wallet.account_choices() {
            Ok(c) => c,
            Err(e) => {
                self.status = e.user_message();
                return KeyOutcome::Consumed;
            }
        };
        if choices.is_empty() {
            self.status = "No Vaughan wallets installed".into();
            return KeyOutcome::Consumed;
        }
        let cur_addr = self.recipient.value().trim();
        let cur = choices
            .iter()
            .position(|(idx, _)| {
                wallet
                    .account_address(*idx)
                    .ok()
                    .is_some_and(|a| a.eq_ignore_ascii_case(cur_addr))
            })
            .unwrap_or(usize::MAX);
        let next = if down {
            if cur == usize::MAX {
                0
            } else {
                (cur + 1) % choices.len()
            }
        } else if cur == usize::MAX || cur == 0 {
            choices.len() - 1
        } else {
            cur - 1
        };
        let Some((idx, label)) = choices.get(next) else {
            return KeyOutcome::Consumed;
        };
        let addr = match wallet.account_address(*idx) {
            Ok(a) => a,
            Err(e) => {
                self.status = e.user_message();
                return KeyOutcome::Consumed;
            }
        };
        self.recipient.set_value(&addr);
        self.recipient_pick_label = Some(label.clone());
        self.stealth = None;
        self.status = format!("F4: {label} · {}", short_addr(&addr));
        KeyOutcome::Consumed
    }

    fn tab_focus(&mut self, forward: bool, wallet: &WalletState, handle: &Handle) -> KeyOutcome {
        match self.focus {
            Focus::Idle => {
                self.focus = if forward {
                    Focus::Recipient
                } else {
                    Focus::Amount
                };
                self.status.clear();
                KeyOutcome::Consumed
            }
            Focus::Recipient => {
                self.focus = if forward { Focus::Coin } else { Focus::Amount };
                KeyOutcome::Consumed
            }
            Focus::Coin => {
                if forward {
                    if let Err(e) = self.apply_coin_field(wallet, handle) {
                        self.status = e;
                        return KeyOutcome::Consumed;
                    }
                    self.focus = Focus::Amount;
                } else {
                    self.focus = Focus::Recipient;
                }
                KeyOutcome::Consumed
            }
            Focus::Amount => {
                self.focus = if forward {
                    Focus::Recipient
                } else {
                    Focus::Coin
                };
                KeyOutcome::Consumed
            }
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
                        "Pending — auto-checking · r to re-check now".into()
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
    ///
    /// Re-polls while status is still unknown or [`TxStatus::Pending`]. The first
    /// call after broadcast (from `apply_job_result` followup) runs immediately;
    /// later calls from the UI tick loop are throttled (~3s at 80ms ticks).
    pub fn followup_poll_status(&mut self) -> Option<UiJob> {
        self.followup_poll_status_inner(false)
    }

    /// Tick-driven re-check while the Done screen is waiting for inclusion.
    pub fn tick_poll_status(&mut self) -> Option<UiJob> {
        self.followup_poll_status_inner(true)
    }

    fn followup_poll_status_inner(&mut self, from_tick: bool) -> Option<UiJob> {
        if self.stage != Stage::Done || self.busy != Busy::Idle {
            return None;
        }
        let waiting = match self.receipt_status {
            None => !from_tick || self.tick.is_multiple_of(36),
            // ~3s between polls at an 80ms UI tick.
            Some(vaughan_core::chains::TxStatus::Pending) => self.tick.is_multiple_of(36),
            Some(_) => false,
        };
        if !waiting {
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
                Busy::Sending => {
                    if wallet.active_is_hardware().unwrap_or(false) {
                        "Trezor: PIN pad → confirm on device → broadcast"
                    } else {
                        "broadcasting"
                    }
                }
                Busy::PollingStatus => "checking receipt",
                Busy::Idle => "",
            };
            format!("{} {label}…", spinner_frame(self.tick))
        } else {
            self.status.clone()
        };

        match self.stage {
            Stage::Input => {
                let [to_area, coin_area, amount_area] = Layout::vertical([
                    Constraint::Length(3),
                    Constraint::Length(3),
                    Constraint::Length(3),
                ])
                .areas(content);

                render_fkey_labeled_input(
                    frame,
                    to_area,
                    "F4",
                    &self.recipient_label(),
                    &self.recipient,
                    self.focus == Focus::Recipient,
                );
                let coin_label = self.coin_label(wallet);
                render_fkey_labeled_input(
                    frame,
                    coin_area,
                    "F5",
                    &coin_label,
                    &self.coin,
                    self.focus == Focus::Coin,
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
                    "F6",
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

                let mut text = vec![
                    Line::from(format!(
                        "Send {} {} to:",
                        self.amount.value(),
                        self.token
                            .as_ref()
                            .map(|t| t.symbol.as_str())
                            .unwrap_or(&net.native_symbol)
                    )),
                    if let Some(t) = &self.token {
                        Line::from(Span::styled(
                            format!("token {}", t.address),
                            Style::default().fg(Color::DarkGray),
                        ))
                    } else {
                        Line::from("")
                    },
                ];
                if let Some(hint) = &stealth_hint {
                    text.push(Line::from(Span::styled(
                        hint.clone(),
                        Style::default().fg(Color::Yellow),
                    )));
                } else {
                    let mut to_spans = Vec::new();
                    if let Some(name) = self.resolve_recipient_name(wallet) {
                        to_spans.push(Span::styled(
                            name,
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD),
                        ));
                        to_spans.push(Span::raw(" · "));
                    }
                    to_spans.extend(brand::colored_address_spans(self.recipient.value()));
                    text.push(Line::from(to_spans));
                }
                text.extend([
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
                ]);
                let hw_sending =
                    self.busy == Busy::Sending && wallet.active_is_hardware().unwrap_or(false);
                if hw_sending {
                    text.extend([
                        Line::from(Span::styled(
                            "Trezor signing",
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD),
                        )),
                        Line::from(
                            "  1. PIN pad only if the device asks (skipped when already unlocked)",
                        ),
                        Line::from("  2. Confirm amount + recipient on the device"),
                        Line::from("  3. Broadcast when the device returns the signature"),
                        Line::from(""),
                        Line::from(Span::styled(
                            "Follow the overlay · Esc on overlay cancels",
                            Style::default().fg(Color::DarkGray),
                        )),
                    ]);
                } else {
                    text.extend([
                        Line::from("Gas speed (↑↓ or 1–5):"),
                        speed_line('1', FeeSpeed::Slow),
                        speed_line('2', FeeSpeed::Normal),
                        speed_line('3', FeeSpeed::Fast),
                        speed_line('4', FeeSpeed::Ape),
                        speed_line('5', FeeSpeed::Custom),
                        custom_hint,
                        Line::from(""),
                        Line::from("Enter — broadcast   Esc — cancel"),
                    ]);
                }
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
                let waiting = matches!(
                    self.receipt_status,
                    None | Some(vaughan_core::chains::TxStatus::Pending)
                );
                let status_line = match self.receipt_status {
                    Some(vaughan_core::chains::TxStatus::Pending) => {
                        format!(
                            "Status:   {} Pending — waiting for a block…",
                            spinner_frame(self.tick)
                        )
                    }
                    Some(vaughan_core::chains::TxStatus::Confirmed) => {
                        "Status:   Confirmed".to_string()
                    }
                    Some(vaughan_core::chains::TxStatus::Failed) => "Status:   Failed".to_string(),
                    None => format!("Status:   {} checking receipt…", spinner_frame(self.tick)),
                };
                let status_style = match self.receipt_status {
                    Some(vaughan_core::chains::TxStatus::Confirmed) => {
                        Style::default().fg(Color::Green)
                    }
                    Some(vaughan_core::chains::TxStatus::Failed) => Style::default().fg(Color::Red),
                    _ => Style::default().fg(Color::Yellow),
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
                    Line::from(Span::styled(status_line, status_style)),
                    Line::from(""),
                    Line::from(back),
                ];
                let inner = brand::render_faded_box(frame, content, None);
                let [info, bar_area] = Layout::vertical([
                    Constraint::Min(0),
                    Constraint::Length(if waiting { 3 } else { 0 }),
                ])
                .areas(inner);
                frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), info);
                if waiting {
                    // Indeterminate bounce — we don't know confirm % from RPC.
                    let t = self.tick % 40;
                    let ratio = if t <= 20 {
                        t as f64 / 20.0
                    } else {
                        (40 - t) as f64 / 20.0
                    }
                    .clamp(0.08, 1.0);
                    frame.render_widget(
                        Gauge::default()
                            .gauge_style(Style::default().fg(Color::Yellow).bg(Color::DarkGray))
                            .ratio(ratio)
                            .label(format!("{} inclusion…", spinner_frame(self.tick))),
                        bar_area,
                    );
                }
            }
        }

        frame.render_widget(status_paragraph(&status), status_area);
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &WalletState,
        handle: &Handle,
        _events: &EventBus,
    ) -> KeyOutcome {
        if self.busy != Busy::Idle {
            return KeyOutcome::Consumed;
        }
        match self.stage {
            Stage::Input => {
                // F4 / F5 / F6 jump to recipient / coin / amount from any input focus.
                if let KeyCode::F(4) = key.code {
                    self.focus = Focus::Recipient;
                    return KeyOutcome::Consumed;
                }
                if let KeyCode::F(5) = key.code {
                    self.focus = Focus::Coin;
                    if self.status.is_empty()
                        || self.status.starts_with("Tab fields")
                        || self.status.starts_with("F4:")
                    {
                        self.status = "F5 · ↑↓ select asset".into();
                    }
                    return KeyOutcome::Consumed;
                }
                if let KeyCode::F(6) = key.code {
                    self.focus = Focus::Amount;
                    return KeyOutcome::Consumed;
                }
                match self.focus {
                    Focus::Idle => match key.code {
                        KeyCode::Tab | KeyCode::BackTab => {
                            self.tab_focus(key.code == KeyCode::Tab, wallet, handle)
                        }
                        KeyCode::Up | KeyCode::Down => {
                            self.focus = Focus::Recipient;
                            self.cycle_recipient_from_wallet(wallet, key.code == KeyCode::Down)
                        }
                        KeyCode::Enter | KeyCode::Char(' ') => {
                            self.focus = Focus::Recipient;
                            self.status.clear();
                            KeyOutcome::Consumed
                        }
                        _ => KeyOutcome::NotHandled,
                    },
                    Focus::Recipient => {
                        if key.code == KeyCode::Esc {
                            return if self.home_mode {
                                self.focus = Focus::Idle;
                                self.status =
                                    "Tab fields · ↑↓ wallets F4 · ↑↓ assets F5 · F4/F5/F6".into();
                                KeyOutcome::Consumed
                            } else {
                                KeyOutcome::Back
                            };
                        }
                        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
                            return self.tab_focus(key.code == KeyCode::Tab, wallet, handle);
                        }
                        if matches!(key.code, KeyCode::Up | KeyCode::Down) {
                            return self
                                .cycle_recipient_from_wallet(wallet, key.code == KeyCode::Down);
                        }
                        match self.recipient.handle_key(key) {
                            InputAction::Ignored => KeyOutcome::NotHandled,
                            InputAction::Submitted => {
                                self.focus = Focus::Coin;
                                KeyOutcome::Consumed
                            }
                            InputAction::Consumed => {
                                self.recipient_pick_label = None;
                                KeyOutcome::Consumed
                            }
                        }
                    }
                    Focus::Coin => {
                        if key.code == KeyCode::Esc {
                            self.focus = if self.home_mode {
                                Focus::Idle
                            } else {
                                Focus::Recipient
                            };
                            return KeyOutcome::Consumed;
                        }
                        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
                            return self.tab_focus(key.code == KeyCode::Tab, wallet, handle);
                        }
                        if matches!(key.code, KeyCode::Up | KeyCode::Down) {
                            return self.cycle_coin_from_wallet(
                                wallet,
                                handle,
                                key.code == KeyCode::Down,
                            );
                        }
                        match self.coin.handle_key(key) {
                            InputAction::Ignored => KeyOutcome::NotHandled,
                            InputAction::Submitted => {
                                self.coin_override = true;
                                match self.apply_coin_field(wallet, handle) {
                                    Ok(()) => {
                                        self.status.clear();
                                        self.focus = Focus::Amount;
                                    }
                                    Err(e) => self.status = e,
                                }
                                KeyOutcome::Consumed
                            }
                            InputAction::Consumed => {
                                self.coin_override = true;
                                // Invalidate stale metadata until resolve.
                                if let Some(t) = &self.token {
                                    if !self.coin.value().trim().eq_ignore_ascii_case(&t.address) {
                                        self.token = None;
                                    }
                                }
                                KeyOutcome::Consumed
                            }
                        }
                    }
                    Focus::Amount => {
                        if key.code == KeyCode::Esc {
                            self.focus = if self.home_mode {
                                Focus::Idle
                            } else {
                                Focus::Coin
                            };
                            return KeyOutcome::Consumed;
                        }
                        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
                            return self.tab_focus(key.code == KeyCode::Tab, wallet, handle);
                        }
                        match self.amount.handle_key(key) {
                            InputAction::Ignored => KeyOutcome::NotHandled,
                            InputAction::Submitted => self.begin_estimate(wallet, handle),
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
                    self.focus = Focus::Coin;
                    self.confirm_focus = ConfirmFocus::Speed;
                    return KeyOutcome::Consumed;
                }
                if let KeyCode::F(6) = key.code {
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
                        self.focus = Focus::Coin;
                    } else {
                        self.stage = Stage::Input;
                        self.focus = Focus::Coin;
                    }
                    KeyOutcome::Consumed
                }
                KeyCode::F(6) => {
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
                        KeyOutcome::Back
                    }
                }
                _ => KeyOutcome::NotHandled,
            },
        }
    }

    fn begin_estimate(&mut self, wallet: &WalletState, handle: &Handle) -> KeyOutcome {
        if let Err(e) = self.apply_coin_field(wallet, handle) {
            self.status = e;
            self.focus = Focus::Coin;
            return KeyOutcome::Consumed;
        }
        if let Some(msg) = self.assist_burn_amount_error() {
            self.status = msg;
            return KeyOutcome::Consumed;
        }
        let decimals = self.amount_decimals(wallet);
        match parse_native_amount(self.amount.value(), decimals) {
            Ok(amount) => match self.resolve_recipient(wallet) {
                Ok(to) => {
                    if self.stealth.is_some() && !stealth_power_ok(wallet, handle) {
                        self.stealth = None;
                        self.status =
                            "Stealth send locked: burn ≥13 WZRD from any wallet — Settings → Unlock".into();
                        return KeyOutcome::Consumed;
                    }
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

    fn assist_burn_amount_error(&self) -> Option<String> {
        if !self.assist_burn {
            return None;
        }
        use alloy::primitives::U256;
        use std::str::FromStr;
        use vaughan_core::core::assist_burn_amount_u256;
        let decimals = 18u8;
        let Ok(amount) = parse_native_amount(self.amount.value(), decimals) else {
            return Some("invalid burn amount".into());
        };
        let Ok(wei) = U256::from_str(&amount) else {
            return Some("invalid burn amount".into());
        };
        if wei < assist_burn_amount_u256() {
            return Some("burn at least 13 WZRD in one transfer (no drip)".into());
        }
        None
    }

    fn begin_send(&mut self, wallet: &WalletState) -> KeyOutcome {
        if let Some(msg) = self.assist_burn_amount_error() {
            self.status = msg;
            return KeyOutcome::Consumed;
        }
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

/// Compact `0xabcd…1234` for status / chrome (full address stays in the F4 field).
fn short_addr(addr: &str) -> String {
    let a = addr.trim();
    if a.len() > 12 {
        format!("{}…{}", &a[..6], &a[a.len() - 4..])
    } else {
        a.to_string()
    }
}

fn stealth_power_ok(wallet: &WalletState, handle: &Handle) -> bool {
    use vaughan_core::core::{
        assist_burn_gate_enabled, assist_unlock_bypass, entitlement_chain_id,
        power_features_unlocked_blocking,
    };
    if !assist_burn_gate_enabled() || assist_unlock_bypass() {
        return true;
    }
    let Some(chain_id) = entitlement_chain_id() else {
        return false;
    };
    let addrs = wallet.account_addresses().unwrap_or_default();
    let dir = vaughan_agent::paths::profile_dir(wallet.path());
    power_features_unlocked_blocking(handle, Some(&dir), chain_id, &addrs)
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

#[cfg(test)]
mod tests {
    use super::*;
    use vaughan_core::chains::TokenInfo;

    fn erc20_balance(addr: &str, symbol: &str) -> Balance {
        Balance {
            token: TokenInfo {
                symbol: symbol.into(),
                name: symbol.into(),
                decimals: 18,
                contract_address: Some(addr.into()),
            },
            raw: "0".into(),
            formatted: "0".into(),
            usd_value: None,
        }
    }

    #[test]
    fn sync_from_chrome_skips_when_coin_overridden() {
        let mut v = SendView::home();
        let wzrd = erc20_balance("0x29bab93456c0E97EE931C1554c7C215480aa7766", "WZRD");
        v.apply_balance_coin(&wzrd);
        assert!(v.token.is_some());
        v.coin_override = true;
        v.coin
            .set_value("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        v.token = None;
        let chrome = ChromeSnapshot {
            assets: vec![wzrd],
            asset_idx: 0,
            ..ChromeSnapshot::default()
        };
        v.sync_from_chrome(&chrome);
        assert!(v.token.is_none(), "override must block F2 sync");
        assert_eq!(v.coin.value(), "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    }

    #[test]
    fn sync_from_chrome_applies_f2_when_not_overridden() {
        let mut v = SendView::home();
        assert!(!v.coin_override);
        let wzrd = erc20_balance("0x29bab93456c0E97EE931C1554c7C215480aa7766", "WZRD");
        let chrome = ChromeSnapshot {
            assets: vec![wzrd],
            asset_idx: 0,
            ..ChromeSnapshot::default()
        };
        v.sync_from_chrome(&chrome);
        assert_eq!(v.token.as_ref().map(|t| t.symbol.as_str()), Some("WZRD"));
        assert!(v.coin.value().starts_with("0x"));
        assert_eq!(v.asset_choices.len(), 1);
    }

    #[test]
    fn sync_from_chrome_refreshes_f5_asset_choices_even_when_overridden() {
        let mut v = SendView::home();
        v.coin_override = true;
        let wzrd = erc20_balance("0x29bab93456c0E97EE931C1554c7C215480aa7766", "WZRD");
        let chrome = ChromeSnapshot {
            assets: vec![wzrd],
            asset_idx: 0,
            ..ChromeSnapshot::default()
        };
        v.sync_from_chrome(&chrome);
        assert_eq!(v.asset_choices.len(), 1);
        assert_eq!(v.asset_choices[0].token.symbol, "WZRD");
    }

    #[test]
    fn clear_to_native_clears_token_and_coin() {
        let mut v = SendView::home();
        v.token = Some(TokenCtx {
            address: "0x29bab93456c0E97EE931C1554c7C215480aa7766".into(),
            symbol: "WZRD".into(),
            decimals: 18,
        });
        v.coin
            .set_value("0x29bab93456c0E97EE931C1554c7C215480aa7766");
        v.clear_to_native();
        assert!(v.token.is_none());
        assert!(v.coin.value().is_empty());
    }

    #[test]
    fn recipient_label_includes_picked_wallet_name() {
        let mut v = SendView::home();
        assert_eq!(v.recipient_label(), "Send to");
        v.recipient_pick_label = Some("Trezor 1".into());
        assert_eq!(v.recipient_label(), "Send to · Trezor 1");
    }

    #[test]
    fn short_addr_compacts_long_hex() {
        assert_eq!(
            short_addr("0x1234567890abcdef1234567890abcdef12345678"),
            "0x1234…5678"
        );
        assert_eq!(short_addr("0xabc"), "0xabc");
    }
}
