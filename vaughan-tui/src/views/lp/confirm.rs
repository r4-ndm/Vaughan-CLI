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
use vaughan_core::chains::{Balance, EvmTransaction, Fee, FeeSpeed};
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
    fee_tier_display, format_unit_price, friendly_deploy_action, lp_tx_error_message,
    parse_price_f64, render_unit_price_input, trim_float_string,
};
use super::types::*;

impl LpView {
    pub(crate) fn is_fee_confirm(&self) -> bool {
        self.confirm_ui.is_some()
    }

    pub(crate) fn begin_confirm(
        &mut self,
        tx: EvmTransaction,
        action: LpConfirmAction,
    ) -> KeyOutcome {
        let lines = self.build_confirm_summary(&action);
        self.confirm_ui = Some(Box::new(LpConfirmUi {
            action,
            lines,
            pending_tx: tx.clone(),
            pending_fee_estimate: None,
            base_fee: None,
            speed: FeeSpeed::Normal,
            custom_gas: Input::new(false, "gwei"),
            focus: LpConfirmFocus::Speed,
            pipeline_step: false,
        }));
        self.confirm_lines.clear();
        self.stage = Stage::Confirm;
        self.busy = Busy::EstimatingFee;
        self.status = "Estimating gas…".into();
        KeyOutcome::StartJob(UiJob::EstimateEvmFee { tx })
    }

    pub(crate) fn cancel_confirm(&mut self) {
        if self.lp_deploy_active {
            self.lp_deploy_active = false;
            self.lp_deploy_pending_resume = false;
            self.lp_deploy_last_step = LpDeployLastStep::None;
            self.lp_deploy_sent_step = LpDeployLastStep::None;
            self.lp_pipeline_phase = LpPipelinePhase::None;
            self.lp_pipeline_custom_gwei.clear();
            self.lp_deploy_last_label.clear();
            self.lp_deploy_followup_wait = None;
        }
        if self.lp_enable_in_confirm {
            self.lp_enable_in_confirm = false;
        }
        self.stage = Stage::Input;
        self.confirm_ui = None;
        self.confirm_lines.clear();
        self.busy = Busy::Idle;
    }

    pub(crate) fn open_deploy_confirm(
        &mut self,
        tx: EvmTransaction,
        step: LpDeployLastStep,
        label: String,
    ) {
        match self.lp_pipeline_phase {
            LpPipelinePhase::Review => self.open_add_review_confirm(tx, step, label),
            LpPipelinePhase::Execute => self.open_pipeline_step_confirm(tx, step, label),
            LpPipelinePhase::None => self.open_legacy_deploy_confirm(tx, step, label),
        }
    }

    fn open_legacy_deploy_confirm(
        &mut self,
        tx: EvmTransaction,
        step: LpDeployLastStep,
        label: String,
    ) {
        let action = LpConfirmAction::Deploy { step, label };
        let lines = self.build_confirm_summary(&action);
        self.confirm_ui = Some(Box::new(LpConfirmUi {
            action,
            lines,
            pending_tx: tx.clone(),
            pending_fee_estimate: Some(tx),
            base_fee: None,
            speed: FeeSpeed::Normal,
            custom_gas: Input::new(false, "gwei"),
            focus: LpConfirmFocus::Speed,
            pipeline_step: false,
        }));
        self.confirm_lines.clear();
        self.stage = Stage::Confirm;
        self.busy = Busy::EstimatingFee;
        self.status = "Estimating gas…".into();
    }

    fn open_add_review_confirm(
        &mut self,
        tx: EvmTransaction,
        step: LpDeployLastStep,
        label: String,
    ) {
        let action = LpConfirmAction::AddReview;
        let mut lines = self.build_add_review_summary();
        if step != LpDeployLastStep::AddLiquidity {
            lines.push(Line::from(format!(
                "Setup:   {} before deposit",
                friendly_deploy_action(&label)
            )));
        }
        self.confirm_ui = Some(Box::new(LpConfirmUi {
            action,
            lines,
            pending_tx: tx.clone(),
            pending_fee_estimate: Some(tx),
            base_fee: None,
            speed: FeeSpeed::Normal,
            custom_gas: Input::new(false, "gwei"),
            focus: LpConfirmFocus::Speed,
            pipeline_step: false,
        }));
        self.confirm_lines.clear();
        self.stage = Stage::Confirm;
        self.busy = Busy::EstimatingFee;
        self.status = "Review deposit · Enter to continue".into();
    }

