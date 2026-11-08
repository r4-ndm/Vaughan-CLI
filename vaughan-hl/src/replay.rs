//! Deterministic Multi-Indicator & R:R Replay Backtester (`no I/O, no network`).
//!
//! Runs the exact same `CompiledStrategy` condition evaluator over historical
//! OHLCV candles, resolving intra-bar Stop-Loss vs. Take-Profit conservatively
//! (if both touch within the same candle's High/Low range, Stop-Loss wins).

use serde::{Deserialize, Serialize};

use crate::error::HlError;
use crate::indicators::IndicatorTable;
use crate::market_data::{uk_day_number, Candle};
use crate::strategy::{
    CompiledStrategy, SizingMode, StopLossSpec, T1Target, MIN_ORDER_NOTIONAL_USD,
};

/// Default Hyperliquid perpetuals taker fee in basis points (`3.5 bps = 0.035%`).
pub const DEFAULT_HL_TAKER_FEE_BPS: f64 = 3.5;
/// Default assumed slippage per fill in basis points (`1.5 bps = 0.015%`).
pub const DEFAULT_SLIPPAGE_BPS: f64 = 1.5;

/// Side of a simulated or live position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Long,
    Short,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Long => "LONG",
            Self::Short => "SHORT",
        }
    }
}

/// Reason a position closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitReason {
    StopLoss,
    /// Stop was trailed to breakeven and hit, without any partial being taken.
    BreakevenStop,
    /// `T1` partial was banked, then the runner stopped at breakeven.
    T1BreakevenStop,
    TakeProfit,
    SignalExit,
    EndOfData,
}

impl ExitReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StopLoss => "SL (-1R)",
            Self::BreakevenStop => "BE Stop",
            Self::T1BreakevenStop => "T1 + BE Stop",
            Self::TakeProfit => "TP (Target R)",
            Self::SignalExit => "Indicator Exit",
            Self::EndOfData => "End of Data",
        }
    }
}

/// Record of one completed trade in the replay harness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimulatedTrade {
    pub id: usize,
    pub side: Side,
    pub entry_bar_idx: usize,
    pub exit_bar_idx: usize,
    pub entry_time_ms: u64,
    pub exit_time_ms: u64,
    pub entry_px: f64,
    pub exit_px: f64,
    pub stop_px_initial: f64,
    pub tp_px: f64,
    pub notional_usd: f64,
    /// Initial dollar risk (`1R` before fees).
    pub risk_1r_usd: f64,
    pub gross_pnl_usd: f64,
    pub fees_usd: f64,
    pub net_pnl_usd: f64,
    /// Net realized R-multiple (`net_pnl_usd / risk_1r_usd`).
    pub realized_r: f64,
    pub t1_hit: bool,
    pub exit_reason: ExitReason,
}

/// Aggregate performance report from a deterministic replay run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayReport {
    pub strategy_name: String,
    pub coin: String,
    pub interval: String,
    pub total_candles: usize,
    pub warmup_candles: usize,
    pub initial_equity_usd: f64,
    pub final_equity_usd: f64,
    pub net_pnl_usd: f64,
    pub net_return_pct: f64,
    pub total_fees_usd: f64,
    pub total_trades: usize,
    pub long_trades: usize,
    pub short_trades: usize,
    pub winning_trades: usize,
    pub losing_trades: usize,
    pub win_rate_pct: f64,
    /// Theoretical minimum win rate needed to break even at `take_profit_rr`.
    pub breakeven_win_rate_pct: f64,
    pub profit_factor: f64,
    /// Sum of `realized_r` across all trades.
    pub total_net_r: f64,
    pub avg_win_r: f64,
    pub avg_loss_r: f64,
    /// Expected R-multiple per trade (`total_net_r / total_trades`).
    pub expectancy_r: f64,
    pub expectancy_usd: f64,
    pub max_drawdown_pct: f64,
    pub max_drawdown_r: f64,
    pub trades: Vec<SimulatedTrade>,
}

#[derive(Debug, Clone)]
struct ActiveTarget {
    #[allow(dead_code)]
    rr: f64,
    target_px: f64,
    close_pct: f64,
    move_sl_to_breakeven: bool,
    hit: bool,
}

struct ActivePosition {
    side: Side,
    entry_bar_idx: usize,
    entry_time_ms: u64,
    entry_px: f64,
    stop_px_initial: f64,
    current_stop_px: f64,
    targets: Vec<ActiveTarget>,
    tp_px: f64,
    initial_notional_usd: f64,
    remaining_notional_usd: f64,
    risk_1r_usd: f64,
    realized_gross_pnl_usd: f64,
    fees_paid_usd: f64,
    /// Trading window (UK calendar day) the position was opened in.
    window_key: i64,
}

