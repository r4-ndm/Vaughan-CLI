//! Shared helpers for wiz4rd V3 MCP / agent tools (swap / quote on 943 deploy).
//!
//! V3 LP list/mint/lifecycle tools use [`super::v3_lp`] + catalog venues instead.

use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::ProviderBuilder;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};
use wiz4rd_sdk::config::Config;
use wiz4rd_sdk::pool::{get_pool_info, PoolInfo};
use wiz4rd_sdk::pool_address::get_pool_key;
use wiz4rd_sdk::tx::swap::{apply_slippage, build_swap_exact_in, zero_for_one};
use wiz4rd_sdk::{quote_exact_in, Quote};

use crate::error::AgentError;
use crate::tools::ToolContext;
use vaughan_core::core::wiz4rd::{deployment_for_chain, parse_addr, WPLS_943};

/// Build a wiz4rd SDK [`Config`] for the active chain (943 today).
pub fn config_for_context(context: &ToolContext) -> Result<Config, AgentError> {
    let dep = deployment_for_chain(context.chain_id).ok_or_else(|| {
        AgentError::InvalidToolCall(format!(
            "wiz4rd is not deployed on chain_id {} — use PulseChain testnet 943",
            context.chain_id
        ))
    })?;
    Ok(Config {
        rpc_url: Some(context.rpc_url.clone()),
        chain_id: context.chain_id,
        factory: parse_addr(dep.factory),
        swap_router: parse_addr(dep.swap_router),
        position_manager: parse_addr(dep.position_manager),
        protocol_fee: 0,
        vaughan_provider: None,
        vaughan_origin: None,
    })
}

/// Resolve token arg: address, `WPLS`, `WZRD`, or `native`/`PLS` → (address, is_native).
pub fn resolve_token(raw: &str, chain_id: u64) -> Result<(Address, bool), AgentError> {
    let s = raw.trim();
    if s.eq_ignore_ascii_case("native")
        || s.eq_ignore_ascii_case("pls")
        || s.eq_ignore_ascii_case("eth")
    {
        let wpls = wiz4rd_sdk::tokens::lookup("WPLS", chain_id)
            .map(|t| t.address)
            .or_else(|| parse_addr(WPLS_943))
            .ok_or_else(|| AgentError::InvalidToolCall("no WPLS for chain".into()))?;
        return Ok((wpls, true));
    }
    if s.eq_ignore_ascii_case("wpls") {
        let wpls = wiz4rd_sdk::tokens::lookup("WPLS", chain_id)
            .map(|t| t.address)
            .or_else(|| parse_addr(WPLS_943))
            .ok_or_else(|| AgentError::InvalidToolCall("no WPLS for chain".into()))?;
        return Ok((wpls, false));
    }
    if s.eq_ignore_ascii_case("wzrd") {
        let a = parse_addr(vaughan_core::core::wiz4rd::WZRD_SMOKE_943)
            .ok_or_else(|| AgentError::InvalidToolCall("invalid WZRD address".into()))?;
        return Ok((a, false));
    }
    let addr = Address::from_str(s)
        .map_err(|e| AgentError::InvalidToolCall(format!("Invalid token address: {e}")))?;
    Ok((addr, false))
}

pub async fn load_pool(
    context: &ToolContext,
    token_a: Address,
    token_b: Address,
    fee: u32,
) -> Result<(Config, PoolInfo), AgentError> {
    let cfg = config_for_context(context)?;
    let key = get_pool_key(token_a, token_b, fee);
    let provider = ProviderBuilder::new().connect_http(
        cfg.rpc_url()
            .parse()
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid RPC URL: {e}")))?,
    );
    let info = get_pool_info(&provider, &cfg, key)
        .await
        .map_err(|e| AgentError::ProviderError(format!("get_pool_info: {e}")))?;
    if info.pool.is_zero() {
        return Err(AgentError::ProviderError(
            "pool does not exist for this pair/fee".into(),
        ));
    }
    Ok((cfg, info))
}

pub fn quote_pool(
    pool: &PoolInfo,
    token_in: Address,
    amount_in: U256,
) -> Result<Quote, AgentError> {
    let zfo = zero_for_one(pool, token_in);
    if token_in != pool.token0 && token_in != pool.token1 {
        return Err(AgentError::InvalidToolCall(
            "token_in is not in this pool".into(),
        ));
    }
    quote_exact_in(pool, amount_in, zfo).map_err(|e| AgentError::ProviderError(e.to_string()))
}

pub fn build_exact_in_calldata(
    cfg: &Config,
    pool: &PoolInfo,
    token_in: Address,
    amount_in: U256,
    amount_out_min: U256,
    recipient: Address,
) -> Result<(Address, Bytes, U256), AgentError> {
    let deadline = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() + 600)
        .unwrap_or(u64::MAX);
    let tx = build_swap_exact_in(
        cfg,
        pool,
        token_in,
        amount_in,
        amount_out_min,
        recipient,
        deadline,
        None,
    )
    .map_err(|e| AgentError::InvalidToolCall(e.to_string()))?;
    let to = match tx.to {
        Some(alloy::primitives::TxKind::Call(a)) => a,
        _ => cfg
            .swap_router
            .ok_or_else(|| AgentError::InvalidToolCall("swap tx missing to".into()))?,
    };
    let data = tx
        .input
        .into_input()
        .ok_or_else(|| AgentError::InvalidToolCall("swap tx missing calldata".into()))?;
    Ok((to, data, U256::ZERO))
}

pub fn slippage_min_out(amount_out: U256, slippage_bps: u32) -> U256 {
    apply_slippage(amount_out, slippage_bps)
}
