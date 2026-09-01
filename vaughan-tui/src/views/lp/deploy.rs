#![allow(unused_imports)]
use vaughan_core::core::{V3LpDeployParams, V3LpDeployWait, WalletState};

use crate::app::KeyOutcome;
use crate::jobs::UiJob;

use super::types::*;

impl LpView {
    pub(crate) fn followup_deploy_job(&mut self, wallet: &WalletState) -> Option<UiJob> {
        if let Some(tx) = self
            .confirm_ui
            .as_mut()
            .and_then(|ui| ui.pending_fee_estimate.take())
        {
            return Some(UiJob::EstimateEvmFee { tx });
        }
        if !self.lp_deploy_pending_resume {
            return None;
        }
        self.lp_deploy_pending_resume = false;
        let deploy_wait = self
            .lp_deploy_followup_wait
            .take()
            .unwrap_or(V3LpDeployWait::None);
        let after_step_label = if deploy_wait == V3LpDeployWait::AfterApprove {
            Some(self.lp_deploy_last_label.clone()).filter(|s| !s.is_empty())
        } else {
            None
        };
        match self.build_lp_deploy_job(wallet, deploy_wait, after_step_label) {
            Ok(job) => {
                self.busy = Busy::Loading;
                self.status = match deploy_wait {
                    V3LpDeployWait::AfterCreatePool => "Waiting for createPool on chain…".into(),
                    V3LpDeployWait::AfterInitialize => {
                        "Waiting for pool initialize on chain…".into()
                    }
                    V3LpDeployWait::AfterApprove => "Waiting for approve on chain…".into(),
                    V3LpDeployWait::None => "Preparing next LP step…".into(),
                };
                Some(job)
            }
            Err(e) => {
                self.busy = Busy::Idle;
                self.stage = Stage::Input;
                self.confirm_lines.clear();
                self.lp_deploy_active = false;
                self.lp_pipeline_phase = LpPipelinePhase::None;
                self.lp_pipeline_custom_gwei.clear();
                self.lp_deploy_last_label.clear();
                self.lp_deploy_followup_wait = None;
                self.status = e;
                None
            }
        }
    }

    pub(crate) fn lp_deploy_params(
        &self,
        wallet: &WalletState,
    ) -> Result<V3LpDeployParams, String> {
        let pair = self.sorted_pair()?;
        let from = wallet
            .active_address()
            .map_err(|e| e.user_message())?
            .to_string();
        let pool_initial =
            self.user_price_to_pool_price(pair.first_is_token0, self.initial_price.value())?;
        let (pool_min, pool_max) = self.user_price_range_to_pool_prices(
            pair.first_is_token0,
            self.min_price.value(),
            self.max_price.value(),
        )?;
        let (amount0, amount1) = if pair.first_is_token0 {
            (
                self.amount0.value().to_string(),
                self.amount1.value().to_string(),
            )
        } else {
            (
                self.amount1.value().to_string(),
                self.amount0.value().to_string(),
            )
        };
        if amount0.trim().is_empty() || amount1.trim().is_empty() {
            return Err("Enter both deposit amounts".into());
        }
        Ok(V3LpDeployParams {
            from,
            venue: self.venue,
            chain_id: self.chain_id,
            rpc_url: wallet.active_rpc_url(),
            token0: pair.token0,
            token1: pair.token1,
            fee: self.fee_tier,
            dec0: pair.dec0,
            dec1: pair.dec1,
            pool_initial_price: pool_initial,
            pool_min_price: pool_min,
            pool_max_price: pool_max,
            amount0,
            amount1,
            deposit_on_token0: true,
        })
    }

    pub(crate) fn build_lp_deploy_job(
        &self,
        wallet: &WalletState,
        deploy_wait: V3LpDeployWait,
        after_step_label: Option<String>,
    ) -> Result<UiJob, String> {
        let params = self.lp_deploy_params(wallet)?;
        Ok(UiJob::LpV3PoolDeployStep {
            venue: params.venue,
            chain_id: params.chain_id,
            rpc_url: params.rpc_url,
            from: params.from,
            token0: format!("{:#x}", params.token0),
            token1: format!("{:#x}", params.token1),
            fee: params.fee,
            dec0: params.dec0,
            dec1: params.dec1,
            pool_initial_price: params.pool_initial_price,
            pool_min_price: params.pool_min_price,
            pool_max_price: params.pool_max_price,
            amount0: params.amount0,
            amount1: params.amount1,
            deploy_wait,
            after_step_label,
        })
    }

