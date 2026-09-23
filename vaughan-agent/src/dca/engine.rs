//! DCA engine — orchestrate due plan → quote → SentientTrader → record.
//!
//! No TUI / MCP knowledge. Callers supply the trader.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, Bytes, U256};
use alloy::sol;
use alloy::sol_types::SolCall;
use secrecy::SecretString;
use vaughan_core::core::{
    assert_agg_exec_targets_on_chain, get_v2_pair_address, min_out_after_slippage,
    quote_aggregator, quote_v2_exact_in, venue_swap_router, venue_wrapped_native, AggQuoteRequest,
    AggVenue, DexProtocol, DexVenue,
};

use super::store;
use super::trigger::{advance_schedule, due_indices};
use super::types::{
    DcaPlan, DcaPlanFile, DcaRouteResolved, DcaStatus, SliceLog, MAX_CONSECUTIVE_FAILURES,
    MAX_SLICE_LOG,
};
use crate::error::AgentError;
use crate::sentient::{SentientTrader, SwapExecution};

/// Swap deadline headroom (seconds) so DEX calldata cannot hang forever.
const DEX_DEADLINE_SECS: u64 = 20 * 60;

sol! {
    interface IUniswapV2RouterSwap {
        function swapExactETHForTokens(
            uint256 amountOutMin,
            address[] calldata path,
            address to,
            uint256 deadline
        ) external payable returns (uint256[] memory amounts);
    }
}

/// Outcome of attempting one slice.
#[derive(Debug, Clone)]
pub struct DcaFireResult {
    pub plan_id: String,
    pub ok: bool,
    pub dry_run: bool,
    pub tx_hash: String,
    pub error: String,
}

/// Calldata ready for [`SentientTrader::execute_swap_maybe_dry`].
struct PreparedSlice {
    to: Address,
    data: Bytes,
    value: U256,
    /// V2 pair for multi-RPC reserve quorum (DEX route only).
    pair: Option<Address>,
    /// Aggregator routers need the wider allowlist flag.
    allow_agg: bool,
}

/// Wall-clock unix seconds.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Pick the first due plan id (at most one fire per tick).
pub fn next_due_plan_id(file: &DcaPlanFile, now: u64) -> Option<String> {
    due_indices(&file.plans, now)
        .into_iter()
        .next()
        .map(|i| file.plans[i].id.clone())
}

/// Quote native → token_out via aggregator (rejects preview-only).
#[allow(clippy::too_many_arguments)]
async fn quote_slice_agg(
    venue: AggVenue,
    token_out: Address,
    amount_in: U256,
    slippage_bps: u32,
    chain_id: u64,
    account: Option<Address>,
    piteas_dir: Option<&Path>,
    vault_password: Option<&SecretString>,
) -> Result<PreparedSlice, AgentError> {
    let slippage_percent = slippage_bps as f64 / 100.0;
    let req = AggQuoteRequest {
        token_in: Address::ZERO,
        token_out,
        token_in_is_native: true,
        token_out_is_native: false,
        amount_in,
        slippage_percent,
        account,
    };
    let quote = quote_aggregator(venue, &req, chain_id, piteas_dir, vault_password)
        .await
        .map_err(|e| AgentError::ProviderError(e.to_string()))?;
    if !quote.is_executable() {
        return Err(AgentError::InvalidToolCall(
            "aggregator returned preview-only quote — refusing DCA fire".into(),
        ));
    }
    assert_agg_exec_targets_on_chain(chain_id, quote.tx.to, quote.spender)
        .map_err(|e| AgentError::SecurityViolation(e.to_string()))?;
    if quote.tx.value != amount_in {
        return Err(AgentError::SecurityViolation(format!(
            "aggregator tx.value {} != plan slice {} — refusing DCA fire",
            quote.tx.value, amount_in
        )));
    }
    Ok(PreparedSlice {
        to: quote.tx.to,
        data: quote.tx.data.clone(),
        value: quote.tx.value,
        pair: None,
        allow_agg: true,
    })
}

