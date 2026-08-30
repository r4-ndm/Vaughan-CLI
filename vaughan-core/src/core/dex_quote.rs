//! Direct DEX swap quotes for the TUI (read-only, no signing).
//!
//! - **V3:** local exact-in math via [`wiz4rd-sdk`] on chains with a wiz4rd deploy (943 today).
//!   Single- and multi-hop paths share the same fee tier per hop (Dex TUI packed path).
//! - **V2:** `router.getAmountsOut` via `eth_call` (Uni V2–compatible routers).

use alloy::primitives::{Address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::sol;
use alloy::sol_types::SolCall;

use crate::core::wiz4rd::{deployment_for_chain, parse_addr};
use crate::error::WalletError;
use wiz4rd_sdk::config::Config;
use wiz4rd_sdk::pool::{get_pool_info, PoolInfo};
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
    quote_v3_path_exact_in(rpc_url, chain_id, &[token_in, token_out], amount_in, fee).await
}

/// Multi-hop V3 exact-in quote: chains local single-pool quotes along `path`.
///
/// Uses the same fee tier for every hop (matches [`encode_v3_path`] / Dex TUI).
/// Approximation caveats are the same as [`quote_exact_in`] — large swaps that
/// cross ticks may diverge slightly from on-chain settlement.
pub async fn quote_v3_path_exact_in(
    rpc_url: &str,
    chain_id: u64,
    path: &[Address],
    amount_in: U256,
    fee: u32,
) -> Result<DexQuote, WalletError> {
    if path.len() < 2 {
        return Err(WalletError::InvalidTransaction(
            "V3 path must contain at least two tokens".into(),
        ));
    }
    if amount_in.is_zero() {
        return Ok(DexQuote {
            amount_out: U256::ZERO,
        });
    }

    let cfg = wiz4rd_config(chain_id, rpc_url)?;
    let provider = connect_http(rpc_url)?;

    let mut amount = amount_in;
    for window in path.windows(2) {
        let token_in = window[0];
        let token_out = window[1];
        if token_in == token_out {
            return Err(WalletError::InvalidTransaction(
                "adjacent path tokens must differ".into(),
            ));
        }

        let key = get_pool_key(token_in, token_out, fee);
        let pool = get_pool_info(&provider, &cfg, key)
            .await
            .map_err(|e| WalletError::NetworkError(format!("pool read: {e}")))?;
        if pool.pool.is_zero() {
            return Err(WalletError::Other(format!(
                "no V3 pool for {token_in:#x} → {token_out:#x} at fee {fee}"
            )));
        }

        amount = quote_v3_hop_exact_in(&pool, token_in, amount)?;
    }

    Ok(DexQuote { amount_out: amount })
}

fn quote_v3_hop_exact_in(
    pool: &PoolInfo,
    token_in: Address,
    amount_in: U256,
) -> Result<U256, WalletError> {
    if amount_in.is_zero() {
        return Ok(U256::ZERO);
    }
    let zfo = zero_for_one(pool, token_in);
    if token_in != pool.token0 && token_in != pool.token1 {
        return Err(WalletError::InvalidTransaction(
            "token in is not in this pool".into(),
        ));
    }
    let Quote { amount_out, .. } = quote_exact_in(pool, amount_in, zfo)
        .map_err(|e| WalletError::Other(format!("V3 quote math: {e}")))?;
    Ok(amount_out)
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
    use alloy::primitives::Address;
    use wiz4rd_sdk::pool_address::PoolKey;

    fn pool_with(
        token0: Address,
        token1: Address,
        liquidity: u128,
        tick: i32,
        fee: u32,
    ) -> PoolInfo {
        use alloy::primitives::aliases::I24;
        use alloy::primitives::aliases::U160;
        let sqrt: U160 =
            wiz4rd_math::get_sqrt_ratio_at_tick(I24::try_from(tick).unwrap()).unwrap();
        let limbs = sqrt.into_limbs();
        let sqrt_price_x96 = U256::from_limbs([limbs[0], limbs[1], limbs[2], 0]);
        PoolInfo {
            pool_key: PoolKey {
                token0,
                token1,
                fee,
            },
            pool: Address::repeat_byte(0x99),
            token0,
            token1,
            fee,
            sqrt_price_x96,
            tick,
            fee_protocol: 0,
            liquidity,
        }
    }

    #[test]
    fn min_out_slippage_rounds_down() {
        let out = U256::from(1_000_000u64);
        assert_eq!(
            min_out_after_slippage(out, DEFAULT_DEX_SLIPPAGE_BPS),
            U256::from(995_000u64)
        );
    }

    #[test]
    fn v3_path_chains_hops_locally() {
        let token_a = Address::repeat_byte(0x11);
        let token_b = Address::repeat_byte(0x22);
        let token_c = Address::repeat_byte(0x33);
        let liq = 10u128.pow(22);
        let pool_ab = pool_with(token_a, token_b, liq, 0, 500);
        let pool_bc = pool_with(token_b, token_c, liq, 0, 500);
        let amount_in = U256::from(10u128.pow(17));

        let one_hop = quote_v3_hop_exact_in(&pool_ab, token_a, amount_in).unwrap();
        assert!(one_hop > U256::ZERO);

        let two_hop =
            quote_v3_hop_exact_in(&pool_bc, token_b, one_hop).unwrap();
        assert!(two_hop > U256::ZERO);
        assert!(two_hop < one_hop, "second hop should reduce output vs mid token");
    }

    #[tokio::test]
    async fn v3_path_rejects_short_path() {
        let err = quote_v3_path_exact_in(
            "http://127.0.0.1:1",
            943,
            &[Address::repeat_byte(0x01)],
            U256::from(1u64),
            500,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, WalletError::InvalidTransaction(_)));
    }
}
