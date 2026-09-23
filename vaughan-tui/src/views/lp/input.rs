#![allow(unused_imports)]
use alloy::primitives::U256;
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
use vaughan_core::chains::Balance;
use vaughan_core::core::wiz4rd::WZRD_SMOKE_943;
use vaughan_core::core::{
    build_v2_add_liquidity_evm, build_v2_remove_liquidity_evm, build_v3_collect_evm,
    build_v3_decrease_evm, build_v3_increase_evm, chain_label, cycle_lp_stack,
    default_full_range_ticks, display_price_range_from_preset, format_display_amount,
    lp_stack_for_chain, lp_stacks_for_chain, lp_v3_venue_picker, min_out_after_slippage,
    v3_preview_mint_deposits_from_amount0, v3_preview_mint_deposits_from_amount1,
    v3_range_ticks_from_human_prices, v3_sqrt_and_tick_for_preview, venue_position_manager,
    venue_swap_router, wpls_for_chain, DexProtocol, DexVenue, LpStack, V2LpPosition,
    V3LpDeployWait, V3PoolLifecycle, V3PositionInfo, WalletState, DEFAULT_DEX_SLIPPAGE_BPS,
};
use vaughan_core::error::WalletError;
use vaughan_provider::EventBus;

use crate::app::KeyOutcome;
use crate::brand;
use crate::input::{Input, InputAction};
use crate::jobs::{spinner_frame, UiJob, UiJobResult};
use crate::views::swap_form::SWAP_DISPLAY_FRAC;
use crate::views::{
    body_areas, cycle_token_picker, manual_edit_resets_token_pick, render_labeled_input,
    render_labeled_input_aligned, status_paragraph, token_symbol_for_address, TOKEN_PICK_UNINIT,
};
use crate::views::{parse_swap_amount, parse_token_address};

use super::helpers::{
    fee_tier_display, format_unit_price, lp_tx_error_message, parse_price_f64,
    render_unit_price_input, trim_float_string,
};
use super::types::*;

