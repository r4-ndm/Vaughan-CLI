//! Swap transaction builders.
//!
//! Builds `TransactionRequest`s against the SwapRouter — pure construction,
//! no signing/sending (that is the CLI/Vaughan layer's job).

use alloy::primitives::{aliases::U160, aliases::U24, Address, U256};
use alloy::rpc::types::TransactionRequest;
use alloy::sol_types::SolCall;

use crate::abi::ISwapRouter;
use crate::config::Config;
use crate::error::{SdkError, SdkResult};
use crate::pool::PoolInfo;

/// Bound constants matching the pool's `MIN_SQRT_RATIO` / `MAX_SQRT_RATIO`
/// (TickMath.sol) — used as the default price limit for a swap direction.
///
/// These must satisfy the pool's `SPL` guard in `swap()`:
/// `zeroForOne ? limit < slot0.sqrtPriceX96 && limit > MIN_SQRT_RATIO
///             : limit > slot0.sqrtPriceX96 && limit < MAX_SQRT_RATIO`.
/// The Uniswap SDK convention is `MIN + 1` / `MAX - 1` — note that `2^160 - 1`
/// is **not** `< MAX_SQRT_RATIO` and reverts every one-for-zero swap (caught
/// by the anvil fork E2E).
const MIN_SQRT_RATIO_PLUS_ONE: U160 = U160::from_limbs([4_295_128_740, 0, 0]);
const MAX_SQRT_RATIO_MINUS_ONE: U160 = U160::from_limbs([
    6_743_328_256_752_651_557,
    17_280_870_778_742_802_505,
    4_294_805_859,
]);

/// Slippage expressed in basis points (1 bp = 0.01%).
pub type BasisPoints = u32;

/// Apply a slippage tolerance to a quote, rounding **down**: the minimum the
/// user accepts. Correct for exact-in `amountOutMinimum`.
pub fn apply_slippage(amount: U256, slippage_bps: BasisPoints) -> U256 {
    amount * U256::from(10_000 - slippage_bps.min(10_000)) / U256::from(10_000)
}

/// Apply a slippage tolerance to a quote, rounding **up**: the maximum the
/// user is willing to spend. Correct for exact-out `amountInMaximum` — the
/// inverse of [`apply_slippage`], which would otherwise *lower* the bound and
/// make the swap revert.
pub fn apply_slippage_up(amount: U256, slippage_bps: BasisPoints) -> U256 {
    amount * U256::from(10_000 + slippage_bps.min(10_000)) / U256::from(10_000)
}

/// Whether a swap spends token0 first (`zero_for_one`). The pool's token
/// ordering comes from `get_pool_key`; `token_in` must be one of the two.
pub fn zero_for_one(pool: &PoolInfo, token_in: Address) -> bool {
    token_in == pool.token0
}

/// Build an exact-input single-hop swap: spends `amount_in` of `token_in`,
/// accepts no less than `amount_out_minimum` of `token_out`.
#[allow(clippy::too_many_arguments)]
pub fn build_swap_exact_in(
    config: &Config,
    pool: &PoolInfo,
    token_in: Address,
    amount_in: U256,
    amount_out_minimum: U256,
    recipient: Address,
    deadline: u64,
    sqrt_price_limit: Option<U160>,
) -> SdkResult<TransactionRequest> {
    let router = config
        .swap_router
        .ok_or(SdkError::MissingAddress("swap_router"))?;
    let token_out = if token_in == pool.token0 {
        pool.token1
    } else {
        pool.token0
    };

    let params = ISwapRouter::ExactInputSingleParams {
        tokenIn: token_in,
        tokenOut: token_out,
        fee: U24::try_from(pool.fee).map_err(|e| SdkError::Math(e.to_string()))?,
        recipient,
        deadline: U256::from(deadline),
        amountIn: amount_in,
        amountOutMinimum: amount_out_minimum,
        sqrtPriceLimitX96: sqrt_price_limit.unwrap_or_else(|| {
            if zero_for_one(pool, token_in) {
                MIN_SQRT_RATIO_PLUS_ONE
            } else {
                MAX_SQRT_RATIO_MINUS_ONE
            }
        }),
    };
    let call = ISwapRouter::exactInputSingleCall { params };
    Ok(TransactionRequest::default()
        .to(router)
        .input(call.abi_encode().into()))
}

