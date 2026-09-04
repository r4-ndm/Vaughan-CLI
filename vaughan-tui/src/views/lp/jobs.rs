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
    fee_tier_display, format_unit_price, friendly_deploy_action, lp_fee_estimate_error,
    lp_network_user_message, lp_tx_error_message, parse_price_f64, render_unit_price_input,
    trim_float_string,
};
use super::types::*;

impl LpView {
    pub fn initial_job(&mut self, wallet: &WalletState) -> Option<UiJob> {
        self.list_job(wallet)
    }

    pub(crate) fn list_job(&mut self, wallet: &WalletState) -> Option<UiJob> {
        let owner = wallet.active_address().ok()?.to_string();
        let rpc = wallet.active_rpc_url();
        self.list_job_for(owner, rpc)
    }

    /// List LP for an already-resolved owner (avoids holding `WalletState` across `&mut self`).
    pub(crate) fn list_job_for(&mut self, owner: String, rpc_url: String) -> Option<UiJob> {
        if !self.lp_supported() {
            return None;
        }
        self.list_gen = self.list_gen.wrapping_add(1);
        let list_gen = self.list_gen;
        // Clear immediately so a previous account's rows never linger under a new F3.
        self.v3_positions.clear();
        self.v2_positions.clear();
        self.sel = 0;
        self.list_action_idx = None;
        match self.stack {
            LpStack::V3 { .. } => Some(UiJob::LpListPositions {
                venue: self.venue,
                chain_id: self.chain_id,
                rpc_url,
                owner,
                list_gen,
            }),
            LpStack::V2 { venue } => Some(UiJob::LpListV2Positions {
                venue,
                chain_id: self.chain_id,
                rpc_url,
                owner,
                list_gen,
            }),
        }
    }

