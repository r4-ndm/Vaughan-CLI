//! DEX swap view: any Uniswap V2– or V3–compatible router on PulseChain.
//!
//! When the DEX row is focused: **↑/↓** pick venue, **←/→** pick V2 or V3.
//! Calldata lives in [`dex_calldata`]. OTC / Balancer venues are listed but
//! not swap-wired yet (different ABIs).

use alloy::primitives::{Address, U256};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};
use std::str::FromStr;
use tokio::runtime::Handle;
use vaughan_core::chains::{Balance, EvmTransaction, Fee, FeeSpeed};
use vaughan_core::core::is_allowed_dex_router;
use vaughan_core::core::wiz4rd::{deployment_for_chain, WIZ4RD_FEE_TIERS, WZRD_SMOKE_943};
use vaughan_core::core::{
    chain_label, format_base_units, format_display_amount, min_out_after_slippage,
    missing_router_hint, venue_quoter_v2, venue_swap_router, wpls_for_chain, DexProtocol, DexVenue,
    WalletState, DEFAULT_DEX_SLIPPAGE_BPS,
};
use vaughan_core::error::WalletError;
use vaughan_provider::EventBus;

use crate::app::KeyOutcome;
use crate::brand;
use crate::input::{Input, InputAction};
use crate::jobs::{spinner_frame, UiJobResult};
use crate::views::dex_calldata::{
    build_approve_tx, build_swap_tx, encode_v3_path_hops, hop_tokens, DexSwapRequest,
};
use crate::views::{
    body_areas, cycle_token_picker, manual_edit_resets_token_pick, native_pls_label,
    parse_min_out_amount, parse_swap_amount, parse_token_address, status_paragraph,
    token_symbol_for_address, TOKEN_PICK_UNINIT,
};

/// Common V3 fee tiers. Includes Pancake/wiz4rd `2500` and `20000` (2%).
const FEE_TIERS: &[u32] = &[100, 500, 2500, 3000, 10_000, 20_000];

/// Longest Dex swap field label — value column starts after this width.
const SWAP_LABEL_WIDTH: usize = 16;

