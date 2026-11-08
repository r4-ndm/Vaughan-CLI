//! Multi-indicator TOML strategy configuration, rule expression compiler, and risk validation.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::HlError;
use crate::indicators::{IndicatorConfig, IndicatorTable, MetricId};
use crate::market_data::{uk_minutes_of_day, validate_interval, Candle};

/// Minimum allowed notional order size in USD (Hyperliquid enforces a $10 minimum; $12 gives tick buffer).
pub const MIN_ORDER_NOTIONAL_USD: f64 = 12.0;

/// Allowed trading directions for a strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TradeDirection {
    LongOnly,
    ShortOnly,
    #[default]
    Both,
}

impl TradeDirection {
    pub fn allows_long(self) -> bool {
        matches!(self, Self::LongOnly | Self::Both)
    }

    pub fn allows_short(self) -> bool {
        matches!(self, Self::ShortOnly | Self::Both)
    }
}

/// Market & leverage settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarketConfig {
    pub coin: String,
    pub interval: String,
    #[serde(default = "default_leverage")]
    pub leverage: u32,
    #[serde(default)]
    pub is_cross: bool,
    #[serde(default)]
    pub direction: TradeDirection,
    #[serde(default)]
    pub additional_coins: Vec<String>,
}

fn default_leverage() -> u32 {
    3
}

/// Position sizing mode (`1R` risk percentage vs fixed USD notional).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SizingMode {
    #[default]
    RiskPct,
    FixedUsd,
}

/// Position sizing configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SizingConfig {
    #[serde(default)]
    pub mode: SizingMode,
    /// Percentage of current equity risked per trade (`1R`), e.g. `1.0` = 1% risk.
    #[serde(default = "default_risk_pct")]
    pub risk_pct: f64,
    /// Fixed notional USD per trade when `mode = "fixed_usd"`.
    #[serde(default = "default_fixed_usd")]
    pub fixed_usd: f64,
    /// Starting account equity for replay simulations.
    #[serde(default = "default_initial_equity")]
    pub initial_equity_usd: f64,
}

impl Default for SizingConfig {
    fn default() -> Self {
        Self {
            mode: SizingMode::RiskPct,
            risk_pct: default_risk_pct(),
            fixed_usd: default_fixed_usd(),
            initial_equity_usd: default_initial_equity(),
        }
    }
}

fn default_risk_pct() -> f64 {
    1.0
}

fn default_fixed_usd() -> f64 {
    10.0
}

fn default_initial_equity() -> f64 {
    1_000.0
}

/// Stop-loss distance model (defines `1R`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StopLossSpec {
    /// Stop distance = `multiplier * ATR(period)`, bounded between `min_stop_bps` and `max_stop_bps`.
    Atr {
        multiplier: f64,
        #[serde(default = "default_min_stop_bps")]
        min_stop_bps: u32,
        #[serde(default = "default_max_stop_bps")]
        max_stop_bps: u32,
    },
    /// Fixed stop distance in basis points (`100 bps = 1.0%`).
    FixedBps { bps: u32 },
}

fn default_min_stop_bps() -> u32 {
    25
}

fn default_max_stop_bps() -> u32 {
    250
}

impl StopLossSpec {
    /// Maximum possible stop-loss distance in basis points (used for liquidation safety validation).
    pub fn max_stop_bps(&self) -> u32 {
        match self {
            Self::Atr { max_stop_bps, .. } => *max_stop_bps,
            Self::FixedBps { bps } => *bps,
        }
    }

    /// Compute the stop distance fraction (`0.01` = 1%) at bar `idx`.
    pub fn stop_fraction(&self, entry_px: f64, atr_opt: Option<f64>) -> Option<f64> {
        match self {
            Self::Atr {
                multiplier,
                min_stop_bps,
                max_stop_bps,
            } => {
                if min_stop_bps > max_stop_bps || !multiplier.is_finite() || *multiplier <= 0.0 {
                    return None;
                }
                let atr = atr_opt?;
                if !atr.is_finite()
                    || atr <= 0.0
                    || !entry_px.is_finite()
                    || entry_px <= f64::EPSILON
                {
                    return None;
                }
                let raw_frac = (atr * *multiplier) / entry_px;
                if !raw_frac.is_finite() {
                    return None;
                }
                let min_f = f64::from(*min_stop_bps) / 10_000.0;
                let max_f = f64::from(*max_stop_bps) / 10_000.0;
                Some(raw_frac.clamp(min_f, max_f))
            }
            Self::FixedBps { bps } => {
                if *bps == 0 {
                    None
                } else {
                    Some(f64::from(*bps) / 10_000.0)
                }
            }
        }
    }
}

/// Target specification for multi-tier scaling out (e.g. 1:1, 1:2, 1:3.15, 1:4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TargetSpec {
    /// R-multiple at which this target triggers (e.g. 1.0 for 1:1, 2.0 for 1:2).
    pub rr: f64,
    /// Percentage of initial position to close at this target (e.g. 25.0 for 25%).
    pub close_pct: f64,
    /// Whether to move the stop-loss to breakeven once this target triggers.
    #[serde(default)]
    pub move_sl_to_breakeven: bool,
}

/// Optional Target 1 (`T1`) partial take-profit & breakeven stop adjustment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct T1Target {
    /// R-multiple at which `T1` triggers (e.g. `1.5` for `+1.5R`).
    pub rr: f64,
    /// Percentage of position to close at `T1` (e.g. `50.0` for 50%).
    pub close_pct: f64,
    /// Move the stop-loss to breakeven once `T1` hits. With `close_pct = 0` this
    /// is a pure breakeven trigger: nothing is closed and the full position runs.
    #[serde(default = "default_true")]
    pub move_sl_to_breakeven: bool,
}