    pub(crate) fn build_lp_enable_check_job(&self, wallet: &WalletState) -> Result<UiJob, String> {
        let params = self.lp_deploy_params(wallet)?;
        Ok(UiJob::LpEnableCheck {
            venue: params.venue,
            chain_id: params.chain_id,
            rpc_url: params.rpc_url,
            from: params.from,
            token0: format!("{:#x}", params.token0),
            token1: format!("{:#x}", params.token1),
            fee: params.fee,
            dec0: params.dec0,
            dec1: params.dec1,
            pool_initial_price: params.pool_initial_price,
            pool_min_price: params.pool_min_price,
            pool_max_price: params.pool_max_price,
            amount0: params.amount0,
            amount1: params.amount1,
        })
    }

    pub(crate) fn build_lp_enable_prepare_job(
        &self,
        wallet: &WalletState,
        symbol: String,
    ) -> Result<UiJob, String> {
        let params = self.lp_deploy_params(wallet)?;
        Ok(UiJob::LpEnablePrepare {
            venue: params.venue,
            chain_id: params.chain_id,
            rpc_url: params.rpc_url,
            from: params.from,
            token0: format!("{:#x}", params.token0),
            token1: format!("{:#x}", params.token1),
            fee: params.fee,
            dec0: params.dec0,
            dec1: params.dec1,
            pool_initial_price: params.pool_initial_price,
            pool_min_price: params.pool_min_price,
            pool_max_price: params.pool_max_price,
            amount0: params.amount0,
            amount1: params.amount1,
            symbol,
        })
    }

    pub(crate) fn build_lp_enable_wait_job(&self, wallet: &WalletState) -> Result<UiJob, String> {
        let params = self.lp_deploy_params(wallet)?;
        Ok(UiJob::LpEnableWait {
            venue: params.venue,
            chain_id: params.chain_id,
            rpc_url: params.rpc_url,
            from: params.from,
            token0: format!("{:#x}", params.token0),
            token1: format!("{:#x}", params.token1),
            fee: params.fee,
            dec0: params.dec0,
            dec1: params.dec1,
            pool_initial_price: params.pool_initial_price,
            pool_min_price: params.pool_min_price,
            pool_max_price: params.pool_max_price,
            amount0: params.amount0,
            amount1: params.amount1,
            after_step_label: self.lp_enable_last_label.clone(),
        })
    }

    pub(crate) fn start_add_liquidity_job(&mut self, wallet: &WalletState) -> KeyOutcome {
        if self.pool_quote_inflight {
            self.status = "Still verifying pool on chain… wait a moment, then Enter again".into();
            return KeyOutcome::Consumed;
        }
        if let Err(e) = self.validate_pair_selection() {
            self.status = e;
            return KeyOutcome::Consumed;
        }
        if self.amount0.value().trim().is_empty() || self.amount1.value().trim().is_empty() {
            self.status = "Enter both deposit amounts".into();
            return KeyOutcome::Consumed;
        }
        if !self.enables_ready_for_add() {
            return self.start_enable_confirm(wallet);
        }
        self.lp_deploy_active = true;
        self.lp_deploy_pending_resume = false;
        self.lp_deploy_last_step = LpDeployLastStep::None;
        self.lp_deploy_sent_step = LpDeployLastStep::None;
        self.lp_pipeline_phase = LpPipelinePhase::Review;
        self.lp_pipeline_custom_gwei.clear();
        self.busy = Busy::Loading;
        self.status = "Checking pool…".into();
        match self.build_lp_deploy_job(wallet, V3LpDeployWait::None, None) {
            Ok(job) => KeyOutcome::StartJob(job),
            Err(e) => {
                self.busy = Busy::Idle;
                self.lp_deploy_active = false;
                self.status = e;
                KeyOutcome::Consumed
            }
        }
    }
}