/// Quote + encode Uni V2 `swapExactETHForTokens` (wrapped native → token_out).
#[allow(clippy::too_many_arguments)]
async fn quote_slice_dex_v2(
    rpc_url: &str,
    venue: DexVenue,
    token_out: Address,
    amount_in: U256,
    slippage_bps: u32,
    chain_id: u64,
    recipient: Address,
    now: u64,
) -> Result<PreparedSlice, AgentError> {
    let router = venue_swap_router(venue, DexProtocol::V2, chain_id).ok_or_else(|| {
        AgentError::InvalidToolCall(format!(
            "DEX venue has no V2 swap router on chain {chain_id}"
        ))
    })?;
    let wrapped = venue_wrapped_native(venue, chain_id).ok_or_else(|| {
        AgentError::InvalidToolCall(format!(
            "no wrapped native for {} on chain {chain_id}",
            venue.label()
        ))
    })?;
    let path = vec![wrapped, token_out];
    let quoted = quote_v2_exact_in(rpc_url, router, amount_in, &path)
        .await
        .map_err(|e| AgentError::ProviderError(e.to_string()))?;
    if quoted.amount_out.is_zero() {
        return Err(AgentError::InvalidToolCall(
            "DEX quote returned zero amount_out — pair may be missing or illiquid".into(),
        ));
    }
    let pair = get_v2_pair_address(rpc_url, venue, chain_id, wrapped, token_out)
        .await
        .ok();
    let min_out = min_out_after_slippage(quoted.amount_out, slippage_bps);
    let deadline = U256::from(now.saturating_add(DEX_DEADLINE_SECS));
    let call = IUniswapV2RouterSwap::swapExactETHForTokensCall {
        amountOutMin: min_out,
        path,
        to: recipient,
        deadline,
    };
    Ok(PreparedSlice {
        to: router,
        data: Bytes::from(call.abi_encode()),
        value: amount_in,
        pair,
        allow_agg: false,
    })
}

/// Prepare a slice for the plan's resolved route (agg or DEX V2).
#[allow(clippy::too_many_arguments)]
async fn prepare_slice(
    route: DcaRouteResolved,
    token_out: Address,
    amount_in: U256,
    slippage_bps: u32,
    chain_id: u64,
    account: Address,
    rpc_url: &str,
    piteas_dir: Option<&Path>,
    vault_password: Option<&SecretString>,
    now: u64,
) -> Result<PreparedSlice, AgentError> {
    match route {
        DcaRouteResolved::Agg(venue) => {
            quote_slice_agg(
                venue,
                token_out,
                amount_in,
                slippage_bps,
                chain_id,
                Some(account),
                piteas_dir,
                vault_password,
            )
            .await
        }
        DcaRouteResolved::Dex { venue, protocol } => {
            if protocol != DexProtocol::V2 {
                return Err(AgentError::InvalidToolCall(
                    "DCA direct DEX currently supports V2 only (use route=agg for V3 venues)"
                        .into(),
                ));
            }
            quote_slice_dex_v2(
                rpc_url,
                venue,
                token_out,
                amount_in,
                slippage_bps,
                chain_id,
                account,
                now,
            )
            .await
        }
    }
}

/// Apply a successful or failed fire to the in-memory plan.
pub fn record_outcome(
    plan: &mut DcaPlan,
    now: u64,
    outcome: Result<&SwapExecution, &str>,
    amount_in: &U256,
) {
    match outcome {
        Ok(exec) => {
            plan.consecutive_failures = 0;
            plan.slices_done = plan.slices_done.saturating_add(1);
            if let Ok(spent) = plan.spent_u256() {
                plan.spent_wei = spent.saturating_add(*amount_in).to_string();
            }
            advance_schedule(plan, now);
            if plan.slices_done >= plan.max_slices {
                plan.status = DcaStatus::Done;
            }
            push_log(
                plan,
                SliceLog {
                    at: now,
                    ok: true,
                    tx_hash: format!("{:#x}", exec.tx_hash),
                    dry_run: exec.dry_run || plan.dry_run,
                    error: String::new(),
                    amount_in_wei: amount_in.to_string(),
                },
            );
        }
        Err(msg) => {
            plan.consecutive_failures = plan.consecutive_failures.saturating_add(1);
            if plan.consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                plan.status = DcaStatus::Paused;
            }
            // Still advance so we do not hammer the same second on transient errors.
            advance_schedule(plan, now);
            push_log(
                plan,
                SliceLog {
                    at: now,
                    ok: false,
                    tx_hash: String::new(),
                    dry_run: plan.dry_run,
                    error: msg.to_string(),
                    amount_in_wei: amount_in.to_string(),
                },
            );
        }
    }
}