/// Max fractional digits for Expected / Minimum received display (DEX-style).
const SWAP_DISPLAY_FRAC: usize = 5;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConfirmFocus {
    Speed,
    CustomGas,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConfirmStep {
    Approve,
    Swap,
}

enum Stage {
    Input,
    Confirm(ConfirmStep),
    Done,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Focus {
    /// No field selected (Enter to leave edit mode).
    None,
    /// DEX venue + V2/V3 row (↑/↓ venue, ←/→ protocol when selected).
    Dex,
    Router,
    TokenIn,
    TokenOut,
    Fee,
    Amount,
    MinOut,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Busy {
    Idle,
    CheckingAllowance,
    EstimatingFee,
    Approving,
    Swapping,
}

pub struct DexView {
    stage: Stage,
    focus: Focus,
    venue: DexVenue,
    protocol: DexProtocol,
    fee: u32,
    router: Input,
    token_in: Input,
    token_out: Input,
    token_in_pick: usize,
    token_out_pick: usize,
    token_in_editing: bool,
    token_out_editing: bool,
    amount: Input,
    min_out: Input,
    native_in: bool,
    wpls: Option<Address>,
    chain_id: u64,
    busy: Busy,
    tick: u64,
    status: String,
    tx_hash: Option<String>,
    approve_hash: Option<String>,
    confirm_lines: Vec<String>,
    confirm_focus: ConfirmFocus,
    custom_gas: Input,
    speed: FeeSpeed,
    pending_step: Option<ConfirmStep>,
    pending_tx: Option<EvmTransaction>,
    base_fee: Option<Fee>,
    pending_auto_fee_estimate: bool,
    /// After allowance RPC check, queue confirm + fee estimate.
    pending_start_confirm: Option<ConfirmStep>,
    /// Debounced live quote (expected output amount).
    quote_gen: u64,
    quote_loading_gen: Option<u64>,
    quote_debounce: u8,
    expected_out: Option<U256>,
    quote_error: Option<String>,
    /// V3 hop list from the last successful quote (must match swap calldata).
    quote_path: Option<Vec<Address>>,
    /// Per-hop fee tiers from quote (`path.len() - 1`).
    quote_hop_fees: Option<Vec<u32>>,
}

impl Default for DexView {
    fn default() -> Self {
        Self {
            stage: Stage::Input,
            focus: Focus::None,
            venue: DexVenue::PulseX,
            protocol: DexProtocol::V2,
            fee: 3000,
            router: Input::new(false, "0x…"),
            token_in: Input::new(false, "↑↓ pick · Space = tPLS/PLS"),
            token_out: Input::new(false, "↑↓ pick token"),
            token_in_pick: TOKEN_PICK_UNINIT,
            token_out_pick: TOKEN_PICK_UNINIT,
            token_in_editing: false,
            token_out_editing: false,
            amount: Input::new(false, "0.0"),
            min_out: Input::new(false, "0"),
            native_in: true,
            wpls: None,
            chain_id: 0,
            busy: Busy::Idle,
            tick: 0,
            status: String::new(),
            tx_hash: None,
            approve_hash: None,
            confirm_lines: Vec::new(),
            confirm_focus: ConfirmFocus::Speed,
            custom_gas: Input::new(false, "gwei"),
            speed: FeeSpeed::Normal,
            pending_step: None,
            pending_tx: None,
            base_fee: None,
            pending_auto_fee_estimate: false,
            pending_start_confirm: None,
            quote_gen: 0,
            quote_loading_gen: None,
            quote_debounce: 0,
            expected_out: None,
            quote_error: None,
            quote_path: None,
            quote_hop_fees: None,
        }
    }
}

impl DexView {
    /// Prefill from the active chain (Wiz4rd V3 on 943; PulseX V2 elsewhere).
    pub fn for_chain(chain_id: u64) -> Self {
        let mut v = Self {
            chain_id,
            ..Self::default()
        };
        if chain_id == 943 {
            v.venue = DexVenue::Wiz4rd;
            v.protocol = DexProtocol::V3;
            v.fee = 500;
            v.token_out.set_value(WZRD_SMOKE_943);
            v.token_out_pick = TOKEN_PICK_UNINIT;
        }
        v.amount.set_value("1");
        v.min_out.set_value("0");
        v.focus = Focus::None;
        v.apply_venue_defaults(true);
        v.status = String::new();
        v.mark_quote_stale();
        v
    }

    /// Schedule a debounced pool/router quote after form edits.
    pub fn mark_quote_stale(&mut self) {
        if matches!(self.stage, Stage::Input) {
            self.quote_debounce = 4;
            self.quote_path = None;
            self.quote_hop_fees = None;
        }
    }

    /// Advance quote debounce without touching the wallet lock (UI tick).
    ///
    /// Returns `true` when debounce elapsed and [`Self::start_quote_job`] should run.
    pub fn tick_quote_debounce(&mut self) -> bool {
        if !matches!(self.stage, Stage::Input) || self.busy != Busy::Idle {
            return false;
        }
        if self.quote_debounce == 0 || self.quote_loading_gen.is_some() {
            return false;
        }
        self.quote_debounce -= 1;
        self.quote_debounce == 0
    }

    fn apply_venue_defaults(&mut self, overwrite_router: bool) {
        if let Some(wpls) = wpls_for_chain(self.chain_id) {
            self.wpls = Some(wpls);
        }

        if self.venue == DexVenue::Custom && !overwrite_router {
            return;
        }

        // wiz4rd is V3-only — snap protocol when selecting the venue.
        if self.venue == DexVenue::Wiz4rd {
            self.protocol = DexProtocol::V3;
        }

        match venue_swap_router(self.venue, self.protocol, self.chain_id) {
            Some(router) => {
                if overwrite_router || self.venue != DexVenue::Custom {
                    self.router.set_value(format!("{router}"));
                }
            }
            None if self.venue != DexVenue::Custom => {
                self.router.set_value("");
                self.status = missing_router_hint(self.venue, self.protocol, self.chain_id);
            }
            None => {}
        }
    }

    pub fn set_tick(&mut self, tick: u64) {
        self.tick = tick;
    }

    /// True while a swap broadcast job is completing (not approve-only).
    pub fn is_completing_swap(&self) -> bool {
        matches!(self.busy, Busy::Swapping)
    }

    /// Token out field (for persisting custom imports after a successful swap).
    pub fn token_out_address(&self) -> Option<String> {
        let t = self.token_out.value().trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    }

    pub fn apply_job_result(&mut self, result: UiJobResult) {
        match result {
            UiJobResult::Fee(Ok(fee)) => {
                self.busy = Busy::Idle;
                self.base_fee = Some(fee);
                self.speed = FeeSpeed::Normal;
                self.confirm_focus = ConfirmFocus::Speed;
                self.custom_gas.set_value("");
                if let Some(step) = self.pending_step.take() {
                    self.stage = Stage::Confirm(step);
                }
                if matches!(self.stage, Stage::Confirm(ConfirmStep::Swap)) {
                    self.status = "Ready — confirm swap (Enter send · Esc cancel)".into();
                } else if matches!(self.stage, Stage::Confirm(ConfirmStep::Approve)) {
                    self.status = "Ready — confirm approve (Enter send · Esc cancel)".into();
                } else {
                    self.status.clear();
                }
            }
            UiJobResult::Fee(Err(e)) => {
                self.busy = Busy::Idle;
                self.pending_tx = None;
                self.base_fee = None;
                self.pending_step = None;
                self.status = dex_fee_estimate_error(&e);
                self.stage = Stage::Input;
            }
            UiJobResult::Send(Ok(receipt)) => match self.busy {
                Busy::Approving => {
                    let hash = receipt.hash;
                    self.busy = Busy::Idle;
                    self.approve_hash = Some(hash.clone());
                    self.status = "Approve sent — waiting for on-chain allowance…".into();
                    self.pending_auto_fee_estimate = true;
                }
                Busy::Swapping => {
                    let hash = receipt.hash;
                    self.busy = Busy::Idle;
                    self.tx_hash = Some(hash);
                    self.stage = Stage::Done;
                    self.status = "Swap broadcast.".into();
                }
                Busy::Idle | Busy::EstimatingFee | Busy::CheckingAllowance => {}
            },
            UiJobResult::Send(Err(e)) => {
                self.busy = Busy::Idle;
                self.status = e.user_message();
                self.stage = Stage::Input;
            }
            UiJobResult::DexAllowanceCheck(result) => {
                self.busy = Busy::Idle;
                match result {
                    Ok(true) => {
                        self.pending_start_confirm = Some(ConfirmStep::Swap);
                        self.status = "Router allowance OK — preparing swap confirm…".into();
                    }
                    Ok(false) => {
                        self.pending_start_confirm = Some(ConfirmStep::Approve);
                        self.status = "Approve required (step 1/2)…".into();
                    }
                    Err(e) => {
                        self.status = format!(
                            "Allowance check failed — {} · retry with F4",
                            e.user_message()
                        );
                    }
                }
            }
            UiJobResult::DexQuote { quote_gen, result } => {
                if self.quote_loading_gen != Some(quote_gen) {
                    return;
                }
                self.quote_loading_gen = None;
                match result {
                    Ok(q) => {
                        self.quote_error = None;
                        self.expected_out = Some(q.amount_out);
                        self.quote_path = Some(q.path);
                        self.quote_hop_fees = Some(q.hop_fees.clone());
                        if let Some(&fee) = q.hop_fees.first() {
                            if fee > 0 {
                                self.fee = fee;
                            }
                        } else if q.fee_tier > 0 {
                            self.fee = q.fee_tier;
                        }
                        if self.min_out.value().trim().is_empty()
                            || self.min_out.value().trim() == "0"
                        {
                            let min =
                                min_out_after_slippage(q.amount_out, DEFAULT_DEX_SLIPPAGE_BPS);
                            self.min_out.set_value(Self::format_swap_token_amount(min));
                        }
                    }
                    Err(e) => {
                        self.expected_out = None;
                        self.quote_path = None;
                        self.quote_hop_fees = None;
                        self.quote_error = Some(e.user_message());
                    }
                }
            }
            _ => {}
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, _wallet: &WalletState, assets: &[Balance]) {
        let [body, status] = body_areas(area);
        match self.stage {
            Stage::Input => self.render_input(frame, body, assets),
            Stage::Confirm(_) => self.render_confirm(frame, body),
            Stage::Done => self.render_done(frame, body),
        }
        let status_text = match self.busy {
            Busy::CheckingAllowance => {
                format!("{} checking router allowance…", spinner_frame(self.tick))
            }
            Busy::EstimatingFee => format!("{} estimating fee…", spinner_frame(self.tick)),
            Busy::Approving => format!("{} approving router spend…", spinner_frame(self.tick)),
            Busy::Swapping => format!("{} broadcasting swap…", spinner_frame(self.tick)),
            Busy::Idle => self.status.clone(),
        };
        frame.render_widget(status_paragraph(&status_text), status);
    }

    fn render_input(&self, frame: &mut Frame, area: Rect, assets: &[Balance]) {
        let show_router = self.venue == DexVenue::Custom;
        let show_v3_fee = self.protocol == DexProtocol::V3;
        let mut constraints = vec![
            Constraint::Length(1), // title
            Constraint::Length(1), // venue
            Constraint::Length(3), // in
            Constraint::Length(1), // ↓
            Constraint::Length(3), // out
            Constraint::Length(3), // amount
            Constraint::Length(1), // expected out
            Constraint::Length(3), // min received
        ];
        if show_v3_fee {
            constraints.push(Constraint::Length(1));
        }
        if show_router {
            constraints.push(Constraint::Length(3));
        }
        constraints.push(Constraint::Length(1)); // footer hint
        constraints.push(Constraint::Min(0));
        let chunks = Layout::vertical(constraints).split(area);
        let mut i = 0;

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " Swap ",
                Style::default()
                    .fg(brand::accent_color())
                    .add_modifier(Modifier::BOLD),
            )))
            .alignment(Alignment::Center),
            chunks[i],
        );
        i += 1;

        let dex_style = if self.focus == Focus::Dex {
            Style::default()
                .fg(brand::accent_color())
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default().fg(brand::body_color())
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("{} · {}  ", self.venue.label(), self.protocol.label()),
                    dex_style,
                ),
                Span::styled(
                    "↑↓ venue · ←→ version",
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
            .alignment(Alignment::Center),
            chunks[i],
        );
        i += 1;

        self.render_swap_token_field(
            frame,
            chunks[i],
            "In",
            &self.token_in,
            self.focus == Focus::TokenIn,
            self.native_in,
            assets,
            self.token_in_editing,
            area.width,
        );
        i += 1;

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "↓",
                Style::default().fg(Color::DarkGray),
            )))
            .alignment(Alignment::Center),
            chunks[i],
        );
        i += 1;

        self.render_swap_token_field(
            frame,
            chunks[i],
            "Out",
            &self.token_out,
            self.focus == Focus::TokenOut,
            false,
            assets,
            self.token_out_editing,
            area.width,
        );
        i += 1;

        self.render_swap_text_field(
            frame,
            chunks[i],
            "Amount",
            &self.amount,
            self.focus == Focus::Amount,
        );
        i += 1;

        self.render_expected_out_line(frame, chunks[i], assets);
        i += 1;

        self.render_min_out_field(frame, chunks[i], assets, self.focus == Focus::MinOut);
        i += 1;

        if show_v3_fee {
            let fee_style = if self.focus == Focus::Fee && !self.v3_auto_fee() {
                Style::default()
                    .fg(brand::accent_color())
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(self.pool_fee_line(), fee_style)))
                    .alignment(Alignment::Center),
                chunks[i],
            );
            i += 1;
        }

        if show_router {
            self.render_swap_text_field(
                frame,
                chunks[i],
                "Router",
                &self.router,
                self.focus == Focus::Router,
            );
            i += 1;
        }

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "Tab · select · Enter · deselect · F4 swap · ↑↓ tokens · Esc · 0.5% slippage",
                Style::default().fg(Color::DarkGray),
            )))
            .alignment(Alignment::Center),
            chunks[i],
        );
    }

    fn field_label_style() -> Style {
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD)
    }

    fn field_label_span(label: &str) -> Span<'static> {
        Span::styled(
            format!("{:<SWAP_LABEL_WIDTH$} ", label),
            Self::field_label_style(),
        )
    }

    /// Short label for overlay on banner-aligned token rows (avoids clobbering the ticker).
    fn field_label_short_span(label: &str) -> Span<'static> {
        Span::styled(format!("{label} "), Self::field_label_style())
    }

    fn format_swap_token_amount(wei: U256) -> String {
        format_display_amount(&wei.to_string(), 18, SWAP_DISPLAY_FRAC)
    }

    fn token_out_symbol<'a>(&self, assets: &'a [Balance]) -> &'a str {
        let raw = self.token_out.value().trim();
        token_symbol_for_address(assets, raw)
            .or_else(|| crate::views::token_symbol_hint(raw, self.chain_id))
            .unwrap_or("tokens")
    }

    /// Label on the left; amount + ticker centred across the full row width.
    fn render_centered_amount_row(
        frame: &mut Frame,
        area: Rect,
        label: &str,
        value: Line<'_>,
        focused: bool,
        bordered: bool,
    ) {
        let inner = if bordered {
            brand::render_field_box(frame, area, focused)
        } else {
            Rect {
                x: area.x.saturating_add(1),
                y: area.y,
                width: area.width.saturating_sub(2),
                height: area.height.max(1),
            }
        };
        frame.render_widget(Paragraph::new(value).alignment(Alignment::Center), inner);
        frame.render_widget(
            Paragraph::new(Line::from(Self::field_label_span(label))),
            Rect {
                x: inner.x,
                y: inner.y,
                width: (SWAP_LABEL_WIDTH + 1) as u16,
                height: 1,
            },
        );
    }

    fn render_expected_out_line(&self, frame: &mut Frame, area: Rect, assets: &[Balance]) {
        let sym = self.token_out_symbol(assets);
        let value_style = Style::default().fg(Color::DarkGray);
        let value = if self.quote_loading_gen.is_some() {
            Line::from(Span::styled(
                format!("{} quoting…", spinner_frame(self.tick)),
                value_style,
            ))
        } else if let Some(out) = self.expected_out {
            Line::from(Span::styled(
                format!("~{}  {sym}", Self::format_swap_token_amount(out)),
                value_style,
            ))
        } else if let Some(err) = &self.quote_error {
            Line::from(Span::styled(format!("— {err}"), value_style))
        } else {
            return;
        };
        Self::render_centered_amount_row(frame, area, "Expected", value, false, false);
    }

    fn render_min_out_field(
        &self,
        frame: &mut Frame,
        area: Rect,
        assets: &[Balance],
        focused: bool,
    ) {
        if focused {
            self.render_swap_text_field(frame, area, "Minimum received", &self.min_out, true);
            return;
        }
        let sym = self.token_out_symbol(assets);
        let token_style = Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);
        let value = if self.min_out.value().trim().is_empty() {
            Line::from(Span::styled(
                self.min_out.placeholder(),
                Style::default().fg(Color::DarkGray),
            ))
        } else if let Ok(wei) = self.parse_min_out() {
            Line::from(Span::styled(
                format!("{}  {sym}", Self::format_swap_token_amount(wei)),
                token_style,
            ))
        } else {
            Line::from(Span::raw(self.min_out.value()))
        };
        Self::render_centered_amount_row(frame, area, "Minimum received", value, false, true);
    }

    pub(crate) fn start_quote_job(&mut self, wallet: &WalletState) -> Option<crate::jobs::UiJob> {
        self.quote_gen += 1;
        let quote_gen = self.quote_gen;
        match self.build_quote_job(wallet, quote_gen) {
            Ok(job) => {
                self.quote_loading_gen = Some(quote_gen);
                self.quote_error = None;
                Some(job)
            }
            Err(msg) => {
                self.expected_out = None;
                self.quote_error = Some(msg);
                None
            }
        }
    }

    fn build_quote_job(
        &self,
        wallet: &WalletState,
        quote_gen: u64,
    ) -> Result<crate::jobs::UiJob, String> {
        if self.venue.unsupported_reason().is_some() {
            return Err("venue not supported for swap".into());
        }
        let router = self.router.value().trim();
        if router.is_empty() {
            return Err("router not set".into());
        }
        let amount_in = self.parse_amount_in()?;
        if amount_in.is_zero() {
            return Err("enter an amount to quote".into());
        }
        let hops = self.parsed_hops()?;
        let path: Vec<String> = hops.iter().map(|a| a.to_string()).collect();
        let token_in = self.resolve_token_in()?;
        let token_out = parse_token_address(self.token_out.value(), "Token out")?;
        let net = wallet.networks().active();
        let (rpc_url, _) = wallet.rpc_endpoints_for(net);
        Ok(crate::jobs::UiJob::DexQuote {
            quote_gen,
            chain_id: self.chain_id,
            rpc_url,
            protocol_v2: self.protocol == DexProtocol::V2,
            router: router.to_string(),
            quoter: (self.protocol == DexProtocol::V3)
                .then(|| venue_quoter_v2(self.venue, self.chain_id))
                .flatten()
                .map(|a| a.to_string()),
            amount_in: amount_in.to_string(),
            fee: self.fee,
            path,
            token_in: token_in.to_string(),
            token_out: token_out.to_string(),
            wpls: self.wpls.map(|a| a.to_string()),
            native_in: self.native_in,
        })
    }

    fn render_swap_text_field(
        &self,
        frame: &mut Frame,
        area: Rect,
        label: &str,
        input: &Input,
        focused: bool,
    ) {
        let inner = brand::render_field_box(frame, area, focused);
        let mut spans = vec![Self::field_label_span(label)];
        if input.value().is_empty() {
            if focused {
                spans.extend(input.line().spans);
            } else {
                spans.push(Span::styled(
                    input.placeholder(),
                    Style::default().fg(Color::DarkGray),
                ));
            }
        } else if focused {
            spans.extend(input.line().spans);
        } else {
            spans.push(Span::raw(input.value()));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), inner);
    }

    /// **In** / **Out** leg: `{SYMBOL} {rainbow contract}` (yellow ticker · Dioxus address colors).
    #[allow(clippy::too_many_arguments)]
    fn render_swap_token_field(
        &self,
        frame: &mut Frame,
        area: Rect,
        label: &str,
        input: &Input,
        focused: bool,
        native_in: bool,
        assets: &[Balance],
        editing: bool,
        screen_width: u16,
    ) {
        let inner = brand::render_field_box(frame, area, focused);

        if focused && editing {
            let mut spans = vec![Self::field_label_span(label)];
            spans.extend(input.line().spans);
            frame.render_widget(Paragraph::new(Line::from(spans)), inner);
            return;
        }

        let token_style = Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);

        if native_in && label == "In" && input.value().trim().is_empty() {
            let sym = native_pls_label(self.chain_id);
            let style = if focused {
                Style::default()
                    .fg(brand::accent_color())
                    .add_modifier(Modifier::BOLD)
            } else {
                token_style
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(sym, style))).alignment(Alignment::Center),
                inner,
            );
            frame.render_widget(
                Paragraph::new(Line::from(Self::field_label_span(label))),
                Rect {
                    x: inner.x,
                    y: inner.y,
                    width: (SWAP_LABEL_WIDTH + 1) as u16,
                    height: 1,
                },
            );
            return;
        }

        let raw = input.value().trim();
        if raw.is_empty() {
            let mut spans = vec![Self::field_label_span(label)];
            if focused {
                spans.extend(input.line().spans);
            } else {
                spans.push(Span::styled(
                    input.placeholder(),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            frame.render_widget(Paragraph::new(Line::from(spans)), inner);
            return;
        }

        let sym = token_symbol_for_address(assets, raw)
            .or_else(|| crate::views::token_symbol_hint(raw, self.chain_id))
            .unwrap_or("???");

        // Banner-aligned ticker + contract inside the box; short label painted on top.
        frame.render_widget(
            Paragraph::new(brand::colored_token_address_under_augha(
                sym,
                raw,
                screen_width,
                inner.x,
            )),
            inner,
        );
        let label_w = (label.chars().count() + 1) as u16;
        frame.render_widget(
            Paragraph::new(Line::from(Self::field_label_short_span(label))),
            Rect {
                x: inner.x,
                y: inner.y,
                width: label_w,
                height: 1,
            },
        );
    }

    /// ↑/↓ on Token in / Token out cycles wallet assets (from chrome asset list).
    pub fn cycle_focused_token_picker(&mut self, assets: &[Balance], forward: bool) -> bool {
        if !matches!(self.stage, Stage::Input) {
            return false;
        }
        let changed = match self.focus {
            Focus::TokenIn => {
                self.token_in_editing = false;
                cycle_token_picker(
                    assets,
                    false,
                    &mut self.token_in_pick,
                    forward,
                    &mut self.native_in,
                    &mut self.token_in,
                    &mut self.status,
                )
            }
            Focus::TokenOut => {
                self.token_out_editing = false;
                cycle_token_picker(
                    assets,
                    true,
                    &mut self.token_out_pick,
                    forward,
                    &mut self.native_in,
                    &mut self.token_out,
                    &mut self.status,
                )
            }
            _ => false,
        };
        if changed {
            self.mark_quote_stale();
        }
        changed
    }

    fn on_focus_left(&mut self, old: Focus) {
        match old {
            Focus::TokenIn => self.token_in_editing = false,
            Focus::TokenOut => self.token_out_editing = false,
            _ => {}
        }
    }

    fn render_confirm(&self, frame: &mut Frame, area: Rect) {
        let title = match self.stage {
            Stage::Confirm(ConfirmStep::Approve) => " Confirm approve (1/2) ",
            Stage::Confirm(ConfirmStep::Swap) => {
                if self.native_in {
                    " Confirm swap "
                } else {
                    " Confirm swap (2/2) "
                }
            }
            _ => " Confirm ",
        };
        let inner = brand::render_faded_box(frame, area, Some(brand::fade_line(title)));
        let mut lines: Vec<Line> = self
            .confirm_lines
            .iter()
            .map(|l| Line::from(l.clone()))
            .collect();

        let fee = self.selected_fee();
        let fee_ref = fee.as_ref();
        let fee_total = fee_ref.map(|f| f.total.as_str()).unwrap_or("—");
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

        lines.push(Line::from(""));
        lines.push(Line::from(format!(
            "Gas (max cap): {fee_total}  [{}]",
            self.speed.label()
        )));
        lines.push(Line::from(format!(
            "          {}",
            fee_detail.as_deref().unwrap_or("—")
        )));
        lines.push(Line::from(""));
        lines.push(Line::from("Gas speed (↑↓ or 1–5):"));

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
        lines.push(speed_line('1', FeeSpeed::Slow));
        lines.push(speed_line('2', FeeSpeed::Normal));
        lines.push(speed_line('3', FeeSpeed::Fast));
        lines.push(speed_line('4', FeeSpeed::Ape));
        lines.push(speed_line('5', FeeSpeed::Custom));

        let custom_editing =
            self.speed == FeeSpeed::Custom && self.confirm_focus == ConfirmFocus::CustomGas;
        if self.speed == FeeSpeed::Custom {
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
            lines.push(Line::from(spans));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Enter / y broadcast · Esc / n back",
            Style::default().fg(brand::accent_color()),
        )));
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
    }

    fn render_done(&self, frame: &mut Frame, area: Rect) {
        let inner = brand::render_faded_box(frame, area, Some(brand::fade_line(" Swap sent ")));
        let mut lines = Vec::new();
        let amount_in = self.parse_amount_in().unwrap_or(U256::ZERO);
        let min_out = self.parse_min_out().unwrap_or(U256::ZERO);
        let in_sym = self.token_in_symbol();
        let out_sym = self.token_out_symbol_str();
        lines.push(Line::from(format!(
            "swapped:  {} {} → {} {}",
            format_base_units(&amount_in.to_string(), 18),
            in_sym,
            format_base_units(&min_out.to_string(), 18),
            out_sym
        )));
        if self.protocol == DexProtocol::V3 {
            lines.push(Line::from(format!(
                "fee:      {}",
                self.fee_tier_confirm_label()
            )));
        }
        lines.push(Line::from(""));
        if let Some(a) = &self.approve_hash {
            lines.push(Line::from(format!("approve:  {a}")));
        }
        let hash = self.tx_hash.as_deref().unwrap_or("(none)");
        lines.push(Line::from(format!("swap:     {hash}")));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Enter new swap · Esc home",
            Style::default().fg(brand::accent_color()),
        )));
        frame.render_widget(Paragraph::new(lines), inner);
    }

    pub fn allows_footer_shortcuts(&self) -> bool {
        match self.stage {
            Stage::Input => match self.focus {
                Focus::None | Focus::Dex | Focus::Fee => true,
                Focus::TokenIn => !self.token_in_editing,
                Focus::TokenOut => !self.token_out_editing,
                Focus::Router | Focus::Amount | Focus::MinOut => false,
            },
            Stage::Confirm(_) => self.confirm_focus != ConfirmFocus::CustomGas,
            Stage::Done => true,
        }
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
            Stage::Input => self.handle_input_key(key, wallet),
            Stage::Confirm(step) => self.handle_confirm_key(key, wallet, step),
            Stage::Done => match key.code {
                KeyCode::Enter => {
                    let chain_id = wallet.networks().active().chain_id;
                    *self = Self::for_chain(chain_id);
                    KeyOutcome::Consumed
                }
                KeyCode::Esc => KeyOutcome::Back,
                _ => KeyOutcome::NotHandled,
            },
        }
    }

    fn v3_auto_fee(&self) -> bool {
        self.protocol == DexProtocol::V3 && deployment_for_chain(self.chain_id).is_some()
    }

    fn pool_fee_line(&self) -> String {
        if self.v3_auto_fee() {
            if self.quote_loading_gen.is_some() {
                return "Pool fee: finding best route…".into();
            }
            if self.expected_out.is_some() {
                if let Some(fees) = &self.quote_hop_fees {
                    if fees.len() > 1 && fees.iter().any(|f| fees.first() != Some(f)) {
                        let parts: Vec<String> =
                            fees.iter().map(|f| fee_tier_display(*f)).collect();
                        return format!("Pool fees: {} (best route)", parts.join(" → "));
                    }
                }
                return format!("Pool fee: {} (best route)", fee_tier_display(self.fee));
            }
            return "Pool fee: auto (best quote)".into();
        }
        format!("Pool fee: {}  (←/→)", fee_tier_display(self.fee))
    }

    fn fee_tiers(&self) -> &'static [u32] {
        if self.chain_id == 943 && self.protocol == DexProtocol::V3 {
            WIZ4RD_FEE_TIERS
        } else {
            FEE_TIERS
        }
    }

    fn cycle_fee(&mut self, forward: bool) {
        let tiers = self.fee_tiers();
        let Some(idx) = tiers.iter().position(|f| *f == self.fee) else {
            self.fee = tiers[0];
            return;
        };
        let next = if forward {
            (idx + 1) % tiers.len()
        } else {
            (idx + tiers.len() - 1) % tiers.len()
        };
        self.fee = tiers[next];
    }

    /// ↑/↓ on the venue row (default focus or venue focused). Called from the app
    /// before the global token picker so DEX selection is not swallowed.
    pub fn cycle_venue_selector(&mut self, forward: bool) -> bool {
        if !matches!(self.stage, Stage::Input) || self.busy != Busy::Idle {
            return false;
        }
        if !matches!(self.focus, Focus::None | Focus::Dex) {
            return false;
        }
        self.venue = if forward {
            self.venue.next()
        } else {
            self.venue.prev()
        };
        if venue_swap_router(self.venue, self.protocol, self.chain_id).is_none()
            && venue_swap_router(self.venue, self.protocol.toggle(), self.chain_id).is_some()
        {
            self.protocol = self.protocol.toggle();
        }
        self.apply_venue_defaults(true);
        self.focus = Focus::Dex;
        if self.venue == DexVenue::Custom {
            self.status = "Paste a Uni V2/V3-compatible router address".into();
        } else if venue_swap_router(self.venue, self.protocol, self.chain_id).is_none() {
            self.status = missing_router_hint(self.venue, self.protocol, self.chain_id);
        } else {
            self.status.clear();
        }
        self.mark_quote_stale();
        true
    }

    fn focus_tab_forward(&self) -> Focus {
        match self.focus {
            Focus::None => Focus::Dex,
            Focus::Dex => Focus::TokenIn,
            Focus::TokenIn => Focus::Amount,
            Focus::Amount => Focus::TokenOut,
            Focus::TokenOut => Focus::MinOut,
            Focus::MinOut => {
                if self.protocol == DexProtocol::V3 && !self.v3_auto_fee() {
                    Focus::Fee
                } else if self.venue == DexVenue::Custom {
                    Focus::Router
                } else {
                    Focus::Dex
                }
            }
            Focus::Fee => {
                if self.venue == DexVenue::Custom {
                    Focus::Router
                } else {
                    Focus::Dex
                }
            }
            Focus::Router => Focus::Dex,
        }
    }

    fn focus_tab_backward(&self) -> Focus {
        match self.focus {
            Focus::None => self.last_tab_focus(),
            Focus::Dex => Focus::None,
            Focus::Router => {
                if self.protocol == DexProtocol::V3 && !self.v3_auto_fee() {
                    Focus::Fee
                } else {
                    Focus::MinOut
                }
            }
            Focus::Fee => Focus::MinOut,
            Focus::MinOut => Focus::TokenOut,
            Focus::TokenOut => Focus::Amount,
            Focus::Amount => Focus::TokenIn,
            Focus::TokenIn => Focus::Dex,
        }
    }

    fn last_tab_focus(&self) -> Focus {
        if self.venue == DexVenue::Custom {
            Focus::Router
        } else if self.protocol == DexProtocol::V3 && !self.v3_auto_fee() {
            Focus::Fee
        } else if self.protocol == DexProtocol::V3 || self.venue == DexVenue::Custom {
            Focus::MinOut
        } else {
            Focus::Dex
        }
    }

    fn deselect_focus(&mut self) {
        let old = self.focus;
        self.on_focus_left(old);
        self.focus = Focus::None;
    }

    fn confirm_swap(&mut self, wallet: &WalletState) -> KeyOutcome {
        match self.validate_fields() {
            Ok(()) => {
                if let Err(msg) = self.require_v3_quote_for_swap() {
                    self.status = msg;
                    return KeyOutcome::Consumed;
                }
                if self.native_in {
                    return self.begin_confirm(wallet, ConfirmStep::Swap);
                }
                let Some(job) = self.build_allowance_check_job(wallet) else {
                    self.status = "Could not start swap — check token, router, and amount".into();
                    return KeyOutcome::Consumed;
                };
                self.busy = Busy::CheckingAllowance;
                self.status = "Checking router allowance…".into();
                KeyOutcome::StartJob(job)
            }
            Err(msg) => {
                self.status = msg;
                KeyOutcome::Consumed
            }
        }
    }

    fn build_allowance_check_job(&self, wallet: &WalletState) -> Option<crate::jobs::UiJob> {
        let token_in = self.resolve_token_in().ok()?;
        let router = Address::from_str(self.router.value().trim()).ok()?;
        let amount_in = self.parse_amount_in().ok()?;
        let owner = wallet.active_address().ok()?.to_string();
        let (rpc_url, _) = wallet.rpc_endpoints_for(wallet.networks().active());
        Some(crate::jobs::UiJob::DexAllowanceCheck {
            rpc_url,
            token_in: token_in.to_string(),
            owner,
            router: router.to_string(),
            amount_in: amount_in.to_string(),
        })
    }

    /// V3 swaps need a live pool quote — router simulation reverts without liquidity.
    fn require_v3_quote_for_swap(&self) -> Result<(), String> {
        if self.protocol != DexProtocol::V3 {
            return Ok(());
        }
        if let Some(err) = &self.quote_error {
            return Err(format!("No swap quote — {err}"));
        }
        if self.expected_out.is_none() {
            return Err(
                "Waiting for swap quote — enter amount and wait for Expected to fill".into(),
            );
        }
        Ok(())
    }

    fn handle_input_key(&mut self, key: KeyEvent, wallet: &WalletState) -> KeyOutcome {
        match key.code {
            KeyCode::Esc => {
                if self.focus != Focus::None {
                    self.deselect_focus();
                    KeyOutcome::Consumed
                } else {
                    KeyOutcome::Back
                }
            }
            KeyCode::Up | KeyCode::Down => {
                if self.cycle_venue_selector(matches!(key.code, KeyCode::Down)) {
                    KeyOutcome::Consumed
                } else {
                    KeyOutcome::NotHandled
                }
            }
            KeyCode::Left | KeyCode::Right => {
                let forward = matches!(key.code, KeyCode::Right);
                match self.focus {
                    Focus::Dex => {
                        self.protocol = self.protocol.toggle();
                        let overwrite = self.venue != DexVenue::Custom;
                        self.apply_venue_defaults(overwrite);
                        if self.venue != DexVenue::Custom && self.focus == Focus::Router {
                            self.focus = Focus::Dex;
                        }
                        if self.venue == DexVenue::Custom {
                            self.status = "Paste a Uni V2/V3-compatible router address".into();
                        } else if venue_swap_router(self.venue, self.protocol, self.chain_id)
                            .is_none()
                        {
                            self.status =
                                missing_router_hint(self.venue, self.protocol, self.chain_id);
                        } else {
                            self.status.clear();
                        }
                        self.mark_quote_stale();
                        KeyOutcome::Consumed
                    }
                    Focus::Fee if self.protocol == DexProtocol::V3 && !self.v3_auto_fee() => {
                        self.cycle_fee(forward);
                        self.mark_quote_stale();
                        KeyOutcome::Consumed
                    }
                    Focus::Router
                    | Focus::TokenIn
                    | Focus::TokenOut
                    | Focus::Amount
                    | Focus::MinOut => {
                        let input = match self.focus {
                            Focus::Router => &mut self.router,
                            Focus::TokenIn => &mut self.token_in,
                            Focus::TokenOut => &mut self.token_out,
                            Focus::Amount => &mut self.amount,
                            Focus::MinOut => &mut self.min_out,
                            Focus::Dex | Focus::Fee | Focus::None => unreachable!(),
                        };
                        match input.handle_key(key) {
                            InputAction::Ignored => KeyOutcome::NotHandled,
                            InputAction::Consumed | InputAction::Submitted => {
                                if manual_edit_resets_token_pick(key.code) {
                                    match self.focus {
                                        Focus::TokenIn => {
                                            self.token_in_pick = TOKEN_PICK_UNINIT;
                                            self.token_in_editing = true;
                                        }
                                        Focus::TokenOut => {
                                            self.token_out_pick = TOKEN_PICK_UNINIT;
                                            self.token_out_editing = true;
                                        }
                                        _ => {}
                                    }
                                }
                                self.mark_quote_stale();
                                KeyOutcome::Consumed
                            }
                        }
                    }
                    _ => KeyOutcome::Consumed,
                }
            }
            KeyCode::Tab => {
                let old = self.focus;
                self.focus = self.focus_tab_forward();
                self.on_focus_left(old);
                KeyOutcome::Consumed
            }
            KeyCode::BackTab => {
                let old = self.focus;
                self.focus = self.focus_tab_backward();
                self.on_focus_left(old);
                KeyOutcome::Consumed
            }
            KeyCode::Char(' ') if self.focus == Focus::TokenIn => {
                self.native_in = !self.native_in;
                self.token_in_editing = false;
                if self.native_in {
                    self.token_in.set_value("");
                    self.token_in_pick = TOKEN_PICK_UNINIT;
                } else {
                    self.token_in_pick = TOKEN_PICK_UNINIT;
                }
                self.mark_quote_stale();
                KeyOutcome::Consumed
            }
            KeyCode::F(4) => self.confirm_swap(wallet),
            KeyCode::Enter if self.focus != Focus::None => {
                self.deselect_focus();
                KeyOutcome::Consumed
            }
            KeyCode::Enter => KeyOutcome::NotHandled,
            _ => {
                if matches!(self.focus, Focus::None | Focus::Dex | Focus::Fee) {
                    return if self.focus == Focus::None {
                        KeyOutcome::NotHandled
                    } else {
                        KeyOutcome::Consumed
                    };
                }
                let (input, pick) = match self.focus {
                    Focus::Router => (&mut self.router, None),
                    Focus::TokenIn => (&mut self.token_in, Some(&mut self.token_in_pick)),
                    Focus::TokenOut => (&mut self.token_out, Some(&mut self.token_out_pick)),
                    Focus::Amount => (&mut self.amount, None),
                    Focus::MinOut => (&mut self.min_out, None),
                    Focus::None | Focus::Dex | Focus::Fee => unreachable!(),
                };
                match input.handle_key(key) {
                    InputAction::Ignored => KeyOutcome::NotHandled,
                    InputAction::Submitted => {
                        self.deselect_focus();
                        KeyOutcome::Consumed
                    }
                    InputAction::Consumed => {
                        if let Some(p) = pick {
                            if manual_edit_resets_token_pick(key.code) {
                                *p = TOKEN_PICK_UNINIT;
                                match self.focus {
                                    Focus::TokenIn => self.token_in_editing = true,
                                    Focus::TokenOut => self.token_out_editing = true,
                                    _ => {}
                                }
                            }
                        }
                        self.mark_quote_stale();
                        KeyOutcome::Consumed
                    }
                }
            }
        }
    }

    fn begin_confirm(&mut self, wallet: &WalletState, step: ConfirmStep) -> KeyOutcome {
        match self.prepare_confirm_job(wallet, step) {
            Ok(job) => KeyOutcome::StartJob(job),
            Err(msg) => {
                self.status = msg;
                KeyOutcome::Consumed
            }
        }
    }

    fn prepare_confirm_job(
        &mut self,
        wallet: &WalletState,
        step: ConfirmStep,
    ) -> Result<crate::jobs::UiJob, String> {
        self.enter_confirm_lines(step);
        let tx = match step {
            ConfirmStep::Approve => self.build_approve_tx(wallet)?,
            ConfirmStep::Swap => self.build_swap_tx(wallet)?,
        };
        self.pending_step = Some(step);
        self.pending_tx = Some(tx.clone());
        self.base_fee = None;
        self.speed = FeeSpeed::Normal;
        self.confirm_focus = ConfirmFocus::Speed;
        self.custom_gas.set_value("");
        self.stage = Stage::Confirm(step);
        self.busy = Busy::EstimatingFee;
        self.status.clear();
        Ok(crate::jobs::UiJob::EstimateEvmFee { tx })
    }

    fn format_amount_line(&self, label: &str, wei: U256, suffix: &str) -> String {
        let human = format_base_units(&wei.to_string(), 18);
        format!("{label:<9} {human} {suffix}")
    }

    fn token_in_symbol(&self) -> String {
        let raw = self.token_in.value().trim();
        token_symbol_for_address(&[], raw)
            .or_else(|| crate::views::token_symbol_hint(raw, self.chain_id))
            .unwrap_or("TOKEN")
            .to_string()
    }

    fn token_out_symbol_str(&self) -> String {
        let raw = self.token_out.value().trim();
        token_symbol_for_address(&[], raw)
            .or_else(|| crate::views::token_symbol_hint(raw, self.chain_id))
            .unwrap_or("TOKEN")
            .to_string()
    }

    fn hop_display(&self, hops: &[Address]) -> String {
        hops.iter()
            .map(|a| {
                let raw = format!("{a:#x}");
                token_symbol_for_address(&[], &raw)
                    .or_else(|| crate::views::token_symbol_hint(&raw, self.chain_id))
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("{a:#x}"))
            })
            .collect::<Vec<_>>()
            .join(" → ")
    }

    fn fee_tier_confirm_label(&self) -> String {
        format!("{} ({})", fee_tier_display(self.fee), self.fee)
    }

    fn enter_confirm_lines(&mut self, step: ConfirmStep) {
        let hops = self
            .parsed_hops()
            .map(|p| self.hop_display(&p))
            .unwrap_or_else(|_| "(invalid path)".into());

        let amount_in = self.parse_amount_in().unwrap_or(U256::ZERO);
        let min_out = self.parse_min_out().unwrap_or(U256::ZERO);
        let token_in_sym = self.token_in_symbol();
        let token_out_sym = self.token_out_symbol_str();
        let token_in_addr = self
            .resolve_token_in()
            .map(|a| format!("{a:#x}"))
            .unwrap_or_else(|_| self.token_in.value().trim().to_string());

        self.confirm_lines = match step {
            ConfirmStep::Approve => vec![
                format!(
                    "{} {} token→token: approve then swap",
                    self.venue.label(),
                    self.protocol.label()
                ),
                String::new(),
                format!("token:   {token_in_sym} ({token_in_addr})"),
                format!("spender: {}", self.router.value().trim()),
                self.format_amount_line("amount:", amount_in, &token_in_sym),
                format!("path:    {hops}"),
            ],
            ConfirmStep::Swap => {
                let mut lines = vec![
                    format!(
                        "DEX:      {} ({})",
                        self.venue.label(),
                        self.protocol.label()
                    ),
                    format!("router:   {}", self.router.value().trim()),
                    format!("path:     {hops}"),
                ];
                if self.protocol == DexProtocol::V3 {
                    lines.push(format!("fee tier: {}", self.fee_tier_confirm_label()));
                }
                lines.push(self.format_amount_line(
                    "amount:",
                    amount_in,
                    if self.native_in {
                        native_pls_label(self.chain_id)
                    } else {
                        &token_in_sym
                    },
                ));
                lines.push(self.format_amount_line("min out:", min_out, &token_out_sym));
                if self.native_in {
                    lines.push(format!(
                        "value:    {} {}  (attached to tx, not gas)",
                        format_base_units(&amount_in.to_string(), 18),
                        native_pls_label(self.chain_id)
                    ));
                }
                lines.push(format!(
                    "mode:     {}",
                    if self.native_in {
                        "native → token"
                    } else {
                        "token → token"
                    }
                ));
                if let Some(a) = &self.approve_hash {
                    lines.push(format!("approve:  {a}"));
                }
                lines
            }
        };
        if matches!(step, ConfirmStep::Approve) {
            self.status.clear();
        }
    }

    fn selected_fee(&self) -> Option<Fee> {
        let base = self.base_fee.as_ref()?;
        match self.speed {
            FeeSpeed::Custom => base.with_custom_max_fee_gwei(self.custom_gas.value()).ok(),
            speed => Some(base.with_speed(speed)),
        }
    }

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

    fn begin_broadcast(&mut self, step: ConfirmStep) -> KeyOutcome {
        let fee = match self.speed {
            FeeSpeed::Custom => match self
                .base_fee
                .as_ref()
                .map(|f| f.with_custom_max_fee_gwei(self.custom_gas.value()))
            {
                Some(Ok(f)) => f,
                Some(Err(e)) => {
                    self.status = e;
                    self.confirm_focus = ConfirmFocus::CustomGas;
                    return KeyOutcome::Consumed;
                }
                None => {
                    self.status = "fee estimate missing — Esc back and retry".into();
                    return KeyOutcome::Consumed;
                }
            },
            _ => {
                let Some(fee) = self.selected_fee() else {
                    self.status = "fee estimate missing — Esc back and retry".into();
                    return KeyOutcome::Consumed;
                };
                fee
            }
        };
        let Some(tx) = self.pending_tx.take() else {
            self.status = "transaction payload missing — Esc back and retry".into();
            return KeyOutcome::Consumed;
        };
        self.base_fee = None;
        self.busy = match step {
            ConfirmStep::Approve => Busy::Approving,
            ConfirmStep::Swap => Busy::Swapping,
        };
        self.status.clear();
        KeyOutcome::StartJob(crate::jobs::UiJob::SendEvmWithFee { tx, fee })
    }

    fn handle_confirm_key(
        &mut self,
        key: KeyEvent,
        _wallet: &mut WalletState,
        step: ConfirmStep,
    ) -> KeyOutcome {
        match key.code {
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                if self.confirm_focus == ConfirmFocus::CustomGas {
                    self.confirm_focus = ConfirmFocus::Speed;
                    return KeyOutcome::Consumed;
                }
                self.stage = Stage::Input;
                self.confirm_focus = ConfirmFocus::Speed;
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
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                if self.speed == FeeSpeed::Custom && self.confirm_focus == ConfirmFocus::CustomGas {
                    match self
                        .base_fee
                        .as_ref()
                        .map(|f| f.with_custom_max_fee_gwei(self.custom_gas.value()))
                    {
                        Some(Ok(_)) => self.begin_broadcast(step),
                        Some(Err(e)) => {
                            self.status = e;
                            KeyOutcome::Consumed
                        }
                        None => {
                            self.status = "fee estimate missing — Esc back and retry".into();
                            KeyOutcome::Consumed
                        }
                    }
                } else {
                    self.begin_broadcast(step)
                }
            }
            _ if self.confirm_focus == ConfirmFocus::CustomGas => {
                match self.custom_gas.handle_key(key) {
                    InputAction::Ignored => KeyOutcome::NotHandled,
                    InputAction::Submitted => self.begin_broadcast(step),
                    InputAction::Consumed => KeyOutcome::Consumed,
                }
            }
            _ => KeyOutcome::Consumed,
        }
    }

    fn resolve_token_in(&self) -> Result<Address, String> {
        if self.native_in {
            self.wpls
                .ok_or_else(|| "native→token needs WPLS on PulseChain — switch network".into())
        } else {
            Address::from_str(self.token_in.value().trim()).map_err(|e| format!("Token in: {e}"))
        }
    }

    fn parsed_hops(&self) -> Result<Vec<Address>, String> {
        if self.protocol == DexProtocol::V3 {
            if let Some(path) = &self.quote_path {
                return Ok(path.clone());
            }
        }
        let token_in = self.resolve_token_in()?;
        let token_out = parse_token_address(self.token_out.value(), "Token out")?;
        hop_tokens(token_in, token_out, self.wpls, self.native_in)
    }

    fn validate_fields(&self) -> Result<(), String> {
        let router = Address::from_str(self.router.value().trim())
            .map_err(|e| format!("bad router: {e}"))?;
        if self.venue != DexVenue::Custom && !is_allowed_dex_router(self.chain_id, router) {
            return Err(format!(
                "router {router:#x} is not in Vaughan’s curated DEX catalog for {} — \
                 use Custom to paste an unlisted router",
                chain_label(self.chain_id)
            ));
        }
        let hops = self.parsed_hops()?;
        if self.protocol == DexProtocol::V3 {
            let fees = self
                .quote_hop_fees
                .clone()
                .unwrap_or_else(|| vec![self.fee; hops.len().saturating_sub(1)]);
            if fees.len() != hops.len().saturating_sub(1) {
                return Err("quote hop fees out of sync — wait for Expected quote".into());
            }
            let _ = encode_v3_path_hops(&hops, &fees)?;
        }
        let amount_in = self.parse_amount_in()?;
        let _min_out = self.parse_min_out()?;
        if amount_in.is_zero() {
            return Err("amount must be > 0".into());
        }
        Ok(())
    }

    fn parse_amount_in(&self) -> Result<U256, String> {
        parse_swap_amount(self.amount.value(), "amount", 18)
    }

    fn parse_min_out(&self) -> Result<U256, String> {
        parse_min_out_amount(self.min_out.value(), "min out", 18)
    }

    fn build_approve_tx(&self, wallet: &WalletState) -> Result<EvmTransaction, String> {
        self.validate_fields()?;
        let router = Address::from_str(self.router.value().trim())
            .map_err(|e| format!("bad router: {e}"))?;
        let token_in = self.resolve_token_in()?;
        let amount_in = self.parse_amount_in()?;
        let from = wallet.active_address().map_err(|e| e.user_message())?;
        let chain_id = wallet.networks().active().chain_id;
        Ok(build_approve_tx(
            token_in, router, amount_in, from, chain_id,
        ))
    }

    fn build_swap_tx(&self, wallet: &WalletState) -> Result<EvmTransaction, String> {
        self.validate_fields()?;
        let router = Address::from_str(self.router.value().trim())
            .map_err(|e| format!("bad router: {e}"))?;
        let token_in = self.resolve_token_in()?;
        let token_out = parse_token_address(self.token_out.value(), "Token out")?;
        let amount_in = self.parse_amount_in()?;
        let min_out = self.parse_min_out()?;
        let to_addr = wallet.active_address().map_err(|e| e.user_message())?;
        let recipient = Address::from_str(to_addr).map_err(|e| format!("bad account: {e}"))?;
        let chain_id = wallet.networks().active().chain_id;

        build_swap_tx(&DexSwapRequest {
            protocol: self.protocol,
            router,
            token_in,
            token_out,
            wpls: self.wpls,
            native_in: self.native_in,
            amount_in,
            min_out,
            fee: self.fee,
            hop_fees: self.quote_hop_fees.clone(),
            recipient,
            from: to_addr.to_string(),
            chain_id,
            hops: self.parsed_hops().ok(),
        })
    }

    /// After approve broadcast or allowance check, queue the next Dex job.
    pub fn followup_job(&mut self, wallet: &WalletState) -> Option<crate::jobs::UiJob> {
        if let Some(step) = self.pending_start_confirm.take() {
            match self.prepare_confirm_job(wallet, step) {
                Ok(job) => return Some(job),
                Err(e) => {
                    self.stage = Stage::Input;
                    self.status = e;
                    return None;
                }
            }
        }
        if !self.pending_auto_fee_estimate {
            return None;
        }
        self.pending_auto_fee_estimate = false;
        let step = ConfirmStep::Swap;
        self.enter_confirm_lines(step);
        let tx = match self.build_swap_tx(wallet) {
            Ok(tx) => tx,
            Err(e) => {
                self.stage = Stage::Input;
                self.pending_step = None;
                self.pending_tx = None;
                self.status = format!("Swap prep failed after approve: {e}");
                return None;
            }
        };
        let token_in = match self.resolve_token_in() {
            Ok(a) => a,
            Err(e) => {
                self.stage = Stage::Input;
                self.status = e;
                return None;
            }
        };
        let router = match Address::from_str(self.router.value().trim()) {
            Ok(a) => a,
            Err(e) => {
                self.stage = Stage::Input;
                self.status = format!("bad router: {e}");
                return None;
            }
        };
        let amount_in = match self.parse_amount_in() {
            Ok(a) => a,
            Err(e) => {
                self.stage = Stage::Input;
                self.status = e;
                return None;
            }
        };
        let owner = match wallet.active_address() {
            Ok(a) => a.to_string(),
            Err(e) => {
                self.stage = Stage::Input;
                self.status = e.user_message();
                return None;
            }
        };
        let (rpc_url, _) = wallet.rpc_endpoints_for(wallet.networks().active());
        self.pending_step = Some(step);
        self.pending_tx = Some(tx.clone());
        self.base_fee = None;
        self.speed = FeeSpeed::Normal;
        self.confirm_focus = ConfirmFocus::Speed;
        self.custom_gas.set_value("");
        self.stage = Stage::Confirm(step);
        self.busy = Busy::EstimatingFee;
        self.status = "Waiting for approve on chain…".into();
        Some(crate::jobs::UiJob::DexSwapEstimateAfterApprove {
            rpc_url,
            token_in: token_in.to_string(),
            owner,
            router: router.to_string(),
            amount_in: amount_in.to_string(),
            tx,
        })
    }
}

