//! V3 LP deploy orchestrator: wait → prepare → propose loop for Brews.
//!
//! Persists in-flight jobs under `{profile_dir}/lp_deploy_jobs/` so MCP/TUI can
//! resume multi-step pipelines across approval cards.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, Bytes, U256};
use serde::{Deserialize, Serialize};
use wiz4rd_sdk::allowance::build_approve_tx;
use wiz4rd_sdk::pool_address::{compute_pool_address, get_pool_key};

use crate::chains::EvmTransaction;
use crate::core::dex_catalog::{
    parse_dex_venue_label, venue_pool_deployer, venue_position_manager,
};
use crate::core::dex_lp::{
    build_v3_create_pool_evm, build_v3_initialize_pool_from_human_price_evm, build_v3_mint_evm,
    lp_deploy_fixup_swapped_amounts, v3_lp_deploy_mint_amounts, v3_lp_mint_tick_range,
    v3_lp_prepare_deploy_step, v3_lp_run_deploy_wait, v3_pool_lifecycle, V3LpDeployContext,
    V3LpDeployParams, V3LpDeployWait, V3PoolLifecycle,
};
use crate::core::dex_quote::{min_out_after_slippage, DEFAULT_DEX_SLIPPAGE_BPS};
use crate::core::proposal::{ProposalType, TxProposal};
use crate::core::WalletState;
use crate::error::WalletError;

/// Serializable mirror of [`V3LpDeployParams`] for job persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredLpDeployParams {
    pub from: String,
    pub venue: String,
    pub chain_id: u64,
    pub rpc_url: String,
    pub token0: String,
    pub token1: String,
    pub fee: u32,
    pub dec0: u8,
    pub dec1: u8,
    pub pool_initial_price: String,
    pub pool_min_price: String,
    pub pool_max_price: String,
    pub amount0: String,
    pub amount1: String,
    /// One-sided deposit leg when the Brew used a single-token deposit.
    #[serde(default = "default_deposit_on_token0")]
    pub deposit_on_token0: bool,
}

fn default_deposit_on_token0() -> bool {
    true
}

impl From<&V3LpDeployParams> for StoredLpDeployParams {
    fn from(p: &V3LpDeployParams) -> Self {
        Self {
            from: p.from.clone(),
            venue: p.venue.label().to_string(),
            chain_id: p.chain_id,
            rpc_url: p.rpc_url.clone(),
            token0: format!("{:#x}", p.token0),
            token1: format!("{:#x}", p.token1),
            fee: p.fee,
            dec0: p.dec0,
            dec1: p.dec1,
            pool_initial_price: p.pool_initial_price.clone(),
            pool_min_price: p.pool_min_price.clone(),
            pool_max_price: p.pool_max_price.clone(),
            amount0: p.amount0.clone(),
            amount1: p.amount1.clone(),
            deposit_on_token0: p.deposit_on_token0,
        }
    }
}

impl TryFrom<&StoredLpDeployParams> for V3LpDeployParams {
    type Error = WalletError;

    fn try_from(s: &StoredLpDeployParams) -> Result<Self, Self::Error> {
        let venue = parse_dex_venue_label(&s.venue)
            .ok_or_else(|| WalletError::Other(format!("unknown LP venue {:?}", s.venue)))?;
        let mut params = V3LpDeployParams {
            from: s.from.clone(),
            venue,
            chain_id: s.chain_id,
            rpc_url: s.rpc_url.clone(),
            token0: s
                .token0
                .parse()
                .map_err(|_| WalletError::InvalidTransaction("token0".into()))?,
            token1: s
                .token1
                .parse()
                .map_err(|_| WalletError::InvalidTransaction("token1".into()))?,
            fee: s.fee,
            dec0: s.dec0,
            dec1: s.dec1,
            pool_initial_price: s.pool_initial_price.clone(),
            pool_min_price: s.pool_min_price.clone(),
            pool_max_price: s.pool_max_price.clone(),
            amount0: s.amount0.clone(),
            amount1: s.amount1.clone(),
            deposit_on_token0: s.deposit_on_token0,
        };
        lp_deploy_fixup_swapped_amounts(&mut params);
        Ok(params)
    }
}

/// Job status for a multi-step LP deploy Brew.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LpDeployJobStatus {
    Active,
    Done,
    Failed,
}