    fn open_pipeline_step_confirm(
        &mut self,
        tx: EvmTransaction,
        step: LpDeployLastStep,
        label: String,
    ) {
        let action = LpConfirmAction::Deploy {
            step,
            label: label.clone(),
        };
        let mut lines = Vec::new();
        if let Some((n, total)) = deploy_step_number(step) {
            lines.push(Line::from(format!("Add LP · step {n}/{total}")));
        }
        lines.push(Line::from(format!(
            "Action:  {}",
            self.friendly_deploy_action_with_symbols(&label)
        )));
        lines.push(Line::from(format!(
            "Gas:     {} (from review)",
            self.lp_pipeline_speed.label()
        )));
        self.confirm_ui = Some(Box::new(LpConfirmUi {
            action,
            lines,
            pending_tx: tx.clone(),
            pending_fee_estimate: Some(tx),
            base_fee: None,
            speed: self.lp_pipeline_speed,
            custom_gas: Input::new(false, "gwei"),
            focus: LpConfirmFocus::Speed,
            pipeline_step: true,
        }));
        if !self.lp_pipeline_custom_gwei.is_empty() {
            if let Some(ui) = self.confirm_ui.as_mut() {
                ui.custom_gas
                    .set_value(self.lp_pipeline_custom_gwei.clone());
            }
        }
        self.confirm_lines.clear();
        self.stage = Stage::Confirm;
        self.busy = Busy::EstimatingFee;
        self.status = "Enter send · Esc cancel".into();
    }

