//! Offline quote builder.
//!
//! Estimates swap amounts from live pool state using the on-chain swap math
//! (`get_next_sqrt_price_from_input/output` + amount deltas), **without** a
//! transaction. This is the same math `QuoterV2` performs on-chain, evaluated
//! locally against `slot0` + `liquidity`.
//!
//! ⚠️ Approximation: quotes use the pool's *current* liquidity. Crossing an
//! initialized tick changes the usable liquidity mid-swap, so large swaps can
//! diverge slightly from the on-chain result. The CLI will re-quote at
//! execution time (Phase 4/5) and enforce slippage bounds.

use alloy::primitives::{aliases::U160, U256};

use crate::error::{SdkError, SdkResult};
use crate::pool::PoolInfo;

/// Result of an offline quote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quote {
    /// Amount of the output token received (exact-in) / requested (exact-out).
    pub amount_out: U256,
    /// Amount of the input token spent (exact-in) / required (exact-out).
    pub amount_in: U256,
}

/// Convert fee (hundredths of a bip, e.g. 500) to the fee fraction applied to
/// the input: `fee / 1_000_000`.
fn fee_fraction(fee: u32) -> U256 {
    U256::from(fee)
}

/// Truncate a U256 sqrt price to U160 (Q64.96 values always fit in 160 bits;
/// the truncation is lossless by construction).
fn u160(v: U256) -> U160 {
    U160::from_limbs([v.into_limbs()[0], v.into_limbs()[1], v.into_limbs()[2]])
}

/// Quote an exact-input swap: how much output for `amount_in` of the input
/// token, after the pool fee. `zero_for_one` = input is token0.
pub fn quote_exact_in(pool: &PoolInfo, amount_in: U256, zero_for_one: bool) -> SdkResult<Quote> {
    if pool.liquidity == 0 {
        return Err(SdkError::Math("pool has no liquidity".into()));
    }
    if amount_in.is_zero() {
        return Ok(Quote {
            amount_in: U256::ZERO,
            amount_out: U256::ZERO,
        });
    }

    let sqrt_price_x96 = u160(pool.sqrt_price_x96);

    let amount_in_after_fee =
        amount_in * (U256::from(1_000_000u64) - fee_fraction(pool.fee)) / U256::from(1_000_000u64);

    let next = wiz4rd_math::sqrt_price_math::get_next_sqrt_price_from_input(
        sqrt_price_x96,
        pool.liquidity,
        amount_in_after_fee,
        zero_for_one,
    )
    .map_err(|e| SdkError::Math(e.to_string()))?;

    let amount_out = if zero_for_one {
        wiz4rd_math::sqrt_price_math::get_amount_1_delta(
            sqrt_price_x96,
            next,
            pool.liquidity,
            false,
        )
    } else {
        wiz4rd_math::sqrt_price_math::get_amount_0_delta(
            sqrt_price_x96,
            next,
            pool.liquidity,
            false,
        )
    }
    .map_err(|e| SdkError::Math(e.to_string()))?;

    Ok(Quote {
        amount_in,
        amount_out,
    })
}

/// Price impact as a percent: how far the execution price (quote) is from the
/// pool's mid price.
///
/// Display/warning only — settlement uses the slippage bounds. Returns `0.0`
/// when the pool price is unavailable or the quote amounts are degenerate
/// (zero input/output).
pub fn price_impact_pct(
    pool: &PoolInfo,
    amount_in: U256,
    amount_out: U256,
    zero_for_one: bool,
) -> f64 {
    let mid = pool.price_f64();
    if mid <= 0.0 || mid.is_nan() {
        return 0.0;
    }
    let f = |v: U256| -> f64 { v.to_string().parse().unwrap_or(f64::NAN) };
    let exec = if zero_for_one {
        f(amount_out) / f(amount_in)
    } else {
        f(amount_in) / f(amount_out)
    };
    if !exec.is_finite() {
        return 0.0;
    }
    ((exec - mid).abs() / mid) * 100.0
}