fn default_true() -> bool {
    true
}

/// Exit & Risk-to-Reward (`R:R`) configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExitConfig {
    pub stop_loss: StopLossSpec,
    /// Optional partial take-profit (`T1`) + breakeven stop trigger.
    #[serde(default)]
    pub t1: Option<T1Target>,
    /// Optional multi-tier targets (e.g. 1:1, 1:2, 1:3.15, 1:4). If specified, takes precedence.
    #[serde(default)]
    pub targets: Vec<TargetSpec>,
    /// Full take-profit target in R-multiples (`1R` = stop distance; e.g. `3.0` = `1:3 R:R`).
    #[serde(default = "default_tp_rr")]
    pub take_profit_rr: f64,
    /// Optional indicator conditions that force an early exit of an open Long position (`OR`-ed).
    #[serde(default)]
    pub long_exit_any: Vec<String>,
    /// Optional indicator conditions that force an early exit of an open Short position (`OR`-ed).
    #[serde(default)]
    pub short_exit_any: Vec<String>,
    /// Minimum bars to wait after a trade closes before entering a new position.
    #[serde(default = "default_cooldown_bars")]
    pub cooldown_bars: usize,
}

impl ExitConfig {
    /// Return effective targets in ascending R:R order.
    pub fn resolved_targets(&self) -> Vec<TargetSpec> {
        if !self.targets.is_empty() {
            let mut sorted = self.targets.clone();
            sorted.sort_by(|a, b| a.rr.partial_cmp(&b.rr).unwrap_or(std::cmp::Ordering::Equal));
            return sorted;
        }
        if let Some(t1) = &self.t1 {
            let remaining_pct = (100.0 - t1.close_pct).max(0.0);
            vec![
                TargetSpec {
                    rr: t1.rr,
                    close_pct: t1.close_pct,
                    move_sl_to_breakeven: t1.move_sl_to_breakeven,
                },
                TargetSpec {
                    rr: self.take_profit_rr,
                    close_pct: remaining_pct,
                    move_sl_to_breakeven: false,
                },
            ]
        } else {
            vec![TargetSpec {
                rr: self.take_profit_rr,
                close_pct: 100.0,
                move_sl_to_breakeven: false,
            }]
        }
    }
}

fn default_tp_rr() -> f64 {
    3.0
}

fn default_cooldown_bars() -> usize {
    2
}

/// Entry rule group (`all` = AND, `any` = OR; if both are present, both groups must hold).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EntryRuleGroup {
    #[serde(default)]
    pub all: Vec<String>,
    #[serde(default)]
    pub any: Vec<String>,
}

/// Entry section for Long and Short setups.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EntrySection {
    #[serde(default)]
    pub long: EntryRuleGroup,
    #[serde(default)]
    pub short: EntryRuleGroup,
    /// Maximum entries allowed per KST regime (KST histogram positive = bullish,
    /// negative = bearish). The counter resets whenever the regime flips.
    #[serde(default)]
    pub max_trades_per_kst_turn: Option<usize>,
    /// Maximum entries allowed inside one trading window (a UK calendar day).
    #[serde(default)]
    pub max_trades_per_window: Option<usize>,
    /// Stop taking new entries in a window once this many losing trades have closed.
    #[serde(default)]
    pub max_losses_per_window: Option<usize>,
}

/// Single trading window in UK local time (`Europe/London`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionWindow {
    pub start: String,
    pub end: String,
}

/// Optional daily trading window(s) in UK local time (`Europe/London`, BST/GMT
/// aware). `start` is inclusive and `end` exclusive; a window may wrap past
/// midnight (e.g. `22:00`–`02:00`). Supports either a single `start`/`end` pair
/// or multiple `windows = [{ start = "08:00", end = "13:00" }, ...]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SessionConfig {
    #[serde(default)]
    pub start: Option<String>,
    #[serde(default)]
    pub end: Option<String>,
    #[serde(default)]
    pub windows: Vec<SessionWindow>,
}

/// Execution cadence for the automated profit siphon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReinvestCadence {
    #[default]
    Weekly,
    Daily,
    PerTrade,
}

impl ReinvestCadence {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Weekly => "weekly",
            Self::Daily => "daily",
            Self::PerTrade => "per_trade",
        }
    }
}

/// Automated Perp-to-Spot Profit Siphon policy (PulseChain spot accumulation).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutoReinvestConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Percentage of net realized perp profits to auto-reinvest (default: 20.0).
    #[serde(default = "default_profit_pct")]
    pub profit_pct: f64,
    /// Target ERC-20 contract address or native symbol (default: meme coin BTC 0xF7bf2A938f971D7e4811A1170C43d651d21A0F81).
    #[serde(default = "default_target_token")]
    pub target_token: String,
    /// User-friendly label for the target token (default: "BTC").
    #[serde(default = "default_target_symbol")]
    pub target_token_symbol: String,
    /// Target EVM chain ID (default: 369 for PulseChain).
    #[serde(default = "default_target_chain_id")]
    pub target_chain_id: u64,
    /// Execution cadence: "weekly" (default), "daily", or "per_trade".
    #[serde(default)]
    pub cadence: ReinvestCadence,
    /// Settlement day of week for weekly cadence (default: "Sunday").
    #[serde(default = "default_epoch_day")]
    pub weekly_epoch_day: String,
    /// UTC time for weekly settlement (default: "00:00").
    #[serde(default = "default_epoch_time")]
    pub weekly_epoch_time: String,
    /// Minimum profit threshold in USD to trigger a batch (default: $25.00).
    #[serde(default = "default_min_threshold_usd")]
    pub min_profit_threshold_usd: f64,
    /// Maximum allowed slippage in basis points (default: 100 = 1.00%).
    #[serde(default = "default_slippage_bps")]
    pub max_slippage_bps: u32,
    /// Execution routing: "auto" (Vaughan Ag meta-aggregator), or specific DEX like "pulsex", "9inch", etc.
    #[serde(default = "default_route")]
    pub route: String,
    /// Optional explicit PLS-per-USD conversion rate. If None, derived dynamically via DAI -> WPLS quote.
    #[serde(default)]
    pub pls_per_usd: Option<f64>,
    /// Simulation mode without broadcasting on-chain transactions.
    #[serde(default)]
    pub dry_run: bool,
}

