//! Human V3 pool prices → usable ticks (token1 per token0, decimal-adjusted).
//!
//! Range preset math follows PancakeSwap / 9mm frontend behavior (Pancake
//! `pancake-frontend` + `@pancakeswap/v3-sdk` Position price bounds). Narrow
//! presets use linear ±% on the display price; wide presets (≥50%) use the
//! multiplicative upper bound `center / (1 − p)` so 50% → 0.5×–2.0× on
//! dex.9mm.pro-style URLs.

use alloy::primitives::Address;
use uniswap_v3_sdk::prelude::sdk_core::prelude::{BigInt, CurrencyAmount, Price, Token};

use crate::fee_tiers::tick_spacing;
use crate::{nearest_usable_tick, price_to_closest_tick};

fn v3_token(chain_id: u64, addr: Address, decimals: u8, label: &str) -> Token {
    Token::new(
        chain_id,
        addr,
        decimals,
        Some(label.to_string()),
        Some(label.to_string()),
        0,
        0,
    )
}

/// Parse a human decimal string into raw token units at `decimals` fractional digits.
fn scale_human_decimal(raw: &str, decimals: u8) -> Result<u64, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("price is empty".into());
    }
    let (whole, frac) = match s.split_once('.') {
        None => (s, ""),
        Some((w, f)) => (w, f),
    };
    if whole.is_empty() && frac.is_empty() {
        return Err("invalid price".into());
    }
    if frac.len() > decimals as usize {
        return Err(format!("max {decimals} decimal places"));
    }
    let mut digits = if whole.is_empty() {
        String::new()
    } else {
        whole.to_string()
    };
    digits.push_str(frac);
    digits.push_str(&"0".repeat(decimals as usize - frac.len()));
    digits
        .parse::<u64>()
        .map_err(|e| format!("invalid price digits: {e}"))
}

fn u64_to_bigint(v: u64) -> BigInt {
    if v <= u32::MAX as u64 {
        return BigInt::from(v as u32);
    }
    let lo = (v & 0xFFFF_FFFF) as u32;
    let hi = (v >> 32) as u32;
    (BigInt::from(hi) << 32) + BigInt::from(lo)
}

/// Closest usable tick for `price_token1_per_token0` (human, quote per 1 base).
#[allow(clippy::too_many_arguments)]
pub fn pool_price_to_usable_tick(
    chain_id: u64,
    token0: Address,
    token1: Address,
    dec0: u8,
    dec1: u8,
    price_token1_per_token0: &str,
    fee: u32,
) -> Result<i32, String> {
    let t0 = v3_token(chain_id, token0, dec0, "T0");
    let t1 = v3_token(chain_id, token1, dec1, "T1");
    let base_raw = BigInt::from(10u32).pow(dec0 as u32);
    let quote_raw = u64_to_bigint(scale_human_decimal(price_token1_per_token0, dec1)?);
    let base_amount =
        CurrencyAmount::from_raw_amount(t0.clone(), base_raw).map_err(|e| e.to_string())?;
    let quote_amount =
        CurrencyAmount::from_raw_amount(t1.clone(), quote_raw).map_err(|e| e.to_string())?;
    let price = Price::from_currency_amounts(base_amount, quote_amount);
    let tick = price_to_closest_tick(&price).map_err(|e| e.to_string())?;
    let spacing = tick_spacing(fee).ok_or_else(|| format!("unsupported fee tier {fee}"))?;
    let raw = i32::try_from(tick).map_err(|e| e.to_string())?;
    Ok(nearest_usable_tick(raw, spacing))
}

/// Usable `(tick_lower, tick_upper)` from human min/max prices (token1 per token0).
#[allow(clippy::too_many_arguments)]
pub fn pool_price_range_to_usable_ticks(
    chain_id: u64,
    token0: Address,
    token1: Address,
    dec0: u8,
    dec1: u8,
    min_price_token1_per_token0: &str,
    max_price_token1_per_token0: &str,
    fee: u32,
) -> Result<(i32, i32), String> {
    let lo = pool_price_to_usable_tick(
        chain_id,
        token0,
        token1,
        dec0,
        dec1,
        min_price_token1_per_token0,
        fee,
    )?;
    let hi = pool_price_to_usable_tick(
        chain_id,
        token0,
        token1,
        dec0,
        dec1,
        max_price_token1_per_token0,
        fee,
    )?;
    if lo >= hi {
        return Err("min price must be below max price (token1 per token0)".into());
    }
    Ok((lo, hi))
}

/// Human token1-per-token0 price at `tick` (decimal-adjusted for `dec0`/`dec1`).
#[allow(clippy::too_many_arguments)]
pub fn pool_tick_to_human_price(
    chain_id: u64,
    token0: Address,
    token1: Address,
    dec0: u8,
    dec1: u8,
    tick: i32,
) -> Result<String, String> {
    use crate::tick_to_price;
    use alloy::primitives::aliases::I24;

    let t0 = v3_token(chain_id, token0, dec0, "T0");
    let t1 = v3_token(chain_id, token1, dec1, "T1");
    let price = tick_to_price(t0, t1, I24::try_from(tick).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    price.to_significant(12, None).map_err(|e| e.to_string())
}

/// Display-price min/max for a Pancake / 9mm-style preset chip (token1 per token0).
///
/// - Below 50%: linear symmetric band — `center × (1 ± p/100)`.
/// - At 50% and above: lower `center × (1 − p/100)`, upper `center / (1 − p/100)`.
pub fn display_price_range_from_preset(
    center_price_token1_per_token0: f64,
    preset_percent: f64,
) -> (f64, f64) {
    let p = preset_percent / 100.0;
    let min = center_price_token1_per_token0 * (1.0 - p);
    let max = if preset_percent >= 50.0 {
        center_price_token1_per_token0 / (1.0 - p)
    } else {
        center_price_token1_per_token0 * (1.0 + p)
    };
    (min, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    #[test]
    fn range_ticks_order_when_pool_prices_ascend() {
        let t0 = address!("0x0000000000000000000000000000000000000001");
        let t1 = address!("0x0000000000000000000000000000000000000002");
        let (lo, hi) = pool_price_range_to_usable_ticks(369, t0, t1, 18, 18, "1.0", "2.0", 10_000)
            .expect("ascending pool prices");
        assert!(lo < hi);
    }

    #[test]
    fn preset_ten_percent_matches_9mm_url() {
        let center = 0.001_265_324;
        let (min, max) = display_price_range_from_preset(center, 10.0);
        assert!((min - 0.001_138_792_05).abs() < 1e-9);
        assert!((max - 0.001_391_856_95).abs() < 1e-9);
    }

    #[test]
    fn preset_fifty_percent_matches_9mm_url() {
        let center = 0.001_265_324;
        let (min, max) = display_price_range_from_preset(center, 50.0);
        assert!((min - 0.000_632_662_25).abs() < 1e-9);
        assert!((max - 0.002_530_649).abs() < 1e-9);
    }
}