/// Quote an exact-output swap: how much input is required to receive
/// `amount_out` of the output token, after the pool fee.
pub fn quote_exact_out(pool: &PoolInfo, amount_out: U256, zero_for_one: bool) -> SdkResult<Quote> {
    if pool.liquidity == 0 {
        return Err(SdkError::Math("pool has no liquidity".into()));
    }
    if amount_out.is_zero() {
        return Ok(Quote {
            amount_in: U256::ZERO,
            amount_out: U256::ZERO,
        });
    }

    let sqrt_price_x96 = u160(pool.sqrt_price_x96);

    // Gross output (fee is taken on the input side of an exact-out swap).
    let amount_out_gross =
        amount_out * U256::from(1_000_000u64) / (U256::from(1_000_000u64) - fee_fraction(pool.fee));

    let next = wiz4rd_math::sqrt_price_math::get_next_sqrt_price_from_output(
        sqrt_price_x96,
        pool.liquidity,
        amount_out_gross,
        zero_for_one,
    )
    .map_err(|e| SdkError::Math(e.to_string()))?;

    let amount_in = if zero_for_one {
        wiz4rd_math::sqrt_price_math::get_amount_0_delta(sqrt_price_x96, next, pool.liquidity, true)
    } else {
        wiz4rd_math::sqrt_price_math::get_amount_1_delta(sqrt_price_x96, next, pool.liquidity, true)
    }
    .map_err(|e| SdkError::Math(e.to_string()))?;

    Ok(Quote {
        amount_in,
        amount_out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool_with(liquidity: u128, tick: i32, fee: u32) -> PoolInfo {
        let sqrt: alloy::primitives::aliases::U160 = wiz4rd_math::get_sqrt_ratio_at_tick(
            alloy::primitives::aliases::I24::try_from(tick).unwrap(),
        )
        .unwrap();
        let limbs = sqrt.into_limbs();
        let sqrt_price_x96 = U256::from_limbs([limbs[0], limbs[1], limbs[2], 0]);
        PoolInfo {
            pool_key: crate::pool_address::PoolKey {
                token0: alloy::primitives::Address::repeat_byte(0x11),
                token1: alloy::primitives::Address::repeat_byte(0x22),
                fee,
            },
            pool: alloy::primitives::Address::repeat_byte(0x33),
            token0: alloy::primitives::Address::repeat_byte(0x11),
            token1: alloy::primitives::Address::repeat_byte(0x22),
            fee,
            sqrt_price_x96,
            tick,
            fee_protocol: 0,
            liquidity,
        }
    }

    #[test]
    fn exact_in_zero_input_is_zero() {
        let pool = pool_with(1_000_000_000, 0, 500);
        let q = quote_exact_in(&pool, U256::ZERO, true).unwrap();
        assert_eq!(q.amount_out, U256::ZERO);
        assert_eq!(q.amount_in, U256::ZERO);
    }

    #[test]
    fn exact_in_pays_fee_on_input() {
        let pool = pool_with(10u128.pow(20), 0, 10_000);
        let amount_in = U256::from(10u128.pow(18)); // 1 token, 18 decimals
        let q = quote_exact_in(&pool, amount_in, true).unwrap();
        // Fee 1% -> at most 0.99 token equivalent of price impact; output must
        // be less than the fee-less amount and greater than 0.
        assert!(q.amount_out > U256::ZERO);
        assert!(q.amount_out < amount_in);
    }

    #[test]
    fn exact_out_requires_more_than_gross() {
        let pool = pool_with(10u128.pow(20), 0, 500);
        let out = U256::from(10u128.pow(18));
        let q = quote_exact_out(&pool, out, true).unwrap();
        assert!(q.amount_in > out, "fee makes input exceed output");
    }

    #[test]
    fn exact_in_out_roundtrip_is_close() {
        let pool = pool_with(10u128.pow(22), 0, 500);
        let amount_in = U256::from(10u128.pow(17)); // small relative to liquidity
        let q_in = quote_exact_in(&pool, amount_in, true).unwrap();
        let q_out = quote_exact_out(&pool, q_in.amount_out, true).unwrap();
        // Rounding + fee asymmetry: input required should be within 1% of the
        // original input.
        let diff = if q_out.amount_in > amount_in {
            q_out.amount_in - amount_in
        } else {
            amount_in - q_out.amount_in
        };
        assert!(
            diff <= amount_in / U256::from(100),
            "roundtrip drift too large: {diff}"
        );
    }

    #[test]
    fn no_liquidity_errors() {
        let pool = pool_with(0, 0, 500);
        assert!(quote_exact_in(&pool, U256::from(1u64), true).is_err());
        assert!(quote_exact_out(&pool, U256::from(1u64), true).is_err());
    }
}