fn default_profit_pct() -> f64 {
    20.0
}
fn default_target_token() -> String {
    "0xF7bf2A938f971D7e4811A1170C43d651d21A0F81".to_string()
}
fn default_target_symbol() -> String {
    "BTC".to_string()
}
fn default_target_chain_id() -> u64 {
    369
}
fn default_epoch_day() -> String {
    "Sunday".to_string()
}
fn default_epoch_time() -> String {
    "00:00".to_string()
}
fn default_min_threshold_usd() -> f64 {
    25.0
}
fn default_slippage_bps() -> u32 {
    100
}
fn default_route() -> String {
    "auto".to_string()
}

impl Default for AutoReinvestConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            profit_pct: default_profit_pct(),
            target_token: default_target_token(),
            target_token_symbol: default_target_symbol(),
            target_chain_id: default_target_chain_id(),
            cadence: ReinvestCadence::Weekly,
            weekly_epoch_day: default_epoch_day(),
            weekly_epoch_time: default_epoch_time(),
            min_profit_threshold_usd: default_min_threshold_usd(),
            max_slippage_bps: default_slippage_bps(),
            route: default_route(),
            pls_per_usd: None,
            dry_run: false,
        }
    }
}

/// Top-level `strategy.toml` document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrategyConfig {
    pub name: Option<String>,
    pub market: MarketConfig,
    #[serde(default)]
    pub sizing: SizingConfig,
    #[serde(default)]
    pub indicators: IndicatorConfig,
    #[serde(default)]
    pub session: Option<SessionConfig>,
    pub entry: EntrySection,
    pub exit: ExitConfig,
    #[serde(default)]
    pub auto_reinvest: Option<AutoReinvestConfig>,
}

/// Parse a `"HH:MM"` clock time into minutes since midnight.
fn parse_hhmm(raw: &str) -> Result<u32, HlError> {
    let trimmed = raw.trim();
    let (h, m) = trimmed.split_once(':').ok_or_else(|| {
        HlError::InvalidStrategy(format!("session time '{raw}' must look like \"HH:MM\""))
    })?;
    let hour: u32 = h.trim().parse().map_err(|_| {
        HlError::InvalidStrategy(format!("session time '{raw}' has an invalid hour"))
    })?;
    let minute: u32 = m.trim().parse().map_err(|_| {
        HlError::InvalidStrategy(format!("session time '{raw}' has an invalid minute"))
    })?;
    if hour > 23 || minute > 59 {
        return Err(HlError::InvalidStrategy(format!(
            "session time '{raw}' is out of range (00:00-23:59)"
        )));
    }
    Ok(hour * 60 + minute)
}

/// Right-hand operand in a parsed rule expression (either a constant `f64` or another `MetricId`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RightOperand {
    Constant(f64),
    Metric(MetricId),
}

/// Comparison or crossover operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionOp {
    Lt,
    Lte,
    Gt,
    Gte,
    CrossesAbove,
    CrossesBelow,
}

/// Compiled single condition expression.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompiledCondition {
    pub left: MetricId,
    pub op: ConditionOp,
    pub right: RightOperand,
}

impl CompiledCondition {
    /// Parse a human-readable rule expression such as:
    /// - `"kst_line crosses_above kst_signal"`
    /// - `"rsi crosses_above 30.0"`
    /// - `"rsi < 55.0"`
    /// - `"close > ema_trend"`
    pub fn parse(expr: &str) -> Result<Self, HlError> {
        let tokens: Vec<&str> = expr.split_whitespace().collect();
        if tokens.len() != 3 {
            return Err(HlError::InvalidStrategy(format!(
                "invalid condition '{expr}': expected '<metric> <op> <metric_or_number>' (3 tokens)"
            )));
        }

        let left = MetricId::parse(tokens[0]).ok_or_else(|| {
            HlError::InvalidStrategy(format!(
                "unknown metric '{}' in condition '{expr}'",
                tokens[0]
            ))
        })?;

        let op = match tokens[1].to_ascii_lowercase().as_str() {
            "<" | "lt" => ConditionOp::Lt,
            "<=" | "lte" => ConditionOp::Lte,
            ">" | "gt" => ConditionOp::Gt,
            ">=" | "gte" => ConditionOp::Gte,
            "crosses_above" | "cross_above" | "x>" => ConditionOp::CrossesAbove,
            "crosses_below" | "cross_below" | "x<" => ConditionOp::CrossesBelow,
            other => {
                return Err(HlError::InvalidStrategy(format!(
                    "unknown operator '{other}' in '{expr}'; use <, <=, >, >=, crosses_above, or crosses_below"
                )))
            }
        };

        let right = if let Ok(val) = tokens[2].parse::<f64>() {
            if !val.is_finite() {
                return Err(HlError::InvalidStrategy(format!(
                    "non-finite numeric constant '{}' in '{expr}'",
                    tokens[2]
                )));
            }
            RightOperand::Constant(val)
        } else if let Some(m) = MetricId::parse(tokens[2]) {
            RightOperand::Metric(m)
        } else {
            return Err(HlError::InvalidStrategy(format!(
                "invalid right-hand operand '{}' in '{expr}': not a number or known metric",
                tokens[2]
            )));
        };

        Ok(Self { left, op, right })
    }