/// Run a deterministic replay of `strategy` over `candles`.
pub fn run_replay(
    candles: &[Candle],
    strategy: &CompiledStrategy,
) -> Result<ReplayReport, HlError> {
    run_replay_with_source(candles, strategy, None)
}

/// Replay with an extra candle series available to higher-timeframe filter
/// blocks that declare `source = "<interval>"` (e.g. a weekly filter).
pub fn run_replay_with_source(
    candles: &[Candle],
    strategy: &CompiledStrategy,
    htf_source: Option<&[Candle]>,
) -> Result<ReplayReport, HlError> {
    let table =
        IndicatorTable::compute_with_source(candles, &strategy.config.indicators, htf_source);
    run_replay_with_table(
        candles,
        &table,
        strategy,
        DEFAULT_HL_TAKER_FEE_BPS,
        DEFAULT_SLIPPAGE_BPS,
    )
}

/// Run a deterministic replay using a precomputed `IndicatorTable` (used by `sweep` to avoid recomputing series).
pub fn run_replay_with_table(
    candles: &[Candle],
    table: &IndicatorTable,
    strategy: &CompiledStrategy,
    taker_fee_bps: f64,
    slippage_bps: f64,
) -> Result<ReplayReport, HlError> {
    let warmup = strategy.config.indicators.required_warmup_bars();
    if candles.len() <= warmup {
        return Err(HlError::InsufficientHistory {
            have: candles.len(),
            need: warmup + 1,
        });
    }

    let cost_rate = (taker_fee_bps + slippage_bps) / 10_000.0;
    let initial_equity = strategy.config.sizing.initial_equity_usd;
    let mut equity = initial_equity;
    let mut peak_equity = initial_equity;
    let mut max_drawdown_pct = 0.0;

    let mut cum_r = 0.0;
    let mut peak_r = 0.0;
    let mut max_drawdown_r = 0.0;

    let mut active: Option<ActivePosition> = None;
    let mut last_exit_bar: Option<usize> = None;
    let mut trades: Vec<SimulatedTrade> = Vec::new();

    // Optional throttle: at most N entries per KST regime, reset on a flip.
    let max_per_kst_turn = strategy.config.entry.max_trades_per_kst_turn;
    let mut kst_bullish: Option<bool> = None;
    let mut kst_turn_entries: usize = 0;

    // Optional per-window limits: a window is one UK calendar day, so counters
    // reset at local midnight regardless of the [session] hours.
    let max_per_window = strategy.config.entry.max_trades_per_window;
    let max_losses_per_window = strategy.config.entry.max_losses_per_window;
    let mut window_key: Option<i64> = None;
    let mut window_entries: usize = 0;
    let mut window_losses: usize = 0;

    for idx in warmup..candles.len() {
        let bar = &candles[idx];

        let bar_window = uk_day_number(bar.open_time_ms);
        if window_key != Some(bar_window) {
            window_key = Some(bar_window);
            window_entries = 0;
            window_losses = 0;
        }

        if let Some(hist) = table.kst_hist.get(idx).copied().flatten() {
            let bullish = hist >= 0.0;
            if kst_bullish != Some(bullish) {
                kst_bullish = Some(bullish);
                kst_turn_entries = 0;
            }
        }

        // 1. If a position is open, evaluate intra-bar SL / T1 / TP, then close-of-bar signal exit.
        if let Some(mut pos) = active.take() {
            let mut closed_this_bar: Option<(f64, ExitReason)> = None;

            // 1a. Conservative check: did High/Low breach current_stop_px first?
            let sl_hit = match pos.side {
                Side::Long => bar.low <= pos.current_stop_px,
                Side::Short => bar.high >= pos.current_stop_px,
            };

            if sl_hit {
                let hit_any = pos.targets.iter().any(|t| t.hit);
                let reason = if hit_any {
                    if pos.remaining_notional_usd < pos.initial_notional_usd {
                        ExitReason::T1BreakevenStop
                    } else {
                        ExitReason::BreakevenStop
                    }
                } else {
                    ExitReason::StopLoss
                };
                closed_this_bar = Some((pos.current_stop_px, reason));
            } else {
                // 1b. Check targets in sequence
                let mut all_hit = true;
                for t in pos.targets.iter_mut() {
                    if !t.hit {
                        let reached = match pos.side {
                            Side::Long => bar.high >= t.target_px,
                            Side::Short => bar.low <= t.target_px,
                        };
                        if reached {
                            t.hit = true;
                            let close_frac = (t.close_pct / 100.0).clamp(0.0, 1.0);
                            if close_frac > 0.0 {
                                let closed_notional = (pos.initial_notional_usd * close_frac)
                                    .min(pos.remaining_notional_usd);
                                if closed_notional > 0.0 {
                                    let price_ret = match pos.side {
                                        Side::Long => (t.target_px - pos.entry_px) / pos.entry_px,
                                        Side::Short => (pos.entry_px - t.target_px) / pos.entry_px,
                                    };
                                    pos.realized_gross_pnl_usd += closed_notional * price_ret;
                                    pos.fees_paid_usd += closed_notional * cost_rate;
                                    pos.remaining_notional_usd -= closed_notional;
                                }
                            }
                            if t.move_sl_to_breakeven {
                                pos.current_stop_px = pos.entry_px;
                            }
                        } else {
                            all_hit = false;
                            break;
                        }
                    }
                }

                if pos.remaining_notional_usd <= 1e-4 || all_hit {
                    let last_px = pos.targets.last().map(|t| t.target_px).unwrap_or(pos.tp_px);
                    closed_this_bar = Some((last_px, ExitReason::TakeProfit));
                } else {
                    // 1c. Check indicator-based early exit on bar close
                    let sig_exit = match pos.side {
                        Side::Long => strategy.should_signal_exit_long(candles, table, idx),
                        Side::Short => strategy.should_signal_exit_short(candles, table, idx),
                    };
                    if sig_exit {
                        closed_this_bar = Some((bar.close, ExitReason::SignalExit));
                    }
                }
            }

            if let Some((exit_px, reason)) = closed_this_bar {
                let price_ret = match pos.side {
                    Side::Long => (exit_px - pos.entry_px) / pos.entry_px,
                    Side::Short => (pos.entry_px - exit_px) / pos.entry_px,
                };
                let gross = pos.realized_gross_pnl_usd + pos.remaining_notional_usd * price_ret;
                let fees = pos.fees_paid_usd + pos.remaining_notional_usd * cost_rate;
                let net = gross - fees;
                let realized_r = if pos.risk_1r_usd > f64::EPSILON {
                    net / pos.risk_1r_usd
                } else {
                    0.0
                };

                // Losing trades count against the window they were opened in.
                if realized_r < 0.0 && window_key == Some(pos.window_key) {
                    window_losses += 1;
                }

                equity = (equity + net).max(0.0);
                if equity > peak_equity {
                    peak_equity = equity;
                } else if peak_equity > f64::EPSILON {
                    let dd = ((peak_equity - equity) / peak_equity) * 100.0;
                    if dd > max_drawdown_pct {
                        max_drawdown_pct = dd;
                    }
                }

                cum_r += realized_r;
                if cum_r > peak_r {
                    peak_r = cum_r;
                } else {
                    let dd_r = peak_r - cum_r;
                    if dd_r > max_drawdown_r {
                        max_drawdown_r = dd_r;
                    }
                }

                trades.push(SimulatedTrade {
                    id: trades.len() + 1,
                    side: pos.side,
                    entry_bar_idx: pos.entry_bar_idx,
                    exit_bar_idx: idx,
                    entry_time_ms: pos.entry_time_ms,
                    exit_time_ms: bar.close_time_ms,
                    entry_px: pos.entry_px,
                    exit_px,
                    stop_px_initial: pos.stop_px_initial,
                    tp_px: pos.tp_px,
                    notional_usd: pos.initial_notional_usd,
                    risk_1r_usd: pos.risk_1r_usd,
                    gross_pnl_usd: gross,
                    fees_usd: fees,
                    net_pnl_usd: net,
                    realized_r,
                    t1_hit: pos.targets.iter().any(|t| t.hit),
                    exit_reason: reason,
                });

                last_exit_bar = Some(idx);
                continue;
            } else {
                active = Some(pos);
                continue;
            }
        }

        // 2. When flat, respect cooldown_bars and check entry signals on closed bar `idx`.
        if let Some(last_bar) = last_exit_bar {
            if idx.saturating_sub(last_bar) <= strategy.config.exit.cooldown_bars {
                continue;
            }
        }

        // 2b. The session window is enforced inside `should_enter_*`, so every
        // consumer (replay, chart dots) sees the same gated signal.

        if equity < MIN_ORDER_NOTIONAL_USD {
            break;
        }

        if let Some(max) = max_per_kst_turn {
            if kst_bullish.is_some() && kst_turn_entries >= max {
                continue;
            }
        }

        if let Some(max) = max_per_window {
            if window_entries >= max {
                continue;
            }
        }
        if let Some(max) = max_losses_per_window {
            if window_losses >= max {
                continue;
            }
        }

        let enter_long = strategy.should_enter_long(candles, table, idx);
        let enter_short = !enter_long && strategy.should_enter_short(candles, table, idx);

        let Some(side) = (if enter_long {
            Some(Side::Long)
        } else if enter_short {
            Some(Side::Short)
        } else {
            None
        }) else {
            continue;
        };

        let entry_px = bar.close;
        let atr_opt = table.atr.get(idx).copied().flatten();
        let Some(stop_frac) = strategy
            .config
            .exit
            .stop_loss
            .stop_fraction(entry_px, atr_opt)
        else {
            continue;
        };
        if stop_frac <= 1e-5 {
            continue;
        }

        let max_notional = equity * f64::from(strategy.config.market.leverage);
        let notional_usd = match strategy.config.sizing.mode {
            SizingMode::RiskPct => {
                let target_risk_usd = equity * (strategy.config.sizing.risk_pct / 100.0);
                (target_risk_usd / stop_frac).min(max_notional)
            }
            SizingMode::FixedUsd => strategy.config.sizing.fixed_usd.min(max_notional),
        };

        if notional_usd < MIN_ORDER_NOTIONAL_USD {
            continue;
        }

        let risk_1r_usd = notional_usd * stop_frac;
        let stop_dist_px = entry_px * stop_frac;
        let tp_dist_px = stop_dist_px * strategy.config.exit.take_profit_rr;

        let (stop_px, tp_px) = match side {
            Side::Long => (entry_px - stop_dist_px, entry_px + tp_dist_px),
            Side::Short => (entry_px + stop_dist_px, entry_px - tp_dist_px),
        };

        let targets: Vec<ActiveTarget> = strategy
            .config
            .exit
            .resolved_targets()
            .into_iter()
            .map(|t| {
                let d = stop_dist_px * t.rr;
                let target_px = match side {
                    Side::Long => entry_px + d,
                    Side::Short => entry_px - d,
                };
                ActiveTarget {
                    rr: t.rr,
                    target_px,
                    close_pct: t.close_pct,
                    move_sl_to_breakeven: t.move_sl_to_breakeven,
                    hit: false,
                }
            })
            .collect();

        let entry_fee = notional_usd * cost_rate;
        active = Some(ActivePosition {
            side,
            entry_bar_idx: idx,
            entry_time_ms: bar.close_time_ms,
            entry_px,
            stop_px_initial: stop_px,
            current_stop_px: stop_px,
            targets,
            tp_px,
            initial_notional_usd: notional_usd,
            remaining_notional_usd: notional_usd,
            risk_1r_usd,
            realized_gross_pnl_usd: 0.0,
            fees_paid_usd: entry_fee,
            window_key: bar_window,
        });
        kst_turn_entries += 1;
        window_entries += 1;
    }

    // Build aggregate statistics
    let total_trades = trades.len();
    let long_trades = trades.iter().filter(|t| t.side == Side::Long).count();
    let short_trades = trades.iter().filter(|t| t.side == Side::Short).count();
    let winning_trades = trades.iter().filter(|t| t.net_pnl_usd > 0.0).count();
    let losing_trades = trades.iter().filter(|t| t.net_pnl_usd <= 0.0).count();

    let win_rate_pct = if total_trades > 0 {
        (winning_trades as f64 / total_trades as f64) * 100.0
    } else {
        0.0
    };

    let rr = strategy.config.exit.take_profit_rr;
    let breakeven_win_rate_pct = (1.0 / (1.0 + rr)) * 100.0;

    let gross_wins_usd: f64 = trades
        .iter()
        .filter(|t| t.net_pnl_usd > 0.0)
        .map(|t| t.net_pnl_usd)
        .sum();
    let gross_losses_usd: f64 = trades
        .iter()
        .filter(|t| t.net_pnl_usd < 0.0)
        .map(|t| t.net_pnl_usd.abs())
        .sum();

    let profit_factor = if gross_losses_usd > f64::EPSILON {
        gross_wins_usd / gross_losses_usd
    } else if gross_wins_usd > 0.0 {
        99.9
    } else {
        0.0
    };

    let total_net_r: f64 = trades.iter().map(|t| t.realized_r).sum();
    let avg_win_r = if winning_trades > 0 {
        trades
            .iter()
            .filter(|t| t.realized_r > 0.0)
            .map(|t| t.realized_r)
            .sum::<f64>()
            / winning_trades as f64
    } else {
        0.0
    };
    let avg_loss_r = if losing_trades > 0 {
        trades
            .iter()
            .filter(|t| t.realized_r <= 0.0)
            .map(|t| t.realized_r)
            .sum::<f64>()
            / losing_trades as f64
    } else {
        0.0
    };

    let expectancy_r = if total_trades > 0 {
        total_net_r / total_trades as f64
    } else {
        0.0
    };

    let net_pnl_usd = equity - initial_equity;
    let net_return_pct = (net_pnl_usd / initial_equity) * 100.0;
    let total_fees_usd: f64 = trades.iter().map(|t| t.fees_usd).sum();
    let expectancy_usd = if total_trades > 0 {
        net_pnl_usd / total_trades as f64
    } else {
        0.0
    };

    Ok(ReplayReport {
        strategy_name: strategy.config.name.clone().unwrap_or_else(|| {
            format!(
                "{} {}",
                strategy.config.market.coin, strategy.config.market.interval
            )
        }),
        coin: strategy.config.market.coin.clone(),
        interval: strategy.config.market.interval.clone(),
        total_candles: candles.len(),
        warmup_candles: warmup,
        initial_equity_usd: initial_equity,
        final_equity_usd: equity,
        net_pnl_usd,
        net_return_pct,
        total_fees_usd,
        total_trades,
        long_trades,
        short_trades,
        winning_trades,
        losing_trades,
        win_rate_pct,
        breakeven_win_rate_pct,
        profit_factor,
        total_net_r,
        avg_win_r,
        avg_loss_r,
        expectancy_r,
        expectancy_usd,
        max_drawdown_pct,
        max_drawdown_r,
        trades,
    })
}

