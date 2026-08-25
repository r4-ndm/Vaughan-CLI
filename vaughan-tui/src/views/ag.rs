//! Aggregator (Ag) view — best-route quotes without a partner API key.
//!
//! Live today: SquirrelSwap (default), PulseSwap, Piteas. ↑/↓ venue ·
//! Space = native PLS in · amount as human units (e.g. `1` or `0.01`) · Enter quote.

use alloy::primitives::{Address, U256};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use std::str::FromStr;
use tokio::runtime::Handle;
use vaughan_core::chains::EvmTransaction;
use vaughan_core::core::{
    format_base_units, parse_native_amount, AggAccess, AggQuote, AggVenue, WalletState,
};
use vaughan_provider::EventBus;

use crate::app::{KeyOutcome, Screen};
use crate::brand;
use crate::input::{Input, InputAction};
use crate::jobs::{spinner_frame, UiJob, UiJobResult};
use crate::views::dex_calldata::build_approve_tx;
use crate::views::{body_areas, render_labeled_input, status_paragraph};

/// PulseX (PLSX) on PulseChain mainnet — default Ag `Out` token.
const PLSX_369: &str = "0x95B303987A60C71504D99Aa1b13B4DA07b0790ab";

fn wpls_for_chain(chain_id: u64) -> &'static str {
    match chain_id {
        369 => "0xA1077a294dDE1B09bB078844df40758a5D0f9a27",
        943 => "0x70499adEBB11Efd915E3b69E700c331778628707",
        _ => "",
    }
}

/// Parse Ag amount: prefer human decimals (`1`, `0.01`); bare integers ≥ 1e15
/// are treated as wei for power users / pasted Brain values. Suffix `wei` forces wei.
fn parse_ag_amount(raw: &str) -> Result<U256, String> {
    let t = raw.trim();
    if t.is_empty() {
        return Err("amount: enter e.g. 1 or 0.01 (PLS)".into());
    }
    let (strip, force_wei) = if let Some(s) = t.strip_suffix("wei") {
        (s.trim(), true)
    } else {
        (t, false)
    };
    if force_wei || (strip.chars().all(|c| c.is_ascii_digit()) && strip.len() >= 15) {
        let wei = U256::from_str(strip).map_err(|_| "amount: invalid wei integer".to_string())?;
        if wei.is_zero() {
            return Err("amount: need non-zero".into());
        }
        return Ok(wei);
    }
    let wei_str = parse_native_amount(strip, 18).map_err(|e| e.user_message())?;
    let wei = U256::from_str(&wei_str).map_err(|_| "amount: parse failed".to_string())?;
    if wei.is_zero() {
        return Err("amount: need non-zero".into());
    }
    Ok(wei)
}

fn fmt_token_amount(wei: &U256) -> String {
    format_base_units(&wei.to_string(), 18)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConfirmStep {
    Approve,
    Swap,
}

impl ConfirmStep {
    fn label(self) -> &'static str {
        match self {
            Self::Approve => "approve spender",
            Self::Swap => "send aggregator swap",
        }
    }
}

enum Stage {
    Input,
    Confirm(ConfirmStep),
    Done,
}

#[derive(PartialEq, Eq)]
enum Focus {
    Venue,
    TokenIn,
    TokenOut,
    Amount,
    Slippage,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Busy {
    Idle,
    Quoting,
    Approving,
    Swapping,
}

pub struct AgView {
    stage: Stage,
    focus: Focus,
    venue: AggVenue,
    token_in: Input,
    token_out: Input,
    amount: Input,
    slippage: Input,
    native_in: bool,
    chain_id: u64,
    busy: Busy,
    tick: u64,
    status: String,
    quote: Option<AggQuote>,
    confirm_lines: Vec<String>,
    tx_hash: Option<String>,
    approve_hash: Option<String>,
}

impl Default for AgView {
    fn default() -> Self {
        Self {
            stage: Stage::Input,
            focus: Focus::Amount,
            venue: AggVenue::SquirrelSwap,
            token_in: Input::new(false, "0x token in (Space = native PLS)…"),
            token_out: Input::new(false, "0x token out…"),
            amount: Input::new(false, "amount (e.g. 1 or 0.01)"),
            slippage: Input::new(false, "0.5"),
            native_in: true,
            chain_id: 0,
            busy: Busy::Idle,
            tick: 0,
            status: String::new(),
            quote: None,
            confirm_lines: Vec::new(),
            tx_hash: None,
            approve_hash: None,
        }
    }
}

impl AgView {
    pub fn for_chain(chain_id: u64) -> Self {
        Self::for_chain_prefill(chain_id, None, None)
    }