    /// Evaluate this condition at closed candle index `idx`.
    /// Crossover operators (`CrossesAbove`, `CrossesBelow`) also inspect `idx - 1`.
    pub fn evaluate(&self, candles: &[Candle], table: &IndicatorTable, idx: usize) -> bool {
        let Some(cur_left) = table.get(candles, idx, self.left) else {
            return false;
        };
        let Some(cur_right) = (match self.right {
            RightOperand::Constant(v) => Some(v),
            RightOperand::Metric(m) => table.get(candles, idx, m),
        }) else {
            return false;
        };

        match self.op {
            ConditionOp::Lt => cur_left < cur_right,
            ConditionOp::Lte => cur_left <= cur_right,
            ConditionOp::Gt => cur_left > cur_right,
            ConditionOp::Gte => cur_left >= cur_right,
            ConditionOp::CrossesAbove => {
                if idx == 0 {
                    return false;
                }
                let Some(prev_left) = table.get(candles, idx - 1, self.left) else {
                    return false;
                };
                let Some(prev_right) = (match self.right {
                    RightOperand::Constant(v) => Some(v),
                    RightOperand::Metric(m) => table.get(candles, idx - 1, m),
                }) else {
                    return false;
                };
                prev_left <= prev_right && cur_left > cur_right
            }
            ConditionOp::CrossesBelow => {
                if idx == 0 {
                    return false;
                }
                let Some(prev_left) = table.get(candles, idx - 1, self.left) else {
                    return false;
                };
                let Some(prev_right) = (match self.right {
                    RightOperand::Constant(v) => Some(v),
                    RightOperand::Metric(m) => table.get(candles, idx - 1, m),
                }) else {
                    return false;
                };
                prev_left >= prev_right && cur_left < cur_right
            }
        }
    }
}

/// Compiled entry/exit rule set ready for fast evaluation.
#[derive(Debug, Clone)]
pub struct CompiledStrategy {
    pub config: StrategyConfig,
    /// Resolved primary session window as `(start, end)` minutes since midnight, UK local time.
    pub session: Option<(u32, u32)>,
    /// All resolved session windows in minutes since midnight, UK local time.
    pub sessions: Vec<(u32, u32)>,
    pub long_all: Vec<CompiledCondition>,
    pub long_any: Vec<CompiledCondition>,
    pub short_all: Vec<CompiledCondition>,
    pub short_any: Vec<CompiledCondition>,
    pub long_exit_any: Vec<CompiledCondition>,
    pub short_exit_any: Vec<CompiledCondition>,
}

/// Estimate isolated liquidation distance in basis points for `leverage`
/// assuming a conservative `50 bps` (0.5%) maintenance margin rate.
pub fn estimated_liq_distance_bps(leverage: u32) -> u32 {
    if leverage == 0 {
        return 0;
    }
    let gross_bps = 10_000u32 / leverage;
    gross_bps.saturating_sub(50)
}

impl CompiledStrategy {
    /// Load and validate a `StrategyConfig` from a TOML file on disk.
    pub fn from_toml_file(path: &Path) -> Result<Self, HlError> {
        let raw = std::fs::read_to_string(path)?;
        Self::from_toml_str(&raw)
    }

    /// Parse and validate a `StrategyConfig` from a TOML string.
    pub fn from_toml_str(raw: &str) -> Result<Self, HlError> {
        let config: StrategyConfig = toml::from_str(raw)?;
        Self::compile(config)
    }