/// Actionable copy when `eth_estimateGas` reverts (common on bad min-out or balance).
fn dex_fee_estimate_error(err: &WalletError) -> String {
    match err {
        WalletError::GasEstimationFailed(detail) => {
            let lower = detail.to_ascii_lowercase();
            if lower.contains("too little received") {
                return "Swap would revert: min receive is too high for this amount — set Min receive to 0 or lower it."
                    .into();
            }
            if lower.contains("insufficient funds") || lower.contains("insufficient balance") {
                return "Swap would revert: not enough tPLS/PLS for swap value + gas — lower amount or fund the wallet."
                    .into();
            }
            if detail.contains("data: \"0x\"") || detail.trim() == "execution reverted" {
                return "Swap would revert: no usable V3 liquidity for this pair. \
                        Add LP on this pair or try a smaller amount."
                    .into();
            }
            format!(
                "Could not estimate swap gas (router simulation reverted). \
                 Add LP, set Min receive to 0, or lower amount. ({detail})"
            )
        }
        _ => err.user_message(),
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

/// Human-readable V3 pool fee (500 → 0.05%).
fn fee_tier_display(fee: u32) -> String {
    let pct = fee as f64 / 10_000.0;
    if pct >= 1.0 {
        format!("{pct:.1}%")
    } else {
        format!("{pct:.2}%")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_requires_positive_amount() {
        let mut v = DexView::for_chain(943);
        v.amount.set_value("0");
        let err = v.validate_fields().unwrap_err();
        assert!(err.contains("must be > 0"), "{err}");
    }

    #[test]
    fn dex_fee_estimate_error_surfaces_too_little_received() {
        let msg = dex_fee_estimate_error(&WalletError::GasEstimationFailed(
            "execution reverted: Too little received".into(),
        ));
        assert!(msg.contains("min receive"), "{msg}");
    }

    #[test]
    fn native_pls_label_by_chain() {
        assert_eq!(native_pls_label(943), "tPLS");
        assert_eq!(native_pls_label(369), "PLS");
    }

    #[test]
    fn fee_tier_display_formats_percent() {
        assert_eq!(fee_tier_display(500), "0.05%");
        assert_eq!(fee_tier_display(20_000), "2.0%");
    }

    #[test]
    fn fee_tier_confirm_label_includes_bps() {
        let v = DexView::for_chain(943);
        assert_eq!(v.fee_tier_confirm_label(), "0.05% (500)");
        let mut v2 = DexView::for_chain(943);
        v2.fee = 20_000;
        assert_eq!(v2.fee_tier_confirm_label(), "2.0% (20000)");
    }

    #[test]
    fn up_down_cycles_venue_from_default_focus() {
        let mut v = DexView::for_chain(369);
        let start = v.venue;
        assert!(v.cycle_venue_selector(true));
        assert_ne!(v.venue, start);
        assert_eq!(v.focus, Focus::Dex);
    }

    #[test]
    fn tab_from_none_focuses_venue_row_first() {
        let v = DexView::for_chain(369);
        assert_eq!(v.focus, Focus::None);
        assert_eq!(v.focus_tab_forward(), Focus::Dex);
    }
}