    /// Ag screen with optional amount / token-out from an intent macro.
    pub fn for_chain_prefill(chain_id: u64, amount: Option<&str>, token_out: Option<&str>) -> Self {
        let mut v = Self {
            chain_id,
            ..Self::default()
        };
        v.slippage.set_value("0.5");
        v.amount.set_value(amount.unwrap_or("1").trim().to_string());
        if let Some(wpls) = non_empty(wpls_for_chain(chain_id)) {
            v.token_in.set_value(wpls);
            v.native_in = true;
        }
        if let Some(out) = token_out.filter(|s| !s.trim().is_empty()) {
            v.token_out.set_value(out.trim().to_string());
        } else if chain_id == 369 {
            v.token_out.set_value(PLSX_369);
        }
        if chain_id == 369 {
            v.status =
                "Squirrel · native PLS → PLSX · amount e.g. 1 · Enter quote · Space toggles native"
                    .into();
        } else {
            v.status = match chain_id {
                943 => "Aggregators are mainnet (369) — switch Net or use Dex on testnet".into(),
                _ => "PulseChain aggregators need chain 369".into(),
            };
        }
        v.refresh_venue_status();
        v
    }

    fn refresh_venue_status(&mut self) {
        if self.chain_id != 369 {
            return;
        }
        self.status = match self.venue.access() {
            AggAccess::LiveNoKey => format!(
                "{} · {} — amount in PLS units · Enter to quote",
                self.venue.label(),
                self.venue.blurb()
            ),
            AggAccess::NeedsApiKey(why) | AggAccess::ListedOnly(why) => {
                format!("{} — {} ({why})", self.venue.label(), self.venue.blurb())
            }
        };
    }

    pub fn set_tick(&mut self, tick: u64) {
        self.tick = tick;
    }