    /// Compile and validate a `StrategyConfig`.
    pub fn compile(config: StrategyConfig) -> Result<Self, HlError> {
        if config.market.coin.trim().is_empty() {
            return Err(HlError::InvalidStrategy(
                "market.coin cannot be empty".into(),
            ));
        }
        // Derived intervals (e.g. 10m, aggregated from 5m) are not venue intervals.
        if crate::market_data::derived_source_interval(&config.market.interval).is_none() {
            validate_interval(&config.market.interval)?;
        }

        // A higher-timeframe block must sit above the interval it is built from and
        // be a whole multiple of it, otherwise its bars cannot be resampled.
        let mut sources: Vec<String> = Vec::new();
        for (slot, block) in [
            ("htf", config.indicators.htf.as_ref()),
            ("htf2", config.indicators.htf2.as_ref()),
        ] {
            let Some(htf) = block else { continue };
            validate_interval(&htf.interval)?;
            let from = match &htf.source {
                Some(source) => {
                    validate_interval(source)?;
                    sources.push(source.clone());
                    source.clone()
                }
                None => config.market.interval.clone(),
            };
            let base = crate::market_data::interval_duration_ms(&from)?;
            let step = crate::market_data::interval_duration_ms(&htf.interval)?;
            if step <= base || step % base != 0 {
                return Err(HlError::InvalidStrategy(format!(
                    "indicators.{slot}.interval '{}' must be a whole multiple of '{from}' and higher than it",
                    htf.interval
                )));
            }
        }
        if sources.len() == 2 && sources[0] != sources[1] {
            return Err(HlError::InvalidStrategy(format!(
                "indicators.htf.source and indicators.htf2.source must match ({} vs {}) — only one extra candle series is loaded",
                sources[0], sources[1]
            )));
        }

        if config.market.leverage == 0 || config.market.leverage > 50 {
            return Err(HlError::InvalidStrategy(format!(
                "market.leverage must be between 1 and 50 (got {})",
                config.market.leverage
            )));
        }

        // Validate indicator periods and multipliers (prevent 0-period or overflow warmup)
        let ind = &config.indicators;
        for (name, p) in [
            ("rsi.period", ind.rsi.period),
            ("rsi.ma_period", ind.rsi.ma_period),
            ("ema_fast.period", ind.ema_fast.period),
            ("ema_trend.period", ind.ema_trend.period),
            ("macd.fast", ind.macd.fast),
            ("macd.slow", ind.macd.slow),
            ("macd.signal", ind.macd.signal),
            ("adx.period", ind.adx.period),
            ("atr.period", ind.atr.period),
            ("stoch_rsi.rsi_period", ind.stoch_rsi.rsi_period),
            ("stoch_rsi.stoch_period", ind.stoch_rsi.stoch_period),
            ("stoch_rsi.k_smooth", ind.stoch_rsi.k_smooth),
            ("stoch_rsi.d_smooth", ind.stoch_rsi.d_smooth),
            ("connors_rsi.rsi_period", ind.connors_rsi.rsi_period),
            ("connors_rsi.streak_period", ind.connors_rsi.streak_period),
            ("connors_rsi.rank_period", ind.connors_rsi.rank_period),
            ("supertrend.atr_period", ind.supertrend.atr_period),
            ("bollinger.period", ind.bollinger.period),
            ("vol_ratio.period", ind.vol_ratio.period),
            ("kst.r1", ind.kst.r1),
            ("kst.r2", ind.kst.r2),
            ("kst.r3", ind.kst.r3),
            ("kst.r4", ind.kst.r4),
            ("kst.s1", ind.kst.s1),
            ("kst.s2", ind.kst.s2),
            ("kst.s3", ind.kst.s3),
            ("kst.s4", ind.kst.s4),
            ("kst.signal", ind.kst.signal),
        ] {
            if p == 0 || p > 2_000 {
                return Err(HlError::InvalidStrategy(format!(
                    "indicators.{name} must be in 1..=2000 (got {p})"
                )));
            }
        }
        if !ind.bollinger.std_dev.is_finite()
            || ind.bollinger.std_dev <= 0.0
            || ind.bollinger.std_dev > 10.0
        {
            return Err(HlError::InvalidStrategy(
                "indicators.bollinger.std_dev must be finite and in (0.0, 10.0]".into(),
            ));
        }
        if !ind.supertrend.multiplier.is_finite()
            || ind.supertrend.multiplier <= 0.0
            || ind.supertrend.multiplier > 20.0
        {
            return Err(HlError::InvalidStrategy(
                "indicators.supertrend.multiplier must be finite and in (0.0, 20.0]".into(),
            ));
        }

        // Validate sizing & $12 minimum notional
        match config.sizing.mode {
            SizingMode::FixedUsd => {
                if !config.sizing.fixed_usd.is_finite()
                    || config.sizing.fixed_usd < MIN_ORDER_NOTIONAL_USD
                {
                    return Err(HlError::RiskViolation(format!(
                        "sizing.fixed_usd (${:.2}) is below minimum safe Hyperliquid notional (${:.2})",
                        config.sizing.fixed_usd, MIN_ORDER_NOTIONAL_USD
                    )));
                }
            }
            SizingMode::RiskPct => {
                if !config.sizing.risk_pct.is_finite()
                    || config.sizing.risk_pct <= 0.0
                    || config.sizing.risk_pct > 10.0
                {
                    return Err(HlError::RiskViolation(format!(
                        "sizing.risk_pct ({:.2}%) must be in (0.0, 10.0]",
                        config.sizing.risk_pct
                    )));
                }
                if !config.sizing.initial_equity_usd.is_finite()
                    || config.sizing.initial_equity_usd < 10.0
                {
                    return Err(HlError::RiskViolation(
                        "sizing.initial_equity_usd must be >= $10.0".into(),
                    ));
                }
            }
        }

        // Validate StopLossSpec invariants (prevent clamp(min, max) panic if min_stop_bps > max_stop_bps)
        if let StopLossSpec::Atr {
            multiplier,
            min_stop_bps,
            max_stop_bps,
        } = &config.exit.stop_loss
        {
            if !multiplier.is_finite() || *multiplier <= 0.0 || *multiplier > 20.0 {
                return Err(HlError::InvalidStrategy(
                    "exit.stop_loss.multiplier must be finite and in (0.0, 20.0]".into(),
                ));
            }
            if *min_stop_bps == 0 || min_stop_bps > max_stop_bps {
                return Err(HlError::InvalidStrategy(format!(
                    "exit.stop_loss.min_stop_bps ({min_stop_bps}) must be > 0 and <= max_stop_bps ({max_stop_bps})"
                )));
            }
        }

        // Validate Liquidation vs Stop-Loss distance
        let max_sl_bps = config.exit.stop_loss.max_stop_bps();
        if max_sl_bps == 0 {
            return Err(HlError::InvalidStrategy(
                "exit.stop_loss bps must be > 0".into(),
            ));
        }
        let liq_bps = estimated_liq_distance_bps(config.market.leverage);
        let safe_sl_cap_bps = (liq_bps * 70) / 100;
        if max_sl_bps > safe_sl_cap_bps {
            return Err(HlError::RiskViolation(format!(
                "stop_loss max ({max_sl_bps} bps) is too close to estimated {}x liquidation distance (~{liq_bps} bps); must be <= {safe_sl_cap_bps} bps (70% safety buffer)",
                config.market.leverage
            )));
        }

        // Validate R:R targets
        if !config.exit.take_profit_rr.is_finite()
            || config.exit.take_profit_rr <= 0.2
            || config.exit.take_profit_rr > 100.0
        {
            return Err(HlError::InvalidStrategy(
                "exit.take_profit_rr must be finite and in (0.2, 100.0]".into(),
            ));
        }
        if let Some(t1) = &config.exit.t1 {
            if !t1.rr.is_finite() || t1.rr <= 0.0 || t1.rr >= config.exit.take_profit_rr {
                return Err(HlError::InvalidStrategy(format!(
                    "exit.t1.rr ({:.2}) must be > 0 and < take_profit_rr ({:.2})",
                    t1.rr, config.exit.take_profit_rr
                )));
            }
            if !t1.close_pct.is_finite() || t1.close_pct < 0.0 || t1.close_pct >= 100.0 {
                return Err(HlError::InvalidStrategy(
                    "exit.t1.close_pct must be between 0 and 100 (exclusive); 0 = breakeven trigger only"
                        .into(),
                ));
            }
            if t1.close_pct == 0.0 && !t1.move_sl_to_breakeven {
                return Err(HlError::InvalidStrategy(
                    "exit.t1 with close_pct = 0 must set move_sl_to_breakeven = true".into(),
                ));
            }
        }

        for (i, target) in config.exit.targets.iter().enumerate() {
            if !target.rr.is_finite() || target.rr <= 0.0 {
                return Err(HlError::InvalidStrategy(format!(
                    "exit.targets[{}].rr ({:.2}) must be > 0",
                    i, target.rr
                )));
            }
            if !target.close_pct.is_finite() || target.close_pct <= 0.0 || target.close_pct > 100.0
            {
                return Err(HlError::InvalidStrategy(format!(
                    "exit.targets[{}].close_pct ({:.2}) must be in (0, 100]",
                    i, target.close_pct
                )));
            }
        }

        let compile_list = |list: &[String]| -> Result<Vec<CompiledCondition>, HlError> {
            list.iter().map(|s| CompiledCondition::parse(s)).collect()
        };

        let mut sessions = Vec::new();
        if let Some(s) = &config.session {
            if let (Some(start_raw), Some(end_raw)) = (&s.start, &s.end) {
                let start = parse_hhmm(start_raw)?;
                let end = parse_hhmm(end_raw)?;
                if start == end {
                    return Err(HlError::InvalidStrategy(
                        "session.start and session.end must differ".into(),
                    ));
                }
                sessions.push((start, end));
            } else if s.start.is_some() || s.end.is_some() {
                return Err(HlError::InvalidStrategy(
                    "session requires both 'start' and 'end' when configured as a single window"
                        .into(),
                ));
            }
            for w in &s.windows {
                let start = parse_hhmm(&w.start)?;
                let end = parse_hhmm(&w.end)?;
                if start == end {
                    return Err(HlError::InvalidStrategy(
                        "session window start and end must differ".into(),
                    ));
                }
                sessions.push((start, end));
            }
            if sessions.is_empty() {
                return Err(HlError::InvalidStrategy(
                    "session config has no valid windows; specify 'start'/'end' or 'windows = [...]'".into(),
                ));
            }
        }
        let session = sessions.first().copied();

        let long_all = compile_list(&config.entry.long.all)?;
        let long_any = compile_list(&config.entry.long.any)?;
        let short_all = compile_list(&config.entry.short.all)?;
        let short_any = compile_list(&config.entry.short.any)?;
        let long_exit_any = compile_list(&config.exit.long_exit_any)?;
        let short_exit_any = compile_list(&config.exit.short_exit_any)?;

        if config.entry.max_trades_per_window == Some(0) {
            return Err(HlError::InvalidStrategy(
                "entry.max_trades_per_window must be at least 1".into(),
            ));
        }
        if config.entry.max_losses_per_window == Some(0) {
            return Err(HlError::InvalidStrategy(
                "entry.max_losses_per_window must be at least 1".into(),
            ));
        }

        if config.market.direction.allows_long() && long_all.is_empty() && long_any.is_empty() {
            return Err(HlError::InvalidStrategy(
                "direction allows Long, but [entry.long] has no conditions".into(),
            ));
        }
        if config.market.direction.allows_short() && short_all.is_empty() && short_any.is_empty() {
            return Err(HlError::InvalidStrategy(
                "direction allows Short, but [entry.short] has no conditions".into(),
            ));
        }

        Ok(Self {
            config,
            session,
            sessions,
            long_all,
            long_any,
            short_all,
            short_any,
            long_exit_any,
            short_exit_any,
        })
    }

