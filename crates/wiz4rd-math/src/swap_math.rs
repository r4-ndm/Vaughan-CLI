//! SwapMath — Solidity-exact implementation.
//!
//! ⚠️ **Why this exists:** the upstream `uniswap-v3-sdk` crate's
//! `compute_swap_step` diverges from the canonical Solidity in one case. In a
//! *partial-fill exact-input* swap, `SwapMath.sol` recomputes `amountIn` from
//! the actual `sqrtRatioNextX96` reached (rounding the delta up), whereas the
//! crate keeps the fee-less input amount (`amountRemainingLessFee`). The
//! difference is dust (a few wei) but it means quotes built with the crate
//! would not match what the on-chain pool actually charges — a hard failure
//! for slippage checks at tight tolerances. This implementation mirrors
//! `SwapMath.sol` from the pinned fork (commit `9868479`) line-for-line.
//!
//! Everything else (FullMath, SqrtPriceMath, liquidity math) is reused from
//! the crate — only the composition here is our own.

use alloy::primitives::{I256, U256, Uint};
use uniswap_v3_sdk::utils::{
    full_math::{mul_div, mul_div_rounding_up},
    sqrt_price_math::{
        get_amount_0_delta, get_amount_1_delta, get_next_sqrt_price_from_input,
        get_next_sqrt_price_from_output,
    },
};

/// Equivalent of Solidity's `1e6` (fee pips denominator).
const MAX_FEE: u64 = 1_000_000;

/// Compute the next sqrt price and the in/out amounts for one swap step.
///
/// Mirrors `SwapMath.computeSwapStep` in the PancakeSwap V3 core fork.
/// Returns `(sqrt_ratio_next_x96, amount_in, amount_out, fee_amount)`.
pub fn compute_swap_step<const BITS: usize, const LIMBS: usize>(
    sqrt_ratio_current_x96: Uint<BITS, LIMBS>,
    sqrt_ratio_target_x96: Uint<BITS, LIMBS>,
    liquidity: u128,
    amount_remaining: I256,
    fee_pips: u32,
) -> Result<(Uint<BITS, LIMBS>, U256, U256, U256), uniswap_v3_sdk::error::Error> {
    let zero_for_one = sqrt_ratio_current_x96 >= sqrt_ratio_target_x96;
    let exact_in = amount_remaining >= I256::ZERO;
    let fee_pips = U256::from(fee_pips);
    let fee_complement = U256::from(MAX_FEE) - fee_pips;

    let sqrt_ratio_next_x96: Uint<BITS, LIMBS>;
    // Initialized by both branches below; declared here so the final in/out
    // recomputation can reference them unconditionally.
    let mut amount_in: U256 = U256::ZERO;
    let mut amount_out: U256 = U256::ZERO;

    if exact_in {
        let amount_remaining_abs = amount_remaining.into_raw();
        let amount_remaining_less_fee = mul_div(amount_remaining_abs, fee_complement, U256::from(MAX_FEE))?;

        amount_in = if zero_for_one {
            get_amount_0_delta(sqrt_ratio_target_x96, sqrt_ratio_current_x96, liquidity, true)?
        } else {
            get_amount_1_delta(sqrt_ratio_current_x96, sqrt_ratio_target_x96, liquidity, true)?
        };

        if amount_remaining_less_fee >= amount_in {
            sqrt_ratio_next_x96 = sqrt_ratio_target_x96;
        } else {
            sqrt_ratio_next_x96 = get_next_sqrt_price_from_input(
                sqrt_ratio_current_x96,
                liquidity,
                amount_remaining_less_fee,
                zero_for_one,
            )?;
        }
    } else {
        let amount_remaining_abs = (-amount_remaining).into_raw();
        amount_out = if zero_for_one {
            get_amount_1_delta(sqrt_ratio_target_x96, sqrt_ratio_current_x96, liquidity, false)?
        } else {
            get_amount_0_delta(sqrt_ratio_current_x96, sqrt_ratio_target_x96, liquidity, false)?
        };

        if amount_remaining_abs >= amount_out {
            sqrt_ratio_next_x96 = sqrt_ratio_target_x96;
        } else {
            sqrt_ratio_next_x96 = get_next_sqrt_price_from_output(
                sqrt_ratio_current_x96,
                liquidity,
                amount_remaining_abs,
                zero_for_one,
            )?;
        }
    }

    // Recompute in/out amounts from the actual next price (this is the step
    // the upstream crate skips for partial-fill exact-in swaps).
    let max = sqrt_ratio_target_x96 == sqrt_ratio_next_x96;
    if zero_for_one {
        amount_in = if max && exact_in {
            amount_in
        } else {
            get_amount_0_delta(sqrt_ratio_next_x96, sqrt_ratio_current_x96, liquidity, true)?
        };
        amount_out = if max && !exact_in {
            amount_out
        } else {
            get_amount_1_delta(sqrt_ratio_next_x96, sqrt_ratio_current_x96, liquidity, false)?
        };
    } else {
        amount_in = if max && exact_in {
            amount_in
        } else {
            get_amount_1_delta(sqrt_ratio_current_x96, sqrt_ratio_next_x96, liquidity, true)?
        };
        amount_out = if max && !exact_in {
            amount_out
        } else {
            get_amount_0_delta(sqrt_ratio_current_x96, sqrt_ratio_next_x96, liquidity, false)?
        };
    }

    // Cap the output amount to not exceed the remaining output amount.
    if !exact_in && amount_out > (-amount_remaining).into_raw() {
        amount_out = (-amount_remaining).into_raw();
    }

    let fee_amount = if exact_in && sqrt_ratio_next_x96 != sqrt_ratio_target_x96 {
        amount_remaining.into_raw() - amount_in
    } else {
        mul_div_rounding_up(amount_in, fee_pips, fee_complement)?
    };

    Ok((sqrt_ratio_next_x96, amount_in, amount_out, fee_amount))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::get_sqrt_ratio_at_tick;
    use alloy::primitives::{aliases::I24, aliases::U160};

    /// The case that exposed the upstream crate's bug: a partial-fill
    /// exact-in swap at fee 10000. Values from the TS SDK vector corpus —
    /// `amountIn` must be recomputed from the reached price, not kept as
    /// `amountRemainingLessFee`.
    #[test]
    fn partial_fill_exact_in_recomputes_amount_in() {
        let current: U160 = get_sqrt_ratio_at_tick(I24::try_from(1000).unwrap()).unwrap();
        let target: U160 = get_sqrt_ratio_at_tick(I24::try_from(2000).unwrap()).unwrap();
        let (_, amount_in, _, _) = compute_swap_step(
            current,
            target,
            2u128.pow(100),
            I256::try_from(2i64.pow(60)).unwrap(),
            10_000,
        )
        .unwrap();
        // From vectors.json (swapStep[2]): amountIn = 1141392289560778496.
        assert_eq!(amount_in, U256::from(1_141_392_289_560_778_496u64));
    }
}
