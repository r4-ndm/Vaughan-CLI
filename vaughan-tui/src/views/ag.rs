//! Aggregator (Ag) view — single-venue quotes by default; optional compare-all.
//!
//! ↑/↓ pick venue or Compare all · Tab through fields · F4 quote.

use alloy::primitives::Address;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use std::collections::HashSet;
use std::str::FromStr;
use tokio::runtime::Handle;
use vaughan_core::chains::Balance;
use vaughan_core::chains::EvmTransaction;
use vaughan_core::core::{
    rank_agg_quote_outcomes, AggAccess, AggQuote, AggQuoteOutcome, AggVenue, WalletState,
};
use vaughan_provider::EventBus;

use crate::app::{KeyOutcome, Screen};
use crate::brand;
use crate::input::{Input, InputAction};
use crate::jobs::{spinner_frame, UiJob, UiJobResult};
use crate::views::dex_calldata::build_approve_tx;
use crate::views::swap_form::{
    fmt_swap_wei_amount, render_form_footer, render_form_title, render_leg_arrow,
    render_plain_confirm, render_selector_line, render_text_field, render_token_field,
    token_display_symbol,
};
use crate::views::{
    body_areas, cycle_token_picker, manual_edit_resets_token_pick, parse_swap_amount,
    parse_token_address, status_paragraph, TOKEN_PICK_UNINIT,
};

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
    ComparePick,
    Confirm(ConfirmStep),
    Done,
}

/// Quote target on the venue row — one aggregator or compare-all.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum VenueChoice {
    Single(AggVenue),
    CompareAll,
}

impl VenueChoice {
    /// ↑/↓ order: live aggregators, Compare all adjacent after Piteas.
    const TARGETS: &'static [VenueChoice] = &[
        VenueChoice::Single(AggVenue::SquirrelSwap),
        VenueChoice::Single(AggVenue::PulseSwap),
        VenueChoice::Single(AggVenue::Piteas),
        VenueChoice::CompareAll,
        VenueChoice::Single(AggVenue::Empseal),
    ];

    fn next(self) -> Self {
        let idx = Self::TARGETS.iter().position(|&c| c == self).unwrap_or(0);
        Self::TARGETS[(idx + 1) % Self::TARGETS.len()]
    }

    fn prev(self) -> Self {
        let idx = Self::TARGETS.iter().position(|&c| c == self).unwrap_or(0);
        let n = Self::TARGETS.len();
        Self::TARGETS[(idx + n - 1) % n]
    }

    fn label(self) -> &'static str {
        match self {
            Self::CompareAll => "Compare all",
            Self::Single(v) => v.label(),
        }
    }

    fn badge(self) -> &'static str {
        match self {
            Self::CompareAll => "LIVE",
            Self::Single(v) if v.is_live() => "LIVE",
            Self::Single(_) => "listed",
        }
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Focus {
    None,
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
    venue_choice: VenueChoice,
    token_in: Input,
    token_out: Input,
    token_in_pick: usize,
    token_out_pick: usize,
    token_in_editing: bool,
    token_out_editing: bool,
    amount: Input,
    slippage: Input,
    native_in: bool,
    chain_id: u64,
    busy: Busy,
    tick: u64,
    status: String,
    quote: Option<AggQuote>,
    /// Last parallel compare pass (every live venue).
    compare: Vec<AggQuoteOutcome>,
    /// Indices into [`Self::compare`] with ok quotes, best `amount_out` first.
    compare_ranked: Vec<usize>,
    compare_pick: usize,
    tx_hash: Option<String>,
    approve_hash: Option<String>,
}