/// Persisted orchestrator state between TUI approval steps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LpDeployJob {
    pub job_id: String,
    pub params: StoredLpDeployParams,
    pub last_label: Option<String>,
    /// Wait to run before the next `prepare` (set after each broadcast).
    #[serde(default)]
    pub pending_wait: V3LpDeployWait,
    pub status: LpDeployJobStatus,
    pub created_at_unix: u64,
    pub explanation: String,
}

/// Dry-run output for `vaughan lp plan`.
#[derive(Debug, Clone)]
pub struct LpDeployPlan {
    pub lifecycle: V3PoolLifecycle,
    pub steps: Vec<String>,
}

/// Ambire batch gas pin (see `vaughan_aa::DEFAULT_BATCH_GAS_LIMIT`).
const LP_BATCH_GAS_BUDGET: u64 = 1_000_000;

/// Estimated gas per LP batch leg for budget warnings (conservative).
const LP_BATCH_EST_GAS_PER_CALL: u64 = 180_000;

/// One atomic EIP-7702 batch for a full LP deploy Brew.
#[derive(Debug, Clone)]
pub struct LpDeployBatchPlan {
    pub steps: Vec<String>,
    pub calls: Vec<EvmTransaction>,
    /// Set when estimated gas exceeds [`LP_BATCH_GAS_BUDGET`].
    pub gas_warning: Option<String>,
}

/// Outcome of one orchestrator tick.
#[derive(Debug, Clone)]
pub enum LpDeployStepOutcome {
    Step {
        tx: Box<EvmTransaction>,
        label: String,
        wait_after: V3LpDeployWait,
    },
    Done,
}

fn jobs_dir(profile_dir: &Path) -> PathBuf {
    profile_dir.join("lp_deploy_jobs")
}

fn job_path(profile_dir: &Path, job_id: &str) -> PathBuf {
    jobs_dir(profile_dir).join(format!("{job_id}.json"))
}