    /// True when a bar opening at `open_time_ms` falls inside the configured UK
    /// trading session. Always true when the strategy has no `[session]` block.
    pub fn in_session(&self, open_time_ms: u64) -> bool {
        if self.sessions.is_empty() {
            return true;
        }
        let minute = uk_minutes_of_day(open_time_ms);
        self.sessions.iter().any(|&(start, end)| {
            if start < end {
                minute >= start && minute < end
            } else {
                minute >= start || minute < end
            }
        })
    }

    /// Check if Long entry conditions hold at closed candle index `idx`.
    pub fn should_enter_long(
        &self,
        candles: &[Candle],
        table: &IndicatorTable,
        idx: usize,
    ) -> bool {
        if !self.config.market.direction.allows_long() {
            return false;
        }
        if !self.in_session(candles[idx].open_time_ms) {
            return false;
        }
        if self.long_all.is_empty() && self.long_any.is_empty() {
            return false;
        }
        let all_ok = self
            .long_all
            .iter()
            .all(|c| c.evaluate(candles, table, idx));
        let any_ok = self.long_any.is_empty()
            || self
                .long_any
                .iter()
                .any(|c| c.evaluate(candles, table, idx));
        all_ok && any_ok
    }

    /// Check if Short entry conditions hold at closed candle index `idx`.
    pub fn should_enter_short(
        &self,
        candles: &[Candle],
        table: &IndicatorTable,
        idx: usize,
    ) -> bool {
        if !self.config.market.direction.allows_short() {
            return false;
        }
        if !self.in_session(candles[idx].open_time_ms) {
            return false;
        }
        if self.short_all.is_empty() && self.short_any.is_empty() {
            return false;
        }
        let all_ok = self
            .short_all
            .iter()
            .all(|c| c.evaluate(candles, table, idx));
        let any_ok = self.short_any.is_empty()
            || self
                .short_any
                .iter()
                .any(|c| c.evaluate(candles, table, idx));
        all_ok && any_ok
    }