impl Default for AgView {
    fn default() -> Self {
        Self {
            stage: Stage::Input,
            focus: Focus::None,
            venue_choice: VenueChoice::Single(AggVenue::SquirrelSwap),
            token_in: Input::new(false, "↑↓ pick · paste · Space = native PLS"),
            token_out: Input::new(false, "↑↓ pick · paste (e.g. PLSX)"),
            token_in_pick: TOKEN_PICK_UNINIT,
            token_out_pick: TOKEN_PICK_UNINIT,
            token_in_editing: false,
            token_out_editing: false,
            amount: Input::new(false, "amount (e.g. 1 or 0.01)"),
            slippage: Input::new(false, "0.5"),
            native_in: true,
            chain_id: 0,
            busy: Busy::Idle,
            tick: 0,
            status: String::new(),
            quote: None,
            compare: Vec::new(),
            compare_ranked: Vec::new(),
            compare_pick: 0,
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
        if let Some(out) = token_out.filter(|s| !s.trim().is_empty()) {
            v.token_out.set_value(out.trim().to_string());
        }
        if chain_id == 369 {
            v.refresh_venue_status();
        } else {
            v.status = match chain_id {
                943 => "Aggregators are mainnet (369) — switch Net or use Dex on testnet".into(),
                _ => "PulseChain aggregators need chain 369".into(),
            };
        }
        v
    }

    fn refresh_venue_status(&mut self) {
        if self.chain_id != 369 {
            return;
        }
        self.status = match self.venue_choice {
            VenueChoice::CompareAll => "Compare all live aggregators — F4 or Enter to quote".into(),
            VenueChoice::Single(v) => match v.access() {
                AggAccess::LiveNoKey => {
                    format!("{} · {} — F4 or Enter to quote", v.label(), v.blurb())
                }
                AggAccess::NeedsApiKey(why) | AggAccess::ListedOnly(why) => {
                    format!("{} — {} ({why})", v.label(), v.blurb())
                }
            },
        };
    }

    fn clear_quotes(&mut self) {
        self.quote = None;
        self.compare.clear();
        self.compare_ranked.clear();
        self.compare_pick = 0;
    }

    fn select_compare_pick(&mut self, pick: usize) {
        self.compare_pick = pick;
        if let Some(&idx) = self.compare_ranked.get(pick) {
            if let Ok(q) = &self.compare[idx].result {
                self.quote = Some(q.clone());
                self.venue_choice = VenueChoice::Single(q.venue);
            }
        }
    }

    fn refresh_compare_status(&mut self, assets: &[Balance]) {
        let Some(q) = self.quote.as_ref() else {
            return;
        };
        let out_sym = token_display_symbol(false, &self.token_out, assets, self.chain_id);
        let out_amt = fmt_swap_wei_amount(&q.amount_out, 18);
        let ok = self.compare_ranked.len();
        self.status = format!(
            "{ok}/{} quotes · ~{out_amt} {out_sym} via {} — ↑↓ pick · Enter confirm",
            self.compare.len(),
            q.venue.label(),
        );
    }

    fn apply_compare_results(&mut self, outcomes: Vec<AggQuoteOutcome>, assets: &[Balance]) {
        self.busy = Busy::Idle;
        self.compare = outcomes;
        self.compare_ranked = rank_agg_quote_outcomes(&self.compare);
        if self.compare_ranked.is_empty() {
            self.quote = None;
            self.stage = Stage::Input;
            self.status = "No aggregator returned a route — check pair / amount".into();
            return;
        }
        self.select_compare_pick(0);
        self.stage = Stage::ComparePick;
        self.refresh_compare_status(assets);
    }

    pub fn set_tick(&mut self, tick: u64) {
        self.tick = tick;
    }

    pub fn apply_job_result(&mut self, result: UiJobResult) {
        match result {
            UiJobResult::AggCompareQuote(outcomes) => {
                self.apply_compare_results(outcomes, &[]);
            }
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

    pub fn render(&self, frame: &mut Frame, area: Rect, _wallet: &WalletState, assets: &[Balance]) {
        let [body, status] = body_areas(area);
        match self.stage {
            Stage::Input => self.render_input(frame, body, assets),
            Stage::ComparePick => self.render_compare_pick(frame, body, assets),
            Stage::Confirm(_) => self.render_confirm(frame, body, assets),
            Stage::Done => self.render_done(frame, body),
        }
        let status_text = match self.busy {
            Busy::Quoting => format!("{} quoting…", spinner_frame(self.tick)),
            Busy::Approving => format!("{} approving…", spinner_frame(self.tick)),
            Busy::Swapping => format!("{} swapping…", spinner_frame(self.tick)),
            Busy::Idle => self.status.clone(),
        };
        frame.render_widget(status_paragraph(&status_text), status);
    }

    fn render_input(&self, frame: &mut Frame, area: Rect, assets: &[Balance]) {
        let chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(area);
        let mut i = 0;

        render_form_title(frame, chunks[i], " Ag ");
        i += 1;

        let (primary, hint) = (
            Line::from(format!(
                "{} [{}]",
                self.venue_choice.label(),
                self.venue_choice.badge()
            )),
            "↑↓ venue · Tab next",
        );
        render_selector_line(frame, chunks[i], primary, hint, self.focus == Focus::Venue);
        i += 1;

        render_token_field(
            frame,
            chunks[i],
            "In",
            &self.token_in,
            self.focus == Focus::TokenIn,
            self.native_in,
            assets,
            self.token_in_editing,
            area.width,
            self.chain_id,
        );
        i += 1;

        render_leg_arrow(frame, chunks[i]);
        i += 1;

        render_token_field(
            frame,
            chunks[i],
            "Out",
            &self.token_out,
            self.focus == Focus::TokenOut,
            false,
            assets,
            self.token_out_editing,
            area.width,
            self.chain_id,
        );
        i += 1;

        render_text_field(
            frame,
            chunks[i],
            "Amount",
            &self.amount,
            self.focus == Focus::Amount,
        );
        i += 1;

        render_text_field(
            frame,
            chunks[i],
            "Slippage",
            &self.slippage,
            self.focus == Focus::Slippage,
        );
        i += 1;

        render_form_footer(
            frame,
            chunks[i],
            "Tab · select field · ↑↓ venue · F4 quote · Space native in",
        );
    }

    fn render_compare_pick(&self, frame: &mut Frame, area: Rect, assets: &[Balance]) {
        let body = self.build_compare_pick_lines(assets);
        render_plain_confirm(
            frame,
            area,
            " Compare ",
            body,
            "↑↓ pick · Enter confirm · Esc back",
        );
    }

    fn build_compare_pick_lines(&self, assets: &[Balance]) -> Vec<Line<'static>> {
        let out_sym = token_display_symbol(false, &self.token_out, assets, self.chain_id);
        let ranked_set: HashSet<usize> = self.compare_ranked.iter().copied().collect();
        let mut lines = vec![
            Line::from(format!(
                "{}/{} aggregators returned a route",
                self.compare_ranked.len(),
                self.compare.len()
            )),
            Line::from(""),
        ];

        for (pick_i, &idx) in self.compare_ranked.iter().enumerate() {
            let o = &self.compare[idx];
            let Some(q) = o.result.as_ref().ok() else {
                continue;
            };
            let amt = fmt_swap_wei_amount(&q.amount_out, 18);
            let marker = if pick_i == self.compare_pick {
                "★"
            } else {
                " "
            };
            let mut style = Style::default();
            if pick_i == self.compare_pick {
                style = style.fg(brand::accent_color()).add_modifier(Modifier::BOLD);
            }
            lines.push(Line::from(Span::styled(
                format!("{marker} {:<10} ~{amt} {out_sym}", o.venue.label()),
                style,
            )));
        }

        for (idx, o) in self.compare.iter().enumerate() {
            if ranked_set.contains(&idx) {
                continue;
            }
            let msg = o
                .result
                .as_ref()
                .err()
                .map(|e| e.user_message())
                .unwrap_or_else(|| "no route".into());
            let short = truncate_chars(&msg, 40);
            lines.push(Line::from(Span::styled(
                format!("  {:<10} {short}", o.venue.label()),
                Style::default().fg(Color::DarkGray),
            )));
        }
        lines
    }

    fn render_done(&self, frame: &mut Frame, area: Rect) {
        let inner = brand::render_faded_box(frame, area, Some(brand::fade_line(" Ag sent ")));
        let mut lines = Vec::new();
        if let Some(a) = &self.approve_hash {
            lines.push(Line::from(format!("approve: {a}")));
        }
        let hash = self.tx_hash.as_deref().unwrap_or("(none)");
        lines.push(Line::from(format!("swap:    {hash}")));
        lines.push(Line::from(""));
        lines.push(Line::from("Enter new quote · Esc home"));
        frame.render_widget(Paragraph::new(lines), inner);
    }

    /// ↑/↓ on In / Out cycles wallet assets (from chrome asset list).
    pub fn cycle_focused_token_picker(&mut self, assets: &[Balance], forward: bool) -> bool {
        if !matches!(self.stage, Stage::Input) {
            return false;
        }
        match self.focus {
            Focus::TokenIn => {
                self.token_in_editing = false;
                let changed = cycle_token_picker(
                    assets,
                    false,
                    &mut self.token_in_pick,
                    forward,
                    &mut self.native_in,
                    &mut self.token_in,
                    &mut self.status,
                );
                if changed {
                    self.clear_quotes();
                }
                changed
            }
            Focus::TokenOut => {
                self.token_out_editing = false;
                let changed = cycle_token_picker(
                    assets,
                    true,
                    &mut self.token_out_pick,
                    forward,
                    &mut self.native_in,
                    &mut self.token_out,
                    &mut self.status,
                );
                if changed {
                    self.clear_quotes();
                }
                changed
            }
            _ => false,
        }
    }

    fn render_confirm(&self, frame: &mut Frame, area: Rect, assets: &[Balance]) {
        let step = match self.stage {
            Stage::Confirm(s) => s,
            _ => ConfirmStep::Swap,
        };
        let title = match step {
            ConfirmStep::Approve => " Approve ",
            ConfirmStep::Swap => " Confirm swap ",
        };
        let body = self.build_confirm_lines(step, assets);
        render_plain_confirm(frame, area, title, body, "Enter confirm · Esc cancel");
    }

    fn build_confirm_lines(&self, step: ConfirmStep, assets: &[Balance]) -> Vec<Line<'static>> {
        let Some(q) = self.quote.as_ref() else {
            return vec![Line::from("No quote.")];
        };
        let in_sym = token_display_symbol(self.native_in, &self.token_in, assets, self.chain_id);
        let out_sym = token_display_symbol(false, &self.token_out, assets, self.chain_id);
        let in_amt = fmt_swap_wei_amount(&q.amount_in, 18);
        let out_amt = fmt_swap_wei_amount(&q.amount_out, 18);
        let slippage = self.slippage.value().trim().to_string();

        let step_name = match step {
            ConfirmStep::Approve => "Approve",
            ConfirmStep::Swap => "Swap",
        };

        let mut lines = vec![
            Line::from(format!("{} · {step_name}", q.venue.label())),
            Line::from(""),
        ];

        match step {
            ConfirmStep::Approve => {
                lines.push(Line::from("Allow router to spend"));
                lines.push(Line::from(format!("  {in_amt} {in_sym}")));
                lines.push(Line::from(""));
                lines.push(Line::from("Swap follows on the next step."));
            }
            ConfirmStep::Swap => {
                lines.push(Line::from(format!("Pay      {in_amt} {in_sym}")));
                lines.push(Line::from(format!("Receive  ~{out_amt} {out_sym}")));
                if !slippage.is_empty() {
                    lines.push(Line::from(format!("Slippage  {slippage}%")));
                }
            }
        }
        lines
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
                    self.clear_quotes();
                    KeyOutcome::Consumed
                }
                KeyCode::Enter | KeyCode::Char('y') => self.confirm_step(step, wallet),
                _ => KeyOutcome::Consumed,
            },
            Stage::ComparePick => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::Input;
                    self.clear_quotes();
                    KeyOutcome::Consumed
                }
                KeyCode::Up | KeyCode::Down => {
                    if self.compare_ranked.is_empty() {
                        return KeyOutcome::Consumed;
                    }
                    let n = self.compare_ranked.len();
                    self.compare_pick = if matches!(key.code, KeyCode::Down) {
                        (self.compare_pick + 1) % n
                    } else {
                        (self.compare_pick + n - 1) % n
                    };
                    self.select_compare_pick(self.compare_pick);
                    self.refresh_compare_status(&[]);
                    KeyOutcome::Consumed
                }
                KeyCode::Enter => {
                    self.enter_confirm();
                    KeyOutcome::Consumed
                }
                _ => KeyOutcome::Consumed,
            },
            Stage::Input => self.handle_input_key(key, wallet),
        }
    }

    fn handle_input_key(&mut self, key: KeyEvent, wallet: &WalletState) -> KeyOutcome {
        match key.code {
            KeyCode::Esc => KeyOutcome::Navigate(Screen::Dashboard),
            KeyCode::Up | KeyCode::Down if self.focus == Focus::Venue => {
                self.venue_choice = if matches!(key.code, KeyCode::Down) {
                    self.venue_choice.next()
                } else {
                    self.venue_choice.prev()
                };
                self.clear_quotes();
                self.refresh_venue_status();
                KeyOutcome::Consumed
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
            KeyCode::Char(' ') if matches!(self.focus, Focus::Venue | Focus::TokenIn) => {
                if self.focus == Focus::TokenIn {
                    self.native_in = !self.native_in;
                    self.token_in_editing = false;
                    if self.native_in {
                        self.token_in.set_value("");
                        self.token_in_pick = TOKEN_PICK_UNINIT;
                    } else {
                        self.token_in_pick = TOKEN_PICK_UNINIT;
                    }
                    self.status = if self.native_in {
                        "native PLS in".into()
                    } else {
                        "ERC-20 in".into()
                    };
                    self.clear_quotes();
                }
                KeyOutcome::Consumed
            }
            KeyCode::F(4) => self.start_quote(wallet),
            KeyCode::Enter if self.focus != Focus::None => {
                self.deselect_focus();
                KeyOutcome::Consumed
            }
            KeyCode::Enter => self.start_quote(wallet),
            _ => {
                let input = match self.focus {
                    Focus::TokenIn => {
                        Some((&mut self.token_in, Some(&mut self.token_in_pick), true))
                    }
                    Focus::TokenOut => {
                        Some((&mut self.token_out, Some(&mut self.token_out_pick), false))
                    }
                    Focus::Amount => Some((&mut self.amount, None, false)),
                    Focus::Slippage => Some((&mut self.slippage, None, false)),
                    Focus::None | Focus::Venue => None,
                };
                let Some((input, pick, is_in)) = input else {
                    return KeyOutcome::Consumed;
                };
                match input.handle_key(key) {
                    InputAction::Ignored => KeyOutcome::NotHandled,
                    _ => {
                        if let Some(p) = pick {
                            if manual_edit_resets_token_pick(key.code) {
                                *p = TOKEN_PICK_UNINIT;
                                if is_in {
                                    self.token_in_editing = true;
                                } else {
                                    self.token_out_editing = true;
                                }
                            }
                        }
                        self.clear_quotes();
                        KeyOutcome::Consumed
                    }
                }
            }
        }
    }

    fn focus_tab_forward(&self) -> Focus {
        match self.focus {
            Focus::None => Focus::Venue,
            Focus::Venue => Focus::TokenIn,
            Focus::TokenIn => Focus::TokenOut,
            Focus::TokenOut => Focus::Amount,
            Focus::Amount => Focus::Slippage,
            Focus::Slippage => Focus::None,
        }
    }

    fn focus_tab_backward(&self) -> Focus {
        match self.focus {
            Focus::None => Focus::Slippage,
            Focus::Venue => Focus::None,
            Focus::TokenIn => Focus::Venue,
            Focus::TokenOut => Focus::TokenIn,
            Focus::Amount => Focus::TokenOut,
            Focus::Slippage => Focus::Amount,
        }
    }

    fn on_focus_left(&mut self, old: Focus) {
        match old {
            Focus::TokenIn => self.token_in_editing = false,
            Focus::TokenOut => self.token_out_editing = false,
            _ => {}
        }
    }

    fn deselect_focus(&mut self) {
        let old = self.focus;
        self.on_focus_left(old);
        self.focus = Focus::None;
    }

    fn start_quote(&mut self, wallet: &WalletState) -> KeyOutcome {
        if self.chain_id != 369 {
            self.status = "Aggregators need PulseChain mainnet (369)".into();
            return KeyOutcome::Consumed;
        }
        let token_out = match parse_token_address(self.token_out.value(), "Token out") {
            Ok(a) => a,
            Err(msg) => {
                self.status = msg;
                return KeyOutcome::Consumed;
            }
        };
        let token_in = if self.native_in {
            Address::ZERO
        } else {
            match parse_token_address(self.token_in.value(), "Token in") {
                Ok(a) => a,
                Err(msg) => {
                    self.status = msg;
                    return KeyOutcome::Consumed;
                }
            }
        };
        let amount = match parse_swap_amount(self.amount.value(), "amount", 18) {
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

        self.clear_quotes();
        self.busy = Busy::Quoting;

        match self.venue_choice {
            VenueChoice::CompareAll => {
                self.status = "quoting all live aggregators…".into();
                KeyOutcome::StartJob(UiJob::AggCompareQuote {
                    token_in: token_in.to_string(),
                    token_out: token_out.to_string(),
                    amount: amount.to_string(),
                    slippage,
                    native_in: self.native_in,
                    native_out: false,
                    account: account.map(|a| a.to_string()),
                })
            }
            VenueChoice::Single(venue) => {
                if !venue.is_live() {
                    self.busy = Busy::Idle;
                    self.refresh_venue_status();
                    return KeyOutcome::Consumed;
                }
                self.status = format!("quoting {}…", venue.label());
                KeyOutcome::StartJob(UiJob::AggQuote {
                    venue,
                    token_in: token_in.to_string(),
                    token_out: token_out.to_string(),
                    amount: amount.to_string(),
                    slippage,
                    native_in: self.native_in,
                    native_out: false,
                    account: account.map(|a| a.to_string()),
                })
            }
        }
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
        let out_sym = token_display_symbol(false, &self.token_out, &[], self.chain_id);
        let out_amt = fmt_swap_wei_amount(&q.amount_out, 18);
        self.stage = Stage::Confirm(step);
        self.status = format!(
            "{} — receive ~{out_amt} {out_sym} · Enter to {}",
            q.venue.label(),
            step.label()
        );
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

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    format!(
        "{}…",
        s.chars().take(max.saturating_sub(1)).collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::parse_swap_amount;
    use alloy::primitives::U256;

    #[test]
    fn venue_cycle_piteas_adjacent_compare_all() {
        let piteas = VenueChoice::Single(AggVenue::Piteas);
        assert_eq!(piteas.next(), VenueChoice::CompareAll);
        assert_eq!(VenueChoice::CompareAll.prev(), piteas);
    }

    #[test]
    fn parse_human_pls() {
        assert_eq!(
            parse_swap_amount("1", "amount", 18).unwrap(),
            U256::from_str("1000000000000000000").unwrap()
        );
        assert_eq!(
            parse_swap_amount("0.01", "amount", 18).unwrap(),
            U256::from_str("10000000000000000").unwrap()
        );
    }

    #[test]
    fn parse_wei_scale_and_suffix() {
        let wei = U256::from_str("1000000000000000000").unwrap();
        assert_eq!(
            parse_swap_amount("1000000000000000000", "amount", 18).unwrap(),
            wei
        );
        assert_eq!(
            parse_swap_amount("1000000000000000000wei", "amount", 18).unwrap(),
            wei
        );
    }
}
