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
    build_v3_decrease_evm, build_v3_increase_evm, chain_label, default_full_range_ticks,
    display_price_range_from_preset, format_display_amount, lp_stack_for_chain, lp_v3_venue_picker,
    min_out_after_slippage, v3_preview_mint_deposits_from_amount0,
    v3_preview_mint_deposits_from_amount1, v3_range_ticks_from_human_prices,
    v3_sqrt_and_tick_for_preview, venue_position_manager, venue_swap_router, wpls_for_chain,
    DexProtocol, DexVenue, LpStack, V2LpPosition, V3LpDeployWait, V3PoolLifecycle, V3PositionInfo,
    WalletState, DEFAULT_DEX_SLIPPAGE_BPS,
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
    pub(crate) fn lp_supported(&self) -> bool {
        match self.stack {
            LpStack::V3 { .. } => venue_position_manager(self.venue, self.chain_id).is_some(),
            LpStack::V2 { .. } => {
                venue_swap_router(self.venue, DexProtocol::V2, self.chain_id).is_some()
            }
        }
    }

    pub(crate) fn default_status_hint(&self) -> String {
        if self.lp_supported() {
            format!(
                "{} · {} · ←→ tab · r reload · Esc back",
                self.venue.label(),
                self.stack.label()
            )
        } else {
            format!(
                "{} {} unavailable on {} — switch network (F1)",
                self.venue.label(),
                self.stack.label(),
                chain_label(self.chain_id)
            )
        }
    }

    /// Token pair defaults when the venue changes. Does not touch fee tier (use ←→).
    pub(crate) fn apply_venue_token_defaults(&mut self, fill_price_hints: bool) {
        match (self.chain_id, self.venue) {
            (943, DexVenue::Wiz4rd) => {
                self.token0.set_value(WZRD_SMOKE_943);
                if let Some(wpls) = wpls_for_chain(self.chain_id) {
                    self.token1.set_value(format!("{wpls}"));
                }
                self.dec0.set_value("18");
                self.dec1.set_value("18");
            }
            (369, DexVenue::NineInch) => {
                self.token0.set_value(HEX_MAINNET);
                if let Some(wpls) = wpls_for_chain(self.chain_id) {
                    self.token1.set_value(format!("{wpls}"));
                }
                self.dec0.set_value("8");
                self.dec1.set_value("18");
            }
            (369, DexVenue::NineMm) => {
                if let Some(wpls) = wpls_for_chain(self.chain_id) {
                    self.token0.set_value(format!("{wpls}"));
                }
                self.token1
                    .set_value("0x7b39712Ef45F7dcED2bBDF11F3D5046bA61dA719");
                self.dec0.set_value("18");
                self.dec1.set_value("18");
                if fill_price_hints {
                    self.initial_price.set_value("0.001265324");
                    let (min, max) = display_price_range_from_preset(0.001_265_324, 50.0);
                    self.min_price.set_value(trim_float_string(min));
                    self.max_price.set_value(trim_float_string(max));
                    self.range_preset_idx = 5;
                    self.range_preset_applied = Some(5);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn apply_initial_fee_defaults(&mut self) {
        match (self.chain_id, self.venue) {
            (943, DexVenue::Wiz4rd) => self.fee_tier = 500,
            (369, DexVenue::NineInch) => self.fee_tier = 2500,
            (369, DexVenue::NineMm) => self.fee_tier = 10_000,
            _ => {}
        }
    }

    pub(crate) fn sync_decrease_from_selection(&mut self) {
        let idx = self.decrease_preset_applied.unwrap_or(0);
        self.decrease_preset_idx = idx;
        self.apply_decrease_preset(idx);
    }

    pub(crate) fn selected_position_liquidity(&self) -> Option<alloy::primitives::U256> {
        match self.stack {
            LpStack::V3 { .. } => self
                .v3_positions
                .get(self.sel)
                .map(|p| alloy::primitives::U256::from(p.liquidity)),
            LpStack::V2 { .. } => self.v2_positions.get(self.sel).map(|p| p.lp_balance),
        }
    }

    pub(crate) fn cycle_decrease_preset(&mut self, forward: bool) {
        let n = super::types::DECREASE_PRESETS.len();
        self.decrease_preset_idx = if forward {
            (self.decrease_preset_idx + 1) % n
        } else {
            (self.decrease_preset_idx + n - 1) % n
        };
        self.apply_decrease_preset(self.decrease_preset_idx);
    }

    pub(crate) fn apply_decrease_preset(&mut self, idx: usize) {
        let Some(total) = self.selected_position_liquidity() else {
            self.liquidity.set_value(String::new());
            return;
        };
        if total.is_zero() {
            self.liquidity.set_value("0".to_string());
            self.decrease_preset_idx = idx;
            self.decrease_preset_applied = Some(idx);
            return;
        }
        let pct = super::types::DECREASE_PRESETS
            .get(idx)
            .map(|(_, pct)| *pct)
            .unwrap_or(25);
        let remove = if pct >= 100 {
            total
        } else {
            total * alloy::primitives::U256::from(pct) / alloy::primitives::U256::from(100u64)
        };
        let remove = if remove.is_zero() {
            alloy::primitives::U256::from(1u64)
        } else {
            remove
        }
        .min(total);
        self.decrease_preset_idx = idx;
        self.decrease_preset_applied = Some(idx);
        self.liquidity.set_value(remove.to_string());
    }

    pub(crate) fn clear_decrease_preset_if_edited(&mut self) {
        self.decrease_preset_applied = None;
    }

    pub(crate) fn on_manage_tab(&self) -> bool {
        matches!(self.tab, Tab::Decrease | Tab::Remove)
    }

    /// Manage actions offered after Enter on a List row.
    pub(crate) fn list_manage_actions(&self) -> &'static [Tab] {
        match self.stack {
            LpStack::V3 { .. } => &[Tab::Increase, Tab::Decrease, Tab::Collect],
            LpStack::V2 { .. } => &[Tab::Remove],
        }
    }

    /// Prefer positions with meaningful liquidity (skip Empty/Dust for ↑↓).
    pub(crate) fn position_has_liquidity(&self, idx: usize) -> bool {
        match self.stack {
            LpStack::V3 { .. } => self
                .v3_positions
                .get(idx)
                .is_some_and(|p| p.liquidity > 1),
            LpStack::V2 { .. } => self
                .v2_positions
                .get(idx)
                .is_some_and(|p| !p.lp_balance.is_zero()),
        }
    }

    pub(crate) fn list_len(&self) -> usize {
        match self.stack {
            LpStack::V3 { .. } => self.v3_positions.len(),
            LpStack::V2 { .. } => self.v2_positions.len(),
        }
    }

    /// Indices to walk with ↑↓ — prefer positions that still hold liquidity.
    pub(crate) fn list_nav_indices(&self) -> Vec<usize> {
        let len = self.list_len();
        let liquid: Vec<usize> = (0..len).filter(|&i| self.position_has_liquidity(i)).collect();
        if liquid.is_empty() {
            (0..len).collect()
        } else {
            liquid
        }
    }

    /// Keep `sel` on a navigable row (prefer liquid).
    pub(crate) fn clamp_list_sel(&mut self) {
        let nav = self.list_nav_indices();
        if nav.is_empty() {
            self.sel = 0;
            self.list_action_idx = None;
            return;
        }
        if !nav.contains(&self.sel) {
            self.sel = nav[0];
        }
        self.sync_decrease_from_selection();
    }

    pub(crate) fn move_list_sel(&mut self, down: bool) {
        let nav = self.list_nav_indices();
        if nav.is_empty() {
            return;
        }
        let pos = nav.iter().position(|&i| i == self.sel).unwrap_or(0);
        let next = if down {
            (pos + 1) % nav.len()
        } else if pos == 0 {
            nav.len() - 1
        } else {
            pos - 1
        };
        self.sel = nav[next];
        self.list_action_idx = None;
        self.sync_decrease_from_selection();
    }

    pub(crate) fn open_list_actions(&mut self) {
        if self.list_len() == 0 {
            self.status = "No positions — Add LP or press r to reload".into();
            return;
        }
        self.clamp_list_sel();
        self.list_action_idx = Some(0);
        // Key shortcuts live on the bottom hint bar only (avoid duplicating status).
        self.status = match self.stack {
            LpStack::V3 { .. } => self
                .v3_positions
                .get(self.sel)
                .and_then(|p| {
                    super::helpers::v3_manage_hint(p.liquidity, p.tokens_owed0, p.tokens_owed1)
                })
                .unwrap_or("")
                .into(),
            LpStack::V2 { .. } => String::new(),
        };
    }

    /// Letter shortcut from the focused position view (`i` / `d` / `c`, or V2 `r`).
    pub(crate) fn apply_list_action_key(&mut self, key: char) -> bool {
        if self.list_action_idx.is_none() {
            return false;
        }
        let tab = match (self.stack, key.to_ascii_lowercase()) {
            (LpStack::V3 { .. }, 'i') => Tab::Increase,
            (LpStack::V3 { .. }, 'd') => Tab::Decrease,
            (LpStack::V3 { .. }, 'c') => Tab::Collect,
            (LpStack::V2 { .. }, 'r') => Tab::Remove,
            _ => return false,
        };
        self.list_action_idx = None;
        self.tab = tab;
        self.on_tab_changed();
        let pos_label = match self.stack {
            LpStack::V3 { .. } => self
                .v3_positions
                .get(self.sel)
                .map(|p| format!("NFT #{}", p.token_id))
                .unwrap_or_else(|| format!("row {}", self.sel + 1)),
            LpStack::V2 { .. } => format!("pair {}", self.sel + 1),
        };
        self.status = format!(
            "{} · {pos_label} · ←→ tabs · Enter confirm · Esc list",
            tab.label()
        );
        true
    }

    /// Leave List action focus and open the chosen manage tab for `sel`.
    pub(crate) fn enter_list_action(&mut self) {
        let actions = self.list_manage_actions();
        let Some(tab) = actions.first().copied() else {
            self.list_action_idx = None;
            return;
        };
        self.list_action_idx = None;
        self.tab = tab;
        self.on_tab_changed();
        let pos_label = match self.stack {
            LpStack::V3 { .. } => self
                .v3_positions
                .get(self.sel)
                .map(|p| format!("NFT #{}", p.token_id))
                .unwrap_or_else(|| format!("row {}", self.sel + 1)),
            LpStack::V2 { .. } => format!("pair {}", self.sel + 1),
        };
        self.status = format!(
            "{} · {pos_label} · ←→ tabs · Enter confirm · Esc list",
            tab.label()
        );
    }

    pub(crate) fn close_list_actions(&mut self) {
        self.list_action_idx = None;
        self.status = "↑↓ select · Enter open · ←→ tabs · r reload".into();
    }

    pub(crate) fn on_tab_changed(&mut self) {
        self.focus = Focus::None;
        self.list_action_idx = None;
        self.reset_enable_state();
        if self.tab == Tab::AddLp {
            self.add_step = AddStep::SelectPair;
            self.clear_pool_quote();
        }
        if matches!(self.tab, Tab::Decrease | Tab::Remove) {
            if self.decrease_preset_applied.is_none() {
                self.decrease_preset_idx = 0;
            }
            self.sync_decrease_from_selection();
        }
    }

    pub(crate) fn clear_pool_quote(&mut self) {
        self.pool_lifecycle = None;
        self.pool_sqrt_x96 = None;
        self.pool_tick = None;
        self.v3_custom_range = false;
        self.pool_quote_inflight = false;
    }

    /// Assume a new pool until background RPC confirms otherwise (local preview only).
    pub(crate) fn begin_optimistic_pool_preview(&mut self) {
        self.pool_lifecycle = Some(V3PoolLifecycle::Missing);
        self.pool_sqrt_x96 = None;
        self.pool_tick = None;
        self.v3_custom_range = false;
        self.pool_quote_inflight = false;
    }

    pub(crate) fn spawn_pool_quote_job(&mut self, wallet: &WalletState) -> KeyOutcome {
        let Some(job) = self.pool_quote_job(wallet) else {
            return KeyOutcome::Consumed;
        };
        self.pool_quote_inflight = true;
        self.refresh_price_deposit_status();
        KeyOutcome::StartJob(job)
    }

    pub(crate) fn pool_quote_job(&self, wallet: &WalletState) -> Option<UiJob> {
        if !matches!(self.stack, LpStack::V3 { .. }) {
            return None;
        }
        let pair = self.sorted_pair().ok()?;
        Some(UiJob::LpV3PoolQuote {
            venue: self.venue,
            chain_id: self.chain_id,
            rpc_url: wallet.active_rpc_url(),
            token0: format!("{:#x}", pair.token0),
            token1: format!("{:#x}", pair.token1),
            fee: self.fee_tier,
            dec0: pair.dec0,
            dec1: pair.dec1,
        })
    }

    pub(crate) fn apply_pool_quote(&mut self, quote: vaughan_core::core::V3LpPoolQuote) {
        self.pool_quote_inflight = false;
        if self.add_step != AddStep::PriceDeposit {
            return;
        }
        if let Some(fee) = quote.suggested_fee_tier {
            self.fee_tier = fee;
        }
        self.pool_lifecycle = Some(quote.lifecycle);
        self.pool_sqrt_x96 = quote.sqrt_price_x96;
        self.pool_tick = quote.tick;
        if quote.lifecycle == V3PoolLifecycle::Ready {
            if let Some(pool_price) = quote.pool_price_token1_per_token0 {
                if let Ok(pair) = self.sorted_pair() {
                    let user = self.pool_price_to_user_price(pair.first_is_token0, &pool_price);
                    self.initial_price.set_value(user);
                }
            }
        }
        self.resync_range_bounds_from_preset();
        self.sync_amount1_from_price();
        self.normalize_v3_price_focus();
        if self.range_preset_applied.is_some() {
            self.focus_v3_deposit_after_preset();
        }
        self.refresh_price_deposit_status();
        if quote.suggested_fee_tier.is_some() {
            self.status = format!(
                "Pool found at {} — fee tier updated · pick range · e enable · Enter add",
                fee_tier_display(self.fee_tier)
            );
        }
        self.schedule_enable_recheck();
    }

    pub(crate) fn needs_v3_starting_price(&self) -> bool {
        matches!(
            self.pool_lifecycle,
            Some(V3PoolLifecycle::Missing) | Some(V3PoolLifecycle::Uninitialized { .. }) | None
        )
    }

    pub(crate) fn on_v3_price_deposit(&self) -> bool {
        matches!(self.stack, LpStack::V3 { .. }) && self.add_step == AddStep::PriceDeposit
    }

    /// Keep keyboard focus on a visible field (pool quote / custom-range toggles hide inputs).
    pub(crate) fn normalize_v3_price_focus(&mut self) {
        if !self.on_v3_price_deposit() {
            return;
        }
        match self.focus {
            Focus::MinPrice | Focus::MaxPrice if !self.v3_custom_range => {
                self.focus = Focus::RangePresets;
            }
            Focus::InitialPrice
                if !self.needs_v3_starting_price()
                    || (!self.v3_custom_range
                        && self.pool_lifecycle == Some(V3PoolLifecycle::Ready)) =>
            {
                self.focus = Focus::Amount0;
            }
            _ => {}
        }
    }

    pub(crate) fn focus_v3_deposit_after_preset(&mut self) {
        if !self.on_v3_price_deposit() {
            return;
        }
        self.normalize_v3_price_focus();
        self.focus = self.next_v3_focus_after_presets();
    }

    pub(crate) fn next_v3_focus_after_presets(&self) -> Focus {
        if self.v3_custom_range {
            Focus::MinPrice
        } else if self.needs_v3_starting_price() {
            Focus::InitialPrice
        } else {
            Focus::Amount0
        }
    }

    pub(crate) fn toggle_v3_custom_range(&mut self) {
        self.v3_custom_range = !self.v3_custom_range;
        self.status = if self.v3_custom_range {
            "Fine-tune min / current / max · a = back to presets only".into()
        } else {
            "Preset range · a = adjust min/max prices later".into()
        };
        if !self.v3_custom_range
            && (matches!(self.focus, Focus::MinPrice | Focus::MaxPrice)
                || (self.focus == Focus::InitialPrice && !self.needs_v3_starting_price()))
        {
            self.focus = Focus::RangePresets;
        }
        self.normalize_v3_price_focus();
    }

    /// Plain-language hint for which deposit field(s) matter at the current price + range.
    pub(crate) fn v3_deposit_guidance(&self, sym0: &str, sym1: &str) -> String {
        let pair = match self.sorted_pair() {
            Ok(p) => p,
            Err(_) => return format!("Enter how much {sym0} to deposit"),
        };
        let pool_initial =
            match self.user_price_to_pool_price(pair.first_is_token0, self.initial_price.value()) {
                Ok(s) if !s.trim().is_empty() => s,
                _ => return format!("Set starting price, then enter {sym0} amount"),
            };
        let (pool_min, pool_max) = self
            .user_price_range_to_pool_prices(
                pair.first_is_token0,
                self.min_price.value(),
                self.max_price.value(),
            )
            .unwrap_or_default();
        let (sqrt, tick) = match v3_sqrt_and_tick_for_preview(
            self.chain_id,
            pair.token0,
            pair.token1,
            pair.dec0,
            pair.dec1,
            self.fee_tier,
            self.pool_sqrt_x96,
            self.pool_tick,
            &pool_initial,
        ) {
            Ok(v) => v,
            Err(_) => {
                return format!("Enter {sym0} — {sym1} is calculated automatically");
            }
        };
        let (tick_lower, tick_upper) =
            match if pool_min.trim().is_empty() || pool_max.trim().is_empty() {
                default_full_range_ticks(self.fee_tier)
            } else {
                v3_range_ticks_from_human_prices(
                    self.chain_id,
                    pair.token0,
                    pair.token1,
                    pair.dec0,
                    pair.dec1,
                    pool_min.trim(),
                    pool_max.trim(),
                    self.fee_tier,
                )
            } {
                Ok(t) => t,
                Err(_) => {
                    return format!("Enter {sym0} — {sym1} is calculated automatically");
                }
            };
        let _ = sqrt;
        let (ui_primary, ui_auto, pool_token0_sym, pool_token1_sym) = if pair.first_is_token0 {
            (sym0, sym1, sym0, sym1)
        } else {
            (sym0, sym1, sym1, sym0)
        };
        if tick < tick_lower {
            let only = pool_token0_sym;
            if only == ui_primary {
                format!("Price is below your range — only {only} is needed")
            } else {
                format!(
                    "Price is below your range — only {only} is needed (widen preset or a to adjust)"
                )
            }
        } else if tick >= tick_upper {
            let only = pool_token1_sym;
            if only == ui_primary {
                format!("Price is above your range — only {only} is needed")
            } else {
                format!(
                    "Price is above your range — only {only} is needed (widen preset or a to adjust)"
                )
            }
        } else {
            format!("Enter {ui_primary} — {ui_auto} fills in automatically")
        }
    }

    pub(crate) fn simple_range_explainer(&self) -> &'static str {
        "Earn swap fees while the price stays inside your range · wider = safer · narrower = higher yield"
    }

    pub(crate) fn render_simple_range_summary(
        &self,
        frame: &mut Frame,
        area: Rect,
        sym0: &str,
        sym1: &str,
    ) {
        let inner = brand::render_faded_box(frame, area, Some(brand::fade_line(" Your range ")));
        let current = self.initial_price.value().trim();
        let current_line = if current.is_empty() {
            Line::from(Span::styled(
                format!("Current: set 1 {sym0} = … {sym1} below"),
                Style::default().fg(Color::DarkGray),
            ))
        } else {
            Line::from(vec![
                Span::styled("Current ", Style::default().fg(Color::DarkGray)),
                Span::raw(format_unit_price(current, sym0, sym1)),
            ])
        };
        let band = self.range_band_label();
        let band_line = Line::from(vec![
            Span::styled("Width ", Style::default().fg(Color::DarkGray)),
            Span::raw(band),
        ]);
        let fee_line = Line::from(Span::styled(
            self.v3_deposit_guidance(sym0, sym1),
            Style::default().fg(brand::body_color()),
        ));
        frame.render_widget(
            Paragraph::new(vec![current_line, band_line, fee_line]).alignment(Alignment::Center),
            inner,
        );
    }

    pub(crate) fn sync_decimals_for_token(addr: &str, dec: &mut Input, assets: &[Balance]) {
        let want = addr.trim();
        if want.is_empty() {
            return;
        }
        if let Some(b) = assets.iter().find(|b| {
            b.token
                .contract_address
                .as_ref()
                .is_some_and(|a| a.eq_ignore_ascii_case(want))
        }) {
            dec.set_value(b.token.decimals.to_string());
        }
    }

    pub(crate) fn center_price_f64(&self) -> Option<f64> {
        parse_price_f64(self.initial_price.value()).ok()
    }

    pub(crate) fn range_band_label(&self) -> String {
        let min_raw = self.min_price.value().trim();
        let max_raw = self.max_price.value().trim();
        if min_raw.is_empty() && max_raw.is_empty() {
            return "Full range".into();
        }
        let Some(center) = self.center_price_f64() else {
            return "Set current price".into();
        };
        if center <= 0.0 {
            return "—".into();
        }
        let mut parts = Vec::new();
        if let Ok(min) = parse_price_f64(min_raw) {
            let lo = (1.0 - min / center) * 100.0;
            if lo.is_finite() {
                parts.push(format!("-{lo:.1}%"));
            }
        }
        if let Ok(max) = parse_price_f64(max_raw) {
            let hi = (max / center - 1.0) * 100.0;
            if hi.is_finite() {
                parts.push(format!("+{hi:.1}%"));
            }
        }
        if parts.is_empty() {
            "Custom range".into()
        } else {
            parts.join(" / ")
        }
    }

    pub(crate) fn cycle_range_preset_highlight(&mut self, forward: bool) {
        let n = RANGE_PRESETS.len();
        self.range_preset_idx = if forward {
            (self.range_preset_idx + 1) % n
        } else {
            (self.range_preset_idx + n - 1) % n
        };
    }

    pub(crate) fn apply_range_preset(&mut self, idx: usize) {
        if idx >= RANGE_PRESETS.len() {
            return;
        }
        self.range_preset_idx = idx;
        self.range_preset_applied = Some(idx);
        let (_, pct) = RANGE_PRESETS[idx];
        match pct {
            None => {
                self.min_price.set_value("");
                self.max_price.set_value("");
            }
            Some(p) => {
                let center = self.center_price_f64().unwrap_or(1.0);
                let (min, max) = display_price_range_from_preset(center, p);
                self.min_price.set_value(trim_float_string(min));
                self.max_price.set_value(trim_float_string(max));
                if self.initial_price.value().trim().is_empty() {
                    self.initial_price.set_value(trim_float_string(center));
                }
                self.sync_amount1_from_price();
            }
        }
        self.refresh_price_deposit_status();
        self.focus_v3_deposit_after_preset();
    }

    pub(crate) fn sync_amount1_from_price(&mut self) {
        let Some(ctx) = self.v3_deposit_preview_context() else {
            return;
        };
        let V3DepositPreviewContext {
            pair,
            pool_min,
            pool_max,
            sqrt,
            tick,
        } = ctx;

        if pair.first_is_token0 {
            let amount0_wei = match parse_swap_amount(self.amount0.value(), "amount0", pair.dec0) {
                Ok(w) => w,
                Err(_) => return,
            };
            if amount0_wei.is_zero() {
                return;
            }
            match v3_preview_mint_deposits_from_amount0(
                self.chain_id,
                pair.token0,
                pair.token1,
                pair.dec0,
                pair.dec1,
                self.fee_tier,
                sqrt,
                tick,
                &pool_min,
                &pool_max,
                amount0_wei,
            ) {
                Ok((_a0, a1)) => {
                    self.amount1.set_value(format_display_amount(
                        &a1.to_string(),
                        pair.dec1,
                        SWAP_DISPLAY_FRAC,
                    ));
                }
                Err(_) => self.sync_amount1_simple(),
            }
        } else {
            let amount1_wei = match parse_swap_amount(self.amount0.value(), "amount0", pair.dec1) {
                Ok(w) => w,
                Err(_) => return,
            };
            if amount1_wei.is_zero() {
                return;
            }
            match v3_preview_mint_deposits_from_amount1(
                self.chain_id,
                pair.token0,
                pair.token1,
                pair.dec0,
                pair.dec1,
                self.fee_tier,
                sqrt,
                tick,
                &pool_min,
                &pool_max,
                amount1_wei,
            ) {
                Ok((a0, _a1)) => {
                    self.amount1.set_value(format_display_amount(
                        &a0.to_string(),
                        pair.dec0,
                        SWAP_DISPLAY_FRAC,
                    ));
                }
                Err(_) => self.sync_amount1_simple(),
            }
        }
    }

    pub(crate) fn sync_amount0_from_price(&mut self) {
        let Some(ctx) = self.v3_deposit_preview_context() else {
            return;
        };
        let V3DepositPreviewContext {
            pair,
            pool_min,
            pool_max,
            sqrt,
            tick,
        } = ctx;

        if pair.first_is_token0 {
            let amount1_wei = match parse_swap_amount(self.amount1.value(), "amount1", pair.dec1) {
                Ok(w) => w,
                Err(_) => return,
            };
            if amount1_wei.is_zero() {
                return;
            }
            match v3_preview_mint_deposits_from_amount1(
                self.chain_id,
                pair.token0,
                pair.token1,
                pair.dec0,
                pair.dec1,
                self.fee_tier,
                sqrt,
                tick,
                &pool_min,
                &pool_max,
                amount1_wei,
            ) {
                Ok((a0, _a1)) => {
                    self.amount0.set_value(format_display_amount(
                        &a0.to_string(),
                        pair.dec0,
                        SWAP_DISPLAY_FRAC,
                    ));
                }
                Err(_) => self.sync_amount0_simple(),
            }
        } else {
            let amount0_wei = match parse_swap_amount(self.amount1.value(), "amount1", pair.dec0) {
                Ok(w) => w,
                Err(_) => return,
            };
            if amount0_wei.is_zero() {
                return;
            }
            match v3_preview_mint_deposits_from_amount0(
                self.chain_id,
                pair.token0,
                pair.token1,
                pair.dec0,
                pair.dec1,
                self.fee_tier,
                sqrt,
                tick,
                &pool_min,
                &pool_max,
                amount0_wei,
            ) {
                Ok((_a0, a1)) => {
                    self.amount0.set_value(format_display_amount(
                        &a1.to_string(),
                        pair.dec1,
                        SWAP_DISPLAY_FRAC,
                    ));
                }
                Err(_) => self.sync_amount0_simple(),
            }
        }
    }

    pub(crate) fn v3_deposit_preview_context(&self) -> Option<V3DepositPreviewContext> {
        let pair = self.sorted_pair().ok()?;
        let pool_initial = self
            .user_price_to_pool_price(pair.first_is_token0, self.initial_price.value())
            .ok()?;
        if pool_initial.trim().is_empty() {
            return None;
        }
        let (pool_min, pool_max) = self
            .user_price_range_to_pool_prices(
                pair.first_is_token0,
                self.min_price.value(),
                self.max_price.value(),
            )
            .unwrap_or_default();
        let (sqrt, tick) = v3_sqrt_and_tick_for_preview(
            self.chain_id,
            pair.token0,
            pair.token1,
            pair.dec0,
            pair.dec1,
            self.fee_tier,
            self.pool_sqrt_x96,
            self.pool_tick,
            &pool_initial,
        )
        .ok()?;
        Some(V3DepositPreviewContext {
            pair,
            pool_min,
            pool_max,
            sqrt,
            tick,
        })
    }

    pub(crate) fn sync_amount1_simple(&mut self) {
        let Ok(a0) = self.amount0.value().trim().parse::<f64>() else {
            return;
        };
        let Some(p) = self.center_price_f64() else {
            return;
        };
        if a0 <= 0.0 || p <= 0.0 {
            return;
        }
        self.amount1.set_value(trim_float_string(a0 * p));
    }

    pub(crate) fn sync_amount0_simple(&mut self) {
        let Ok(a1) = self.amount1.value().trim().parse::<f64>() else {
            return;
        };
        let Some(p) = self.center_price_f64() else {
            return;
        };
        if a1 <= 0.0 || p <= 0.0 {
            return;
        }
        self.amount0.set_value(trim_float_string(a1 / p));
    }

    pub(crate) fn refresh_price_deposit_status(&mut self) {
        if self.add_step != AddStep::PriceDeposit {
            return;
        }
        if let Some(idx) = self.range_preset_applied {
            let (label, pct) = RANGE_PRESETS[idx];
            if pct.is_none() {
                self.status = "Full range — set starting price if new pool, then deposits".into();
            } else if self.needs_v3_starting_price() {
                self.status =
                    format!("Range {label} — edit starting price below, then deposit amounts");
            } else {
                self.status = format!("Range {label} — enter deposit amounts (Tab between fields)");
            }
        } else {
            match self.pool_lifecycle {
                Some(V3PoolLifecycle::Ready) => {
                    self.status = format!(
                        "{} · price loaded · pick a range preset (a to fine-tune later)",
                        self.venue.label()
                    );
                }
                Some(V3PoolLifecycle::Missing) => {
                    self.status =
                        "New pool — pick range preset · set starting price · then deposit".into();
                }
                Some(V3PoolLifecycle::Uninitialized { .. }) => {
                    self.status =
                        "Pool needs starting price — pick range preset · then deposit".into();
                }
                None => {}
            }
        }
        if self.pool_quote_inflight {
            self.status.push_str(" · verifying on chain…");
        }
    }

    pub(crate) fn resync_range_bounds_from_preset(&mut self) {
        let Some(idx) = self.range_preset_applied else {
            return;
        };
        let (_, pct) = RANGE_PRESETS[idx];
        match pct {
            None => {
                self.min_price.set_value("");
                self.max_price.set_value("");
            }
            Some(p) => {
                let center = self.center_price_f64().unwrap_or(1.0);
                let (min, max) = display_price_range_from_preset(center, p);
                self.min_price.set_value(trim_float_string(min));
                self.max_price.set_value(trim_float_string(max));
            }
        }
    }

    pub(crate) fn on_v3_price_field_edited(&mut self) {
        match self.focus {
            Focus::InitialPrice => self.resync_range_bounds_from_preset(),
            Focus::MinPrice | Focus::MaxPrice => self.clear_range_preset_if_price_edited(),
            _ => {}
        }
    }

    pub(crate) fn clear_range_preset_if_price_edited(&mut self) {
        self.range_preset_applied = None;
    }

    pub(crate) fn sorted_pair(&self) -> Result<SortedPair, String> {
        let first = parse_token_address(self.token0.value(), "first token")?;
        let second = parse_token_address(self.token1.value(), "second token")?;
        let dec_first = Self::parse_decimals(self.dec0.value(), "decimals0")?;
        let dec_second = Self::parse_decimals(self.dec1.value(), "decimals1")?;
        if first < second {
            Ok(SortedPair {
                token0: first,
                token1: second,
                dec0: dec_first,
                dec1: dec_second,
                first_is_token0: true,
            })
        } else {
            Ok(SortedPair {
                token0: second,
                token1: first,
                dec0: dec_second,
                dec1: dec_first,
                first_is_token0: false,
            })
        }
    }

    pub(crate) fn user_price_to_pool_price(
        &self,
        first_is_token0: bool,
        user_price: &str,
    ) -> Result<String, String> {
        vaughan_core::core::user_price_to_pool_price(first_is_token0, user_price)
            .map_err(|e| e.user_message())
    }

    /// Map UI min/max (2nd per 1st) to ascending pool prices (token1 per token0).
    pub(crate) fn user_price_range_to_pool_prices(
        &self,
        first_is_token0: bool,
        user_min: &str,
        user_max: &str,
    ) -> Result<(String, String), String> {
        vaughan_core::core::user_price_range_to_pool_prices(first_is_token0, user_min, user_max)
            .map_err(|e| e.user_message())
    }

    pub(crate) fn pool_price_to_user_price(
        &self,
        first_is_token0: bool,
        pool_price: &str,
    ) -> String {
        vaughan_core::core::pool_price_to_user_price(first_is_token0, pool_price)
    }

    pub(crate) fn sync_amount1_from_ratio(&mut self) {
        let Ok(a0) = self.amount0.value().trim().parse::<f64>() else {
            return;
        };
        let Ok(r) = self.ratio.value().trim().parse::<f64>() else {
            return;
        };
        if a0 <= 0.0 || r <= 0.0 {
            return;
        }
        self.amount1.set_value(trim_float_string(a0 * r));
    }

    pub(crate) fn sync_ratio_from_amounts(&mut self) {
        let Ok(a0) = self.amount0.value().trim().parse::<f64>() else {
            return;
        };
        let Ok(a1) = self.amount1.value().trim().parse::<f64>() else {
            return;
        };
        if a0 <= 0.0 {
            return;
        }
        self.ratio.set_value(trim_float_string(a1 / a0));
    }
}
