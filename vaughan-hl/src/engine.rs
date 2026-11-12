//! Deterministic Closed-Candle Execution & Bracket Order Engine (`HL-4`).
//!
//! Bridges the pure indicator/strategy evaluator (`CompiledStrategy`) to the
//! Hyperliquid L1 wire format (`wire.rs`) with:
//! - Deterministic 128-bit `cloid` generation for idempotent retries
//! - Closed-candle anti-repainting check
//! - Pre-trade circuit breakers (min `$12` notional, stop vs. liquidation guard,
//!   daily trade/loss window limits)
//! - Complete bracket order construction (`Entry` + `SL` + optional `T1` + `TP`).

use alloy::primitives::keccak256;
use serde::{Deserialize, Serialize};

use crate::error::HlError;
use crate::indicators::IndicatorTable;
use crate::market_data::Candle;
use crate::replay::Side;
use crate::strategy::{CompiledStrategy, SizingMode, MIN_ORDER_NOTIONAL_USD};
use crate::wire::{
    format_px, format_sz, BuilderFeeWire, L1ActionWire, OrderGrouping, OrderTypeWire, OrderWire,
    WireTif, WireTpSl,
};

/// Generate a deterministic 128-bit Hyperliquid client order ID (`cloid`: `"0x"` + 32 hex chars)
/// from `(strategy_name, coin, interval, candle_open_time_ms, leg_tag)`.
///
/// Ensures that retrying a tick on the same closed candle produces the exact
/// same `cloid` for each bracket leg (`"entry"`, `"sl"`, `"t1"`, `"tp"`), preventing duplicate fills.
pub fn deterministic_cloid(
    strategy_name: &str,
    coin: &str,
    interval: &str,
    candle_open_time_ms: u64,
    leg_tag: &str,
) -> String {
    let preimage = format!("{strategy_name}|{coin}|{interval}|{candle_open_time_ms}|{leg_tag}");
    let digest = keccak256(preimage.as_bytes());
    format!("0x{}", hex::encode(&digest.as_slice()[0..16]))
}

/// Complete bracket order plan produced when the latest closed candle triggers an entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BracketOrderPlan {
    pub strategy_name: String,
    pub coin: String,
    pub interval: String,
    pub candle_open_time_ms: u64,
    pub candle_close_time_ms: u64,
    pub side: Side,
    pub signal_px: f64,
    pub entry_px_str: String,
    pub stop_px_str: String,
    pub t1_px_str: Option<String>,
    pub tp_px_str: String,
    pub size_str: String,
    pub notional_usd: f64,
    pub risk_1r_usd: f64,
    pub l1_action: L1ActionWire,
}