fn push_log(plan: &mut DcaPlan, entry: SliceLog) {
    plan.slice_log.push(entry);
    if plan.slice_log.len() > MAX_SLICE_LOG {
        let drop_n = plan.slice_log.len() - MAX_SLICE_LOG;
        plan.slice_log.drain(0..drop_n);
    }
}

/// Persist an outcome without clobbering concurrent cancel/pause/add.
///
/// Reloads the file and mutates only this plan. If the user paused or
/// cancelled during the fire, the slice is still logged (spend accounting
/// must match chain) but the user's status wins.
fn persist_outcome(
    dir: &Path,
    plan_id: &str,
    now: u64,
    outcome: Result<&SwapExecution, &str>,
    amount_in: &U256,
) -> Result<(), AgentError> {
    store::update_plan(dir, plan_id, |p| {
        apply_outcome_preserving_status(p, now, outcome, amount_in);
        Ok(())
    })?;
    Ok(())
}

fn apply_outcome_preserving_status(
    plan: &mut DcaPlan,
    now: u64,
    outcome: Result<&SwapExecution, &str>,
    amount_in: &U256,
) {
    let prior = plan.status;
    record_outcome(plan, now, outcome, amount_in);
    if prior != DcaStatus::Active {
        plan.status = prior;
    }
}

/// Fire one plan by id: quote → execute → persist.
pub async fn fire_plan(
    dir: &Path,
    plan_id: &str,
    trader: &SentientTrader,
    chain_id: u64,
    piteas_dir: Option<&Path>,
    vault_password: Option<&SecretString>,
    now: u64,
) -> Result<DcaFireResult, AgentError> {
    let file = store::load_plans(dir)?;
    let plan =
        file.plans.iter().find(|p| p.id == plan_id).ok_or_else(|| {
            AgentError::InvalidToolCall(format!("dca plan `{plan_id}` not found"))
        })?;

    if plan.status != DcaStatus::Active {
        return Err(AgentError::InvalidToolCall(format!(
            "plan `{plan_id}` is {}",
            plan.status.as_str()
        )));
    }
    if !super::trigger::is_due(plan, now) {
        return Err(AgentError::InvalidToolCall(format!(
            "plan `{plan_id}` is not due yet"
        )));
    }
    if plan.chain_id == 0 || plan.account.trim().is_empty() {
        return Err(AgentError::SecurityViolation(format!(
            "plan `{plan_id}` predates chain/wallet binding — cancel it and create a new plan"
        )));
    }
    if plan.chain_id != chain_id {
        return Err(AgentError::SecurityViolation(format!(
            "plan `{plan_id}` bound to chain {} — active chain is {chain_id}",
            plan.chain_id
        )));
    }
    let bound_account = plan
        .account_address()
        .map_err(AgentError::InvalidToolCall)?;
    if bound_account != trader.address() {
        return Err(AgentError::SecurityViolation(format!(
            "plan `{plan_id}` bound to {bound_account:#x} — active wallet is {:#x}",
            trader.address()
        )));
    }
    plan.venue
        .validate_for_chain(chain_id)
        .map_err(AgentError::InvalidToolCall)?;
    let token_out = plan
        .token_out_address()
        .map_err(AgentError::InvalidToolCall)?;
    let amount_in = plan
        .slice_amount_u256()
        .map_err(AgentError::InvalidToolCall)?;
    let route = plan.venue.resolve().map_err(AgentError::InvalidToolCall)?;
    let plan_dry = plan.dry_run;

    let rpc_url = trader.primary_rpc_url().ok_or_else(|| {
        AgentError::InvalidToolCall("No RPC endpoints configured for DCA fire".into())
    })?;

    let prepared = match prepare_slice(
        route,
        token_out,
        amount_in,
        plan.max_slippage_bps,
        chain_id,
        trader.address(),
        rpc_url,
        piteas_dir,
        vault_password,
        now,
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            let msg = e.to_string();
            let _ = persist_outcome(dir, plan_id, now, Err(msg.as_str()), &amount_in);
            return Ok(DcaFireResult {
                plan_id: plan_id.to_string(),
                ok: false,
                dry_run: plan_dry,
                tx_hash: String::new(),
                error: msg,
            });
        }
    };

    // Belt-and-suspenders: native value on the wire must match the plan slice.
    if prepared.value != amount_in {
        let msg = format!(
            "prepared value {} != plan slice {amount_in} — refusing DCA fire",
            prepared.value
        );
        let _ = persist_outcome(dir, plan_id, now, Err(msg.as_str()), &amount_in);
        return Ok(DcaFireResult {
            plan_id: plan_id.to_string(),
            ok: false,
            dry_run: plan_dry,
            tx_hash: String::new(),
            error: msg,
        });
    }

    let exec = trader
        .execute_swap_maybe_dry(
            prepared.to,
            prepared.pair,
            prepared.data,
            prepared.value,
            prepared.value, // breaker sizes against the value we actually send
            plan.max_slippage_bps,
            plan_dry,
            prepared.allow_agg,
        )
        .await;

    match exec {
        Ok(outcome) => {
            persist_outcome(dir, plan_id, now, Ok(&outcome), &amount_in)?;
            Ok(DcaFireResult {
                plan_id: plan_id.to_string(),
                ok: true,
                dry_run: outcome.dry_run,
                tx_hash: format!("{:#x}", outcome.tx_hash),
                error: String::new(),
            })
        }
        Err(e) => {
            let msg = e.to_string();
            let _ = persist_outcome(dir, plan_id, now, Err(msg.as_str()), &amount_in);
            Ok(DcaFireResult {
                plan_id: plan_id.to_string(),
                ok: false,
                dry_run: plan_dry || trader.is_dry_run(),
                tx_hash: String::new(),
                error: msg,
            })
        }
    }
}

