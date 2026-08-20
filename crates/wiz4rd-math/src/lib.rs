//! Concentrated-liquidity math for wiz4rd-swap.
//!
//! This crate wraps [`uniswap-v3-sdk`]'s battle-tested V3 math. PancakeSwap
//! forked Uniswap V3's math unchanged, so the modules here are exact matches
//! for the on-chain PancakeSwap V3 contracts (TickMath, SqrtPriceMath,
//! FullMath, LiquidityMath, SwapMath, fee growth / tokens owed).
//!
//! The only PancakeSwap-specific divergence — CREATE2 pool address derivation
//! with `PancakeV3PoolDeployer` + the PancakeSwap `POOL_INIT_CODE_HASH` —
//! lives in [`wiz4rd_sdk`](https://docs.rs/wiz4rd-sdk), not here.

#[cfg(test)]
mod parity;

pub mod swap_math;

/// The raw `uniswap-v3-sdk` utils module, re-exported so consumers can reach
/// deeper helpers (e.g. `sqrt_price_math::get_next_sqrt_price_from_input`)
/// without depending on the crate directly.
pub use uniswap_v3_sdk::utils;

/// Sqrt-price math (next-price + amount deltas) — the swap/quote core.
pub use uniswap_v3_sdk::utils::sqrt_price_math;

pub use uniswap_v3_sdk::utils::{
    full_math::FullMath,
    get_fee_growth_inside::get_fee_growth_inside,
    get_tokens_owed::get_tokens_owed,
    liquidity_math::add_delta,
    max_liquidity_for_amounts::max_liquidity_for_amounts,
    nearest_usable_tick::nearest_usable_tick,
    price_tick_conversions::{price_to_closest_tick, tick_to_price},
    sqrt_price_math::{get_amount_0_delta, get_amount_1_delta, SqrtPriceMath},
    swap_math::SwapState,
    tick_math::{get_sqrt_ratio_at_tick, get_tick_at_sqrt_ratio, MAX_TICK, MIN_TICK},
};

/// Solidity-exact swap step computation. ⚠️ Our own implementation, NOT the
/// upstream crate's: the crate's `compute_swap_step` keeps the fee-less input
/// amount on partial-fill exact-in swaps instead of recomputing it from the
/// reached price, diverging from on-chain behavior by a few wei. See
/// [`swap_math`] for details.
pub use swap_math::compute_swap_step;

/// Fee tiers supported by the PancakeSwap V3 factory (constructor-enabled).
pub mod fee_tiers {
    /// 0.01% — tick spacing 1
    pub const FEE_100: u32 = 100;
    /// 0.05% — tick spacing 10
    pub const FEE_500: u32 = 500;
    /// 0.25% — tick spacing 50 (⚠️ PancakeSwap uses 50, not Uniswap's 60)
    pub const FEE_2500: u32 = 2500;
    /// 1% — tick spacing 200
    pub const FEE_10000: u32 = 10000;

    /// Tick spacing for a fee tier, per the PancakeSwap V3 factory.
    pub const fn tick_spacing(fee: u32) -> Option<i32> {
        match fee {
            FEE_100 => Some(1),
            FEE_500 => Some(10),
            FEE_2500 => Some(50),
            FEE_10000 => Some(200),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{aliases::I24, aliases::U160, U256};

    /// Smoke tests proving the vendored math works with our types (tick 0
    /// must map to sqrtPriceX96 = 2^96, the canonical V3 anchor).
    #[test]
    fn tick_zero_maps_to_sqrt_price_2_pow_96() {
        let sqrt: U160 = get_sqrt_ratio_at_tick(I24::from_limbs([0])).unwrap();
        assert_eq!(sqrt, U160::from(2u128.pow(96)));
    }

    #[test]
    fn sqrt_price_round_trips_to_tick() {
        let sqrt: U160 = get_sqrt_ratio_at_tick(I24::from_limbs([0])).unwrap();
        let tick = get_tick_at_sqrt_ratio(sqrt).unwrap();
        assert_eq!(tick, I24::from_limbs([0]));
    }

    #[test]
    fn full_math_mul_div_matches_known_value() {
        let r = FullMath::mul_div(
            U256::from(2u128.pow(100)),
            U256::from(3u128.pow(50)),
            U256::from(5u128.pow(40)),
        )
        .unwrap();
        assert_eq!(
            r,
            U256::from_str_radix("100060375637836737551707627", 10).unwrap()
        );
    }

    #[test]
    fn fee_tier_spacings_match_pancakeswap_factory() {
        assert_eq!(fee_tiers::tick_spacing(100), Some(1));
        assert_eq!(fee_tiers::tick_spacing(500), Some(10));
        assert_eq!(fee_tiers::tick_spacing(2500), Some(50)); // PancakeSwap, not 60
        assert_eq!(fee_tiers::tick_spacing(10000), Some(200));
        assert_eq!(fee_tiers::tick_spacing(3000), None);
    }
}
