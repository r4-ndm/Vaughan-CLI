//! PancakeSwap-style **Enable** (NPM approve) on the add-LP deposit form before review.

use vaughan_core::core::WalletState;

use crate::app::KeyOutcome;
use crate::jobs::UiJob;

use super::helpers::lp_network_user_message;
use super::types::*;

impl LpView {
    pub(crate) fn reset_enable_state(&mut self) {
        self.lp_enable_first = None;
        self.lp_enable_second = None;
        self.lp_enable_check_inflight = false;
        self.lp_enable_pending_resume = false;
        self.lp_enable_recheck_pending = false;
        self.lp_enable_last_label.clear();
        self.lp_enable_in_confirm = false;
    }

    pub(crate) fn apply_chain_enable_status(&mut self, chain_t0_ok: bool, chain_t1_ok: bool) {
        let Ok(pair) = self.sorted_pair() else {
            return;
        };
        if pair.first_is_token0 {
            self.lp_enable_first = Some(chain_t0_ok);
            self.lp_enable_second = Some(chain_t1_ok);
        } else {
            self.lp_enable_first = Some(chain_t1_ok);
            self.lp_enable_second = Some(chain_t0_ok);
        }
    }

    pub(crate) fn enable_deferred_for_new_pool(&self) -> bool {
        self.lp_enable_first.is_none() && self.lp_enable_second.is_none()
    }

    pub(crate) fn enables_ready_for_add(&self) -> bool {
        if self.enable_deferred_for_new_pool() {
            return true;
        }
        !matches!(self.lp_enable_first, Some(false))
            && !matches!(self.lp_enable_second, Some(false))
    }

    pub(crate) fn form_symbol_hint(&self, first_field: bool) -> String {
        let raw = if first_field {
            self.token0.value()
        } else {
            self.token1.value()
        };
        crate::views::token_symbol_for_address(&[], raw.trim())
            .or_else(|| crate::views::token_symbol_hint(raw.trim(), self.chain_id))
            .unwrap_or("TOKEN")
            .to_string()
    }

    pub(crate) fn next_enable_symbol(&self) -> Option<String> {
        let sym0 = self.form_symbol_hint(true);
        let sym1 = self.form_symbol_hint(false);
        if self.lp_enable_first == Some(false) {
            return Some(sym0);
        }
        if self.lp_enable_second == Some(false) {
            return Some(sym1);
        }
        None
    }

    pub(crate) fn enable_status_line(&self, sym0: &str, sym1: &str) -> Option<String> {
        if !self.on_v3_price_deposit() {
            return None;
        }
        if self.lp_enable_check_inflight {
            return Some("Enable:  checking token access…".into());
        }
        if self.enable_deferred_for_new_pool() {
            return Some("Enable:  after pool exists (create + initialize run first)".into());
        }
        let mark = |ok: Option<bool>| match ok {
            Some(true) => "✓",
            Some(false) => "○",
            None => "?",
        };
        Some(format!(
            "Enable:  {} {sym0} · {} {sym1} · e enable next · Enter add when both ✓",
            mark(self.lp_enable_first),
            mark(self.lp_enable_second),
        ))
    }

    pub(crate) fn schedule_enable_recheck(&mut self) {
        if self.on_v3_price_deposit() {
            self.lp_enable_recheck_pending = true;
        }
    }

    pub(crate) fn spawn_enable_check(&mut self, wallet: &WalletState) -> Option<UiJob> {
        if !self.on_v3_price_deposit() {
            return None;
        }
        if self.amount0.value().trim().is_empty() || self.amount1.value().trim().is_empty() {
            return None;
        }
        let job = self.build_lp_enable_check_job(wallet).ok()?;
        self.lp_enable_check_inflight = true;
        Some(job)
    }

    pub(crate) fn apply_enable_check(
        &mut self,
        result: Result<Option<(bool, bool)>, vaughan_core::error::WalletError>,
    ) {
        self.lp_enable_check_inflight = false;
        match result {
            Ok(None) => {
                self.lp_enable_first = None;
                self.lp_enable_second = None;
            }
            Ok(Some((t0, t1))) => {
                self.apply_chain_enable_status(t0, t1);
                if self.enables_ready_for_add() {
                    self.status = "Tokens enabled — Enter to review add".into();
                } else if let Some(sym) = self.next_enable_symbol() {
                    self.status = format!("Enable {sym} (e) before add · Enter when both ✓");
                }
            }
            Err(e) => {
                self.status = format!("Enable check failed — {}", lp_network_user_message(&e));
            }
        }
    }

    pub(crate) fn start_enable_confirm(&mut self, wallet: &WalletState) -> KeyOutcome {
        if self.lp_enable_check_inflight {
            self.status = "Still checking token access…".into();
            return KeyOutcome::Consumed;
        }
        if self.enables_ready_for_add() {
            self.status = "Both tokens already enabled — Enter to review add".into();
            return KeyOutcome::Consumed;
        }
        let Some(next_sym) = self.next_enable_symbol() else {
            self.status = "Nothing to enable — Enter to review add".into();
            return KeyOutcome::Consumed;
        };
        self.busy = Busy::Loading;
        self.status = format!("Preparing Enable {next_sym}…");
        match self.build_lp_enable_prepare_job(wallet, next_sym) {
            Ok(job) => KeyOutcome::StartJob(job),
            Err(e) => {
                self.busy = Busy::Idle;
                self.status = e;
                KeyOutcome::Consumed
            }
        }
    }

    pub(crate) fn open_enable_confirm(
        &mut self,
        tx: vaughan_core::chains::EvmTransaction,
        symbol: String,
        label: String,
    ) {
        let action = LpConfirmAction::Enable {
            symbol: symbol.clone(),
            label: label.clone(),
        };
        let lines = self.build_enable_confirm_summary(&action);
        self.lp_enable_in_confirm = true;
        self.lp_enable_last_label = label;
        self.confirm_ui = Some(Box::new(LpConfirmUi {
            action,
            lines,
            pending_tx: tx.clone(),
            pending_fee_estimate: Some(tx),
            base_fee: None,
            speed: vaughan_core::chains::FeeSpeed::Normal,
            custom_gas: crate::input::Input::new(false, "gwei"),
            focus: LpConfirmFocus::Speed,
            pipeline_step: false,
        }));
        self.confirm_lines.clear();
        self.stage = Stage::Confirm;
        self.busy = Busy::EstimatingFee;
        self.status = "Enable token · Enter send · Esc cancel".into();
    }

    pub fn followup_job(&mut self, wallet: &WalletState) -> Option<UiJob> {
        if let Some(job) = self.enable_followup_job(wallet) {
            return Some(job);
        }
        if let Some(job) = self.followup_deploy_job(wallet) {
            return Some(job);
        }
        if self.lp_reload_pending {
            self.lp_reload_pending = false;
            if let Some(job) = self.list_job(wallet) {
                self.busy = Busy::Loading;
                self.status = "Refreshing positions…".into();
                return Some(job);
            }
        }
        None
    }

    pub(crate) fn enable_followup_job(&mut self, wallet: &WalletState) -> Option<UiJob> {
        if self.lp_enable_recheck_pending {
            self.lp_enable_recheck_pending = false;
            return self.spawn_enable_check(wallet);
        }
        if !self.lp_enable_pending_resume {
            return None;
        }
        self.lp_enable_pending_resume = false;
        let job = self.build_lp_enable_wait_job(wallet).ok()?;
        self.busy = Busy::Loading;
        self.status = "Waiting for Enable on chain…".into();
        Some(job)
    }
}