    pub fn apply_job_result(&mut self, result: UiJobResult) {
        match result {
            UiJobResult::LpPositions {
                list_gen,
                owner,
                result,
            } => {
                if list_gen != self.list_gen {
                    return;
                }
                self.busy = Busy::Idle;
                match result {
                    Ok(rows) => {
                        self.v3_positions = rows;
                        self.list_action_idx = None;
                        self.clamp_list_sel();
                        self.status = format!(
                            "{} · {} V3 position(s) · {} · ↑↓ select · Enter open",
                            self.venue.label(),
                            self.v3_positions.len(),
                            short_addr(&owner)
                        );
                    }
                    Err(e) => {
                        self.status = lp_network_user_message(&e);
                    }
                }
            }
            UiJobResult::LpV2Positions {
                list_gen,
                owner,
                result,
            } => {
                if list_gen != self.list_gen {
                    return;
                }
                self.busy = Busy::Idle;
                match result {
                    Ok(rows) => {
                        self.v2_positions = rows;
                        self.list_action_idx = None;
                        self.clamp_list_sel();
                        self.status = format!(
                            "{} · {} V2 position(s) · {} · ↑↓ select · Enter open",
                            self.venue.label(),
                            self.v2_positions.len(),
                            short_addr(&owner)
                        );
                    }
                    Err(e) => {
                        self.status = lp_network_user_message(&e);
                    }
                }
            }
            UiJobResult::LpV3PoolDeployStep(Ok((tx, label))) => {
                let step = LpDeployLastStep::from_deploy_label(&label);
                self.lp_deploy_last_step = step;
                self.lp_deploy_last_label = label.clone();
                self.open_deploy_confirm(tx, step, label);
            }
            UiJobResult::LpEnableCheck(result) => {
                self.busy = Busy::Idle;
                self.apply_enable_check(result);
            }
            UiJobResult::LpEnablePrepare(Ok((tx, label, symbol))) => {
                self.busy = Busy::Idle;
                self.open_enable_confirm(tx, symbol, label);
            }
            UiJobResult::LpEnablePrepare(Err(e)) => {
                self.busy = Busy::Idle;
                self.stage = Stage::Input;
                self.status = lp_network_user_message(&e);
            }
            UiJobResult::LpEnableWait(Ok(())) => {
                self.busy = Busy::Idle;
                self.lp_enable_recheck_pending = true;
                self.status = "Enable confirmed — rechecking tokens…".into();
            }
            UiJobResult::LpEnableWait(Err(e)) => {
                self.busy = Busy::Idle;
                self.status = lp_network_user_message(&e);
            }
            UiJobResult::Fee(Ok(fee)) if self.confirm_ui.is_some() => {
                self.busy = Busy::Idle;
                if let Some(ui) = self.confirm_ui.as_mut() {
                    ui.base_fee = Some(fee);
                    if !ui.pipeline_step {
                        ui.speed = vaughan_core::chains::FeeSpeed::Normal;
                        ui.focus = LpConfirmFocus::Speed;
                    }
                }
                self.status = if self
                    .confirm_ui
                    .as_ref()
                    .is_some_and(|ui| matches!(ui.action, LpConfirmAction::AddReview))
                {
                    "Review deposit · Enter to continue".into()
                } else if self
                    .confirm_ui
                    .as_ref()
                    .is_some_and(|ui| matches!(ui.action, LpConfirmAction::Enable { .. }))
                {
                    "Enable token · Enter send · Esc cancel".into()
                } else {
                    "Enter send · Esc cancel".into()
                };
            }
            UiJobResult::Fee(Err(e)) if self.confirm_ui.is_some() => {
                self.cancel_confirm();
                self.status = lp_fee_estimate_error(&e);
            }
            UiJobResult::LpV3PoolDeployStep(Err(e)) => {
                self.busy = Busy::Idle;
                if self.lp_deploy_active {
                    self.stage = Stage::Input;
                    self.confirm_ui = None;
                    self.confirm_lines.clear();
                    self.lp_pipeline_phase = LpPipelinePhase::None;
                    self.lp_pipeline_custom_gwei.clear();
                    self.lp_deploy_last_label.clear();
                    self.lp_deploy_followup_wait = None;
                }
                self.status = lp_tx_error_message(&e, self.lp_deploy_last_step);
            }
            UiJobResult::LpV3PoolQuote(Ok(quote)) => {
                self.apply_pool_quote(quote);
            }
            UiJobResult::LpV3PoolQuote(Err(e)) => {
                if self.add_step != AddStep::PriceDeposit {
                    self.pool_quote_inflight = false;
                    return;
                }
                self.pool_quote_inflight = false;
                self.pool_lifecycle = Some(V3PoolLifecycle::Missing);
                self.status = format!(
                    "Pool lookup failed — {} · using your prices (rechecks before send)",
                    lp_network_user_message(&e)
                );
                self.resync_range_bounds_from_preset();
                self.sync_amount1_from_price();
            }
            UiJobResult::Send(Ok(receipt)) => {
                self.busy = Busy::Idle;
                self.confirm_ui = None;
                if self.lp_enable_in_confirm {
                    self.lp_enable_in_confirm = false;
                    self.stage = Stage::Input;
                    self.confirm_lines.clear();
                    self.lp_enable_pending_resume = true;
                    self.status = format!("Enable sent ({}) — waiting on chain…", receipt.hash);
                    return;
                }
                let sent = self.lp_deploy_sent_step;
                self.lp_deploy_sent_step = LpDeployLastStep::None;
                if self.lp_deploy_active {
                    if sent == LpDeployLastStep::AddLiquidity {
                        self.stage = Stage::Input;
                        self.confirm_lines.clear();
                        self.lp_deploy_active = false;
                        self.lp_deploy_pending_resume = false;
                        self.lp_deploy_last_step = LpDeployLastStep::None;
                        self.lp_pipeline_phase = LpPipelinePhase::None;
                        self.lp_pipeline_custom_gwei.clear();
                        self.lp_deploy_last_label.clear();
                        self.lp_deploy_followup_wait = None;
                        self.status = format!("LP added ({})", receipt.hash);
                    } else {
                        self.lp_deploy_followup_wait = Some(match sent {
                            LpDeployLastStep::CreatePool => V3LpDeployWait::AfterCreatePool,
                            LpDeployLastStep::Initialize => V3LpDeployWait::AfterInitialize,
                            LpDeployLastStep::Approve => V3LpDeployWait::AfterApprove,
                            _ => V3LpDeployWait::None,
                        });
                        self.lp_deploy_pending_resume = true;
                        self.stage = Stage::Confirm;
                        self.confirm_lines = vec![Line::from(format!(
                            "Confirmed ({}) — preparing next LP step…",
                            receipt.hash
                        ))];
                        self.status = "Preparing next LP step…".into();
                    }
                } else {
                    self.stage = Stage::Input;
                    self.confirm_lines.clear();
                    self.lp_reload_pending = true;
                    self.status = format!("LP tx ok ({}) — refreshing positions…", receipt.hash);
                }
            }
            UiJobResult::Send(Err(e)) => {
                self.busy = Busy::Idle;
                self.lp_deploy_sent_step = LpDeployLastStep::None;
                self.lp_enable_in_confirm = false;
                self.stage = Stage::Input;
                self.confirm_ui = None;
                self.confirm_lines.clear();
                self.status = lp_tx_error_message(&e, self.lp_deploy_last_step);
            }
            _ => {}
        }
    }
}

fn short_addr(addr: &str) -> String {
    let a = addr.trim();
    if a.len() <= 12 {
        return a.to_string();
    }
    format!("{}…{}", &a[..6], &a[a.len().saturating_sub(4)..])
}