/// Save job state (0600 dir on unix).
pub fn lp_deploy_job_save(profile_dir: &Path, job: &LpDeployJob) -> Result<(), WalletError> {
    let dir = jobs_dir(profile_dir);
    fs::create_dir_all(&dir).map_err(|e| WalletError::Other(format!("lp job dir: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }
    let path = job_path(profile_dir, &job.job_id);
    let json = serde_json::to_string_pretty(job)
        .map_err(|e| WalletError::Other(format!("serialize lp job: {e}")))?;
    fs::write(&path, json).map_err(|e| WalletError::Other(format!("write lp job: {e}")))?;
    Ok(())
}

/// Load a job by id.
pub fn lp_deploy_job_load(profile_dir: &Path, job_id: &str) -> Result<LpDeployJob, WalletError> {
    let path = job_path(profile_dir, job_id);
    let raw = fs::read_to_string(&path)
        .map_err(|_| WalletError::Other(format!("lp deploy job {job_id} not found")))?;
    serde_json::from_str(&raw).map_err(|e| WalletError::Other(format!("parse lp job: {e}")))
}

/// Create a new job and persist it.
pub fn lp_deploy_job_create(
    profile_dir: &Path,
    job_id: impl Into<String>,
    params: &V3LpDeployParams,
    explanation: impl Into<String>,
) -> Result<LpDeployJob, WalletError> {
    let job = LpDeployJob {
        job_id: job_id.into(),
        params: StoredLpDeployParams::from(params),
        last_label: None,
        pending_wait: V3LpDeployWait::None,
        status: LpDeployJobStatus::Active,
        created_at_unix: now_unix(),
        explanation: explanation.into(),
    };
    lp_deploy_job_save(profile_dir, &job)?;
    Ok(job)
}

/// Plan pipeline steps without broadcasting (lifecycle-aware).
pub async fn lp_deploy_plan(params: &V3LpDeployParams) -> Result<LpDeployPlan, WalletError> {
    let lifecycle = v3_pool_lifecycle(
        &params.rpc_url,
        params.venue,
        params.chain_id,
        params.token0,
        params.token1,
        params.fee,
    )
    .await?;
    let mut steps = Vec::new();
    match lifecycle {
        V3PoolLifecycle::Missing => {
            steps.push("createPool".into());
            steps.push("initialize".into());
        }
        V3PoolLifecycle::Uninitialized { .. } => steps.push("initialize".into()),
        V3PoolLifecycle::Ready => {}
    }
    steps.push("approve token0 for LP".into());
    steps.push("approve token1 for LP".into());
    steps.push("add liquidity".into());
    Ok(LpDeployPlan { lifecycle, steps })
}

/// Preflight: lifecycle + basic deposit validation + ERC-20 balance check.
pub async fn lp_deploy_preflight(params: &V3LpDeployParams) -> Result<(), WalletError> {
    lp_deploy_plan(params).await?;
    if params.pool_initial_price.trim().is_empty()
        && matches!(
            v3_pool_lifecycle(
                &params.rpc_url,
                params.venue,
                params.chain_id,
                params.token0,
                params.token1,
                params.fee,
            )
            .await?,
            V3PoolLifecycle::Missing | V3PoolLifecycle::Uninitialized { .. }
        )
    {
        return Err(WalletError::InvalidTransaction(
            "starting price required for new or uninitialized pool".into(),
        ));
    }
    let (need0, need1) = v3_lp_deploy_mint_amounts(params).await?;
    lp_deploy_check_balances(params, need0, need1).await
}

/// Network gas limit for one LP deploy step (uses `eth_estimateGas`, not the 500k default).
pub async fn lp_deploy_estimate_gas_limit(
    rpc_url: &str,
    chain_id: u64,
    tx: &EvmTransaction,
) -> Result<u64, WalletError> {
    use crate::chains::evm::networks::get_network_by_chain_id;
    use crate::chains::evm::EvmAdapter;
    use crate::chains::{ChainAdapter, ChainTransaction};

    let net = get_network_by_chain_id(chain_id)
        .ok_or_else(|| WalletError::NetworkError(format!("unknown chain id {chain_id}")))?;
    let mut probe = tx.clone();
    probe.gas_limit = None;
    probe.gas_price = None;
    probe.max_fee_per_gas = None;
    probe.max_priority_fee_per_gas = None;
    let adapter = EvmAdapter::new(rpc_url, chain_id, &net.name, &net.fallback_rpc_urls).await?;
    let fee = adapter.estimate_fee(&ChainTransaction::Evm(probe)).await?;
    match fee.details {
        crate::chains::FeeDetails::Evm { gas_limit, .. } => Ok(gas_limit),
        _ => Err(WalletError::InvalidTransaction(
            "LP deploy gas estimate returned non-EVM fee".into(),
        )),
    }
}

/// Same as [`lp_deploy_estimate_gas_limit`] via the unlocked wallet RPC.
pub async fn lp_deploy_wallet_gas_limit(
    wallet: &WalletState,
    tx: &EvmTransaction,
) -> Result<u64, WalletError> {
    let mut probe = tx.clone();
    probe.gas_limit = None;
    probe.gas_price = None;
    probe.max_fee_per_gas = None;
    probe.max_priority_fee_per_gas = None;
    let fee = wallet.estimate_transaction_fee(probe).await?;
    match fee.details {
        crate::chains::FeeDetails::Evm { gas_limit, .. } => Ok(gas_limit),
        _ => Err(WalletError::InvalidTransaction(
            "LP deploy gas estimate returned non-EVM fee".into(),
        )),
    }
}

async fn lp_deploy_check_balances(
    params: &V3LpDeployParams,
    need0: U256,
    need1: U256,
) -> Result<(), WalletError> {
    use alloy::providers::ProviderBuilder;
    use alloy::sol;
    sol! {
        #[sol(rpc)]
        contract Erc20Balance {
            function balanceOf(address owner) external view returns (uint256);
        }
    }
    let owner: Address = params
        .from
        .parse()
        .map_err(|_| WalletError::InvalidTransaction("invalid from".into()))?;
    let url = params
        .rpc_url
        .parse()
        .map_err(|_| WalletError::NetworkError("invalid RPC URL".into()))?;
    let provider = ProviderBuilder::new().connect_http(url);
    if !need0.is_zero() {
        let bal = Erc20Balance::new(params.token0, provider.clone())
            .balanceOf(owner)
            .call()
            .await
            .map_err(|e| WalletError::NetworkError(format!("balance0: {e}")))?;
        if bal < need0 {
            return Err(WalletError::InvalidTransaction(
                "insufficient token0 balance for LP deposit".into(),
            ));
        }
    }
    if !need1.is_zero() {
        let bal = Erc20Balance::new(params.token1, provider)
            .balanceOf(owner)
            .call()
            .await
            .map_err(|e| WalletError::NetworkError(format!("balance1: {e}")))?;
        if bal < need1 {
            return Err(WalletError::InvalidTransaction(
                "insufficient token1 balance for LP deposit".into(),
            ));
        }
    }
    Ok(())
}

/// Encode all deploy steps as sequential AA batch calls (no on-chain waits between legs).
pub async fn build_lp_deploy_batch_calls(
    params: &V3LpDeployParams,
) -> Result<LpDeployBatchPlan, WalletError> {
    if params.token0 >= params.token1 {
        return Err(WalletError::InvalidTransaction(
            "token0 must be sorted below token1".into(),
        ));
    }
    let deployer = venue_pool_deployer(params.venue, params.chain_id).ok_or_else(|| {
        WalletError::Other(format!(
            "batch LP deploy unsupported for {} on chain {}",
            params.venue.label(),
            params.chain_id
        ))
    })?;
    let lifecycle = v3_pool_lifecycle(
        &params.rpc_url,
        params.venue,
        params.chain_id,
        params.token0,
        params.token1,
        params.fee,
    )
    .await?;
    let key = get_pool_key(params.token0, params.token1, params.fee);
    let derived_pool = compute_pool_address(deployer, key);
    let init_price = if params.pool_initial_price.trim().is_empty() {
        params.pool_min_price.trim()
    } else {
        params.pool_initial_price.trim()
    };
    let mut steps = Vec::new();
    let mut calls = Vec::new();

    match lifecycle {
        V3PoolLifecycle::Missing => {
            calls.push(build_v3_create_pool_evm(
                &params.from,
                params.venue,
                params.chain_id,
                &params.rpc_url,
                params.token0,
                params.token1,
                params.fee,
            )?);
            steps.push("createPool".into());
            if init_price.is_empty() {
                return Err(WalletError::InvalidTransaction(
                    "starting price required for new pool batch".into(),
                ));
            }
            calls.push(build_v3_initialize_pool_from_human_price_evm(
                &params.from,
                params.chain_id,
                derived_pool,
                params.token0,
                params.token1,
                params.dec0,
                params.dec1,
                init_price,
                params.fee,
            )?);
            steps.push("initialize".into());
        }
        V3PoolLifecycle::Uninitialized { pool } => {
            if init_price.is_empty() {
                return Err(WalletError::InvalidTransaction(
                    "starting price required for uninitialized pool batch".into(),
                ));
            }
            calls.push(build_v3_initialize_pool_from_human_price_evm(
                &params.from,
                params.chain_id,
                pool,
                params.token0,
                params.token1,
                params.dec0,
                params.dec1,
                init_price,
                params.fee,
            )?);
            steps.push("initialize".into());
        }
        V3PoolLifecycle::Ready => {}
    }

    let npm = venue_position_manager(params.venue, params.chain_id).ok_or_else(|| {
        WalletError::Other(format!(
            "{} has no V3 NPM on chain {}",
            params.venue.label(),
            params.chain_id
        ))
    })?;
    let (amount0, amount1) = v3_lp_deploy_mint_amounts(params).await?;
    if amount0.is_zero() && amount1.is_zero() {
        return Err(WalletError::InvalidTransaction(
            "deposit amounts must be > 0".into(),
        ));
    }
    if !amount0.is_zero() {
        let req = build_approve_tx(params.token0, npm, U256::MAX);
        calls.push(evm_from_approve_req(&params.from, params.chain_id, req)?);
        steps.push("approve token0 for LP".into());
    }
    if !amount1.is_zero() {
        let req = build_approve_tx(params.token1, npm, U256::MAX);
        calls.push(evm_from_approve_req(&params.from, params.chain_id, req)?);
        steps.push("approve token1 for LP".into());
    }

    let (tick_lower, tick_upper) = v3_lp_mint_tick_range(params)?;
    let amount0_min = min_out_after_slippage(amount0, DEFAULT_DEX_SLIPPAGE_BPS);
    let amount1_min = min_out_after_slippage(amount1, DEFAULT_DEX_SLIPPAGE_BPS);
    calls.push(build_v3_mint_evm(
        &params.from,
        params.venue,
        params.chain_id,
        &params.rpc_url,
        params.token0,
        params.token1,
        params.fee,
        tick_lower,
        tick_upper,
        amount0,
        amount1,
        amount0_min,
        amount1_min,
        None,
    )?);
    steps.push("add liquidity".into());

    let est_gas = calls.len() as u64 * LP_BATCH_EST_GAS_PER_CALL;
    let gas_warning = if est_gas > LP_BATCH_GAS_BUDGET {
        Some(format!(
            "estimated ~{est_gas} gas exceeds {LP_BATCH_GAS_BUDGET} batch limit — use step mode"
        ))
    } else {
        None
    };

    Ok(LpDeployBatchPlan {
        steps,
        calls,
        gas_warning,
    })
}

fn evm_from_approve_req(
    from: &str,
    chain_id: u64,
    req: alloy::rpc::types::TransactionRequest,
) -> Result<EvmTransaction, WalletError> {
    let to = req
        .to
        .as_ref()
        .and_then(|t| t.to().copied())
        .ok_or_else(|| WalletError::InvalidTransaction("approve tx missing to".into()))?;
    let data = req.input.input().map(|b| format!("0x{}", hex::encode(b)));
    Ok(EvmTransaction {
        from: from.to_string(),
        to: format!("{to:#x}"),
        value: req.value.unwrap_or_default().to_string(),
        data,
        gas_limit: None,
        gas_price: None,
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
        nonce: None,
        chain_id,
    })
}
/// Run pending on-chain wait, then prepare the next tx.
pub async fn lp_deploy_next_step(
    job: &mut LpDeployJob,
) -> Result<LpDeployStepOutcome, WalletError> {
    if job.status != LpDeployJobStatus::Active {
        return Err(WalletError::Other("lp deploy job is not active".into()));
    }
    let params = V3LpDeployParams::try_from(&job.params)?;
    job.params.amount0 = params.amount0.clone();
    job.params.amount1 = params.amount1.clone();

    if job.pending_wait != V3LpDeployWait::None {
        let ctx = job.last_label.as_ref().map(|label| V3LpDeployContext {
            last_step_label: Some(label.clone()),
        });
        v3_lp_run_deploy_wait(job.pending_wait, &params, ctx.as_ref()).await?;
        job.pending_wait = V3LpDeployWait::None;
    }

    match v3_lp_prepare_deploy_step(&params).await {
        Ok((tx, label)) => {
            let wait_after = wait_after_label(&label);
            job.last_label = Some(label.clone());
            job.pending_wait = wait_after;
            if label == "add liquidity" {
                // mint is the final step — after broadcast we're done
            }
            Ok(LpDeployStepOutcome::Step {
                tx: Box::new(tx),
                label,
                wait_after,
            })
        }
        Err(e) if e.user_message().contains("deposit") => Err(e),
        Err(e) => {
            job.status = LpDeployJobStatus::Failed;
            Err(e)
        }
    }
}

/// Mark job complete after final step broadcast.
pub fn lp_deploy_job_mark_done(job: &mut LpDeployJob) {
    job.status = LpDeployJobStatus::Done;
    job.pending_wait = V3LpDeployWait::None;
}

/// Map step label to post-broadcast wait kind.
pub fn wait_after_label(label: &str) -> V3LpDeployWait {
    match label {
        "createPool" => V3LpDeployWait::AfterCreatePool,
        "initialize" => V3LpDeployWait::AfterInitialize,
        s if s.starts_with("approve") => V3LpDeployWait::AfterApprove,
        "add liquidity" => V3LpDeployWait::None,
        _ => V3LpDeployWait::None,
    }
}

/// Build an MCP [`TxProposal`] for one LP deploy step.
pub fn lp_deploy_step_to_proposal(
    job: &LpDeployJob,
    step_label: &str,
    tx: EvmTransaction,
    gas_limit: u64,
    simulation_success: bool,
    estimated_fee_wei: Option<U256>,
) -> Result<TxProposal, WalletError> {
    let to: Address = tx
        .to
        .parse()
        .map_err(|_| WalletError::InvalidTransaction("invalid to".into()))?;
    let value = U256::from_str_radix(tx.value.trim(), 10)
        .or_else(|_| U256::from_str_radix(tx.value.trim().trim_start_matches("0x"), 16))
        .unwrap_or(U256::ZERO);
    let calldata = match &tx.data {
        Some(d) if !d.is_empty() => Bytes::from(
            hex::decode(d.trim_start_matches("0x"))
                .map_err(|e| WalletError::InvalidTransaction(format!("calldata hex: {e}")))?,
        ),
        _ => Bytes::new(),
    };
    let proposal_id = format!("lp-{}-{}", job.job_id, step_label.replace(' ', "-"));
    let mut proposal = TxProposal::new(
        proposal_id,
        ProposalType::LpDeployStep {
            job_id: job.job_id.clone(),
            step_label: step_label.to_string(),
        },
        to,
        value,
        calldata,
        gas_limit,
        simulation_success,
        job.explanation.clone(),
    )
    .with_chain(job.params.chain_id, None);
    proposal.estimated_fee_wei = estimated_fee_wei;
    Ok(proposal)
}

/// After approve broadcast failed to enqueue mint, fix job legs and try once more.
pub async fn lp_deploy_retry_after_approve(
    profile_dir: &Path,
    wallet: &WalletState,
    job_id: &str,
    session_secret: &[u8],
    source: &str,
) -> Result<Option<TxProposal>, WalletError> {
    let mut job = lp_deploy_job_load(profile_dir, job_id)?;
    if job.status != LpDeployJobStatus::Active {
        return Ok(None);
    }
    if job.pending_wait != V3LpDeployWait::AfterApprove {
        return Ok(None);
    }
    let fixed = V3LpDeployParams::try_from(&job.params)?;
    job.params.amount0 = fixed.amount0;
    job.params.amount1 = fixed.amount1;
    job.pending_wait = V3LpDeployWait::None;
    lp_deploy_job_save(profile_dir, &job)?;
    lp_deploy_advance_after_broadcast(profile_dir, wallet, job_id, session_secret, source).await
}

/// After a step is broadcast: run wait, enqueue next proposal if any.
pub async fn lp_deploy_advance_after_broadcast(
    profile_dir: &Path,
    wallet: &WalletState,
    job_id: &str,
    session_secret: &[u8],
    source: &str,
) -> Result<Option<TxProposal>, WalletError> {
    let mut job = lp_deploy_job_load(profile_dir, job_id)?;
    let step_label = job.last_label.clone().unwrap_or_default();

    if step_label == "add liquidity" {
        lp_deploy_job_mark_done(&mut job);
        lp_deploy_job_save(profile_dir, &job)?;
        return Ok(None);
    }

    match lp_deploy_next_step(&mut job).await? {
        LpDeployStepOutcome::Step { tx, label, .. } => {
            let gas_limit = lp_deploy_wallet_gas_limit(wallet, &tx).await?;
            let mut tx_for_fee = (*tx).clone();
            tx_for_fee.gas_limit = Some(gas_limit);
            let estimated = wallet.estimate_transaction_fee(tx_for_fee).await.ok();
            let fee_wei = estimated.as_ref().and_then(|f| f.total_wei_evm());
            let mut proposal =
                lp_deploy_step_to_proposal(&job, &label, (*tx).clone(), gas_limit, true, fee_wei)?;
            proposal.estimated_fee_wei = fee_wei;
            lp_deploy_job_save(profile_dir, &job)?;
            let queue = crate::core::proposal::ProposalQueue::new(profile_dir);
            queue
                .enqueue(proposal.clone(), source, session_secret)
                .map_err(WalletError::from)?;
            Ok(Some(proposal))
        }
        LpDeployStepOutcome::Done => {
            lp_deploy_job_mark_done(&mut job);
            lp_deploy_job_save(profile_dir, &job)?;
            Ok(None)
        }
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_after_maps_create_pool() {
        assert_eq!(
            wait_after_label("createPool"),
            V3LpDeployWait::AfterCreatePool
        );
    }

    #[test]
    fn wait_after_maps_approve() {
        assert_eq!(
            wait_after_label("approve token0 for LP"),
            V3LpDeployWait::AfterApprove
        );
    }
}