    pub fn apply_job_result(&mut self, result: UiJobResult) {
        match result {
            UiJobResult::AggQuote(Ok(quote)) => {
                self.busy = Busy::Idle;
                self.quote = Some(quote);
                self.enter_confirm();
            }
            UiJobResult::AggQuote(Err(e)) => {
                self.busy = Busy::Idle;
                self.status = e.user_message();
                self.stage = Stage::Input;
            }
            UiJobResult::Send(Ok(receipt)) => match self.busy {
                Busy::Approving => {
                    let hash = receipt.hash;
                    self.busy = Busy::Idle;
                    self.approve_hash = Some(hash.clone());
                    self.status = format!("Approve sent ({hash}). Confirm swap next.");
                    self.stage = Stage::Confirm(ConfirmStep::Swap);
                    self.rebuild_confirm_lines(ConfirmStep::Swap);
                }
                Busy::Swapping => {
                    let hash = receipt.hash;
                    self.busy = Busy::Idle;
                    self.tx_hash = Some(hash);
                    self.stage = Stage::Done;
                    self.status = "Aggregator swap broadcast.".into();
                }
                _ => {}
            },
            UiJobResult::Send(Err(e)) => {
                self.busy = Busy::Idle;
                self.status = e.user_message();
                self.stage = Stage::Input;
            }
            _ => {}
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, _wallet: &WalletState) {
        let [body, status] = body_areas(area);
        match self.stage {
            Stage::Input => self.render_input(frame, body),
            Stage::Confirm(_) => self.render_confirm(frame, body),
            Stage::Done => {
                frame.render_widget(
                    Paragraph::new(format!(
                        "Done.\nApprove: {}\nSwap: {}",
                        self.approve_hash.as_deref().unwrap_or("—"),
                        self.tx_hash.as_deref().unwrap_or("—")
                    )),
                    body,
                );
            }
        }
        let status_text = match self.busy {
            Busy::Quoting => format!("{} quoting…", spinner_frame(self.tick)),
            Busy::Approving => format!("{} approving…", spinner_frame(self.tick)),
            Busy::Swapping => format!("{} swapping…", spinner_frame(self.tick)),
            Busy::Idle => self.status.clone(),
        };
        frame.render_widget(status_paragraph(&status_text), status);
    }

    fn render_input(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(area);

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " AG — SquirrelSwap Brain first (no API key) ",
                Style::default()
                    .fg(brand::accent_color())
                    .add_modifier(Modifier::BOLD),
            ))),
            chunks[0],
        );

        let live = if self.venue.is_live() {
            "LIVE"
        } else {
            "listed"
        };
        let venue_style = if self.focus == Focus::Venue {
            Style::default()
                .fg(brand::accent_color())
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default().fg(brand::body_color())
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("Ag {} [{}]  ", self.venue.label(), live),
                    venue_style,
                ),
                Span::styled("↑/↓ venue", Style::default().fg(brand::body_color())),
            ])),
            chunks[1],
        );

        render_labeled_input(
            frame,
            chunks[2],
            "In",
            &self.token_in,
            self.focus == Focus::TokenIn,
        );
        render_labeled_input(
            frame,
            chunks[3],
            "Out",
            &self.token_out,
            self.focus == Focus::TokenOut,
        );
        render_labeled_input(
            frame,
            chunks[4],
            "Amt",
            &self.amount,
            self.focus == Focus::Amount,
        );
        render_labeled_input(
            frame,
            chunks[5],
            "Slip%",
            &self.slippage,
            self.focus == Focus::Slippage,
        );
        frame.render_widget(
            Paragraph::new(if self.native_in {
                "native PLS in (Space toggles) · Amt is PLS (not wei)"
            } else {
                "ERC-20 in — approve then swap · Amt uses token decimals (18)"
            }),
            chunks[6],
        );
    }

    fn render_confirm(&self, frame: &mut Frame, area: Rect) {
        let text = self.confirm_lines.join("\n");
        frame.render_widget(
            Paragraph::new(format!("{text}\n\nEnter confirm · Esc cancel")),
            area,
        );
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        _handle: &Handle,
        _events: &EventBus,
    ) -> KeyOutcome {
        if self.busy != Busy::Idle {
            return KeyOutcome::Consumed;
        }
        match self.stage {
            Stage::Done => match key.code {
                KeyCode::Enter => {
                    let chain_id = wallet.networks().active().chain_id;
                    *self = Self::for_chain(chain_id);
                    KeyOutcome::Consumed
                }
                KeyCode::Esc => KeyOutcome::Navigate(Screen::Dashboard),
                _ => KeyOutcome::Consumed,
            },
            Stage::Confirm(step) => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::Input;
                    self.quote = None;
                    KeyOutcome::Consumed
                }
                KeyCode::Enter | KeyCode::Char('y') => self.confirm_step(step, wallet),
                _ => KeyOutcome::Consumed,
            },
            Stage::Input => self.handle_input_key(key, wallet),
        }
    }

    fn handle_input_key(&mut self, key: KeyEvent, wallet: &WalletState) -> KeyOutcome {
        match key.code {
            KeyCode::Esc => KeyOutcome::Navigate(Screen::Dashboard),
            KeyCode::Up | KeyCode::Down if self.focus == Focus::Venue => {
                self.venue = if matches!(key.code, KeyCode::Down) {
                    self.venue.next()
                } else {
                    self.venue.prev()
                };
                self.quote = None;
                self.refresh_venue_status();
                KeyOutcome::Consumed
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Venue => Focus::TokenIn,
                    Focus::TokenIn => Focus::TokenOut,
                    Focus::TokenOut => Focus::Amount,
                    Focus::Amount => Focus::Slippage,
                    Focus::Slippage => Focus::Venue,
                };
                KeyOutcome::Consumed
            }
            KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Venue => Focus::Slippage,
                    Focus::TokenIn => Focus::Venue,
                    Focus::TokenOut => Focus::TokenIn,
                    Focus::Amount => Focus::TokenOut,
                    Focus::Slippage => Focus::Amount,
                };
                KeyOutcome::Consumed
            }
            KeyCode::Char(' ') if matches!(self.focus, Focus::Venue | Focus::TokenIn) => {
                self.native_in = !self.native_in;
                if self.native_in {
                    if let Some(w) = non_empty(wpls_for_chain(self.chain_id)) {
                        self.token_in.set_value(w);
                    }
                }
                self.status = if self.native_in {
                    "native PLS in".into()
                } else {
                    "ERC-20 in".into()
                };
                KeyOutcome::Consumed
            }
            KeyCode::Enter => self.start_quote(wallet),
            _ => {
                let input = match self.focus {
                    Focus::TokenIn => &mut self.token_in,
                    Focus::TokenOut => &mut self.token_out,
                    Focus::Amount => &mut self.amount,
                    Focus::Slippage => &mut self.slippage,
                    Focus::Venue => return KeyOutcome::Consumed,
                };
                match input.handle_key(key) {
                    InputAction::Ignored => KeyOutcome::NotHandled,
                    _ => KeyOutcome::Consumed,
                }
            }
        }
    }

    fn start_quote(&mut self, wallet: &WalletState) -> KeyOutcome {
        if self.chain_id != 369 {
            self.status = "Aggregators need PulseChain mainnet (369)".into();
            return KeyOutcome::Consumed;
        }
        if !self.venue.is_live() {
            self.refresh_venue_status();
            return KeyOutcome::Consumed;
        }
        let token_out = match Address::from_str(self.token_out.value().trim()) {
            Ok(a) => a,
            Err(_) => {
                self.status = "token out: invalid address".into();
                return KeyOutcome::Consumed;
            }
        };
        let token_in = if self.native_in {
            Address::ZERO
        } else {
            match Address::from_str(self.token_in.value().trim()) {
                Ok(a) => a,
                Err(_) => {
                    self.status = "token in: invalid address".into();
                    return KeyOutcome::Consumed;
                }
            }
        };
        let amount = match parse_ag_amount(self.amount.value()) {
            Ok(a) => a,
            Err(e) => {
                self.status = e;
                return KeyOutcome::Consumed;
            }
        };
        let slippage: f64 = self.slippage.value().trim().parse().unwrap_or(0.5);
        let account = wallet
            .active_address()
            .ok()
            .and_then(|s| Address::from_str(s).ok());

        self.busy = Busy::Quoting;
        self.status = format!("quoting {}…", self.venue.label());
        KeyOutcome::StartJob(UiJob::AggQuote {
            venue: self.venue,
            token_in: token_in.to_string(),
            token_out: token_out.to_string(),
            amount: amount.to_string(),
            slippage,
            native_in: self.native_in,
            native_out: false,
            account: account.map(|a| a.to_string()),
        })
    }

    fn enter_confirm(&mut self) {
        let Some(q) = self.quote.as_ref() else {
            return;
        };
        let needs_approve = !self.native_in;
        let step = if needs_approve {
            ConfirmStep::Approve
        } else {
            ConfirmStep::Swap
        };
        let out_h = fmt_token_amount(&q.amount_out);
        let status = format!(
            "{} quote · out≈{out_h} · gas≈{:?} · Enter to {}",
            q.venue.label(),
            q.gas_estimate,
            step.label()
        );
        self.rebuild_confirm_lines(step);
        self.stage = Stage::Confirm(step);
        self.status = status;
    }

    fn rebuild_confirm_lines(&mut self, step: ConfirmStep) {
        let Some(q) = self.quote.as_ref() else {
            self.confirm_lines.clear();
            return;
        };
        let in_h = fmt_token_amount(&q.amount_in);
        let out_h = fmt_token_amount(&q.amount_out);
        let value_h = fmt_token_amount(&q.tx.value);
        let in_label = if self.native_in { "PLS" } else { "token" };
        let step_hint = match step {
            ConfirmStep::Approve => "next: approve router to spend your token",
            ConfirmStep::Swap => {
                if self.native_in {
                    "next: broadcast swap (native PLS)"
                } else {
                    "next: broadcast swap (after approve)"
                }
            }
        };
        self.confirm_lines = vec![
            format!("Aggregator: {}", q.venue.label()),
            format!("Confirm: {} — {step_hint}", step.label()),
            format!("Router:   {:#x}", q.tx.to),
            format!("You pay:  {in_h} {in_label}  ({})", q.amount_in),
            format!("You get:  ≈{out_h} out  ({})", q.amount_out),
            format!("Tx value: {value_h} PLS  ({})", q.tx.value),
            format!("Calldata: {} bytes", q.tx.data.len()),
            "Review amounts above — Enter signs & broadcasts this step only.".into(),
        ];
    }

    fn confirm_step(&mut self, step: ConfirmStep, wallet: &WalletState) -> KeyOutcome {
        let Some(q) = self.quote.clone() else {
            self.stage = Stage::Input;
            return KeyOutcome::Consumed;
        };
        let from = match wallet.active_address() {
            Ok(a) => a.to_string(),
            Err(e) => {
                self.status = e.user_message();
                return KeyOutcome::Consumed;
            }
        };
        let chain_id = wallet.networks().active().chain_id;
        match step {
            ConfirmStep::Approve => {
                let token = match Address::from_str(self.token_in.value().trim()) {
                    Ok(a) => a,
                    Err(_) => {
                        self.status = "token in invalid for approve".into();
                        return KeyOutcome::Consumed;
                    }
                };
                let tx = build_approve_tx(token, q.spender, q.amount_in, &from, chain_id);
                self.busy = Busy::Approving;
                KeyOutcome::StartJob(UiJob::SendEvm { tx })
            }
            ConfirmStep::Swap => {
                let data_hex = format!("0x{}", hex::encode(q.tx.data.as_ref()));
                let tx = EvmTransaction {
                    from,
                    to: format!("{:#x}", q.tx.to),
                    value: q.tx.value.to_string(),
                    data: Some(data_hex),
                    gas_limit: None,
                    gas_price: None,
                    max_fee_per_gas: None,
                    max_priority_fee_per_gas: None,
                    nonce: None,
                    chain_id,
                };
                self.busy = Busy::Swapping;
                KeyOutcome::StartJob(UiJob::SendEvm { tx })
            }
        }
    }
}

fn non_empty(s: &str) -> Option<&str> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_human_pls() {
        assert_eq!(
            parse_ag_amount("1").unwrap(),
            U256::from_str("1000000000000000000").unwrap()
        );
        assert_eq!(
            parse_ag_amount("0.01").unwrap(),
            U256::from_str("10000000000000000").unwrap()
        );
    }

    #[test]
    fn parse_wei_scale_and_suffix() {
        let wei = U256::from_str("1000000000000000000").unwrap();
        assert_eq!(parse_ag_amount("1000000000000000000").unwrap(), wei);
        assert_eq!(parse_ag_amount("1000000000000000000wei").unwrap(), wei);
    }
}