/// Build an exact-output single-hop swap: receives exactly `amount_out` of
/// `token_out`, spends no more than `amount_in_maximum` of `token_in`.
#[allow(clippy::too_many_arguments)]
pub fn build_swap_exact_out(
    config: &Config,
    pool: &PoolInfo,
    token_in: Address,
    amount_out: U256,
    amount_in_maximum: U256,
    recipient: Address,
    deadline: u64,
    sqrt_price_limit: Option<U160>,
) -> SdkResult<TransactionRequest> {
    let router = config
        .swap_router
        .ok_or(SdkError::MissingAddress("swap_router"))?;
    let token_out = if token_in == pool.token0 {
        pool.token1
    } else {
        pool.token0
    };

    let params = ISwapRouter::ExactOutputSingleParams {
        tokenIn: token_in,
        tokenOut: token_out,
        fee: U24::try_from(pool.fee).map_err(|e| SdkError::Math(e.to_string()))?,
        recipient,
        deadline: U256::from(deadline),
        amountOut: amount_out,
        amountInMaximum: amount_in_maximum,
        sqrtPriceLimitX96: sqrt_price_limit.unwrap_or_else(|| {
            if zero_for_one(pool, token_in) {
                MIN_SQRT_RATIO_PLUS_ONE
            } else {
                MAX_SQRT_RATIO_MINUS_ONE
            }
        }),
    };
    let call = ISwapRouter::exactOutputSingleCall { params };
    Ok(TransactionRequest::default()
        .to(router)
        .input(call.abi_encode().into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool_address::get_pool_key;

    fn pool(token0: Address, token1: Address) -> PoolInfo {
        PoolInfo {
            pool_key: get_pool_key(token0, token1, 500),
            pool: Address::repeat_byte(0x33),
            token0: token0.min(token1),
            token1: token0.max(token1),
            fee: 500,
            sqrt_price_x96: U256::from(2u128.pow(96)),
            tick: 0,
            fee_protocol: 0,
            liquidity: 10u128.pow(20),
        }
    }

    #[test]
    fn slippage_rounds_down() {
        assert_eq!(
            apply_slippage(U256::from(1_000_000u64), 500),
            U256::from(950_000u64)
        );
        assert_eq!(
            apply_slippage(U256::from(1_000_000u64), 0),
            U256::from(1_000_000u64)
        );
    }

    #[test]
    fn slippage_up_rounds_up_for_exact_out_maximums() {
        // Exact-out must *raise* the max input, never lower it.
        assert_eq!(
            apply_slippage_up(U256::from(1_000_000u64), 500),
            U256::from(1_050_000u64)
        );
        assert_eq!(
            apply_slippage_up(U256::from(1_000_000u64), 0),
            U256::from(1_000_000u64)
        );
        // For a non-zero slippage, the exact-out max is strictly above the
        // exact-in min for the same quote.
        let q = U256::from(1_000_000u64);
        assert!(apply_slippage_up(q, 500) > apply_slippage(q, 500));
    }

    #[test]
    fn default_price_limits_pass_the_pool_spl_guard() {
        // The pool's SPL guard in swap() is:
        //   zeroForOne ? limit < slot0.sqrtPriceX96 && limit > MIN_SQRT_RATIO
        //              : limit > slot0.sqrtPriceX96 && limit < MAX_SQRT_RATIO
        // The defaults must satisfy the strict bound for their direction, and
        // the direction check (`<`/`>` the current price) passes by
        // construction since MIN+1 is the lowest valid ratio and MAX-1 the
        // highest. Regression: `2^160 - 1` was used before, which is not
        // `< MAX_SQRT_RATIO` and reverted every one-for-zero swap.
        use wiz4rd_math::utils::tick_math::{MAX_SQRT_RATIO, MIN_SQRT_RATIO};
        assert!(
            MIN_SQRT_RATIO_PLUS_ONE > MIN_SQRT_RATIO,
            "zero-for-one default above MIN"
        );
        assert!(
            MAX_SQRT_RATIO_MINUS_ONE < MAX_SQRT_RATIO,
            "one-for-zero default below MAX"
        );
        assert!(MIN_SQRT_RATIO_PLUS_ONE < MAX_SQRT_RATIO_MINUS_ONE);
    }

    #[test]
    fn exact_in_builds_router_call() {
        let a = Address::repeat_byte(0x11);
        let b = Address::repeat_byte(0x22);
        let cfg = Config {
            swap_router: Some(Address::repeat_byte(0xaa)),
            ..Config::default()
        };
        let tx = build_swap_exact_in(
            &cfg,
            &pool(a, b),
            a,
            U256::from(10u128.pow(18)),
            U256::from(9u128.pow(18)),
            Address::repeat_byte(0xee),
            1_700_000_000,
            None,
        )
        .unwrap();
        assert_eq!(
            tx.to,
            Some(alloy::primitives::TxKind::Call(Address::repeat_byte(0xaa)))
        );
        // Calldata starts with the exactInputSingle selector.
        let data = tx.input.into_input().unwrap();
        assert_eq!(&data[..4], &ISwapRouter::exactInputSingleCall::SELECTOR);
    }

    #[test]
    fn exact_out_builds_router_call() {
        let a = Address::repeat_byte(0x11);
        let b = Address::repeat_byte(0x22);
        let cfg = Config {
            swap_router: Some(Address::repeat_byte(0xaa)),
            ..Config::default()
        };
        let tx = build_swap_exact_out(
            &cfg,
            &pool(a, b),
            a,
            U256::from(10u128.pow(18)),
            U256::from(11u128.pow(18)),
            Address::repeat_byte(0xee),
            1_700_000_000,
            None,
        )
        .unwrap();
        let data = tx.input.into_input().unwrap();
        assert_eq!(&data[..4], &ISwapRouter::exactOutputSingleCall::SELECTOR);
    }
}