/// Load plans and return the next due id for this tick (if any).
pub fn poll_due_id(dir: &Path, now: u64) -> Result<Option<String>, AgentError> {
    let file = store::load_plans(dir)?;
    Ok(next_due_plan_id(&file, now))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dca::types::{DcaTrigger, DcaVenue};
    use alloy::primitives::B256;

    fn plan() -> DcaPlan {
        DcaPlan {
            id: "dca-t".into(),
            chain_id: 369,
            account: "0x1111111111111111111111111111111111111111".into(),
            token_out: "0x2222222222222222222222222222222222222222".into(),
            slice_amount_wei: "1000".into(),
            trigger: DcaTrigger::Time {
                interval_secs: 3600,
            },
            venue: DcaVenue::default(),
            max_slippage_bps: 100,
            max_slices: 2,
            slices_done: 0,
            spent_wei: "0".into(),
            consecutive_failures: 0,
            status: DcaStatus::Active,
            created_at: 0,
            next_due_at: 0,
            dry_run: true,
            slice_log: vec![],
        }
    }

    #[test]
    fn record_success_advances_and_completes() {
        let mut p = plan();
        let exec = SwapExecution {
            tx_hash: B256::ZERO,
            dry_run: true,
        };
        let amt = U256::from(1000u64);
        record_outcome(&mut p, 100, Ok(&exec), &amt);
        assert_eq!(p.slices_done, 1);
        assert_eq!(p.next_due_at, 3700);
        assert_eq!(p.spent_wei, "1000");
        record_outcome(&mut p, 3700, Ok(&exec), &amt);
        assert_eq!(p.status, DcaStatus::Done);
        assert_eq!(p.slices_done, 2);
    }

    #[test]
    fn cancel_during_fire_keeps_status_but_logs_spend() {
        let mut p = plan();
        p.status = DcaStatus::Cancelled;
        let exec = SwapExecution {
            tx_hash: B256::ZERO,
            dry_run: false,
        };
        let amt = U256::from(1000u64);
        apply_outcome_preserving_status(&mut p, 100, Ok(&exec), &amt);
        assert_eq!(p.status, DcaStatus::Cancelled);
        assert_eq!(p.spent_wei, "1000");
        assert_eq!(p.slice_log.len(), 1);
    }

    #[test]
    fn legacy_plan_without_binding_deserializes_unbound() {
        let json = r#"{"id":"x","token_out":"0x2222222222222222222222222222222222222222","slice_amount_wei":"1","trigger":{"kind":"time","interval_secs":900},"venue":{"kind":"agg","name":"auto"},"max_slippage_bps":100,"max_slices":1,"created_at":1,"next_due_at":1}"#;
        let p: DcaPlan = serde_json::from_str(json).unwrap();
        assert_eq!(p.chain_id, 0);
        assert!(p.account.is_empty());
    }

    #[test]
    fn three_failures_pause() {
        let mut p = plan();
        let amt = U256::from(1u64);
        for _ in 0..3 {
            record_outcome(&mut p, 0, Err("boom"), &amt);
        }
        assert_eq!(p.status, DcaStatus::Paused);
        assert_eq!(p.consecutive_failures, 3);
    }
}