    fn build_add_review_summary(&self) -> Vec<Line<'static>> {
        let net = chain_label(self.chain_id);
        let pair = self.form_pair_label();
        let fee = fee_tier_display(self.fee_tier);
        let (sym0, sym1) = self.form_token_symbols();
        vec![
            Line::from("Add liquidity (review)"),
            Line::from(format!("Pair:    {pair} · {fee}")),
            Line::from(format!(
                "Deposit: {} {sym0} + {} {sym1}",
                self.amount0.value().trim(),
                self.amount1.value().trim()
            )),
            Line::from(format!("Range:   {}", self.range_summary())),
            Line::from(format!("Network: {net}")),
            Line::from(""),
            Line::from("New pools may need create + initialize txs first."),
            Line::from("Enable both tokens on the form before this review."),
        ]
    }

    pub(crate) fn build_enable_confirm_summary(
        &self,
        action: &LpConfirmAction,
    ) -> Vec<Line<'static>> {
        let net = chain_label(self.chain_id);
        let npm = vaughan_core::core::venue_position_manager(self.venue, self.chain_id)
            .map(|a| format!("{a:#x}"))
            .unwrap_or_else(|| "—".into());
        match action {
            LpConfirmAction::Enable { symbol, label } => vec![
                Line::from(format!("Enable {symbol}")),
                Line::from(format!(
                    "Action:  {}",
                    self.friendly_deploy_action_with_symbols(label)
                )),
                Line::from(format!("Spender: NPM {npm}")),
                Line::from(format!("Network: {net}")),
            ],
            _ => vec![Line::from("Enable token")],
        }
    }

    fn friendly_deploy_action_with_symbols(&self, label: &str) -> String {
        let (sym0, sym1) = self.form_token_symbols();
        let mapped = label.replace("token0", &sym0).replace("token1", &sym1);
        friendly_deploy_action(&mapped)
    }

    pub(crate) fn build_confirm_summary(&self, action: &LpConfirmAction) -> Vec<Line<'static>> {
        let net = chain_label(self.chain_id);
        let pair = self.form_pair_label();
        let fee = fee_tier_display(self.fee_tier);
        let mut lines = Vec::new();

        match action {
            LpConfirmAction::Enable { .. } => {
                return self.build_enable_confirm_summary(action);
            }
            LpConfirmAction::AddReview => {
                return self.build_add_review_summary();
            }
            LpConfirmAction::Deploy { step, label } => {
                if let Some((n, total)) = deploy_step_number(*step) {
                    lines.push(Line::from(format!("Add LP · step {n}/{total}")));
                } else {
                    lines.push(Line::from("Add LP"));
                }
                lines.push(Line::from(format!(
                    "Action:  {}",
                    self.friendly_deploy_action_with_symbols(label)
                )));
                lines.push(Line::from(format!("Pair:    {pair} · {fee}")));
                lines.push(Line::from(format!("Network: {net}")));
                match step {
                    LpDeployLastStep::Initialize => {
                        let (sym0, sym1) = self.form_token_symbols();
                        lines.push(Line::from(format!(
                            "Price:   1 {sym0} = {} {sym1}",
                            self.initial_price.value().trim()
                        )));
                    }
                    LpDeployLastStep::AddLiquidity => {
                        let (sym0, sym1) = self.form_token_symbols();
                        lines.push(Line::from(format!(
                            "Deposit: {} {sym0} + {} {sym1}",
                            self.amount0.value().trim(),
                            self.amount1.value().trim()
                        )));
                        lines.push(Line::from(format!("Range:   {}", self.range_summary())));
                    }
                    _ => {}
                }
            }
            LpConfirmAction::Increase => {
                lines.push(Line::from("Increase liquidity"));
                if let Some(p) = self.v3_positions.get(self.sel) {
                    let pair =
                        super::helpers::v3_position_pair_label(self.chain_id, p.token0, p.token1);
                    lines.push(Line::from(format!("Position: {pair} · #{}", p.token_id)));
                }
                let (sym0, sym1) = self.form_token_symbols();
                lines.push(Line::from(format!(
                    "Add:     {} {sym0} + {} {sym1}",
                    self.amount0.value().trim(),
                    self.amount1.value().trim()
                )));
                lines.push(Line::from(format!("Network: {net}")));
            }
            LpConfirmAction::Decrease => {
                lines.push(Line::from("Remove liquidity"));
                if let Some(p) = self.v3_positions.get(self.sel) {
                    let pair =
                        super::helpers::v3_position_pair_label(self.chain_id, p.token0, p.token1);
                    lines.push(Line::from(format!("Position: {pair} · #{}", p.token_id)));
                    lines.push(Line::from(format!(
                        "Remove:  {} units (position {})",
                        self.liquidity.value().trim(),
                        p.liquidity
                    )));
                }
                lines.push(Line::from(format!("Network: {net}")));
                lines.push(Line::from("Next:    Collect tab to receive tokens"));
            }
            LpConfirmAction::Collect => {
                lines.push(Line::from("Collect tokens"));
                if let Some(p) = self.v3_positions.get(self.sel) {
                    let pair =
                        super::helpers::v3_position_pair_label(self.chain_id, p.token0, p.token1);
                    lines.push(Line::from(format!("Position: {pair} · #{}", p.token_id)));
                    lines.push(Line::from(format!(
                        "Owed:    {} / {} (raw units)",
                        p.tokens_owed0, p.tokens_owed1
                    )));
                }
                lines.push(Line::from(format!("Network: {net}")));
            }
            LpConfirmAction::V2Add => {
                lines.push(Line::from("Add V2 liquidity"));
                lines.push(Line::from(format!("Pair:    {pair}")));
                let (sym0, sym1) = self.form_token_symbols();
                lines.push(Line::from(format!(
                    "Deposit: {} {sym0} + {} {sym1}",
                    self.amount0.value().trim(),
                    self.amount1.value().trim()
                )));
                lines.push(Line::from(format!("Network: {net}")));
            }
            LpConfirmAction::V2Remove => {
                lines.push(Line::from("Remove V2 liquidity"));
                lines.push(Line::from(format!("Pair:    {pair}")));
                lines.push(Line::from(format!(
                    "Remove:  {} LP units",
                    self.liquidity.value().trim()
                )));
                lines.push(Line::from(format!("Network: {net}")));
            }
        }
        lines
    }

    fn form_pair_label(&self) -> String {
        let raw0 = self.token0.value().trim();
        let raw1 = self.token1.value().trim();
        if let (Ok(a0), Ok(a1)) = (
            parse_token_address(raw0, "token0"),
            parse_token_address(raw1, "token1"),
        ) {
            return super::helpers::v3_position_pair_label(self.chain_id, a0, a1);
        }
        format!(
            "{}/{}",
            token_symbol_for_address(&[], raw0)
                .or_else(|| crate::views::token_symbol_hint(raw0, self.chain_id))
                .unwrap_or("?"),
            token_symbol_for_address(&[], raw1)
                .or_else(|| crate::views::token_symbol_hint(raw1, self.chain_id))
                .unwrap_or("?")
        )
    }

    fn form_token_symbols(&self) -> (String, String) {
        let raw0 = self.token0.value();
        let raw1 = self.token1.value();
        (
            token_symbol_for_address(&[], raw0)
                .or_else(|| crate::views::token_symbol_hint(raw0, self.chain_id))
                .unwrap_or("TOKEN")
                .to_string(),
            token_symbol_for_address(&[], raw1)
                .or_else(|| crate::views::token_symbol_hint(raw1, self.chain_id))
                .unwrap_or("TOKEN")
                .to_string(),
        )
    }

    fn range_summary(&self) -> String {
        if let Some(idx) = self.range_preset_applied {
            if let Some((label, _)) = super::types::RANGE_PRESETS.get(idx) {
                return format!("{label} range");
            }
        }
        if self.v3_custom_range {
            return format!(
                "{} – {}",
                self.min_price.value().trim(),
                self.max_price.value().trim()
            );
        }
        "Full range".to_string()
    }

    pub(crate) fn selected_fee(&self) -> Option<Fee> {
        let ui = self.confirm_ui.as_ref()?;
        let base = ui.base_fee.as_ref()?;
        let speed = if ui.pipeline_step {
            self.lp_pipeline_speed
        } else {
            ui.speed
        };
        let custom = if ui.pipeline_step {
            self.lp_pipeline_custom_gwei.as_str()
        } else {
            ui.custom_gas.value()
        };
        match speed {
            FeeSpeed::Custom => base.with_custom_max_fee_gwei(custom).ok(),
            s => Some(base.with_speed(s)),
        }
    }

    pub(crate) fn select_confirm_speed(&mut self, speed: FeeSpeed) {
        let Some(ui) = self.confirm_ui.as_mut() else {
            return;
        };
        ui.speed = speed;
        if speed == FeeSpeed::Custom {
            if ui.custom_gas.value().is_empty() {
                if let Some(gwei) = ui.base_fee.as_ref().and_then(max_fee_gwei_display) {
                    ui.custom_gas.set_value(gwei);
                }
            }
            ui.focus = LpConfirmFocus::CustomGas;
        } else {
            ui.focus = LpConfirmFocus::Speed;
        }
    }

    pub(crate) fn confirm_tx(&mut self, tx: EvmTransaction, action: LpConfirmAction) -> KeyOutcome {
        self.begin_confirm(tx, action)
    }

    pub(crate) fn parse_decimals(raw: &str, label: &str) -> Result<u8, String> {
        raw.trim()
            .parse::<u8>()
            .map_err(|_| format!("Invalid {label}"))
    }

    pub(crate) fn parse_liquidity_u128(raw: &str) -> Result<u128, String> {
        raw.trim()
            .parse()
            .map_err(|_| "Invalid liquidity".to_string())
    }

    pub(crate) fn parse_liquidity_u256(raw: &str) -> Result<U256, String> {
        U256::from_str(raw.trim()).map_err(|_| "Invalid LP amount".to_string())
    }

    fn validate_remove_liquidity_u128(remove: u128, total: u128) -> Result<(), String> {
        if remove == 0 {
            return Err(
                "Choose how much to remove (↑↓ presets · Tab custom units · not full position by default)"
                    .into(),
            );
        }
        if remove > total {
            return Err(format!("Cannot remove more than position has ({total})"));
        }
        Ok(())
    }

    fn validate_remove_liquidity_u256(remove: U256, total: U256) -> Result<(), String> {
        if remove.is_zero() {
            return Err(
                "Choose how much to remove (↑↓ presets · Tab custom units · not full position by default)"
                    .into(),
            );
        }
        if remove > total {
            return Err(format!("Cannot remove more than position has ({total})"));
        }
        Ok(())
    }

    pub(crate) fn build_increase_tx(
        &self,
        wallet: &WalletState,
    ) -> Result<vaughan_core::chains::EvmTransaction, String> {
        let pos = self
            .v3_positions
            .get(self.sel)
            .ok_or_else(|| "No position selected".to_string())?;
        let from = wallet
            .active_address()
            .map_err(|e| e.user_message())?
            .to_string();
        let rpc = wallet.active_rpc_url();
        let amount0 = parse_swap_amount(self.amount0.value(), "amount0", 18)?;
        let amount1 = parse_swap_amount(self.amount1.value(), "amount1", 18)?;
        build_v3_increase_evm(
            &from,
            self.venue,
            self.chain_id,
            &rpc,
            pos.token_id,
            amount0,
            amount1,
            min_out_after_slippage(amount0, DEFAULT_DEX_SLIPPAGE_BPS),
            min_out_after_slippage(amount1, DEFAULT_DEX_SLIPPAGE_BPS),
            None,
        )
        .map_err(|e| e.user_message())
    }

    pub(crate) fn build_decrease_tx(
        &self,
        wallet: &WalletState,
    ) -> Result<vaughan_core::chains::EvmTransaction, String> {
        let pos = self
            .v3_positions
            .get(self.sel)
            .ok_or_else(|| "No position selected".to_string())?;
        let liquidity = Self::parse_liquidity_u128(self.liquidity.value())?;
        Self::validate_remove_liquidity_u128(liquidity, pos.liquidity)?;
        let from = wallet
            .active_address()
            .map_err(|e| e.user_message())?
            .to_string();
        let rpc = wallet.active_rpc_url();
        build_v3_decrease_evm(
            &from,
            self.venue,
            self.chain_id,
            &rpc,
            pos.token_id,
            liquidity,
            U256::ZERO,
            U256::ZERO,
            None,
        )
        .map_err(|e| e.user_message())
    }

    pub(crate) fn build_collect_tx(
        &self,
        wallet: &WalletState,
    ) -> Result<vaughan_core::chains::EvmTransaction, String> {
        let pos = self
            .v3_positions
            .get(self.sel)
            .ok_or_else(|| "No position selected".to_string())?;
        let from = wallet
            .active_address()
            .map_err(|e| e.user_message())?
            .to_string();
        let rpc = wallet.active_rpc_url();
        build_v3_collect_evm(
            &from,
            self.venue,
            self.chain_id,
            &rpc,
            pos.token_id,
            None,
            u128::MAX,
            u128::MAX,
        )
        .map_err(|e| e.user_message())
    }

    pub(crate) fn build_v2_add_tx(
        &self,
        wallet: &WalletState,
    ) -> Result<vaughan_core::chains::EvmTransaction, String> {
        self.validate_pair_selection()?;
        let from = wallet
            .active_address()
            .map_err(|e| e.user_message())?
            .to_string();
        let token0 = parse_token_address(self.token0.value(), "first token")?;
        let token1 = parse_token_address(self.token1.value(), "second token")?;
        let dec0 = Self::parse_decimals(self.dec0.value(), "decimals0")?;
        let dec1 = Self::parse_decimals(self.dec1.value(), "decimals1")?;
        let wpls = wpls_for_chain(self.chain_id);
        let native = wpls.filter(|w| token0 == *w || token1 == *w);
        build_v2_add_liquidity_evm(
            &from,
            self.venue,
            self.chain_id,
            token0,
            token1,
            self.amount0.value().trim(),
            self.amount1.value().trim(),
            dec0,
            dec1,
            DEFAULT_DEX_SLIPPAGE_BPS,
            native,
        )
        .map_err(|e| e.user_message())
    }

    pub(crate) fn build_v2_remove_tx(
        &self,
        wallet: &WalletState,
    ) -> Result<vaughan_core::chains::EvmTransaction, String> {
        let pos = self
            .v2_positions
            .get(self.sel)
            .ok_or_else(|| "No position selected".to_string())?;
        let liquidity = Self::parse_liquidity_u256(self.liquidity.value())?;
        Self::validate_remove_liquidity_u256(liquidity, pos.lp_balance)?;
        let from = wallet
            .active_address()
            .map_err(|e| e.user_message())?
            .to_string();
        let wpls = wpls_for_chain(self.chain_id);
        let native = wpls.filter(|w| pos.token0 == *w || pos.token1 == *w);
        build_v2_remove_liquidity_evm(
            &from,
            self.venue,
            self.chain_id,
            pos.token0,
            pos.token1,
            liquidity,
            DEFAULT_DEX_SLIPPAGE_BPS,
            native,
        )
        .map_err(|e| e.user_message())
    }

    pub(crate) fn handle_confirm(&mut self, key: KeyEvent, _wallet: &WalletState) -> KeyOutcome {
        if !self.is_fee_confirm() {
            if matches!(key.code, KeyCode::Esc) {
                self.cancel_confirm();
            }
            return KeyOutcome::Consumed;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                if self
                    .confirm_ui
                    .as_ref()
                    .is_some_and(|ui| ui.focus == LpConfirmFocus::CustomGas)
                {
                    if let Some(ui) = self.confirm_ui.as_mut() {
                        ui.focus = LpConfirmFocus::Speed;
                    }
                    return KeyOutcome::Consumed;
                }
                self.cancel_confirm();
                KeyOutcome::Consumed
            }
            KeyCode::Up => {
                if self.confirm_ui.as_ref().is_some_and(|ui| ui.pipeline_step) {
                    return KeyOutcome::Consumed;
                }
                let next = self
                    .confirm_ui
                    .as_ref()
                    .map(|ui| ui.speed.prev())
                    .unwrap_or(FeeSpeed::Normal);
                self.select_confirm_speed(next);
                KeyOutcome::Consumed
            }
            KeyCode::Down => {
                if self.confirm_ui.as_ref().is_some_and(|ui| ui.pipeline_step) {
                    return KeyOutcome::Consumed;
                }
                let next = self
                    .confirm_ui
                    .as_ref()
                    .map(|ui| ui.speed.next())
                    .unwrap_or(FeeSpeed::Normal);
                self.select_confirm_speed(next);
                KeyOutcome::Consumed
            }
            KeyCode::Char(c)
                if FeeSpeed::from_digit(c).is_some()
                    && !self.confirm_ui.as_ref().is_some_and(|ui| ui.pipeline_step)
                    && !self
                        .confirm_ui
                        .as_ref()
                        .is_some_and(|ui| ui.focus == LpConfirmFocus::CustomGas) =>
            {
                self.select_confirm_speed(FeeSpeed::from_digit(c).unwrap());
                KeyOutcome::Consumed
            }
            KeyCode::Tab
                if self
                    .confirm_ui
                    .as_ref()
                    .is_some_and(|ui| ui.speed == FeeSpeed::Custom) =>
            {
                if let Some(ui) = self.confirm_ui.as_mut() {
                    ui.focus = match ui.focus {
                        LpConfirmFocus::Speed => LpConfirmFocus::CustomGas,
                        LpConfirmFocus::CustomGas => LpConfirmFocus::Speed,
                    };
                }
                KeyOutcome::Consumed
            }
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                if self.busy == Busy::EstimatingFee {
                    return KeyOutcome::Consumed;
                }
                if self
                    .confirm_ui
                    .as_ref()
                    .is_some_and(|ui| matches!(ui.action, LpConfirmAction::AddReview))
                {
                    return self.begin_add_pipeline();
                }
                if self.confirm_ui.as_ref().is_some_and(|ui| {
                    ui.speed == FeeSpeed::Custom && ui.focus == LpConfirmFocus::CustomGas
                }) {
                    let check = self.confirm_ui.as_ref().and_then(|ui| {
                        ui.base_fee
                            .as_ref()
                            .map(|f| f.with_custom_max_fee_gwei(ui.custom_gas.value()))
                    });
                    match check {
                        Some(Ok(_)) => self.begin_confirm_broadcast(),
                        Some(Err(e)) => {
                            self.status = e;
                            KeyOutcome::Consumed
                        }
                        None => {
                            self.status = "Gas estimate missing — Esc and retry".into();
                            KeyOutcome::Consumed
                        }
                    }
                } else {
                    self.begin_confirm_broadcast()
                }
            }
            _ if self
                .confirm_ui
                .as_ref()
                .is_some_and(|ui| ui.focus == LpConfirmFocus::CustomGas) =>
            {
                if let Some(ui) = self.confirm_ui.as_mut() {
                    match ui.custom_gas.handle_key(key) {
                        InputAction::Ignored => KeyOutcome::NotHandled,
                        InputAction::Submitted => self.begin_confirm_broadcast(),
                        InputAction::Consumed => KeyOutcome::Consumed,
                    }
                } else {
                    KeyOutcome::Consumed
                }
            }
            _ => KeyOutcome::Consumed,
        }
    }

    fn begin_add_pipeline(&mut self) -> KeyOutcome {
        let Some(ui) = self.confirm_ui.as_ref() else {
            return KeyOutcome::Consumed;
        };
        if ui.base_fee.is_none() {
            self.status = "Gas estimate missing — Esc and retry".into();
            return KeyOutcome::Consumed;
        }
        self.lp_pipeline_speed = ui.speed;
        self.lp_pipeline_custom_gwei = if ui.speed == FeeSpeed::Custom {
            ui.custom_gas.value().to_string()
        } else {
            String::new()
        };
        self.lp_pipeline_phase = LpPipelinePhase::Execute;
        self.begin_confirm_broadcast()
    }

    fn begin_confirm_broadcast(&mut self) -> KeyOutcome {
        let Some(fee) = self.selected_fee() else {
            self.status = "Gas estimate missing — Esc and retry".into();
            return KeyOutcome::Consumed;
        };
        let Some(ui) = self.confirm_ui.take() else {
            self.status = "Nothing to send — Esc and retry".into();
            return KeyOutcome::Consumed;
        };
        let tx = ui.pending_tx;
        if !matches!(ui.action, LpConfirmAction::Enable { .. }) {
            self.lp_deploy_sent_step = self.lp_deploy_last_step;
            self.lp_deploy_last_label = match &ui.action {
                LpConfirmAction::Deploy { label, .. } => label.clone(),
                LpConfirmAction::AddReview => self.lp_deploy_last_label.clone(),
                _ => String::new(),
            };
        }
        self.busy = Busy::Sending;
        self.status = "Broadcasting…".into();
        KeyOutcome::StartJob(UiJob::SendEvmWithFee { tx, fee })
    }
}

fn deploy_step_number(step: LpDeployLastStep) -> Option<(u8, u8)> {
    match step {
        LpDeployLastStep::CreatePool => Some((1, 4)),
        LpDeployLastStep::Initialize => Some((2, 4)),
        LpDeployLastStep::Approve => Some((3, 4)),
        LpDeployLastStep::AddLiquidity => Some((4, 4)),
        LpDeployLastStep::None => None,
    }
}

fn max_fee_gwei_display(fee: &Fee) -> Option<String> {
    match &fee.details {
        vaughan_core::chains::FeeDetails::Evm {
            max_fee_per_gas, ..
        } => max_fee_per_gas
            .as_deref()
            .and_then(|mf| mf.parse::<u128>().ok())
            .map(|wei| format!("{}", wei as f64 / 1e9)),
        _ => None,
    }
}