    /// Check if an open Long position should exit early on an indicator signal at `idx`.
    pub fn should_signal_exit_long(
        &self,
        candles: &[Candle],
        table: &IndicatorTable,
        idx: usize,
    ) -> bool {
        !self.long_exit_any.is_empty()
            && self
                .long_exit_any
                .iter()
                .any(|c| c.evaluate(candles, table, idx))
    }

    /// Check if an open Short position should exit early on an indicator signal at `idx`.
    pub fn should_signal_exit_short(
        &self,
        candles: &[Candle],
        table: &IndicatorTable,
        idx: usize,
    ) -> bool {
        !self.short_exit_any.is_empty()
            && self
                .short_exit_any
                .iter()
                .any(|c| c.evaluate(candles, table, idx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_multi_indicator_toml() {
        let toml = r#"
            name = "BTC 15m RSI + KST 3R"
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
            all = [
                "kst_line crosses_above kst_signal",
                "rsi > 30.0",
                "rsi < 55.0",
                "close > ema_trend",
            ]

            [entry.short]
            all = [
                "kst_line crosses_below kst_signal",
                "rsi < 70.0",
                "rsi > 45.0",
                "close < ema_trend",
            ]

            [exit]
            stop_loss = { kind = "atr", multiplier = 1.5, max_stop_bps = 250 }
            t1 = { rr = 1.5, close_pct = 50.0, move_sl_to_breakeven = true }
            take_profit_rr = 3.0
        "#;

        let compiled = CompiledStrategy::from_toml_str(toml).expect("valid strategy");
        assert_eq!(compiled.long_all.len(), 4);
        assert_eq!(compiled.short_all.len(), 4);
        assert_eq!(compiled.config.exit.take_profit_rr, 3.0);
    }

    #[test]
    fn parses_rsi_ma_condition_with_ema_smoothing() {
        let toml = r#"
            [market]
            coin = "BTC"
            interval = "15m"
            direction = "long_only"

            [indicators]
            rsi = { period = 14, ma_period = 9, ma_type = "ema" }

            [entry.long]
            all = ["rsi crosses_above rsi_ma", "rsi_ma > 50.0"]

            [exit]
            stop_loss = { kind = "fixed_bps", bps = 100 }
            take_profit_rr = 2.0
        "#;

        let compiled = CompiledStrategy::from_toml_str(toml).expect("valid strategy");
        assert_eq!(compiled.config.indicators.rsi.ma_period, 9);
        assert_eq!(
            compiled.config.indicators.rsi.ma_type,
            crate::indicators::MaType::Ema
        );
        assert_eq!(compiled.long_all[0].left, MetricId::Rsi);
        assert_eq!(
            compiled.long_all[0].right,
            RightOperand::Metric(MetricId::RsiMa)
        );
        assert_eq!(compiled.long_all[1].left, MetricId::RsiMa);
    }

    #[test]
    fn rejects_stop_loss_exceeding_liquidation_safety_buffer() {
        // At 20x leverage, liq_bps = 500 - 50 = 450 bps; 70% cap = 315 bps.
        // Setting stop_loss = 400 bps must fail!
        let toml = r#"
            [market]
            coin = "BTC"
            interval = "15m"
            leverage = 20
            direction = "long_only"

            [entry.long]
            all = ["rsi < 30.0"]

            [exit]
            stop_loss = { kind = "fixed_bps", bps = 400 }
            take_profit_rr = 2.0
        "#;

        let err = CompiledStrategy::from_toml_str(toml).unwrap_err();
        assert!(err.to_string().contains("liquidation distance"));
    }

    #[test]
    fn compiles_uk_session_multiple_windows() {
        let toml = r#"
            [market]
            coin = "xyz:CL"
            interval = "5m"
            direction = "long_only"

            [session]
            windows = [
                { start = "08:00", end = "13:00" },
                { start = "16:00", end = "20:00" },
            ]

            [entry.long]
            all = ["rsi < 30.0"]

            [exit]
            stop_loss = { kind = "fixed_bps", bps = 100 }
            take_profit_rr = 2.0
        "#;
        let compiled = CompiledStrategy::from_toml_str(toml).expect("valid multi-session strategy");
        assert_eq!(compiled.sessions, vec![(480, 780), (960, 1200)]);
        assert_eq!(compiled.session, Some((480, 780)));
    }

    #[test]
    fn compiles_uk_session_window() {
        let toml = r#"
            [market]
            coin = "BTC"
            interval = "15m"
            direction = "long_only"

            [session]
            start = "07:00"
            end = "13:00"

            [entry.long]
            all = ["rsi < 30.0"]

            [exit]
            stop_loss = { kind = "fixed_bps", bps = 100 }
            take_profit_rr = 2.0
        "#;
        let compiled = CompiledStrategy::from_toml_str(toml).expect("valid session strategy");
        assert_eq!(compiled.session, Some((420, 780)));
        // No session block means always tradable.
        let plain = CompiledStrategy::from_toml_str(&toml.replace(
            "[session]\n            start = \"07:00\"\n            end = \"13:00\"",
            "",
        ))
        .expect("valid strategy without session");
        assert_eq!(plain.session, None);
    }

    #[test]
    fn rejects_malformed_session_times() {
        let base = r#"
            [market]
            coin = "BTC"
            interval = "15m"
            direction = "long_only"

            [session]
            start = "{start}"
            end = "{end}"

            [entry.long]
            all = ["rsi < 30.0"]

            [exit]
            stop_loss = { kind = "fixed_bps", bps = 100 }
            take_profit_rr = 2.0
        "#;
        for (start, end) in [("7am", "13:00"), ("25:00", "13:00"), ("07:00", "07:00")] {
            let toml = base.replace("{start}", start).replace("{end}", end);
            assert!(
                CompiledStrategy::from_toml_str(&toml).is_err(),
                "expected '{start}'-'{end}' to be rejected"
            );
        }
    }

    #[test]
    fn rejects_inverted_stop_bounds_nan_constants_and_zero_periods() {
        // min_stop_bps > max_stop_bps would panic in f64::clamp if not caught!
        let inverted_sl = r#"
            [market]
            coin = "BTC"
            interval = "15m"
            direction = "long_only"
            [entry.long]
            all = ["rsi < 30.0"]
            [exit]
            stop_loss = { kind = "atr", multiplier = 1.5, min_stop_bps = 300, max_stop_bps = 100 }
            take_profit_rr = 2.0
        "#;
        assert!(CompiledStrategy::from_toml_str(inverted_sl).is_err());

        // NaN constant in condition expression
        let nan_cond = r#"
            [market]
            coin = "BTC"
            interval = "15m"
            direction = "long_only"
            [entry.long]
            all = ["rsi < NaN"]
            [exit]
            stop_loss = { kind = "fixed_bps", bps = 100 }
            take_profit_rr = 2.0
        "#;
        assert!(CompiledStrategy::from_toml_str(nan_cond).is_err());

        // Zero period in indicator config
        let zero_period = r#"
            [market]
            coin = "BTC"
            interval = "15m"
            direction = "long_only"
            [indicators]
            rsi = { period = 0, ma_period = 14 }
            [entry.long]
            all = ["rsi < 30.0"]
            [exit]
            stop_loss = { kind = "fixed_bps", bps = 100 }
            take_profit_rr = 2.0
        "#;
        assert!(CompiledStrategy::from_toml_str(zero_period).is_err());
    }

    #[test]
    fn parses_auto_reinvest_defaults_btc_meme() {
        let toml_str = r#"
            [market]
            coin = "BTC"
            interval = "15m"
            direction = "long_only"
            [entry.long]
            all = ["rsi < 30.0"]
            [exit]
            stop_loss = { kind = "fixed_bps", bps = 100 }
            take_profit_rr = 2.0

            [auto_reinvest]
            enabled = true
        "#;
        let compiled = CompiledStrategy::from_toml_str(toml_str).unwrap();
        let reinvest = compiled
            .config
            .auto_reinvest
            .expect("should have auto_reinvest");
        assert!(reinvest.enabled);
        assert_eq!(reinvest.profit_pct, 20.0);
        assert_eq!(
            reinvest.target_token,
            "0xF7bf2A938f971D7e4811A1170C43d651d21A0F81"
        );
        assert_eq!(reinvest.target_token_symbol, "BTC");
        assert_eq!(reinvest.target_chain_id, 369);
        assert_eq!(reinvest.cadence, ReinvestCadence::Weekly);
        assert_eq!(reinvest.min_profit_threshold_usd, 25.0);
        assert_eq!(reinvest.max_slippage_bps, 100);
        assert_eq!(reinvest.route, "auto");
    }

    #[test]
    fn parses_custom_auto_reinvest_settings() {
        let toml_str = r#"
            [market]
            coin = "ETH"
            interval = "1h"
            direction = "long_only"
            [entry.long]
            all = ["rsi < 35.0"]
            [exit]
            stop_loss = { kind = "fixed_bps", bps = 150 }
            take_profit_rr = 3.0

            [auto_reinvest]
            enabled = true
            profit_pct = 35.0
            target_token = "0x8a810ea8B0Aeb6d003a091736154777c8bAc828d"
            target_token_symbol = "PLSX"
            cadence = "daily"
            min_profit_threshold_usd = 10.0
            max_slippage_bps = 50
        "#;
        let compiled = CompiledStrategy::from_toml_str(toml_str).unwrap();
        let reinvest = compiled
            .config
            .auto_reinvest
            .expect("should have custom auto_reinvest");
        assert!(reinvest.enabled);
        assert_eq!(reinvest.profit_pct, 35.0);
        assert_eq!(
            reinvest.target_token,
            "0x8a810ea8B0Aeb6d003a091736154777c8bAc828d"
        );
        assert_eq!(reinvest.target_token_symbol, "PLSX");
        assert_eq!(reinvest.cadence, ReinvestCadence::Daily);
        assert_eq!(reinvest.min_profit_threshold_usd, 10.0);
        assert_eq!(reinvest.max_slippage_bps, 50);
    }
}
