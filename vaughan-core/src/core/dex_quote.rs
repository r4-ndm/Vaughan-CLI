//! Direct DEX swap quotes for the TUI (read-only, no signing).
//!
//! - **V3:** local exact-in math via [`wiz4rd-sdk`] on chains with a wiz4rd deploy (943 today).
//! - **V2:** `router.getAmountsOut` via `eth_call` (Uni V2–compatible routers).

use alloy::primitives::{Address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::sol;
use alloy::sol_types::SolCall;

use crate::core::wiz4rd::{deployment_for_chain, parse_addr};
use crate::error::WalletError;
use wiz4rd_sdk::config::Config;
use wiz4rd_sdk::pool::get_pool_info;
use wiz4rd_sdk::pool_address::get_pool_key;
use wiz4rd_sdk::tx::swap::{apply_slippage, zero_for_one, BasisPoints};
use wiz4rd_sdk::{quote_exact_in, Quote};

sol! {
    interface IUniswapV2RouterQuote {
        function getAmountsOut(uint256 amountIn, address[] calldata path)
            external
            view
            returns (uint256[] memory amounts);
    }
}

/// Normalized exact-in quote for Dex / CLI (output token raw units).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DexQuote {
    pub amount_out: U256,
}

/// Default slippage for auto min-out (0.5%).
pub const DEFAULT_DEX_SLIPPAGE_BPS: BasisPoints = 50;

/// Minimum output after slippage tolerance (exact-in swaps).
pub fn min_out_after_slippage(amount_out: U256, slippage_bps: BasisPoints) -> U256 {
    apply_slippage(amount_out, slippage_bps)
}

fn connect_http(rpc_url: &str) -> Result<impl Provider + use<>, WalletError> {
    let url = rpc_url
        .trim()
        .parse()
        .map_err(|e| WalletError::NetworkError(format!("invalid RPC URL: {e}")))?;
    Ok(ProviderBuilder::new().connect_http(url))
}

fn wiz4rd_config(chain_id: u64, rpc_url: &str) -> Result<Config, WalletError> {
    let dep = deployment_for_chain(chain_id).ok_or_else(|| {
        WalletError::Other(format!(
            "V3 pool quote is not wired for chain {chain_id} yet (wiz4rd deploy required)"
        ))
    })?;
    Ok(Config {
        rpc_url: Some(rpc_url.trim().to_string()),
        chain_id,
        factory: parse_addr(dep.factory),
        swap_router: parse_addr(dep.swap_router),
        position_manager: parse_addr(dep.position_manager),
        protocol_fee: 0,
        vaughan_provider: None,
        vaughan_origin: None,
    })
}

/// Single-hop V3 exact-in quote (local math on live pool state).
pub async fn quote_v3_exact_in(
    rpc_url: &str,
    chain_id: u64,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
    fee: u32,
) -> Result<DexQuote, WalletError> {
    if amount_in.is_zero() {
        return Ok(DexQuote {
            amount_out: U256::ZERO,
        });
    }
    if token_in == token_out {
        return Err(WalletError::InvalidTransaction(
            "token in and token out must differ".into(),
        ));
    }

    let cfg = wiz4rd_config(chain_id, rpc_url)?;
    let key = get_pool_key(token_in, token_out, fee);
    let provider = connect_http(rpc_url)?;
    let pool = get_pool_info(&provider, &cfg, key)
        .await
        .map_err(|e| WalletError::NetworkError(format!("pool read: {e}")))?;
    if pool.pool.is_zero() {
        return Err(WalletError::Other(
            "no V3 pool for this pair and fee tier".into(),
        ));
    }

    let zfo = zero_for_one(&pool, token_in);
    if token_in != pool.token0 && token_in != pool.token1 {
        return Err(WalletError::InvalidTransaction(
            "token in is not in this pool".into(),
        ));
    }

    let Quote { amount_out, .. } = quote_exact_in(&pool, amount_in, zfo)
        .map_err(|e| WalletError::Other(format!("V3 quote math: {e}")))?;

    Ok(DexQuote { amount_out })
}

/// V2 exact-in quote via router `getAmountsOut`.
pub async fn quote_v2_exact_in(
    rpc_url: &str,
    router: Address,
    amount_in: U256,
    path: &[Address],
) -> Result<DexQuote, WalletError> {
    if amount_in.is_zero() {
        return Ok(DexQuote {
            amount_out: U256::ZERO,
        });
    }
    if path.len() < 2 {
        return Err(WalletError::InvalidTransaction(
            "swap path must have at least two tokens".into(),
        ));
    }

    let provider = connect_http(rpc_url)?;
    let call = IUniswapV2RouterQuote::getAmountsOutCall {
        amountIn: amount_in,
        path: path.to_vec(),
    };
    let tx = TransactionRequest::default()
        .to(router)
        .input(call.abi_encode().into());
    let raw = provider
        .call(tx)
        .await
        .map_err(|e| WalletError::NetworkError(format!("getAmountsOut: {e}")))?;
    let amounts = IUniswapV2RouterQuote::getAmountsOutCall::abi_decode_returns(&raw)
        .map_err(|e| WalletError::NetworkError(format!("decode getAmountsOut: {e}")))?;
    let amount_out = amounts
        .last()
        .copied()
        .ok_or_else(|| WalletError::NetworkError("getAmountsOut returned empty amounts".into()))?;
    Ok(DexQuote { amount_out })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_out_slippage_rounds_down() {
        let out = U256::from(1_000_000u64);
        assert_eq!(
            min_out_after_slippage(out, DEFAULT_DEX_SLIPPAGE_BPS),
            U256::from(995_000u64)
        );
    }
}