impl LpView {
    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &WalletState,
        handle: &Handle,
        events: &EventBus,
    ) -> KeyOutcome {
        let _ = events;
        let tab_focus = matches!(key.code, KeyCode::Tab | KeyCode::BackTab);
        if self.stage == Stage::Done {
            return self.handle_done_key(key);
        }
        if self.busy == Busy::Sending {
            return KeyOutcome::Consumed;
        }
        if self.stage == Stage::Confirm {
            return self.handle_confirm(key, wallet);
        }
        if self.busy == Busy::Loading && !(self.on_v3_price_deposit() || tab_focus) {
            return KeyOutcome::Consumed;
        }

        if self.tab == Tab::AddLp {
            return self.handle_add_lp_key(key, wallet);
        }

        match key.code {
            KeyCode::F(4) if self.tab == Tab::Transfer => {
                self.focus = Focus::Recipient;
                KeyOutcome::Consumed
            }
            KeyCode::F(5) if self.tab == Tab::Transfer => {
                self.focus = Focus::None;
                self.status = "LP token is locked for this transfer".into();
                KeyOutcome::Consumed
            }
            KeyCode::Up if self.tab == Tab::List && self.list_action_idx.is_some() => {
                self.cycle_list_action(false);
                KeyOutcome::Consumed
            }
            KeyCode::Down if self.tab == Tab::List && self.list_action_idx.is_some() => {
                self.cycle_list_action(true);
                KeyOutcome::Consumed
            }
            KeyCode::Up if self.tab == Tab::List => {
                self.move_list_sel(false);
                KeyOutcome::Consumed
            }
            KeyCode::Down if self.tab == Tab::List => {
                self.move_list_sel(true);
                KeyOutcome::Consumed
            }
            KeyCode::Char('[') | KeyCode::Char(']')
                if self.tab == Tab::List && self.list_action_idx.is_none() =>
            {
                let forward = matches!(key.code, KeyCode::Char(']'));
                self.apply_cycled_lp_stack(forward);
                if let Some(job) = self.list_job(wallet) {
                    self.busy = Busy::Loading;
                    self.status = format!("Loading {} {}…", self.venue.label(), self.stack.label());
                    KeyOutcome::StartJob(job)
                } else {
                    self.status = self.default_status_hint();
                    KeyOutcome::Consumed
                }
            }
            KeyCode::Up
                if matches!(self.tab, Tab::Decrease | Tab::Remove) && self.focus == Focus::None =>
            {
                self.cycle_decrease_preset(false);
                KeyOutcome::Consumed
            }
            KeyCode::Down
                if matches!(self.tab, Tab::Decrease | Tab::Remove) && self.focus == Focus::None =>
            {
                self.cycle_decrease_preset(true);
                KeyOutcome::Consumed
            }
            KeyCode::Tab | KeyCode::BackTab if self.tab == Tab::Increase => {
                let forward = matches!(key.code, KeyCode::Tab);
                self.focus = match (self.focus, forward) {
                    (Focus::None | Focus::Amount1, true) | (Focus::Amount1, false) => {
                        Focus::Amount0
                    }
                    (Focus::Amount0, true) => Focus::Amount1,
                    (Focus::Amount0, false) | (Focus::None, false) => Focus::Amount1,
                    _ => Focus::Amount0,
                };
                KeyOutcome::Consumed
            }
            KeyCode::Tab | KeyCode::BackTab if self.tab == Tab::Transfer => {
                // F5 LP token is locked — Tab only cycles the recipient field.
                self.focus = Focus::Recipient;
                KeyOutcome::Consumed
            }
            KeyCode::Tab | KeyCode::BackTab if matches!(self.tab, Tab::Decrease | Tab::Remove) => {
                self.focus = if self.focus == Focus::Liquidity {
                    Focus::None
                } else {
                    Focus::Liquidity
                };
                KeyOutcome::Consumed
            }
            KeyCode::Enter
                if matches!(
                    self.focus,
                    Focus::Liquidity | Focus::Amount0 | Focus::Amount1 | Focus::Recipient
                ) =>
            {
                self.focus = Focus::None;
                KeyOutcome::Consumed
            }
            // Transfer is locked to one LP — no ←→ tab hopping (Esc → list).
            KeyCode::Left | KeyCode::Right
                if self.tab == Tab::Transfer && self.focus == Focus::None =>
            {
                self.status =
                    "Transfer is locked to this LP — Esc back to list to pick another".into();
                KeyOutcome::Consumed
            }
            KeyCode::Left if self.focus == Focus::None && self.list_action_idx.is_none() => {
                self.tab = self.tab.prev(self.stack);
                self.on_tab_changed();
                KeyOutcome::Consumed
            }
            KeyCode::Right if self.focus == Focus::None && self.list_action_idx.is_none() => {
                self.tab = self.tab.next(self.stack);
                self.on_tab_changed();
                KeyOutcome::Consumed
            }
            KeyCode::Char(c)
                if self.tab == Tab::List
                    && self.list_action_idx.is_some()
                    && self.apply_list_action_key(c) =>
            {
                KeyOutcome::Consumed
            }
            // List idle: `s` = Transfer (do not fall through to global Home Send).
            KeyCode::Char('s') | KeyCode::Char('S') if self.tab == Tab::List => {
                let has = match self.stack {
                    LpStack::V3 { .. } => !self.v3_positions.is_empty(),
                    LpStack::V2 { .. } => !self.v2_positions.is_empty(),
                };
                if has {
                    self.open_manage_tab(Tab::Transfer);
                } else {
                    self.status = "No LP position to transfer".into();
                }
                KeyOutcome::Consumed
            }
            KeyCode::Char('o') | KeyCode::Char('O')
                if self.focus == Focus::None
                    && (self.list_action_idx.is_some()
                        || matches!(
                            self.tab,
                            Tab::Increase
                                | Tab::Decrease
                                | Tab::Collect
                                | Tab::Remove
                                | Tab::Transfer
                        )) =>
            {
                self.open_selected_pool_scanner()
            }
            KeyCode::Char('y') | KeyCode::Char('Y')
                if self.focus == Focus::None
                    && (self.list_action_idx.is_some()
                        || matches!(
                            self.tab,
                            Tab::Increase
                                | Tab::Decrease
                                | Tab::Collect
                                | Tab::Remove
                                | Tab::Transfer
                        )) =>
            {
                self.copy_selected_pool_address()
            }
            KeyCode::Char('r') | KeyCode::Char('R')
                if !(self.tab == Tab::List
                    && self.list_action_idx.is_some()
                    && matches!(self.stack, LpStack::V2 { .. })) =>
            {
                if let Some(job) = self.list_job(wallet) {
                    self.busy = Busy::Loading;
                    self.status = "Loading positions…".into();
                    KeyOutcome::StartJob(job)
                } else {
                    self.status = self.default_status_hint();
                    KeyOutcome::Consumed
                }
            }
            KeyCode::Enter if self.tab == Tab::List && self.list_action_idx.is_some() => {
                self.enter_list_action();
                KeyOutcome::Consumed
            }
            KeyCode::Enter if self.tab == Tab::List => {
                self.open_list_actions();
                KeyOutcome::Consumed
            }
            KeyCode::Enter => self.submit_manage(wallet, handle),
            KeyCode::Esc => {
                if matches!(
                    self.focus,
                    Focus::Liquidity | Focus::Amount0 | Focus::Amount1 | Focus::Recipient
                ) {
                    self.focus = Focus::None;
                    KeyOutcome::Consumed
                } else if self.tab == Tab::List && self.list_action_idx.is_some() {
                    self.close_list_actions();
                    KeyOutcome::Consumed
                } else if matches!(
                    self.tab,
                    Tab::Increase | Tab::Decrease | Tab::Collect | Tab::Remove | Tab::Transfer
                ) {
                    self.clear_transfer_lock();
                    self.tab = Tab::List;
                    self.on_tab_changed();
                    self.status = "↑↓ select · Enter open · ←→ tabs · r reload".into();
                    KeyOutcome::Consumed
                } else {
                    KeyOutcome::Back
                }
            }
            _ if self.focus == Focus::Liquidity => match self.liquidity.handle_key(key) {
                InputAction::Ignored => KeyOutcome::NotHandled,
                InputAction::Submitted => {
                    self.focus = Focus::None;
                    KeyOutcome::Consumed
                }
                InputAction::Consumed => {
                    self.clear_decrease_preset_if_edited();
                    KeyOutcome::Consumed
                }
            },
            _ if self.focus == Focus::Amount0 => match self.amount0.handle_key(key) {
                InputAction::Ignored => KeyOutcome::NotHandled,
                InputAction::Submitted => {
                    self.focus = Focus::Amount1;
                    KeyOutcome::Consumed
                }
                InputAction::Consumed => KeyOutcome::Consumed,
            },
            _ if self.focus == Focus::Amount1 => match self.amount1.handle_key(key) {
                InputAction::Ignored => KeyOutcome::NotHandled,
                InputAction::Submitted => {
                    self.focus = Focus::None;
                    KeyOutcome::Consumed
                }
                InputAction::Consumed => KeyOutcome::Consumed,
            },
            _ if self.focus == Focus::Recipient => match self.transfer_to.handle_key(key) {
                InputAction::Ignored => KeyOutcome::NotHandled,
                InputAction::Submitted => {
                    self.focus = Focus::None;
                    KeyOutcome::Consumed
                }
                InputAction::Consumed => KeyOutcome::Consumed,
            },
            _ => KeyOutcome::NotHandled,
        }
    }

    pub(crate) fn handle_add_lp_key(&mut self, key: KeyEvent, wallet: &WalletState) -> KeyOutcome {
        match key.code {
            KeyCode::Esc => {
                if self.focus != Focus::None {
                    self.deselect_focus();
                    KeyOutcome::Consumed
                } else if matches!(self.stack, LpStack::V3 { .. })
                    && self.add_step == AddStep::PriceDeposit
                {
                    self.add_step = AddStep::SelectPair;
                    self.clear_pool_quote();
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
            KeyCode::Left
                if self.focus == Focus::None && self.add_step != AddStep::PriceDeposit =>
            {
                self.tab = self.tab.prev(self.stack);
                self.on_tab_changed();
                KeyOutcome::Consumed
            }
            KeyCode::Right
                if self.focus == Focus::None && self.add_step != AddStep::PriceDeposit =>
            {
                self.tab = self.tab.next(self.stack);
                self.on_tab_changed();
                KeyOutcome::Consumed
            }
            KeyCode::Left | KeyCode::Right
                if self.add_step == AddStep::PriceDeposit
                    && matches!(self.focus, Focus::None | Focus::RangePresets) =>
            {
                if self.focus == Focus::None {
                    self.focus = Focus::RangePresets;
                }
                self.cycle_range_preset_highlight(matches!(key.code, KeyCode::Right));
                self.apply_range_preset(self.range_preset_idx);
                KeyOutcome::Consumed
            }
            KeyCode::Left | KeyCode::Right if self.focus == Focus::Fee => {
                self.cycle_fee(matches!(key.code, KeyCode::Right));
                KeyOutcome::Consumed
            }
            KeyCode::Left | KeyCode::Right if self.focus == Focus::Venue => KeyOutcome::Consumed,
            KeyCode::Left | KeyCode::Right => {
                let input = self.focused_input_mut();
                if let Some(input) = input {
                    match input.handle_key(key) {
                        InputAction::Ignored => KeyOutcome::NotHandled,
                        InputAction::Consumed | InputAction::Submitted => {
                            self.on_manual_token_edit();
                            if matches!(
                                self.focus,
                                Focus::InitialPrice | Focus::MinPrice | Focus::MaxPrice
                            ) {
                                self.on_v3_price_field_edited();
                            }
                            KeyOutcome::Consumed
                        }
                    }
                } else {
                    KeyOutcome::Consumed
                }
            }
            KeyCode::Tab => {
                self.normalize_v3_price_focus();
                let old = self.focus;
                self.focus = self.focus_tab_forward();
                self.on_focus_left(old);
                KeyOutcome::Consumed
            }
            KeyCode::BackTab => {
                self.normalize_v3_price_focus();
                let old = self.focus;
                self.focus = self.focus_tab_backward();
                self.on_focus_left(old);
                KeyOutcome::Consumed
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                if let Some(job) = self.list_job(wallet) {
                    self.busy = Busy::Loading;
                    self.status = "Loading positions…".into();
                    KeyOutcome::StartJob(job)
                } else {
                    KeyOutcome::Consumed
                }
            }
            KeyCode::Enter if self.focus == Focus::RangePresets => {
                self.apply_range_preset(self.range_preset_idx);
                let old = self.focus;
                self.on_focus_left(old);
                self.focus = self.focus_tab_forward();
                KeyOutcome::Consumed
            }
            KeyCode::Enter
                if self.on_v3_price_deposit()
                    && matches!(
                        self.focus,
                        Focus::InitialPrice
                            | Focus::MinPrice
                            | Focus::MaxPrice
                            | Focus::Amount0
                            | Focus::Amount1
                    ) =>
            {
                let old = self.focus;
                self.on_focus_left(old);
                self.focus = self.focus_tab_forward();
                KeyOutcome::Consumed
            }
            KeyCode::Enter if self.focus != Focus::None => {
                self.deselect_focus();
                KeyOutcome::Consumed
            }
            KeyCode::Char(c)
                if self.add_step == AddStep::PriceDeposit
                    && matches!(self.focus, Focus::None | Focus::RangePresets)
                    && matches!(c, 'e' | 'E')
                    && matches!(self.stack, LpStack::V3 { .. }) =>
            {
                self.start_enable_confirm(wallet)
            }
            KeyCode::Char(c)
                if self.add_step == AddStep::PriceDeposit
                    && matches!(self.focus, Focus::None | Focus::RangePresets)
                    && matches!(c, 'a' | 'A')
                    && matches!(self.stack, LpStack::V3 { .. }) =>
            {
                self.toggle_v3_custom_range();
                KeyOutcome::Consumed
            }
            KeyCode::Enter => self.submit_add_lp(wallet),
            _ => {
                if matches!(
                    self.focus,
                    Focus::None
                        | Focus::Fee
                        | Focus::Venue
                        | Focus::RangePresets
                        | Focus::Liquidity
                ) {
                    return KeyOutcome::NotHandled;
                }
                let (input, pick) = match self.focus {
                    Focus::Token0 => (Some(&mut self.token0), Some(&mut self.token0_pick)),
                    Focus::Token1 => (Some(&mut self.token1), Some(&mut self.token1_pick)),
                    Focus::InitialPrice => (Some(&mut self.initial_price), None),
                    Focus::MinPrice => (Some(&mut self.min_price), None),
                    Focus::MaxPrice => (Some(&mut self.max_price), None),
                    Focus::Ratio => (Some(&mut self.ratio), None),
                    Focus::Amount0 => (Some(&mut self.amount0), None),
                    Focus::Amount1 => (Some(&mut self.amount1), None),
                    Focus::None
                    | Focus::Fee
                    | Focus::Venue
                    | Focus::RangePresets
                    | Focus::Liquidity
                    | Focus::Recipient => unreachable!(),
                };
                let Some(input) = input else {
                    return KeyOutcome::Consumed;
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
                                    Focus::Token0 => self.token0_editing = true,
                                    Focus::Token1 => self.token1_editing = true,
                                    _ => {}
                                }
                            }
                        }
                        if matches!(self.focus, Focus::Ratio | Focus::Amount0 | Focus::Amount1)
                            && manual_edit_resets_token_pick(key.code)
                        {
                            let on_v3_price = matches!(self.stack, LpStack::V3 { .. })
                                && self.add_step == AddStep::PriceDeposit;
                            match self.focus {
                                Focus::Amount0 if on_v3_price => self.sync_amount1_from_price(),
                                Focus::Amount1 if on_v3_price => self.sync_amount0_from_price(),
                                Focus::Ratio | Focus::Amount0 => self.sync_amount1_from_ratio(),
                                Focus::Amount1 if !on_v3_price => self.sync_ratio_from_amounts(),
                                _ => {}
                            }
                        }
                        if matches!(
                            self.focus,
                            Focus::InitialPrice | Focus::MinPrice | Focus::MaxPrice
                        ) {
                            self.on_v3_price_field_edited();
                        }
                        KeyOutcome::Consumed
                    }
                }
            }
        }
    }

    pub(crate) fn on_manual_token_edit(&mut self) {
        match self.focus {
            Focus::Token0 => {
                self.token0_pick = TOKEN_PICK_UNINIT;
                self.token0_editing = true;
            }
            Focus::Token1 => {
                self.token1_pick = TOKEN_PICK_UNINIT;
                self.token1_editing = true;
            }
            _ => {}
        }
    }

    pub(crate) fn focused_input_mut(&mut self) -> Option<&mut Input> {
        match self.focus {
            Focus::Token0 => Some(&mut self.token0),
            Focus::Token1 => Some(&mut self.token1),
            Focus::InitialPrice => Some(&mut self.initial_price),
            Focus::MinPrice => Some(&mut self.min_price),
            Focus::MaxPrice => Some(&mut self.max_price),
            Focus::Ratio => Some(&mut self.ratio),
            Focus::Amount0 => Some(&mut self.amount0),
            Focus::Amount1 => Some(&mut self.amount1),
            Focus::Liquidity => Some(&mut self.liquidity),
            Focus::Recipient => Some(&mut self.transfer_to),
            Focus::None | Focus::Fee | Focus::Venue | Focus::RangePresets => None,
        }
    }

    /// ↑/↓ on the venue row during Add LP pair selection (V2 + V3 stacks).
    pub fn cycle_venue_selector(&mut self, forward: bool) -> bool {
        if self.tab != Tab::AddLp
            || self.stage != Stage::Input
            || self.add_step != AddStep::SelectPair
        {
            return false;
        }
        if !matches!(self.focus, Focus::None | Focus::Venue) {
            return false;
        }
        self.apply_cycled_lp_stack(forward);
        self.focus = Focus::Venue;
        true
    }

    pub(crate) fn apply_cycled_lp_stack(&mut self, forward: bool) {
        let stacks = lp_stacks_for_chain(self.chain_id);
        if stacks.is_empty() {
            self.status = format!("No LP venues on {}", chain_label(self.chain_id));
            return;
        }
        if stacks.len() == 1 {
            self.status = format!(
                "Only {} {} on {} — no other DEX on this chain",
                stacks[0].venue().label(),
                stacks[0].label(),
                chain_label(self.chain_id)
            );
            return;
        }
        let next = cycle_lp_stack(self.stack, self.chain_id, forward);
        self.stack = next;
        self.venue = next.venue();
        self.v3_positions.clear();
        self.v2_positions.clear();
        self.sel = 0;
        self.list_action_idx = None;
        self.clear_transfer_lock();
        if matches!(self.stack, LpStack::V3 { .. })
            && venue_position_manager(self.venue, self.chain_id).is_none()
        {
            self.status = format!(
                "{} V3 LP is on testnet 943 — F1 Network → PulseChain testnet",
                self.venue.label()
            );
            return;
        }
        self.apply_venue_token_defaults(self.venue == DexVenue::NineMm);
        self.apply_initial_fee_defaults();
        self.status = format!(
            "{} · {} · [ ] DEX · r reload",
            self.venue.label(),
            self.stack.label()
        );
    }

    pub(crate) fn cycle_fee(&mut self, forward: bool) {
        let Some(idx) = LP_FEE_TIERS.iter().position(|f| *f == self.fee_tier) else {
            self.fee_tier = 2500;
            return;
        };
        let next = if forward {
            (idx + 1) % LP_FEE_TIERS.len()
        } else {
            (idx + LP_FEE_TIERS.len() - 1) % LP_FEE_TIERS.len()
        };
        self.fee_tier = LP_FEE_TIERS[next];
        self.status = format!("Fee tier {} · ←→ cycle", fee_tier_display(self.fee_tier));
    }

    pub(crate) fn focus_tab_forward(&self) -> Focus {
        let on_price =
            matches!(self.stack, LpStack::V3 { .. }) && self.add_step == AddStep::PriceDeposit;
        if on_price {
            if self.v3_custom_range {
                return match self.focus {
                    Focus::None => Focus::RangePresets,
                    Focus::RangePresets => Focus::MinPrice,
                    Focus::MinPrice => Focus::InitialPrice,
                    Focus::InitialPrice => Focus::MaxPrice,
                    Focus::MaxPrice => Focus::Amount0,
                    Focus::Amount0 => Focus::Amount1,
                    Focus::Amount1 => Focus::None,
                    _ => Focus::RangePresets,
                };
            }
            return match self.focus {
                // Always stop on Range width — Step 2 auto-applies a default preset but
                // the user must still Tab there to change 1% / Full / etc.
                Focus::None => Focus::RangePresets,
                Focus::RangePresets => self.next_v3_focus_after_presets(),
                Focus::InitialPrice => Focus::Amount0,
                Focus::Amount0 => Focus::Amount1,
                Focus::Amount1 => Focus::None,
                _ => Focus::RangePresets,
            };
        }
        if matches!(self.stack, LpStack::V2 { .. }) {
            return match self.focus {
                Focus::None => Focus::Token0,
                Focus::Token0 => Focus::Token1,
                Focus::Token1 => Focus::Ratio,
                Focus::Ratio => Focus::Amount0,
                Focus::Amount0 => Focus::Amount1,
                Focus::Amount1 => Focus::None,
                _ => Focus::Token0,
            };
        }
        match self.focus {
            Focus::None => Focus::Venue,
            Focus::Venue => Focus::Token0,
            Focus::Token0 => Focus::Token1,
            Focus::Token1 => Focus::Fee,
            Focus::Fee => Focus::None,
            _ => Focus::Venue,
        }
    }

    pub(crate) fn focus_tab_backward(&self) -> Focus {
        let on_price =
            matches!(self.stack, LpStack::V3 { .. }) && self.add_step == AddStep::PriceDeposit;
        if on_price {
            if self.v3_custom_range {
                return match self.focus {
                    Focus::None => Focus::Amount1,
                    Focus::Amount1 => Focus::Amount0,
                    Focus::Amount0 => Focus::MaxPrice,
                    Focus::MaxPrice => Focus::InitialPrice,
                    Focus::InitialPrice => Focus::MinPrice,
                    Focus::MinPrice => Focus::RangePresets,
                    Focus::RangePresets => Focus::None,
                    _ => Focus::Amount0,
                };
            }
            return match self.focus {
                Focus::None => Focus::Amount1,
                Focus::Amount1 => Focus::Amount0,
                Focus::Amount0 => {
                    if self.needs_v3_starting_price() {
                        Focus::InitialPrice
                    } else {
                        Focus::RangePresets
                    }
                }
                Focus::InitialPrice => Focus::RangePresets,
                Focus::RangePresets => Focus::None,
                _ => Focus::Amount0,
            };
        }
        if matches!(self.stack, LpStack::V2 { .. }) {
            return match self.focus {
                Focus::None => Focus::Amount1,
                Focus::Amount1 => Focus::Amount0,
                Focus::Amount0 => Focus::Ratio,
                Focus::Ratio => Focus::Token1,
                Focus::Token1 => Focus::Token0,
                Focus::Token0 => Focus::None,
                _ => Focus::Amount1,
            };
        }
        match self.focus {
            Focus::None => Focus::Fee,
            Focus::Fee => Focus::Token1,
            Focus::Token1 => Focus::Token0,
            Focus::Token0 => Focus::Venue,
            Focus::Venue => Focus::None,
            _ => Focus::Fee,
        }
    }

    pub(crate) fn on_focus_left(&mut self, old: Focus) {
        let on_v3_price =
            matches!(self.stack, LpStack::V3 { .. }) && self.add_step == AddStep::PriceDeposit;
        match old {
            Focus::Token0 => self.token0_editing = false,
            Focus::Token1 => self.token1_editing = false,
            Focus::Ratio | Focus::Amount0 if on_v3_price => {
                self.sync_amount1_from_price();
                self.schedule_enable_recheck();
            }
            Focus::Amount1 if on_v3_price => {
                self.sync_amount0_from_price();
                self.schedule_enable_recheck();
            }
            Focus::Ratio | Focus::Amount0 => self.sync_amount1_from_ratio(),
            Focus::Amount1 if !on_v3_price => self.sync_ratio_from_amounts(),
            Focus::InitialPrice | Focus::MinPrice | Focus::MaxPrice if on_v3_price => {
                self.sync_amount1_from_price();
            }
            _ => {}
        }
    }

    pub(crate) fn deselect_focus(&mut self) {
        let old = self.focus;
        self.on_focus_left(old);
        self.focus = Focus::None;
    }

    pub(crate) fn validate_pair_selection(&self) -> Result<(), String> {
        let t0 = parse_token_address(self.token0.value(), "first token")?;
        let t1 = parse_token_address(self.token1.value(), "second token")?;
        if t0 == t1 {
            return Err("Pick two different tokens".into());
        }
        Ok(())
    }

    pub(crate) fn submit_add_lp(&mut self, wallet: &WalletState) -> KeyOutcome {
        if !self.lp_supported() {
            self.status = self.default_status_hint();
            return KeyOutcome::Consumed;
        }
        if matches!(self.stack, LpStack::V3 { .. }) && self.add_step == AddStep::SelectPair {
            match self.validate_pair_selection() {
                Ok(()) => {
                    self.add_step = AddStep::PriceDeposit;
                    self.focus = Focus::None;
                    self.begin_optimistic_pool_preview();
                    if self.range_preset_applied.is_none() {
                        self.apply_range_preset(5);
                    }
                    self.sync_amount1_from_price();
                    self.refresh_price_deposit_status();
                    self.focus_v3_deposit_after_preset();
                    return self.spawn_pool_quote_job(wallet);
                }
                Err(e) => self.status = e,
            }
            return KeyOutcome::Consumed;
        }
        match self.stack {
            LpStack::V3 { .. } => self.start_add_liquidity_job(wallet),
            LpStack::V2 { .. } => match self.build_v2_add_tx(wallet) {
                Ok(tx) => self.confirm_tx(tx, LpConfirmAction::V2Add),
                Err(e) => {
                    self.status = e;
                    KeyOutcome::Consumed
                }
            },
        }
    }

    pub(crate) fn submit_manage(&mut self, wallet: &WalletState, handle: &Handle) -> KeyOutcome {
        if !self.lp_supported() {
            self.status = self.default_status_hint();
            return KeyOutcome::Consumed;
        }
        self.confirm_custom_tokens = wallet.custom_tokens_for_active_chain();
        match self.tab {
            Tab::List => KeyOutcome::Consumed,
            Tab::Increase | Tab::Decrease | Tab::Collect => {
                if let Err(e) = self.refresh_selected_v3_position(wallet, handle) {
                    self.status = e;
                    return KeyOutcome::Consumed;
                }
                match self.tab {
                    Tab::Increase => {
                        if let Some(outcome) = self.try_increase_enable_first(wallet, handle) {
                            return outcome;
                        }
                        match self.build_increase_tx(wallet) {
                            Ok(tx) => self.confirm_tx(tx, LpConfirmAction::Increase),
                            Err(e) => {
                                self.status = e;
                                KeyOutcome::Consumed
                            }
                        }
                    }
                    Tab::Decrease => {
                        if let Some(p) = self.v3_positions.get(self.sel) {
                            if let Some(hint) = super::helpers::v3_manage_hint(
                                p.liquidity,
                                p.tokens_owed0,
                                p.tokens_owed1,
                            ) {
                                self.status = hint.into();
                                return KeyOutcome::Consumed;
                            }
                        }
                        match self.build_decrease_tx(wallet) {
                            Ok(tx) => self.confirm_tx(tx, LpConfirmAction::Decrease),
                            Err(e) => {
                                self.status = e;
                                KeyOutcome::Consumed
                            }
                        }
                    }
                    Tab::Collect => match self.build_collect_tx(wallet) {
                        Ok(tx) => self.confirm_tx(tx, LpConfirmAction::Collect),
                        Err(e) => {
                            self.status = e;
                            KeyOutcome::Consumed
                        }
                    },
                    _ => unreachable!(),
                }
            }
            Tab::Remove => match self.build_v2_remove_tx(wallet) {
                Ok(tx) => self.confirm_tx(tx, LpConfirmAction::V2Remove),
                Err(e) => {
                    self.status = e;
                    KeyOutcome::Consumed
                }
            },
            Tab::Transfer => match self.build_transfer_tx(wallet, handle) {
                Ok(tx) => self.confirm_tx(tx, LpConfirmAction::Transfer),
                Err(e) => {
                    self.status = e;
                    KeyOutcome::Consumed
                }
            },
            Tab::AddLp => KeyOutcome::Consumed,
        }
    }

    /// If NPM allowance is short for the Increase amounts, open Enable confirm first.
    fn try_increase_enable_first(
        &mut self,
        wallet: &WalletState,
        handle: &Handle,
    ) -> Option<KeyOutcome> {
        let pos = self.v3_positions.get(self.sel)?;
        let custom = wallet.custom_tokens_for_active_chain();
        let d0 = super::helpers::decimals_for_token(pos.token0, &[], &custom);
        let d1 = super::helpers::decimals_for_token(pos.token1, &[], &custom);
        let amount0 = parse_swap_amount(self.amount0.value(), "amount0", d0).ok()?;
        let amount1 = parse_swap_amount(self.amount1.value(), "amount1", d1).ok()?;
        let from = wallet.active_address().ok()?.to_string();
        let rpc = wallet.active_rpc_url();
        let token0 = pos.token0;
        let token1 = pos.token1;
        let needed = handle.block_on(vaughan_core::core::v3_lp_increase_enable_tx(
            &from,
            self.venue,
            self.chain_id,
            &rpc,
            token0,
            token1,
            amount0,
            amount1,
        ));
        match needed {
            Ok(Some((tx, label))) => {
                let sym = if label.contains("token0") {
                    super::helpers::symbol_for_token_address(self.chain_id, token0, &[], &custom)
                } else {
                    super::helpers::symbol_for_token_address(self.chain_id, token1, &[], &custom)
                };
                Some(self.open_enable_confirm_outcome(tx, sym, label))
            }
            Ok(None) => None,
            Err(e) => {
                self.status = e.user_message();
                Some(KeyOutcome::Consumed)
            }
        }
    }

    fn open_enable_confirm_outcome(
        &mut self,
        tx: vaughan_core::chains::EvmTransaction,
        symbol: String,
        label: String,
    ) -> KeyOutcome {
        self.open_enable_confirm(tx, symbol, label);
        KeyOutcome::Consumed
    }

    fn handle_done_key(&mut self, key: KeyEvent) -> KeyOutcome {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let Some(hash) = self.done_tx_hash.as_deref().filter(|h| !h.is_empty()) else {
                    self.status = "No transaction hash to copy".into();
                    return KeyOutcome::Consumed;
                };
                match crate::clipboard::copy_text(hash) {
                    Ok(()) => KeyOutcome::Flash("Transaction hash copied".into()),
                    Err(e) => {
                        self.status = e;
                        KeyOutcome::Consumed
                    }
                }
            }
            KeyCode::Char('o') | KeyCode::Char('O') => self.open_done_tx_scanner(),
            KeyCode::Enter | KeyCode::Esc => {
                self.dismiss_done();
                KeyOutcome::Consumed
            }
            _ => KeyOutcome::Consumed,
        }
    }

    fn open_done_tx_scanner(&mut self) -> KeyOutcome {
        use vaughan_core::chains::evm::networks::explorer_tx_url;

        let Some(hash) = self.done_tx_hash.as_deref().filter(|h| !h.is_empty()) else {
            self.status = "No transaction hash".into();
            return KeyOutcome::Consumed;
        };
        let Some(url) = explorer_tx_url(self.chain_id, hash) else {
            self.status = "No block explorer configured for this network".into();
            return KeyOutcome::Consumed;
        };
        match super::helpers::open_explorer_url(&url) {
            Ok(()) => {
                let name = if self.chain_id == 943 {
                    "Look Scanner"
                } else if self.chain_id == 369 {
                    "PulseScan"
                } else {
                    "block explorer"
                };
                KeyOutcome::Flash(format!("Opened {name}"))
            }
            Err(e) => {
                self.status = e;
                KeyOutcome::Consumed
            }
        }
    }

    pub(crate) fn dismiss_done(&mut self) {
        self.done_tx_hash = None;
        self.done_title.clear();
        self.stage = Stage::Input;
        self.tab = Tab::List;
        self.focus = Focus::None;
        self.list_action_idx = None;
        self.clear_transfer_lock();
        self.status = if self.lp_reload_pending || self.busy == Busy::Loading {
            "Refreshing positions…".into()
        } else {
            "↑↓ select · Enter open · ←→ tabs · r reload".into()
        };
    }

    /// Re-read `positions(tokenId)` so decrease amounts match chain (not a stale list).
    pub(crate) fn refresh_selected_v3_position(
        &mut self,
        wallet: &WalletState,
        handle: &Handle,
    ) -> Result<(), String> {
        let token_id = self
            .v3_positions
            .get(self.sel)
            .map(|p| p.token_id)
            .ok_or_else(|| "No position selected".to_string())?;
        let rpc = wallet.active_rpc_url();
        let venue = self.venue;
        let chain_id = self.chain_id;
        let fresh = handle
            .block_on(vaughan_core::core::get_v3_lp_position(
                &rpc, venue, chain_id, token_id,
            ))
            .map_err(|e| e.user_message())?;
        let idx = self.sel;
        if let Some(prev) = self.v3_positions.get(idx).cloned() {
            self.v3_positions[idx] = prev.with_updated_info(fresh);
        }
        self.sync_decrease_from_selection();
        Ok(())
    }
}
