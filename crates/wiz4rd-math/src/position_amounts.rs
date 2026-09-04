//! Principal token amounts for an existing V3 position (liquidity → amount0/1).
//!
//! Matches `@pancakeswap/v3-sdk` / Uni `PositionMath.getToken0Amount` /
//! `getToken1Amount` (round-down deltas). See parity vectors in `parity.rs`.

use alloy::primitives::{aliases::I24, U160, U256};

use crate::{
    get_amount_0_delta, get_amount_1_delta, get_sqrt_ratio_at_tick, MAX_V3_TICK, MIN_V3_TICK,
};

/// Token amounts locked in a position at the current pool price.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct V3PositionAmounts {
    pub amount0: U256,
    pub amount1: U256,
}

/// Principal amounts for `liquidity` in `[tick_lower, tick_upper)` at live pool state.
///
/// Uses round-down amount deltas (`round_up = false`) like PositionMath.
pub fn v3_amounts_from_liquidity(
    sqrt_price_x96: U160,
    tick_current: i32,
    tick_lower: i32,
    tick_upper: i32,
    liquidity: u128,
) -> Result<V3PositionAmounts, String> {
    if tick_lower >= tick_upper {
        return Err("tick_lower must be < tick_upper".into());
    }
    if tick_lower < MIN_V3_TICK || tick_upper > MAX_V3_TICK {
        return Err(format!(
            "tick range must be within [{MIN_V3_TICK}, {MAX_V3_TICK}] (got {tick_lower}..{tick_upper})"
        ));
    }
    if liquidity == 0 {
        return Ok(V3PositionAmounts {
            amount0: U256::ZERO,
            amount1: U256::ZERO,
        });
    }

    let sqrt_a = get_sqrt_ratio_at_tick(I24::try_from(tick_lower).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let sqrt_b = get_sqrt_ratio_at_tick(I24::try_from(tick_upper).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;

    let amount0 = if tick_current < tick_lower {
        get_amount_0_delta(sqrt_a, sqrt_b, liquidity, false).map_err(|e| e.to_string())?
    } else if tick_current < tick_upper {
        get_amount_0_delta(sqrt_price_x96, sqrt_b, liquidity, false).map_err(|e| e.to_string())?
    } else {
        U256::ZERO
    };
    let amount1 = if tick_current < tick_lower {
        U256::ZERO
    } else if tick_current < tick_upper {
        get_amount_1_delta(sqrt_a, sqrt_price_x96, liquidity, false).map_err(|e| e.to_string())?
    } else {
        get_amount_1_delta(sqrt_a, sqrt_b, liquidity, false).map_err(|e| e.to_string())?
    };

    Ok(V3PositionAmounts { amount0, amount1 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::get_sqrt_ratio_at_tick;

    fn u160_at_tick(tick: i32) -> U160 {
        get_sqrt_ratio_at_tick(I24::try_from(tick).unwrap()).unwrap()
    }

    #[test]
    fn zero_liquidity_is_zero_amounts() {
        let sqrt = u160_at_tick(0);
        let a = v3_amounts_from_liquidity(sqrt, 0, -100, 100, 0).unwrap();
        assert!(a.amount0.is_zero() && a.amount1.is_zero());
    }

    #[test]
    fn below_range_is_token0_only() {
        // current below lower → all token0
        let sqrt = u160_at_tick(-200);
        let a = v3_amounts_from_liquidity(sqrt, -200, -100, 100, 1_000_000).unwrap();
        assert!(!a.amount0.is_zero());
        assert!(a.amount1.is_zero());
    }

    #[test]
    fn above_range_is_token1_only() {
        let sqrt = u160_at_tick(200);
        let a = v3_amounts_from_liquidity(sqrt, 200, -100, 100, 1_000_000).unwrap();
        assert!(a.amount0.is_zero());
        assert!(!a.amount1.is_zero());
    }

    #[test]
    fn in_range_has_both() {
        let sqrt = u160_at_tick(0);
        let a = v3_amounts_from_liquidity(sqrt, 0, -1000, 1000, 1_000_000).unwrap();
        assert!(!a.amount0.is_zero());
        assert!(!a.amount1.is_zero());
    }
}
