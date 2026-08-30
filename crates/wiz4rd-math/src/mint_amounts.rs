//! V3 mint deposit coupling — `@pancakeswap/v3-sdk` `Position::from_amount0` +
//! `mint_amounts()`.

use alloy::primitives::{aliases::I24, U160, U256};

use crate::{
    get_amount_0_delta, get_amount_1_delta, get_sqrt_ratio_at_tick, max_liquidity_for_amounts,
};

/// Token amounts required to mint liquidity at the current pool price.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct V3MintAmounts {
    pub amount0: U256,
    pub amount1: U256,
}

/// Mirrors `@pancakeswap/v3-sdk` `Position::from_amount0` then `mint_amounts()`.
pub fn v3_mint_amounts_from_amount0(
    sqrt_price_x96: U160,
    tick_current: i32,
    tick_lower: i32,
    tick_upper: i32,
    amount0_desired: U256,
) -> Result<V3MintAmounts, String> {
    if tick_lower >= tick_upper {
        return Err("tick_lower must be < tick_upper".into());
    }
    let sqrt_a = get_sqrt_ratio_at_tick(I24::try_from(tick_lower).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let sqrt_b = get_sqrt_ratio_at_tick(I24::try_from(tick_upper).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;

    let liquidity = max_liquidity_for_amounts(
        sqrt_price_x96,
        sqrt_a,
        sqrt_b,
        amount0_desired,
        U256::MAX,
        false,
    )
    .to_u128()
    .map_err(|_| "liquidity overflow".to_string())?;

    let (amount0, amount1) = if tick_current < tick_lower {
        (
            get_amount_0_delta(sqrt_a, sqrt_b, liquidity, true).map_err(|e| e.to_string())?,
            U256::ZERO,
        )
    } else if tick_current < tick_upper {
        (
            get_amount_0_delta(sqrt_price_x96, sqrt_b, liquidity, true)
                .map_err(|e| e.to_string())?,
            get_amount_1_delta(sqrt_a, sqrt_price_x96, liquidity, true)
                .map_err(|e| e.to_string())?,
        )
    } else {
        (
            U256::ZERO,
            get_amount_1_delta(sqrt_a, sqrt_b, liquidity, true).map_err(|e| e.to_string())?,
        )
    };

    Ok(V3MintAmounts { amount0, amount1 })
}

/// Mirrors `@pancakeswap/v3-sdk` `Position::from_amount1` then `mint_amounts()`.
pub fn v3_mint_amounts_from_amount1(
    sqrt_price_x96: U160,
    tick_current: i32,
    tick_lower: i32,
    tick_upper: i32,
    amount1_desired: U256,
) -> Result<V3MintAmounts, String> {
    if tick_lower >= tick_upper {
        return Err("tick_lower must be < tick_upper".into());
    }
    let sqrt_a = get_sqrt_ratio_at_tick(I24::try_from(tick_lower).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let sqrt_b = get_sqrt_ratio_at_tick(I24::try_from(tick_upper).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;

    let liquidity = max_liquidity_for_amounts(
        sqrt_price_x96,
        sqrt_a,
        sqrt_b,
        U256::MAX,
        amount1_desired,
        false,
    )
    .to_u128()
    .map_err(|_| "liquidity overflow".to_string())?;

    let (amount0, amount1) = if tick_current < tick_lower {
        (
            get_amount_0_delta(sqrt_a, sqrt_b, liquidity, true).map_err(|e| e.to_string())?,
            U256::ZERO,
        )
    } else if tick_current < tick_upper {
        (
            get_amount_0_delta(sqrt_price_x96, sqrt_b, liquidity, true)
                .map_err(|e| e.to_string())?,
            get_amount_1_delta(sqrt_a, sqrt_price_x96, liquidity, true)
                .map_err(|e| e.to_string())?,
        )
    } else {
        (
            U256::ZERO,
            get_amount_1_delta(sqrt_a, sqrt_b, liquidity, true).map_err(|e| e.to_string())?,
        )
    };

    Ok(V3MintAmounts { amount0, amount1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_range_one_to_one_pool_needs_equal_deposits() {
        let sqrt = get_sqrt_ratio_at_tick(I24::from_limbs([0])).unwrap();
        let amounts =
            v3_mint_amounts_from_amount0(sqrt, 0, -887_220, 887_220, U256::from(10u128.pow(18)))
                .unwrap();
        assert_eq!(amounts.amount0, U256::from(10u128.pow(18)));
        assert_eq!(amounts.amount1, U256::from(10u128.pow(18)));
    }

    #[test]
    fn price_above_range_from_amount1_is_token1_only() {
        let sqrt = get_sqrt_ratio_at_tick(I24::from_limbs([0])).unwrap();
        let amounts =
            v3_mint_amounts_from_amount1(sqrt, 0, -1_000, -100, U256::from(10u128.pow(18)))
                .unwrap();
        assert!(amounts.amount0.is_zero());
        assert_eq!(amounts.amount1, U256::from(10u128.pow(18)));
    }

    #[test]
    fn price_below_range_from_amount0_is_token0_only() {
        let sqrt = get_sqrt_ratio_at_tick(I24::from_limbs([100])).unwrap();
        let amounts =
            v3_mint_amounts_from_amount0(sqrt, 100, 200, 1_000, U256::from(10u128.pow(18)))
                .unwrap();
        assert_eq!(amounts.amount0, U256::from(10u128.pow(18)));
        assert!(amounts.amount1.is_zero());
    }
}
