//! Pool info reader.
//!
//! Resolves the pool address the same way the contracts do — `getPool` on the
//! factory — then reads live pool state. Pool *address derivation* (CREATE2)
//! lives in [`crate::pool_address`]; the on-chain `getPool` is authoritative
//! and also catches init-code-hash drift.

use alloy::primitives::{aliases::U24, Address, U256};
use alloy::providers::Provider;
use alloy::rpc::types::TransactionRequest;
use alloy::sol_types::{SolCall, SolValue};

use crate::abi::{IPancakeV3Factory, IPancakeV3Pool};
use crate::config::Config;
use crate::error::{SdkError, SdkResult};
use crate::pool_address::PoolKey;

/// Live state of a PancakeSwap V3 pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolInfo {
    /// Ordered tokens + fee tier identifying the pool.
    pub pool_key: PoolKey,
    /// The deployed pool address (from `factory.getPool`).
    pub pool: Address,
    /// `token0()` — the numerically smaller address (per PoolKey ordering).
    pub token0: Address,
    /// `token1()` — the numerically larger address.
    pub token1: Address,
    /// Fee tier in hundredths of a bip (e.g. 500 = 0.05%).
    pub fee: u32,
    /// `slot0.sqrtPriceX96` — Q64.96 sqrt of the token1/token0 price.
    pub sqrt_price_x96: U256,
    /// `slot0.tick` — current price tick.
    pub tick: i32,
    /// `slot0.feeProtocol` — protocol fee in hundredths of a bip.
    pub fee_protocol: u32,
    /// Current in-range liquidity.
    pub liquidity: u128,
}

impl PoolInfo {
    /// Approximate token1-per-token0 price as a float, for display only.
    /// Exact math should use the Q64.96 values directly.
    pub fn price_f64(&self) -> f64 {
        let sqrt = self.sqrt_price_x96.to::<u128>();
        let p = (sqrt as f64) / 2f64.powi(96);
        p * p
    }
}

async fn call_view<P: Provider>(provider: &P, to: Address, data: Vec<u8>) -> SdkResult<Vec<u8>> {
    let tx = TransactionRequest::default().to(to).input(data.into());
    let raw = provider.call(tx).await?;
    Ok(raw.to_vec())
}

/// Fetch live state for the pool identified by `key`.
///
/// Requires `config.factory`. Returns an RPC error when no pool exists for the
/// pair/fee yet (`getPool` returns `address(0)` and the subsequent slot0 call
/// reverts) — callers should map that to "pool does not exist".
pub async fn get_pool_info<P: Provider>(
    provider: &P,
    config: &Config,
    key: PoolKey,
) -> SdkResult<PoolInfo> {
    let factory = config.factory.ok_or(SdkError::MissingAddress("factory"))?;

    // pool = factory.getPool(token0, token1, fee)
    let get_pool = IPancakeV3Factory::getPoolCall {
        tokenA: key.token0,
        tokenB: key.token1,
        fee: U24::try_from(key.fee).map_err(|e| SdkError::Math(e.to_string()))?,
    };
    let raw = call_view(provider, factory, get_pool.abi_encode()).await?;
    let pool = Address::abi_decode(&raw).map_err(SdkError::Decode)?;

    // slot0
    let raw = call_view(provider, pool, IPancakeV3Pool::slot0Call {}.abi_encode()).await?;
    let s = IPancakeV3Pool::slot0Call::abi_decode_returns(&raw)
        .map_err(SdkError::Decode)?;

    // liquidity
    let raw = call_view(provider, pool, IPancakeV3Pool::liquidityCall {}.abi_encode()).await?;
    let l = IPancakeV3Pool::liquidityCall::abi_decode_returns(&raw)
        .map_err(SdkError::Decode)?;

    let _ = &s; // silence unused if slot0 decode changes
    let _ = &l;

    Ok(PoolInfo {
        pool_key: key,
        pool,
        token0: key.token0,
        token1: key.token1,
        fee: key.fee,
        sqrt_price_x96: U256::from(s.sqrtPriceX96),
        tick: i32::try_from(s.tick).map_err(|e| SdkError::Math(e.to_string()))?,
        fee_protocol: s.feeProtocol,
        liquidity: l,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live check against PancakeSwap V3 on BSC mainnet (the fork's reference
    /// chain): reads the WBNB/USDT fee-500 pool's real state. Requires network
    /// access; run with `cargo test -p wiz4rd-sdk -- --ignored`.
    ///
    /// Reference: the BSC factory is `0x0BFbCF9fa4f9C56B0F40a671Ad40E0805A0918CB`
    /// (verified live in Phase 1 selector tests).
    #[tokio::test]
    #[ignore = "live network"]
    async fn live_bsc_pool_info() {
        let cfg = Config {
            chain_id: 56,
            rpc_url: Some("https://bsc-dataseed.binance.org".into()),
            factory: Some("0x0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865".parse().unwrap()),
            ..Config::default()
        };
        let provider = cfg.provider().unwrap();
        let wbnb = "0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c".parse().unwrap();
        let usdt = "0x55d398326f99059fF775485246999027B3197955".parse().unwrap();

        let info = get_pool_info_for_tokens(&provider, &cfg, wbnb, usdt, 500).await.unwrap();
        let expected_pool: Address = "0x36696169C63e42cd08ce11f5deeBbCeBae652050".parse().unwrap();
        assert_eq!(info.pool, expected_pool, "pool must match CREATE2 derivation test");
        assert_eq!(info.token0, usdt, "USDT sorts before WBNB numerically");
        assert_eq!(info.token1, wbnb);
        assert_eq!(info.fee, 500);
        assert!(info.liquidity > 0, "WBNB/USDT pool has liquidity");
        assert!(info.sqrt_price_x96 > U256::ZERO);
    }
}

/// Resolve a pool's live state from token addresses + fee tier.
pub async fn get_pool_info_for_tokens<P: Provider>(
    provider: &P,
    config: &Config,
    token_a: Address,
    token_b: Address,
    fee: u32,
) -> SdkResult<PoolInfo> {
    let key = crate::pool_address::get_pool_key(token_a, token_b, fee);
    get_pool_info(provider, config, key).await
}