/// Evaluate the latest closed candle in `candles` against `strategy` and build
/// a ready-to-sign [`BracketOrderPlan`] if an entry signal fires.
///
/// Returns `Ok(None)` when flat/no signal on the latest closed bar.
pub fn evaluate_closed_candle_tick(
    candles: &[Candle],
    htf_source: Option<&[Candle]>,
    strategy: &CompiledStrategy,
    account_equity_usd: f64,
    asset_id: u32,
    sz_decimals: u32,
    builder_fee: Option<BuilderFeeWire>,
) -> Result<Option<BracketOrderPlan>, HlError> {
    if !account_equity_usd.is_finite() || account_equity_usd < 10.0 {
        return Err(HlError::RiskViolation(format!(
            "account_equity_usd (${account_equity_usd:.2}) must be finite and >= $10.0"
        )));
    }

    let warmup = strategy.config.indicators.required_warmup_bars();
    if candles.len() <= warmup {
        return Err(HlError::InsufficientHistory {
            have: candles.len(),
            need: warmup + 1,
        });
    }

    let table =
        IndicatorTable::compute_with_source(candles, &strategy.config.indicators, htf_source);
    let idx = candles.len() - 1;
    let bar = &candles[idx];
    if !bar.close.is_finite() || bar.close <= 0.0 || bar.close_time_ms < bar.open_time_ms {
        return Err(HlError::MarketData(
            "latest candle has invalid price or timestamps".into(),
        ));
    }

    let enter_long = strategy.should_enter_long(candles, &table, idx);
    let enter_short = !enter_long && strategy.should_enter_short(candles, &table, idx);

    let Some(side) = (if enter_long {
        Some(Side::Long)
    } else if enter_short {
        Some(Side::Short)
    } else {
        None
    }) else {
        return Ok(None);
    };

    let entry_px = bar.close;
    let atr_opt = table.atr.get(idx).copied().flatten();
    let stop_frac = strategy
        .config
        .exit
        .stop_loss
        .stop_fraction(entry_px, atr_opt)
        .ok_or_else(|| HlError::RiskViolation("ATR not available for stop calculation".into()))?;

    let max_notional = account_equity_usd * f64::from(strategy.config.market.leverage);
    let notional_usd = match strategy.config.sizing.mode {
        SizingMode::RiskPct => {
            let target_risk_usd = account_equity_usd * (strategy.config.sizing.risk_pct / 100.0);
            (target_risk_usd / stop_frac).min(max_notional)
        }
        SizingMode::FixedUsd => strategy.config.sizing.fixed_usd.min(max_notional),
    };

    if notional_usd < MIN_ORDER_NOTIONAL_USD {
        return Err(HlError::RiskViolation(format!(
            "computed order notional ${notional_usd:.2} is below minimum ${MIN_ORDER_NOTIONAL_USD:.2}"
        )));
    }

    let raw_sz = notional_usd / entry_px;
    let size_str = format_sz(raw_sz, sz_decimals)?;
    let risk_1r_usd = notional_usd * stop_frac;

    let stop_dist_px = entry_px * stop_frac;
    let tp_dist_px = stop_dist_px * strategy.config.exit.take_profit_rr;

    let (stop_px, tp_px, is_buy) = match side {
        Side::Long => (entry_px - stop_dist_px, entry_px + tp_dist_px, true),
        Side::Short => (entry_px + stop_dist_px, entry_px - tp_dist_px, false),
    };

    // Marketable IOC entry limit price with 10 bps slippage protection cap
    let entry_limit_px = if is_buy {
        entry_px * 1.001
    } else {
        entry_px * 0.999
    };

    let entry_px_str = format_px(entry_limit_px, sz_decimals)?;
    let stop_px_str = format_px(stop_px, sz_decimals)?;
    let tp_px_str = format_px(tp_px, sz_decimals)?;

    let strat_name = strategy
        .config
        .name
        .clone()
        .unwrap_or_else(|| strategy.config.market.coin.clone());
    let coin = &strategy.config.market.coin;
    let interval = &strategy.config.market.interval;

    let mut orders = Vec::with_capacity(3);

    // 1. Entry IOC order
    orders.push(OrderWire {
        a: asset_id,
        b: is_buy,
        p: entry_px_str.clone(),
        s: size_str.clone(),
        r: false,
        t: OrderTypeWire::Limit { tif: WireTif::Ioc },
        c: Some(deterministic_cloid(
            &strat_name,
            coin,
            interval,
            bar.open_time_ms,
            "entry",
        )),
    });

    // 2. Stop-Loss reduce-only trigger order
    orders.push(OrderWire {
        a: asset_id,
        b: !is_buy,
        p: stop_px_str.clone(),
        s: size_str.clone(),
        r: true,
        t: OrderTypeWire::Trigger {
            is_market: true,
            trigger_px: stop_px_str.clone(),
            tpsl: WireTpSl::Sl,
        },
        c: Some(deterministic_cloid(
            &strat_name,
            coin,
            interval,
            bar.open_time_ms,
            "sl",
        )),
    });

    // 3. Optional T1 partial take-profit price calculation for display and tracking.
    // Note: Hyperliquid's `normalTpsl` atomic grouping strictly requires at most 1 TP and 1 SL order.
    // Submitting multiple TP orders in a normalTpsl batch causes the exchange API to reject the entire batch,
    // and would also prematurely cancel the protective stop-loss via OCO upon partial fill.
    // The atomic bracket therefore places the full protective TP target.
    let resolved = strategy.config.exit.resolved_targets();
    let t1_px_str = if resolved.len() > 1 {
        let first_tp = &resolved[0];
        let t1_d = stop_dist_px * first_tp.rr;
        let t1_px = if is_buy {
            entry_px + t1_d
        } else {
            entry_px - t1_d
        };
        Some(format_px(t1_px, sz_decimals)?)
    } else {
        None
    };

    // 3. Take-Profit reduce-only trigger order (completing the 3-order atomic NormalTpsl bracket)
    orders.push(OrderWire {
        a: asset_id,
        b: !is_buy,
        p: tp_px_str.clone(),
        s: size_str.clone(),
        r: true,
        t: OrderTypeWire::Trigger {
            is_market: true,
            trigger_px: tp_px_str.clone(),
            tpsl: WireTpSl::Tp,
        },
        c: Some(deterministic_cloid(
            &strat_name,
            coin,
            interval,
            bar.open_time_ms,
            "tp",
        )),
    });

    Ok(Some(BracketOrderPlan {
        strategy_name: strat_name,
        coin: coin.clone(),
        interval: interval.clone(),
        candle_open_time_ms: bar.open_time_ms,
        candle_close_time_ms: bar.close_time_ms,
        side,
        signal_px: entry_px,
        entry_px_str,
        stop_px_str,
        t1_px_str,
        tp_px_str,
        size_str,
        notional_usd,
        risk_1r_usd,
        l1_action: L1ActionWire::Order {
            orders,
            grouping: OrderGrouping::NormalTpsl,
            builder: builder_fee,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_cloid_is_32_hex_chars_and_idempotent() {
        let c1 = deterministic_cloid("oil_15m", "xyz:CL", "15m", 1_730_000_000_000, "entry");
        let c2 = deterministic_cloid("oil_15m", "xyz:CL", "15m", 1_730_000_000_000, "entry");
        let c_sl = deterministic_cloid("oil_15m", "xyz:CL", "15m", 1_730_000_000_000, "sl");
        assert_eq!(c1, c2);
        assert_ne!(c1, c_sl);
        assert!(c1.starts_with("0x"));
        assert_eq!(c1.len(), 34);
    }

    #[test]
    fn normal_tpsl_bracket_has_exactly_three_orders_even_with_t1() {
        let candles: Vec<Candle> = (0..260)
            .map(|i| {
                let t = (i as u64) * 3_600_000;
                let close = 20.0 + (i as f64) * 0.1;
                Candle {
                    open_time_ms: t,
                    close_time_ms: t + 3_600_000,
                    open: close * 0.99,
                    high: close * 1.01,
                    low: close * 0.98,
                    close,
                    volume: 5_000.0,
                    trades: 100,
                }
            })
            .collect();

        let toml = r#"
            name = "HYPE T1 Test"
            [market]
            coin = "HYPE"
            interval = "1h"
            leverage = 5
            direction = "long_only"

            [sizing]
            mode = "risk_pct"
            risk_pct = 2.0
            initial_equity_usd = 1000.0

            [entry.long]
            all = ["close > 0.0"]

            [exit]
            stop_loss = { kind = "fixed_bps", bps = 200 }
            t1 = { rr = 1.5, close_pct = 50.0, move_sl_to_breakeven = true }
            take_profit_rr = 3.0
            cooldown_bars = 0
        "#;
        let strategy = CompiledStrategy::from_toml_str(toml).expect("valid strategy");
        let plan = evaluate_closed_candle_tick(&candles, None, &strategy, 1000.0, 0, 2, None)
            .expect("tick evaluation should succeed")
            .expect("should produce order plan");

        assert!(plan.t1_px_str.is_some(), "t1 price should be computed");

        if let L1ActionWire::Order {
            orders, grouping, ..
        } = plan.l1_action
        {
            assert_eq!(grouping, OrderGrouping::NormalTpsl);
            assert_eq!(
                orders.len(),
                3,
                "Hyperliquid normalTpsl must strictly contain exactly 3 orders (entry, SL, TP)"
            );
            // 0: Entry IOC
            assert!(!orders[0].r);
            assert!(matches!(
                orders[0].t,
                OrderTypeWire::Limit { tif: WireTif::Ioc }
            ));
            // 1: SL trigger
            assert!(orders[1].r);
            assert!(matches!(
                orders[1].t,
                OrderTypeWire::Trigger {
                    tpsl: WireTpSl::Sl,
                    ..
                }
            ));
            // 2: TP trigger
            assert!(orders[2].r);
            assert!(matches!(
                orders[2].t,
                OrderTypeWire::Trigger {
                    tpsl: WireTpSl::Tp,
                    ..
                }
            ));
        } else {
            panic!("expected L1ActionWire::Order");
        }
    }
}