/// One row from a multi-parameter grid sweep.
#[derive(Debug, Clone)]
pub struct SweepRow {
    pub label: String,
    pub total_trades: usize,
    pub win_rate_pct: f64,
    pub total_net_r: f64,
    pub expectancy_r: f64,
    pub profit_factor: f64,
    pub max_drawdown_r: f64,
    pub net_return_pct: f64,
}

/// Grid-sweep `RSI + KST + EMA + ATR/R:R` variations over `candles`, returning results sorted by `total_net_r` descending.
pub fn run_parameter_sweep(
    candles: &[Candle],
    coin: &str,
    interval: &str,
) -> Result<Vec<SweepRow>, HlError> {
    let base_ind = crate::indicators::IndicatorConfig::default();
    let table = IndicatorTable::compute(candles, &base_ind);

    let mut rows = Vec::new();

    let entry_styles: &[(&str, &[&str], &[&str])] = &[
        (
            "KST Cross + RSI(30..58) + EMA200",
            &[
                "kst_line crosses_above kst_signal",
                "rsi > 30.0",
                "rsi < 58.0",
                "close > ema_trend",
            ],
            &[
                "kst_line crosses_below kst_signal",
                "rsi < 70.0",
                "rsi > 42.0",
                "close < ema_trend",
            ],
        ),
        (
            "KST Cross + RSI(25..65) (No EMA200)",
            &[
                "kst_line crosses_above kst_signal",
                "rsi > 25.0",
                "rsi < 65.0",
            ],
            &[
                "kst_line crosses_below kst_signal",
                "rsi < 75.0",
                "rsi > 35.0",
            ],
        ),
        (
            "KST > Signal + RSI Cross 35/65 + EMA200",
            &[
                "kst_line > kst_signal",
                "rsi crosses_above 35.0",
                "close > ema_trend",
            ],
            &[
                "kst_line < kst_signal",
                "rsi crosses_below 65.0",
                "close < ema_trend",
            ],
        ),
        (
            "RSI(<32 / >68) + StochRSI Cross + ADX>18",
            &["rsi < 38.0", "stoch_k crosses_above stoch_d", "adx > 18.0"],
            &["rsi > 62.0", "stoch_k crosses_below stoch_d", "adx > 18.0"],
        ),
        (
            "Bollinger Bounce + RSI(<35 / >65)",
            &["close < bb_lower", "rsi < 35.0"],
            &["close > bb_upper", "rsi > 65.0"],
        ),
    ];

    let atr_mults = [1.5, 2.0];
    let rr_targets: &[(f64, Option<T1Target>, &str)] = &[
        (2.0, None, "2.0R"),
        (3.0, None, "3.0R"),
        (
            3.0,
            Some(T1Target {
                rr: 1.5,
                close_pct: 50.0,
                move_sl_to_breakeven: true,
            }),
            "1.5R(T1+BE)+3.0R",
        ),
    ];

    for (style_name, long_conds, short_conds) in entry_styles {
        for &atr_mult in &atr_mults {
            for (tp_rr, t1_opt, rr_label) in rr_targets {
                let cfg = crate::strategy::StrategyConfig {
                    name: Some(format!("{style_name} | SL={atr_mult}xATR | TP={rr_label}")),
                    market: crate::strategy::MarketConfig {
                        coin: coin.to_string(),
                        interval: interval.to_string(),
                        leverage: 5,
                        is_cross: false,
                        direction: crate::strategy::TradeDirection::Both,
                        additional_coins: vec![],
                    },
                    sizing: crate::strategy::SizingConfig::default(),
                    indicators: base_ind.clone(),
                    session: None,
                    entry: crate::strategy::EntrySection {
                        long: crate::strategy::EntryRuleGroup {
                            all: long_conds.iter().map(|s| (*s).to_string()).collect(),
                            any: vec![],
                        },
                        short: crate::strategy::EntryRuleGroup {
                            all: short_conds.iter().map(|s| (*s).to_string()).collect(),
                            any: vec![],
                        },
                        max_trades_per_kst_turn: None,
                        max_trades_per_window: None,
                        max_losses_per_window: None,
                    },
                    exit: crate::strategy::ExitConfig {
                        stop_loss: StopLossSpec::Atr {
                            multiplier: atr_mult,
                            min_stop_bps: 25,
                            max_stop_bps: 300,
                        },
                        t1: t1_opt.clone(),
                        targets: vec![],
                        take_profit_rr: *tp_rr,
                        long_exit_any: vec![],
                        short_exit_any: vec![],
                        cooldown_bars: 2,
                    },
                    auto_reinvest: None,
                };

                let compiled = CompiledStrategy::compile(cfg)?;
                let report = run_replay_with_table(
                    candles,
                    &table,
                    &compiled,
                    DEFAULT_HL_TAKER_FEE_BPS,
                    DEFAULT_SLIPPAGE_BPS,
                )?;

                if report.total_trades > 0 {
                    rows.push(SweepRow {
                        label: report.strategy_name,
                        total_trades: report.total_trades,
                        win_rate_pct: report.win_rate_pct,
                        total_net_r: report.total_net_r,
                        expectancy_r: report.expectancy_r,
                        profit_factor: report.profit_factor,
                        max_drawdown_r: report.max_drawdown_r,
                        net_return_pct: report.net_return_pct,
                    });
                }
            }
        }
    }

    rows.sort_by(|a, b| {
        b.total_net_r
            .partial_cmp(&a.total_net_r)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_candles(count: usize) -> Vec<Candle> {
        (0..count)
            .map(|i| {
                let t = (i as u64) * 900_000;
                // Smooth cyclical wave so RSI + KST generate clear turns
                let wave = ((i as f64) * 0.18).sin() * 8.0 + (i as f64) * 0.05;
                let close = 100.0 + wave;
                Candle {
                    open_time_ms: t,
                    close_time_ms: t + 900_000,
                    open: close - 0.5,
                    high: close + 2.5,
                    low: close - 2.5,
                    close,
                    volume: 1_000.0 + (i as f64 % 5.0) * 100.0,
                    trades: 50,
                }
            })
            .collect()
    }

    #[test]
    fn caps_entries_per_kst_turn() {
        // Monotonic rise: KST stays bullish, so the per-regime cap applies once.
        let candles: Vec<Candle> = (0..400)
            .map(|i| {
                let t = (i as u64) * 900_000;
                let close = 100.0 * 1.003f64.powi(i);
                Candle {
                    open_time_ms: t,
                    close_time_ms: t + 900_000,
                    open: close * 0.999,
                    high: close * 1.002,
                    low: close * 0.998,
                    close,
                    volume: 1_000.0,
                    trades: 50,
                }
            })
            .collect();

        let toml = r#"
            [market]
            coin = "BTC"
            interval = "15m"
            leverage = 5
            direction = "long_only"

            [entry]
            max_trades_per_kst_turn = 2

            [entry.long]
            all = ["close > 0.0"]

            [exit]
            stop_loss = { kind = "fixed_bps", bps = 100 }
            take_profit_rr = 3.0
            cooldown_bars = 0
        "#;
        let strategy = CompiledStrategy::from_toml_str(toml).unwrap();
        let capped = run_replay(&candles, &strategy).unwrap();

        // Count entries per KST regime run and assert the throttle held.
        let table = IndicatorTable::compute(&candles, &strategy.config.indicators);
        let mut entries_at = vec![0usize; candles.len()];
        for t in &capped.trades {
            entries_at[t.entry_bar_idx] += 1;
        }
        let mut regime: Option<bool> = None;
        let mut run_entries = 0usize;
        let mut max_run_entries = 0usize;
        for (idx, bar_entries) in entries_at.iter().enumerate() {
            if let Some(hist) = table.kst_hist[idx] {
                let bullish = hist >= 0.0;
                if regime != Some(bullish) {
                    regime = Some(bullish);
                    run_entries = 0;
                }
            }
            run_entries += bar_entries;
            max_run_entries = max_run_entries.max(run_entries);
        }
        assert!(
            max_run_entries <= 2,
            "expected at most 2 entries per KST run, saw {max_run_entries}"
        );

        let uncapped_toml = toml.replace("max_trades_per_kst_turn = 2", "");
        let uncapped = run_replay(
            &candles,
            &CompiledStrategy::from_toml_str(&uncapped_toml).unwrap(),
        )
        .unwrap();
        assert!(
            uncapped.total_trades > capped.total_trades,
            "without the cap there should be more entries"
        );
    }

    #[test]
    fn replay_executes_and_tracks_r_multiples() {
        let candles = synthetic_candles(350);
        let toml = r#"
            name = "Synthetic Wave Test"
            [market]
            coin = "BTC"
            interval = "15m"
            leverage = 5
            direction = "both"

            [sizing]
            mode = "risk_pct"
            risk_pct = 1.0
            initial_equity_usd = 1000.0

            [entry.long]
            all = ["kst_line crosses_above kst_signal"]

            [entry.short]
            all = ["kst_line crosses_below kst_signal"]

            [exit]
            stop_loss = { kind = "fixed_bps", bps = 150 }
            t1 = { rr = 1.5, close_pct = 50.0, move_sl_to_breakeven = true }
            take_profit_rr = 3.0
            cooldown_bars = 1
        "#;

        let strategy = CompiledStrategy::from_toml_str(toml).unwrap();
        let report = run_replay(&candles, &strategy).unwrap();
        assert!(
            report.total_trades > 0,
            "expected trades to trigger on wave"
        );
        assert!(report.total_fees_usd > 0.0, "fees must be deducted");
    }

    #[test]
    fn breakeven_only_trigger_closes_nothing_and_exits_flat() {
        // Price runs +1.6R, then comes back through entry: with `close_pct = 0`
        // the full position must still be open when the breakeven stop hits, so
        // the trade resolves at entry price (0R gross, fees only).
        let mk = |i: u64, o: f64, h: f64, l: f64, c: f64| Candle {
            open_time_ms: i * 900_000,
            close_time_ms: i * 900_000 + 899_999,
            open: o,
            high: h,
            low: l,
            close: c,
            volume: 1_000.0,
            trades: 50,
        };
        // Warmup needs ~231 bars before any entry can be taken.
        let mut candles: Vec<Candle> = (0..235).map(|i| mk(i, 100.0, 100.1, 99.9, 100.0)).collect();
        candles.push(mk(235, 100.0, 100.1, 99.9, 100.0)); // entry bar
        candles.push(mk(236, 100.0, 101.7, 100.0, 101.5)); // +1.6R: BE trigger
        candles.push(mk(237, 101.5, 101.6, 99.8, 100.0)); // back through entry
        for i in 238..250 {
            candles.push(mk(i, 100.0, 100.1, 99.9, 100.0));
        }

        let toml = r#"
            [market]
            coin = "BTC"
            interval = "15m"
            leverage = 5
            direction = "long_only"

            [sizing]
            mode = "risk_pct"
            risk_pct = 1.0
            initial_equity_usd = 1000.0

            [entry.long]
            all = ["close > 0.0"]

            [exit]
            stop_loss = { kind = "fixed_bps", bps = 100 }
            t1 = { rr = 1.4, close_pct = 0.0, move_sl_to_breakeven = true }
            take_profit_rr = 3.69
            cooldown_bars = 0
        "#;

        let strategy = CompiledStrategy::from_toml_str(toml).expect("valid strategy");
        let report = run_replay(&candles, &strategy).unwrap();
        let trade = report.trades.first().expect("one trade");
        assert_eq!(
            trade.exit_reason,
            ExitReason::BreakevenStop,
            "must exit at the trailed breakeven stop, not a partial"
        );
        assert!(
            (trade.exit_px - trade.entry_px).abs() < 1e-9,
            "breakeven exit at entry price, got {} vs {}",
            trade.exit_px,
            trade.entry_px
        );
        assert!(
            trade.gross_pnl_usd.abs() < 1e-6,
            "nothing banked on the way up: gross PnL must be 0, got {}",
            trade.gross_pnl_usd
        );
        // Only the round-trip cost is lost (100 bps stop -> ~0.4R of fees).
        assert!(
            trade.realized_r < 0.0 && trade.realized_r > -0.6,
            "BE exit should cost fees only, got {}R",
            trade.realized_r
        );
    }

    #[test]
    fn window_limits_cap_entries_and_losses_per_uk_day() {
        // Falling market: every long is a loser, so the loss cap must bind.
        let candles: Vec<Candle> = (0..480)
            .map(|i| {
                let t = (i as u64) * 900_000;
                let close = 100.0 * 0.998f64.powi(i);
                Candle {
                    open_time_ms: t,
                    close_time_ms: t + 900_000 - 1,
                    open: close * 1.001,
                    high: close * 1.002,
                    low: close * 0.997,
                    close,
                    volume: 1_000.0,
                    trades: 50,
                }
            })
            .collect();

        let base = r#"
            [market]
            coin = "BTC"
            interval = "15m"
            leverage = 5
            direction = "long_only"

            [entry]
            max_trades_per_window = 3
            max_losses_per_window = 2

            [entry.long]
            all = ["close > 0.0"]

            [exit]
            stop_loss = { kind = "fixed_bps", bps = 25 }
            take_profit_rr = 3.0
            cooldown_bars = 0
        "#;
        let strategy = CompiledStrategy::from_toml_str(base).unwrap();
        let capped = run_replay(&candles, &strategy).unwrap();
        assert!(
            capped.total_trades >= 2,
            "expected the loss cap to be tested"
        );

        let mut per_day: std::collections::HashMap<i64, (usize, usize)> =
            std::collections::HashMap::new();
        for t in &capped.trades {
            let day = uk_day_number(candles[t.entry_bar_idx].open_time_ms);
            let slot = per_day.entry(day).or_insert((0, 0));
            slot.0 += 1;
            if t.realized_r < 0.0 {
                slot.1 += 1;
            }
        }
        for (day, (entries, losses)) in &per_day {
            assert!(*entries <= 3, "{entries} entries on UK day {day}, max is 3");
            assert!(*losses <= 2, "{losses} losing trades on UK day {day}");
        }

        // Without the loss cap (but with the entry cap) the day trades more.
        let no_loss_cap = base.replace("max_losses_per_window = 2", "");
        let looser = run_replay(
            &candles,
            &CompiledStrategy::from_toml_str(&no_loss_cap).unwrap(),
        )
        .unwrap();
        assert!(
            capped.total_trades <= looser.total_trades,
            "the loss cap must not add trades"
        );
        let looser_losses: usize = looser.trades.iter().filter(|t| t.realized_r < 0.0).count();
        assert!(
            capped.trades.iter().filter(|t| t.realized_r < 0.0).count() < looser_losses,
            "the loss cap must prevent some losing trades"
        );
    }

    #[test]
    fn session_window_restricts_entries_to_uk_hours() {
        let candles = synthetic_candles(400);
        let toml = r#"
            [market]
            coin = "BTC"
            interval = "15m"
            leverage = 5
            direction = "both"

            [session]
            start = "07:00"
            end = "13:00"

            [entry.long]
            all = ["kst_line crosses_above kst_signal"]

            [entry.short]
            all = ["kst_line crosses_below kst_signal"]

            [exit]
            stop_loss = { kind = "fixed_bps", bps = 150 }
            take_profit_rr = 3.0
            cooldown_bars = 0
        "#;
        let strategy = CompiledStrategy::from_toml_str(toml).unwrap();
        let filtered = run_replay(&candles, &strategy).unwrap();
        assert!(
            filtered.total_trades > 0,
            "expected some entries inside the 07:00-13:00 window"
        );
        for t in &filtered.trades {
            let minute =
                crate::market_data::uk_minutes_of_day(candles[t.entry_bar_idx].open_time_ms);
            assert!(
                (420..780).contains(&minute),
                "entry bar opened at UK {minute} minutes, outside 07:00-13:00"
            );
        }

        let unrestricted = run_replay(
            &candles,
            &CompiledStrategy::from_toml_str(&toml.replace(
                "[session]\n            start = \"07:00\"\n            end = \"13:00\"",
                "",
            ))
            .unwrap(),
        )
        .unwrap();
        assert!(
            unrestricted.trades.iter().any(|t| !(420..780).contains(
                &crate::market_data::uk_minutes_of_day(candles[t.entry_bar_idx].open_time_ms)
            )),
            "synthetic data should contain out-of-session entries to filter out"
        );
        assert!(
            filtered.total_trades < unrestricted.total_trades,
            "session filter should remove entries"
        );
    }
}
