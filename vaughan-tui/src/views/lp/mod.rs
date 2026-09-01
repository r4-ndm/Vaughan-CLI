//! Browserless LP — wiz4rd V3 on testnet 943, 9inch V3 on Pulse mainnet 369.
//!
//! **Add LP** mirrors [9inch V3 add liquidity](https://9inch.io/liquidity/add/v3?chain=pulse)
//! and [9mm V3 range UI](https://dex.9mm.pro/add/PLS/…): pair + fee, four-column
//! price range, ±% presets, then deposits.

mod confirm;
mod deploy;
mod enable;
mod helpers;
mod input;
mod jobs;
mod render;
#[cfg(test)]
mod smoke_tests;
mod state;
#[cfg(test)]
mod tests;
mod types;

pub use types::LpView;

use vaughan_core::chains::Balance;
use vaughan_core::core::{lp_stack_for_chain, DexVenue, LpStack};

use crate::input::Input;
use crate::views::{cycle_token_picker, TOKEN_PICK_UNINIT};

use types::*;

impl LpView {
    pub fn for_chain(chain_id: u64) -> Self {
        let stack = lp_stack_for_chain(chain_id).unwrap_or(LpStack::V3 {
            venue: DexVenue::Wiz4rd,
        });
        let venue = stack.venue();
        let mut v = Self {
            stack,
            venue,
            tab: Tab::AddLp,
            stage: Stage::Input,
            busy: Busy::Idle,
            tick: 0,
            status: String::new(),
            chain_id,
            v3_positions: Vec::new(),
            v2_positions: Vec::new(),
            sel: 0,
            add_step: AddStep::SelectPair,
            focus: Focus::None,
            token0: Input::new(false, "select token"),
            token1: Input::new(false, "select token"),
            token0_pick: TOKEN_PICK_UNINIT,
            token1_pick: TOKEN_PICK_UNINIT,
            token0_editing: false,
            token1_editing: false,
            fee_tier: 2500,
            initial_price: Input::new(false, "0.5"),
            min_price: Input::new(false, "min · 2nd/1st"),
            max_price: Input::new(false, "max · 2nd/1st"),
            ratio: Input::new(false, "ratio · 2nd/1st"),
            amount0: Input::new(false, "0.0"),
            amount1: Input::new(false, "0.0"),
            dec0: Input::new(false, "decimals0"),
            dec1: Input::new(false, "decimals1"),
            liquidity: Input::new(false, "remove · raw units"),
            confirm_lines: Vec::new(),
            confirm_ui: None,
            range_preset_idx: 3,
            range_preset_applied: None,
            decrease_preset_idx: 0,
            decrease_preset_applied: None,
            pool_lifecycle: None,
            pool_sqrt_x96: None,
            pool_tick: None,
            pool_quote_inflight: false,
            v3_custom_range: false,
            lp_deploy_active: false,
            lp_deploy_pending_resume: false,
            lp_deploy_last_step: LpDeployLastStep::None,
            lp_deploy_sent_step: LpDeployLastStep::None,
            lp_pipeline_phase: LpPipelinePhase::None,
            lp_pipeline_speed: vaughan_core::chains::FeeSpeed::Normal,
            lp_pipeline_custom_gwei: String::new(),
            lp_deploy_last_label: String::new(),
            lp_deploy_followup_wait: None,
            lp_enable_first: None,
            lp_enable_second: None,
            lp_enable_check_inflight: false,
            lp_enable_pending_resume: false,
            lp_enable_recheck_pending: false,
            lp_enable_last_label: String::new(),
            lp_enable_in_confirm: false,
        };
        v.amount0.set_value("1");
        v.amount1.set_value("1");
        v.ratio.set_value("1");
        v.initial_price.set_value("1");
        v.dec0.set_value("18");
        v.dec1.set_value("18");
        v.apply_venue_token_defaults(true);
        v.apply_initial_fee_defaults();
        v.status = v.default_status_hint();
        v
    }

    pub fn set_tick(&mut self, tick: u64) {
        self.tick = tick;
    }

    pub fn allows_footer_shortcuts(&self) -> bool {
        if self.stage != Stage::Input {
            if self.stage == Stage::Confirm {
                return self
                    .confirm_ui
                    .as_ref()
                    .map(|ui| ui.focus != LpConfirmFocus::CustomGas)
                    .unwrap_or(true)
                    && !matches!(self.busy, Busy::EstimatingFee | Busy::Sending);
            }
            return false;
        }
        if self.busy == Busy::Sending {
            return false;
        }
        if self.busy == Busy::Loading
            && !(self.on_v3_price_deposit() || self.lp_enable_check_inflight)
        {
            return false;
        }
        if self.tab != Tab::AddLp {
            return true;
        }
        match self.focus {
            Focus::None | Focus::Fee | Focus::Venue | Focus::RangePresets => true,
            Focus::Token0 => !self.token0_editing,
            Focus::Token1 => !self.token1_editing,
            Focus::InitialPrice
            | Focus::MinPrice
            | Focus::MaxPrice
            | Focus::Ratio
            | Focus::Amount0
            | Focus::Amount1
            | Focus::Liquidity => false,
        }
    }

    /// ↑/↓ on token fields cycles wallet assets (F2 list).
    pub fn cycle_focused_token_picker(&mut self, assets: &[Balance], forward: bool) -> bool {
        if self.tab != Tab::AddLp || self.stage != Stage::Input {
            return false;
        }
        if matches!(self.stack, LpStack::V3 { .. }) && self.add_step == AddStep::PriceDeposit {
            return false;
        }
        let mut dummy = false;
        match self.focus {
            Focus::Token0 => {
                self.token0_editing = false;
                let changed = cycle_token_picker(
                    assets,
                    true,
                    &mut self.token0_pick,
                    forward,
                    &mut dummy,
                    &mut self.token0,
                    &mut self.status,
                );
                if changed {
                    let addr = self.token0.value().to_string();
                    Self::sync_decimals_for_token(&addr, &mut self.dec0, assets);
                }
                changed
            }
            Focus::Token1 => {
                self.token1_editing = false;
                let changed = cycle_token_picker(
                    assets,
                    true,
                    &mut self.token1_pick,
                    forward,
                    &mut dummy,
                    &mut self.token1,
                    &mut self.status,
                );
                if changed {
                    let addr = self.token1.value().to_string();
                    Self::sync_decimals_for_token(&addr, &mut self.dec1, assets);
                }
                changed
            }
            _ => false,
        }
    }
}
