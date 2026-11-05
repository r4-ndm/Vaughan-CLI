//! Canonical Multi-Indicator Suite for `vaughan-hl`.
//!
//! Implements TradingView / TA-Lib compatible series computations:
//! - Wilder's SMA-seeded Relative Strength Index (`RSI`) and an optional moving
//!   average of the RSI line (`rsi_ma`)
//! - Martin Pring's Know Sure Thing (`KST`: `kst_line`, `kst_signal`, `kst_hist`)
//! - Exponential & Simple Moving Averages (`EMA`, `SMA`)
//! - Moving Average Convergence Divergence (`MACD`: `macd_line`, `macd_signal`, `macd_hist`)
//! - Wilder's Average True Range (`ATR`)
//! - Wilder's Average Directional Index (`ADX`, `plus_di`, `minus_di`)
//! - Bollinger Bands (`bb_upper`, `bb_middle`, `bb_lower`, `bb_pct_b`)
//! - Stochastic RSI (`stoch_k`, `stoch_d`)
//! - Volume SMA Ratio (`vol_ratio`)

use serde::{Deserialize, Serialize};

use crate::market_data::{interval_duration_ms, Candle};

/// Time-series of optional indicator values aligned 1:1 with candles.
pub type OptSeries = Vec<Option<f64>>;

/// Configuration for all indicators evaluated by a strategy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndicatorConfig {
    #[serde(default = "default_rsi")]
    pub rsi: RsiParams,
    #[serde(default = "default_kst")]
    pub kst: KstParams,
    #[serde(default = "default_ema_fast")]
    pub ema_fast: PeriodParam,
    #[serde(default = "default_ema_trend")]
    pub ema_trend: PeriodParam,
    #[serde(default = "default_macd")]
    pub macd: MacdParams,
    #[serde(default = "default_adx")]
    pub adx: PeriodParam,
    #[serde(default = "default_atr")]
    pub atr: PeriodParam,
    #[serde(default = "default_stoch_rsi")]
    pub stoch_rsi: StochRsiParams,
    #[serde(default = "default_connors_rsi")]
    pub connors_rsi: ConnorsRsiParams,
    #[serde(default = "default_supertrend")]
    pub supertrend: SupertrendParams,
    #[serde(default = "default_bollinger")]
    pub bollinger: BollingerParams,
    #[serde(default = "default_vol_ratio")]
    pub vol_ratio: PeriodParam,
    /// Optional higher-timeframe StochRSI read on the base chart's bars.
    #[serde(default)]
    pub htf: Option<HtfParams>,
    /// Second, independent higher-timeframe StochRSI filter (`htf2_*` metrics).
    /// Use it when the two filters need different timeframes, e.g. 4h + 1w.
    #[serde(default)]
    pub htf2: Option<HtfParams>,
}

/// Higher-timeframe companion series. Candles are resampled from the base
/// interval, StochRSI is computed on the resampled closes, and the result is
/// stepped onto the base timeline only once each higher-timeframe bar closes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HtfParams {
    pub interval: String,
    #[serde(default = "default_stoch_rsi")]
    pub stoch_rsi: StochRsiParams,
    /// Optional candle interval to build this higher timeframe from. The base
    /// chart only spans a few weeks, so a weekly or 3-day series has to come
    /// from its own (older, coarser) candle data instead of being resampled.
    /// When unset the base chart's candles are resampled, as before.
    #[serde(default)]
    pub source: Option<String>,
}

impl Default for IndicatorConfig {
    fn default() -> Self {
        Self {
            rsi: default_rsi(),
            kst: default_kst(),
            ema_fast: default_ema_fast(),
            ema_trend: default_ema_trend(),
            macd: default_macd(),
            adx: default_adx(),
            atr: default_atr(),
            stoch_rsi: default_stoch_rsi(),
            connors_rsi: default_connors_rsi(),
            supertrend: default_supertrend(),
            bollinger: default_bollinger(),
            vol_ratio: default_vol_ratio(),
            htf: None,
            htf2: None,
        }
    }
}

impl IndicatorConfig {
    /// Minimum number of closed candles required so Wilder/EMA/KST series have
    /// fully warmed up without seed drift (`>= 5 * wilder_period` and `>= ema_trend + 50`).
    pub fn required_warmup_bars(&self) -> usize {
        let wilder_max = self
            .rsi
            .period
            .max(self.rsi.ma_period)
            .max(self.atr.period)
            .max(self.adx.period)
            .max(self.stoch_rsi.rsi_period)
            .max(self.supertrend.atr_period)
            * 5;
        let kst_min = self.kst.r4 + self.kst.s4 + self.kst.signal + 20;
        let ema_min = self.ema_trend.period.max(self.ema_fast.period) + 30;
        let crsi_min = self.connors_rsi.rank_period + self.connors_rsi.rsi_period + 10;
        wilder_max.max(kst_min).max(ema_min).max(crsi_min).max(60)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RsiParams {
    pub period: usize,
    /// Period of the moving average applied to the RSI line (`rsi_ma`).
    #[serde(default = "default_rsi_ma_period")]
    pub ma_period: usize,
    /// Moving-average type applied to the RSI line.
    #[serde(default)]
    pub ma_type: MaType,
}

/// Smoothing type for the moving average applied to the RSI line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaType {
    #[default]
    Sma,
    Ema,
}

impl MaType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sma => "SMA",
            Self::Ema => "EMA",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeriodParam {
    pub period: usize,
}

/// Martin Pring's Know Sure Thing (`KST`) parameters.
/// Standard TradingView defaults: `(10, 15, 20, 30, 10, 10, 10, 15, 9)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KstParams {
    pub r1: usize,
    pub r2: usize,
    pub r3: usize,
    pub r4: usize,
    pub s1: usize,
    pub s2: usize,
    pub s3: usize,
    pub s4: usize,
    pub signal: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacdParams {
    pub fast: usize,
    pub slow: usize,
    pub signal: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StochRsiParams {
    pub rsi_period: usize,
    pub stoch_period: usize,
    pub k_smooth: usize,
    pub d_smooth: usize,
}

/// SuperTrend parameters (`atr_period`, `multiplier`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SupertrendParams {
    #[serde(default = "default_st_atr_period")]
    pub atr_period: usize,
    #[serde(default = "default_st_multiplier")]
    pub multiplier: f64,
}

/// Larry Connors' Composite RSI parameters (defaults `3 / 2 / 100`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnorsRsiParams {
    #[serde(default = "default_crsi_rsi_period")]
    pub rsi_period: usize,
    #[serde(default = "default_crsi_streak_period")]
    pub streak_period: usize,
    #[serde(default = "default_crsi_rank_period")]
    pub rank_period: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BollingerParams {
    pub period: usize,
    pub std_dev: f64,
}

fn default_rsi() -> RsiParams {
    RsiParams {
        period: 14,
        ma_period: 14,
        ma_type: MaType::Sma,
    }
}

fn default_rsi_ma_period() -> usize {
    14
}

fn default_kst() -> KstParams {
    KstParams {
        r1: 10,
        r2: 15,
        r3: 20,
        r4: 30,
        s1: 10,
        s2: 10,
        s3: 10,
        s4: 15,
        signal: 9,
    }
}

fn default_ema_fast() -> PeriodParam {
    PeriodParam { period: 21 }
}

fn default_ema_trend() -> PeriodParam {
    PeriodParam { period: 200 }
}

fn default_macd() -> MacdParams {
    MacdParams {
        fast: 12,
        slow: 26,
        signal: 9,
    }
}

fn default_adx() -> PeriodParam {
    PeriodParam { period: 14 }
}

fn default_atr() -> PeriodParam {
    PeriodParam { period: 14 }
}

fn default_stoch_rsi() -> StochRsiParams {
    StochRsiParams {
        rsi_period: 14,
        stoch_period: 14,
        k_smooth: 3,
        d_smooth: 3,
    }
}

fn default_supertrend() -> SupertrendParams {
    SupertrendParams {
        atr_period: 10,
        multiplier: 3.0,
    }
}

fn default_st_atr_period() -> usize {
    10
}

fn default_st_multiplier() -> f64 {
    3.0
}

fn default_connors_rsi() -> ConnorsRsiParams {
    ConnorsRsiParams {
        rsi_period: 3,
        streak_period: 2,
        rank_period: 100,
    }
}

fn default_crsi_rsi_period() -> usize {
    3
}

fn default_crsi_streak_period() -> usize {
    2
}

fn default_crsi_rank_period() -> usize {
    100
}

fn default_bollinger() -> BollingerParams {
    BollingerParams {
        period: 20,
        std_dev: 2.0,
    }
}

fn default_vol_ratio() -> PeriodParam {
    PeriodParam { period: 20 }
}

/// Named metrics available to `strategy.toml` expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricId {
    Close,
    Open,
    High,
    Low,
    Volume,
    Rsi,
    RsiMa,
    KstLine,
    KstSignal,
    KstHist,
    EmaFast,
    EmaTrend,
    MacdLine,
    MacdSignal,
    MacdHist,
    Adx,
    PlusDi,
    MinusDi,
    Atr,
    BbUpper,
    BbMiddle,
    BbLower,
    BbPctB,
    StochK,
    StochD,
    HtfStochK,
    HtfStochD,
    /// Slope of the higher-timeframe `%K` at its last close: `1.0` rising,
    /// `-1.0` falling, held for the whole higher-timeframe bucket.
    HtfStochKSlope,
    Htf2StochK,
    Htf2StochD,
    Htf2StochKSlope,
    ConnorsRsi,
    Supertrend,
    SupertrendDir,
    VolRatio,
}

impl MetricId {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "close" | "price" => Some(Self::Close),
            "open" => Some(Self::Open),
            "high" => Some(Self::High),
            "low" => Some(Self::Low),
            "volume" | "vol" => Some(Self::Volume),
            "rsi" => Some(Self::Rsi),
            "rsi_ma" | "rsi_ma_line" | "rsi_signal" => Some(Self::RsiMa),
            "kst" | "kst_line" => Some(Self::KstLine),
            "kst_signal" | "kst_sig" => Some(Self::KstSignal),
            "kst_hist" | "kst_histogram" => Some(Self::KstHist),
            "ema_fast" => Some(Self::EmaFast),
            "ema_trend" | "ema" => Some(Self::EmaTrend),
            "macd" | "macd_line" => Some(Self::MacdLine),
            "macd_signal" | "macd_sig" => Some(Self::MacdSignal),
            "macd_hist" | "macd_histogram" => Some(Self::MacdHist),
            "adx" => Some(Self::Adx),
            "plus_di" | "+di" => Some(Self::PlusDi),
            "minus_di" | "-di" => Some(Self::MinusDi),
            "atr" => Some(Self::Atr),
            "bb_upper" => Some(Self::BbUpper),
            "bb_middle" | "bb_mid" => Some(Self::BbMiddle),
            "bb_lower" => Some(Self::BbLower),
            "bb_pct_b" | "pct_b" => Some(Self::BbPctB),
            "stoch_k" | "stochrsi_k" => Some(Self::StochK),
            "stoch_d" | "stochrsi_d" => Some(Self::StochD),
            "htf_stoch_k" | "htf_stochrsi_k" => Some(Self::HtfStochK),
            "htf_stoch_d" | "htf_stochrsi_d" => Some(Self::HtfStochD),
            "htf_stoch_k_slope" | "htf_stoch_slope" | "htf_stoch_rising" => {
                Some(Self::HtfStochKSlope)
            }
            "htf2_stoch_k" | "htf2_stochrsi_k" => Some(Self::Htf2StochK),
            "htf2_stoch_d" | "htf2_stochrsi_d" => Some(Self::Htf2StochD),
            "htf2_stoch_k_slope" | "htf2_stoch_slope" | "htf2_stoch_rising" => {
                Some(Self::Htf2StochKSlope)
            }
            "connors_rsi" | "crsi" | "connors" => Some(Self::ConnorsRsi),
            "supertrend" | "st" => Some(Self::Supertrend),
            "supertrend_dir" | "st_dir" | "supertrend_direction" => Some(Self::SupertrendDir),
            "vol_ratio" | "volume_ratio" => Some(Self::VolRatio),
            _ => None,
        }
    }
}

/// Precomputed indicator series aligned 1:1 with a `&[Candle]` slice.
#[derive(Debug, Clone)]
pub struct IndicatorTable {
    pub rsi: Vec<Option<f64>>,
    pub rsi_ma: Vec<Option<f64>>,
    pub kst_line: Vec<Option<f64>>,
    pub kst_signal: Vec<Option<f64>>,
    pub kst_hist: Vec<Option<f64>>,
    pub ema_fast: Vec<Option<f64>>,
    pub ema_trend: Vec<Option<f64>>,
    pub macd_line: Vec<Option<f64>>,
    pub macd_signal: Vec<Option<f64>>,
    pub macd_hist: Vec<Option<f64>>,
    pub adx: Vec<Option<f64>>,
    pub plus_di: Vec<Option<f64>>,
    pub minus_di: Vec<Option<f64>>,
    pub atr: Vec<Option<f64>>,
    pub bb_upper: Vec<Option<f64>>,
    pub bb_middle: Vec<Option<f64>>,
    pub bb_lower: Vec<Option<f64>>,
    pub bb_pct_b: Vec<Option<f64>>,
    pub stoch_k: Vec<Option<f64>>,
    pub stoch_d: Vec<Option<f64>>,
    pub htf_stoch_k: Vec<Option<f64>>,
    pub htf_stoch_d: Vec<Option<f64>>,
    /// Sign of the higher-timeframe `%K` change at its last close (held for the
    /// bucket): `1.0` rising, `-1.0` falling, `0.0` flat.
    pub htf_stoch_k_slope: Vec<Option<f64>>,
    /// Second higher-timeframe block (`[indicators.htf2]`).
    pub htf2_stoch_k: Vec<Option<f64>>,
    pub htf2_stoch_d: Vec<Option<f64>>,
    pub htf2_stoch_k_slope: Vec<Option<f64>>,
    pub connors_rsi: Vec<Option<f64>>,
    pub supertrend: Vec<Option<f64>>,
    pub supertrend_dir: Vec<Option<f64>>,
    pub vol_ratio: Vec<Option<f64>>,
}

impl IndicatorTable {
    /// Precompute all configured indicator series over `candles` in $O(N)$ time.
    pub fn compute(candles: &[Candle], cfg: &IndicatorConfig) -> Self {
        Self::compute_with_source(candles, cfg, None)
    }

    /// Like [`Self::compute`], but lets a higher-timeframe block that declares
    /// `source = "<interval>"` read `htf_source` (candles of that interval)
    /// instead of resampling the base chart's much shorter history.
    pub fn compute_with_source(
        candles: &[Candle],
        cfg: &IndicatorConfig,
        htf_source: Option<&[Candle]>,
    ) -> Self {
        let closes: Vec<f64> = candles.iter().map(|c| c.close).collect();
        let volumes: Vec<f64> = candles.iter().map(|c| c.volume).collect();

        let rsi = compute_rsi_series(&closes, cfg.rsi.period);
        let rsi_ma = compute_rsi_ma_series(&rsi, &cfg.rsi);
        let (kst_line, kst_signal, kst_hist) = compute_kst_series(&closes, &cfg.kst);
        let ema_fast = compute_ema_series(&closes, cfg.ema_fast.period);
        let ema_trend = compute_ema_series(&closes, cfg.ema_trend.period);
        let (macd_line, macd_signal, macd_hist) = compute_macd_series(&closes, &cfg.macd);
        let (adx, plus_di, minus_di) = compute_adx_series(candles, cfg.adx.period);
        let atr = compute_atr_series(candles, cfg.atr.period);
        let (bb_upper, bb_middle, bb_lower, bb_pct_b) =
            compute_bollinger_series(&closes, cfg.bollinger.period, cfg.bollinger.std_dev);
        let (stoch_k, stoch_d) = compute_stoch_rsi_series(&closes, &cfg.stoch_rsi);
        let (htf_stoch_k, htf_stoch_d, htf_stoch_k_slope) =
            htf_block_series(candles, cfg.htf.as_ref(), htf_source);
        let (htf2_stoch_k, htf2_stoch_d, htf2_stoch_k_slope) =
            htf_block_series(candles, cfg.htf2.as_ref(), htf_source);
        let connors_rsi = compute_connors_rsi_series(&closes, &cfg.connors_rsi);
        let (supertrend, supertrend_dir) = compute_supertrend_series(candles, &cfg.supertrend);
        let vol_ratio = compute_volume_ratio_series(&volumes, cfg.vol_ratio.period);

        Self {
            rsi,
            rsi_ma,
            kst_line,
            kst_signal,
            kst_hist,
            ema_fast,
            ema_trend,
            macd_line,
            macd_signal,
            macd_hist,
            adx,
            plus_di,
            minus_di,
            atr,
            bb_upper,
            bb_middle,
            bb_lower,
            bb_pct_b,
            stoch_k,
            stoch_d,
            htf_stoch_k,
            htf_stoch_d,
            htf_stoch_k_slope,
            htf2_stoch_k,
            htf2_stoch_d,
            htf2_stoch_k_slope,
            connors_rsi,
            supertrend,
            supertrend_dir,
            vol_ratio,
        }
    }

    /// Retrieve the value of `metric` at bar index `idx`.
    pub fn get(&self, candles: &[Candle], idx: usize, metric: MetricId) -> Option<f64> {
        let c = candles.get(idx)?;
        match metric {
            MetricId::Close => Some(c.close),
            MetricId::Open => Some(c.open),
            MetricId::High => Some(c.high),
            MetricId::Low => Some(c.low),
            MetricId::Volume => Some(c.volume),
            MetricId::Rsi => self.rsi.get(idx).copied().flatten(),
            MetricId::RsiMa => self.rsi_ma.get(idx).copied().flatten(),
            MetricId::KstLine => self.kst_line.get(idx).copied().flatten(),
            MetricId::KstSignal => self.kst_signal.get(idx).copied().flatten(),
            MetricId::KstHist => self.kst_hist.get(idx).copied().flatten(),
            MetricId::EmaFast => self.ema_fast.get(idx).copied().flatten(),
            MetricId::EmaTrend => self.ema_trend.get(idx).copied().flatten(),
            MetricId::MacdLine => self.macd_line.get(idx).copied().flatten(),
            MetricId::MacdSignal => self.macd_signal.get(idx).copied().flatten(),
            MetricId::MacdHist => self.macd_hist.get(idx).copied().flatten(),
            MetricId::Adx => self.adx.get(idx).copied().flatten(),
            MetricId::PlusDi => self.plus_di.get(idx).copied().flatten(),
            MetricId::MinusDi => self.minus_di.get(idx).copied().flatten(),
            MetricId::Atr => self.atr.get(idx).copied().flatten(),
            MetricId::BbUpper => self.bb_upper.get(idx).copied().flatten(),
            MetricId::BbMiddle => self.bb_middle.get(idx).copied().flatten(),
            MetricId::BbLower => self.bb_lower.get(idx).copied().flatten(),
            MetricId::BbPctB => self.bb_pct_b.get(idx).copied().flatten(),
            MetricId::StochK => self.stoch_k.get(idx).copied().flatten(),
            MetricId::StochD => self.stoch_d.get(idx).copied().flatten(),
            MetricId::HtfStochK => self.htf_stoch_k.get(idx).copied().flatten(),
            MetricId::HtfStochD => self.htf_stoch_d.get(idx).copied().flatten(),
            MetricId::HtfStochKSlope => self.htf_stoch_k_slope.get(idx).copied().flatten(),
            MetricId::Htf2StochK => self.htf2_stoch_k.get(idx).copied().flatten(),
            MetricId::Htf2StochD => self.htf2_stoch_d.get(idx).copied().flatten(),
            MetricId::Htf2StochKSlope => self.htf2_stoch_k_slope.get(idx).copied().flatten(),
            MetricId::ConnorsRsi => self.connors_rsi.get(idx).copied().flatten(),
            MetricId::Supertrend => self.supertrend.get(idx).copied().flatten(),
            MetricId::SupertrendDir => self.supertrend_dir.get(idx).copied().flatten(),
            MetricId::VolRatio => self.vol_ratio.get(idx).copied().flatten(),
        }
    }
}

/// Simple Moving Average (`SMA`) over `values`.
pub fn compute_sma_series(values: &[f64], period: usize) -> Vec<Option<f64>> {
    let n = values.len();
    if period == 0 || n < period {
        return vec![None; n];
    }
    let mut out = vec![None; n];
    let mut sum: f64 = values[..period].iter().sum();
    out[period - 1] = Some(sum / period as f64);
    for i in period..n {
        sum += values[i] - values[i - period];
        out[i] = Some(sum / period as f64);
    }
    out
}

/// SMA over an `Option<f64>` series (starts once `period` consecutive `Some` values exist).
pub fn compute_sma_opt_series(values: &[Option<f64>], period: usize) -> Vec<Option<f64>> {
    let n = values.len();
    if period == 0 || n < period {
        return vec![None; n];
    }
    let mut out = vec![None; n];
    let mut window_sum = 0.0;
    let mut valid_count = 0usize;

    for i in 0..n {
        if let Some(v) = values[i] {
            window_sum += v;
            valid_count += 1;
            if valid_count > period {
                if let Some(old) = values[i - period] {
                    window_sum -= old;
                }
            }
            if valid_count >= period {
                out[i] = Some(window_sum / period as f64);
            }
        } else {
            window_sum = 0.0;
            valid_count = 0;
        }
    }
    out
}

/// Exponential Moving Average (`EMA`) seeded with the SMA of the first `period` values.
pub fn compute_ema_series(values: &[f64], period: usize) -> Vec<Option<f64>> {
    let n = values.len();
    if period == 0 || n < period {
        return vec![None; n];
    }
    let mut out = vec![None; n];
    let sma_seed: f64 = values[..period].iter().sum::<f64>() / period as f64;
    let k = 2.0 / (period as f64 + 1.0);
    let mut ema = sma_seed;
    out[period - 1] = Some(ema);
    for i in period..n {
        ema = (values[i] - ema) * k + ema;
        out[i] = Some(ema);
    }
    out
}

/// Wilder's SMA-seeded Relative Strength Index (`RSI`), matching TradingView & TA-Lib.
pub fn compute_rsi_series(closes: &[f64], period: usize) -> Vec<Option<f64>> {
    let n = closes.len();
    if period == 0 || n <= period {
        return vec![None; n];
    }
    let mut out = vec![None; n];
    let mut gain_sum = 0.0;
    let mut loss_sum = 0.0;

    for i in 1..=period {
        let diff = closes[i] - closes[i - 1];
        if diff >= 0.0 {
            gain_sum += diff;
        } else {
            loss_sum -= diff;
        }
    }

    let p = period as f64;
    let mut avg_gain = gain_sum / p;
    let mut avg_loss = loss_sum / p;

    let calc_rsi = |ag: f64, al: f64| -> f64 {
        if al == 0.0 {
            if ag == 0.0 {
                50.0
            } else {
                100.0
            }
        } else {
            let rs = ag / al;
            100.0 - (100.0 / (1.0 + rs))
        }
    };

    out[period] = Some(calc_rsi(avg_gain, avg_loss));

    for i in (period + 1)..n {
        let diff = closes[i] - closes[i - 1];
        let (gain, loss) = if diff >= 0.0 {
            (diff, 0.0)
        } else {
            (0.0, -diff)
        };
        avg_gain = (avg_gain * (p - 1.0) + gain) / p;
        avg_loss = (avg_loss * (p - 1.0) + loss) / p;
        out[i] = Some(calc_rsi(avg_gain, avg_loss));
    }

    out
}

/// EMA over an `Option<f64>` series, seeded with the mean of the first `period`
/// consecutive `Some` values (mirrors [`compute_ema_series`] on a sparse series).
pub fn compute_ema_opt_series(values: &[Option<f64>], period: usize) -> OptSeries {
    let n = values.len();
    if period == 0 {
        return vec![None; n];
    }
    let mut out = vec![None; n];
    let k = 2.0 / (period as f64 + 1.0);
    let mut ema: Option<f64> = None;
    let mut seed_sum = 0.0;
    let mut seed_count = 0usize;

    for i in 0..n {
        let Some(v) = values[i] else {
            continue;
        };
        match ema {
            None => {
                seed_sum += v;
                seed_count += 1;
                if seed_count >= period {
                    let seed = seed_sum / period as f64;
                    ema = Some(seed);
                    out[i] = Some(seed);
                }
            }
            Some(prev) => {
                let next = (v - prev) * k + prev;
                ema = Some(next);
                out[i] = Some(next);
            }
        }
    }
    out
}

/// Moving average (`SMA` or `EMA`) of the RSI line, exposed as the `rsi_ma` metric.
pub fn compute_rsi_ma_series(rsi: &[Option<f64>], params: &RsiParams) -> OptSeries {
    let period = params.ma_period.max(1);
    match params.ma_type {
        MaType::Sma => compute_sma_opt_series(rsi, period),
        MaType::Ema => compute_ema_opt_series(rsi, period),
    }
}

/// Rate of Change (`ROC`) percentage series: `((close[i] - close[i - period]) / close[i - period]) * 100.0`.
pub fn compute_roc_series(closes: &[f64], period: usize) -> Vec<Option<f64>> {
    let n = closes.len();
    if period == 0 || n <= period {
        return vec![None; n];
    }
    let mut out = vec![None; n];
    for i in period..n {
        let prev = closes[i - period];
        if prev.abs() > f64::EPSILON {
            out[i] = Some(((closes[i] - prev) / prev) * 100.0);
        }
    }
    out
}

/// Martin Pring's Know Sure Thing (`KST`) oscillator:
/// `KST = 1 * SMA(ROC(r1), s1) + 2 * SMA(ROC(r2), s2) + 3 * SMA(ROC(r3), s3) + 4 * SMA(ROC(r4), s4)`
/// `Signal = SMA(KST, sig)`
/// `Hist = KST - Signal`
pub fn compute_kst_series(closes: &[f64], params: &KstParams) -> (OptSeries, OptSeries, OptSeries) {
    let n = closes.len();
    let roc1 = compute_roc_series(closes, params.r1);
    let roc2 = compute_roc_series(closes, params.r2);
    let roc3 = compute_roc_series(closes, params.r3);
    let roc4 = compute_roc_series(closes, params.r4);

    let rcma1 = compute_sma_opt_series(&roc1, params.s1);
    let rcma2 = compute_sma_opt_series(&roc2, params.s2);
    let rcma3 = compute_sma_opt_series(&roc3, params.s3);
    let rcma4 = compute_sma_opt_series(&roc4, params.s4);

    let mut kst_line = vec![None; n];
    for i in 0..n {
        if let (Some(a), Some(b), Some(c), Some(d)) = (rcma1[i], rcma2[i], rcma3[i], rcma4[i]) {
            kst_line[i] = Some(a + 2.0 * b + 3.0 * c + 4.0 * d);
        }
    }

    let kst_signal = compute_sma_opt_series(&kst_line, params.signal);
    let mut kst_hist = vec![None; n];
    for i in 0..n {
        if let (Some(k), Some(s)) = (kst_line[i], kst_signal[i]) {
            kst_hist[i] = Some(k - s);
        }
    }

    (kst_line, kst_signal, kst_hist)
}

/// MACD (`fast_ema - slow_ema`, signal EMA of MACD, and histogram).
pub fn compute_macd_series(
    closes: &[f64],
    params: &MacdParams,
) -> (OptSeries, OptSeries, OptSeries) {
    let n = closes.len();
    let fast = compute_ema_series(closes, params.fast);
    let slow = compute_ema_series(closes, params.slow);

    let mut macd_line = vec![None; n];
    let mut valid_macd = Vec::new();
    let mut first_valid_idx = None;

    for i in 0..n {
        if let (Some(f), Some(s)) = (fast[i], slow[i]) {
            let m = f - s;
            macd_line[i] = Some(m);
            if first_valid_idx.is_none() {
                first_valid_idx = Some(i);
            }
            valid_macd.push(m);
        }
    }

    let mut macd_signal = vec![None; n];
    let mut macd_hist = vec![None; n];

    if let Some(start_idx) = first_valid_idx {
        let sig_sub = compute_ema_series(&valid_macd, params.signal);
        for (offset, sig_opt) in sig_sub.into_iter().enumerate() {
            let idx = start_idx + offset;
            macd_signal[idx] = sig_opt;
            if let (Some(m), Some(s)) = (macd_line[idx], sig_opt) {
                macd_hist[idx] = Some(m - s);
            }
        }
    }

    (macd_line, macd_signal, macd_hist)
}

/// Wilder's Average True Range (`ATR`).
pub fn compute_atr_series(candles: &[Candle], period: usize) -> Vec<Option<f64>> {
    let n = candles.len();
    if period == 0 || n <= period {
        return vec![None; n];
    }
    let mut tr = vec![0.0; n];
    tr[0] = (candles[0].high - candles[0].low).max(0.0);
    for i in 1..n {
        let hl = candles[i].high - candles[i].low;
        let hc = (candles[i].high - candles[i - 1].close).abs();
        let lc = (candles[i].low - candles[i - 1].close).abs();
        tr[i] = hl.max(hc).max(lc);
    }

    let mut out = vec![None; n];
    let p = period as f64;
    let mut atr: f64 = tr[1..=period].iter().sum::<f64>() / p;
    out[period] = Some(atr);
    for i in (period + 1)..n {
        atr = (atr * (p - 1.0) + tr[i]) / p;
        out[i] = Some(atr);
    }
    out
}

/// Wilder's Average Directional Index (`ADX`, `+DI`, `-DI`).
pub fn compute_adx_series(candles: &[Candle], period: usize) -> (OptSeries, OptSeries, OptSeries) {
    let n = candles.len();
    if period == 0 || n <= period * 2 {
        return (vec![None; n], vec![None; n], vec![None; n]);
    }

    let mut tr = vec![0.0; n];
    let mut plus_dm = vec![0.0; n];
    let mut minus_dm = vec![0.0; n];

    for i in 1..n {
        let hl = candles[i].high - candles[i].low;
        let hc = (candles[i].high - candles[i - 1].close).abs();
        let lc = (candles[i].low - candles[i - 1].close).abs();
        tr[i] = hl.max(hc).max(lc);

        let up_move = candles[i].high - candles[i - 1].high;
        let down_move = candles[i - 1].low - candles[i].low;

        if up_move > down_move && up_move > 0.0 {
            plus_dm[i] = up_move;
        }
        if down_move > up_move && down_move > 0.0 {
            minus_dm[i] = down_move;
        }
    }

    let p = period as f64;
    let mut sm_tr: f64 = tr[1..=period].iter().sum();
    let mut sm_plus: f64 = plus_dm[1..=period].iter().sum();
    let mut sm_minus: f64 = minus_dm[1..=period].iter().sum();

    let mut plus_di = vec![None; n];
    let mut minus_di = vec![None; n];
    let mut dx = vec![None; n];

    let calc_di_dx = |tr_val: f64, p_val: f64, m_val: f64| -> (f64, f64, f64) {
        if tr_val <= f64::EPSILON {
            return (0.0, 0.0, 0.0);
        }
        let pdi = 100.0 * (p_val / tr_val);
        let mdi = 100.0 * (m_val / tr_val);
        let sum = pdi + mdi;
        let d = if sum <= f64::EPSILON {
            0.0
        } else {
            100.0 * ((pdi - mdi).abs() / sum)
        };
        (pdi, mdi, d)
    };

    let (pdi0, mdi0, dx0) = calc_di_dx(sm_tr, sm_plus, sm_minus);
    plus_di[period] = Some(pdi0);
    minus_di[period] = Some(mdi0);
    dx[period] = Some(dx0);

    for i in (period + 1)..n {
        sm_tr = sm_tr - (sm_tr / p) + tr[i];
        sm_plus = sm_plus - (sm_plus / p) + plus_dm[i];
        sm_minus = sm_minus - (sm_minus / p) + minus_dm[i];
        let (pdi, mdi, d) = calc_di_dx(sm_tr, sm_plus, sm_minus);
        plus_di[i] = Some(pdi);
        minus_di[i] = Some(mdi);
        dx[i] = Some(d);
    }

    let mut adx = vec![None; n];
    let adx_start = period * 2;
    let dx_seed: f64 = dx[period + 1..=adx_start]
        .iter()
        .filter_map(|v| *v)
        .sum::<f64>()
        / p;
    let mut cur_adx = dx_seed;
    adx[adx_start] = Some(cur_adx);

    for i in (adx_start + 1)..n {
        if let Some(d) = dx[i] {
            cur_adx = (cur_adx * (p - 1.0) + d) / p;
            adx[i] = Some(cur_adx);
        }
    }

    (adx, plus_di, minus_di)
}

/// Bollinger Bands (`upper`, `middle`, `lower`, `%B`).
pub fn compute_bollinger_series(
    closes: &[f64],
    period: usize,
    std_dev_mult: f64,
) -> (OptSeries, OptSeries, OptSeries, OptSeries) {
    let n = closes.len();
    let middle = compute_sma_series(closes, period);
    let mut upper = vec![None; n];
    let mut lower = vec![None; n];
    let mut pct_b = vec![None; n];

    if period == 0 || n < period {
        return (upper, middle, lower, pct_b);
    }

    for i in (period - 1)..n {
        if let Some(mid) = middle[i] {
            let slice = &closes[(i + 1 - period)..=i];
            let variance: f64 = slice
                .iter()
                .map(|v| {
                    let d = *v - mid;
                    d * d
                })
                .sum::<f64>()
                / period as f64;
            let sd = variance.sqrt();
            let up = mid + std_dev_mult * sd;
            let lo = mid - std_dev_mult * sd;
            upper[i] = Some(up);
            lower[i] = Some(lo);
            let width = up - lo;
            if width > f64::EPSILON {
                pct_b[i] = Some((closes[i] - lo) / width);
            } else {
                pct_b[i] = Some(0.5);
            }
        }
    }

    (upper, middle, lower, pct_b)
}

/// Stochastic RSI (`%K` and `%D` lines, 0..100).
pub fn compute_stoch_rsi_series(
    closes: &[f64],
    params: &StochRsiParams,
) -> (Vec<Option<f64>>, Vec<Option<f64>>) {
    let n = closes.len();
    let rsi = compute_rsi_series(closes, params.rsi_period);
    let mut raw_stoch = vec![None; n];

    let sp = params.stoch_period;
    if sp == 0 || n < params.rsi_period + sp {
        return (vec![None; n], vec![None; n]);
    }

    for i in (sp - 1)..n {
        let window = &rsi[(i + 1 - sp)..=i];
        let mut min_rsi = f64::INFINITY;
        let mut max_rsi = f64::NEG_INFINITY;
        let mut cur = 0.0;
        let mut all_valid = true;
        for &opt_v in window {
            match opt_v {
                Some(v) => {
                    if v < min_rsi {
                        min_rsi = v;
                    }
                    if v > max_rsi {
                        max_rsi = v;
                    }
                    cur = v;
                }
                None => {
                    all_valid = false;
                    break;
                }
            }
        }
        if all_valid {
            let range = max_rsi - min_rsi;
            raw_stoch[i] = if range <= f64::EPSILON {
                Some(50.0)
            } else {
                Some(((cur - min_rsi) / range) * 100.0)
            };
        }
    }

    let stoch_k = compute_sma_opt_series(&raw_stoch, params.k_smooth.max(1));
    let stoch_d = compute_sma_opt_series(&stoch_k, params.d_smooth.max(1));
    (stoch_k, stoch_d)
}

/// Higher-timeframe StochRSI stepped onto the base timeline. Candles are bucketed
/// into `htf.interval` bars, StochRSI is computed on the resampled closes, and each
/// value only appears on base bars at or after the higher-timeframe bar's close —
/// so a rule can never see an unfinished 4h bar.
pub fn compute_htf_stoch_rsi_series(
    candles: &[Candle],
    htf: &HtfParams,
) -> (OptSeries, OptSeries, OptSeries) {
    compute_htf_stoch_rsi_from(candles, candles, htf)
}

/// Build a higher-timeframe block's series from `source` bars and step the
/// result onto the `base` bars. `source` is normally the base candles, but a
/// block may name its own (coarser, longer-history) interval — a weekly series
/// cannot be resampled from a base chart that only spans a few weeks.
fn htf_block_series(
    base: &[Candle],
    htf: Option<&HtfParams>,
    htf_source: Option<&[Candle]>,
) -> (OptSeries, OptSeries, OptSeries) {
    let Some(htf) = htf else {
        return (Vec::new(), Vec::new(), Vec::new());
    };
    let n = base.len();
    let empty = || (vec![None; n], vec![None; n], vec![None; n]);
    if htf.source.is_some() {
        match htf_source {
            Some(src) if src.len() > 1 => compute_htf_stoch_rsi_from(base, src, htf),
            _ => empty(),
        }
    } else {
        compute_htf_stoch_rsi_from(base, base, htf)
    }
}

/// Resample `source` into `htf.interval` buckets, compute StochRSI on the bucket
/// closes, and step the values onto `base` at each bucket close.
pub fn compute_htf_stoch_rsi_from(
    base: &[Candle],
    source: &[Candle],
    htf: &HtfParams,
) -> (OptSeries, OptSeries, OptSeries) {
    let n = base.len();
    let mut k_out: OptSeries = vec![None; n];
    let mut d_out: OptSeries = vec![None; n];
    let mut slope_out: OptSeries = vec![None; n];
    if n < 2 || source.len() < 2 {
        return (k_out, d_out, slope_out);
    }
    let Ok(step_ms) = interval_duration_ms(&htf.interval) else {
        return (k_out, d_out, slope_out);
    };
    // Bar length, recovered from the data (`close = open + interval - 1ms`).
    let base_ms = source[0]
        .close_time_ms
        .saturating_sub(source[0].open_time_ms)
        .saturating_add(1);
    if base_ms == 0 || step_ms <= base_ms || step_ms % base_ms != 0 {
        return (k_out, d_out, slope_out);
    }
    let shift = (base_ms - (source[0].open_time_ms % base_ms)) % base_ms;

    // (bucket boundary, close time of the last base bar in it, close price).
    let mut buckets: Vec<(u64, u64, f64)> = Vec::new();
    for c in source {
        let boundary = ((c.open_time_ms + shift) / step_ms + 1) * step_ms - shift;
        match buckets.last_mut() {
            Some(last) if last.0 == boundary => {
                last.1 = c.close_time_ms;
                last.2 = c.close;
            }
            _ => buckets.push((boundary, c.close_time_ms, c.close)),
        }
    }
    // Drop the final, still-forming bucket so no part of an open 4h bar leaks in.
    let buckets: Vec<(u64, u64, f64)> = buckets
        .into_iter()
        .filter(|(boundary, last_close, _)| *last_close + 1 >= *boundary)
        .collect();

    let closes: Vec<f64> = buckets.iter().map(|b| b.2).collect();
    let (k, d) = compute_stoch_rsi_series(&closes, &htf.stoch_rsi);

    // Slope is measured between closed higher-timeframe bars, then held for the
    // whole next bucket, so `%K rising` is a 4h state rather than a single-bar
    // event on the base chart.
    let mut slope: OptSeries = vec![None; k.len()];
    for j in 1..k.len() {
        if let (Some(cur), Some(prev)) = (k[j], k[j - 1]) {
            slope[j] = Some(if cur > prev {
                1.0
            } else if cur < prev {
                -1.0
            } else {
                0.0
            });
        }
    }

    // A value becomes visible on the base bar that closes its higher-timeframe bar.
    let mut j = 0usize;
    for (i, c) in base.iter().enumerate() {
        while j < buckets.len() && buckets[j].1 <= c.close_time_ms {
            j += 1;
        }
        if j == 0 {
            continue;
        }
        k_out[i] = k[j - 1];
        d_out[i] = d[j - 1];
        slope_out[i] = slope[j - 1];
    }
    (k_out, d_out, slope_out)
}

/// ATR-based SuperTrend: returns the trailing line and direction (`1.0` bullish,
/// `-1.0` bearish). The trend flips when price closes through the opposite band.
pub fn compute_supertrend_series(
    candles: &[Candle],
    params: &SupertrendParams,
) -> (OptSeries, OptSeries) {
    let n = candles.len();
    let atr = compute_atr_series(candles, params.atr_period);
    let mut line: OptSeries = vec![None; n];
    let mut dir: OptSeries = vec![None; n];

    let mut prev_upper: Option<f64> = None;
    let mut prev_lower: Option<f64> = None;
    let mut trend = 1.0f64;

    for i in 0..n {
        let Some(atr_val) = atr[i] else { continue };
        let hl2 = (candles[i].high + candles[i].low) / 2.0;
        let basic_upper = hl2 + params.multiplier * atr_val;
        let basic_lower = hl2 - params.multiplier * atr_val;

        let pu = prev_upper.unwrap_or(basic_upper);
        let pl = prev_lower.unwrap_or(basic_lower);

        let prev_close = if i > 0 {
            Some(candles[i - 1].close)
        } else {
            None
        };
        let final_upper = if basic_upper < pu || prev_close.is_some_and(|c| c > pu) {
            basic_upper
        } else {
            pu
        };
        let final_lower = if basic_lower > pl || prev_close.is_some_and(|c| c < pl) {
            basic_lower
        } else {
            pl
        };

        let close = candles[i].close;
        trend = if close > pu {
            1.0
        } else if close < pl {
            -1.0
        } else {
            trend
        };

        line[i] = Some(if trend > 0.0 {
            final_lower
        } else {
            final_upper
        });
        dir[i] = Some(trend);
        prev_upper = Some(final_upper);
        prev_lower = Some(final_lower);
    }

    (line, dir)
}

/// Larry Connors' Composite RSI: mean of the RSI of price, the RSI of the
/// consecutive up/down streak, and the percent-rank of 1-bar returns.
pub fn compute_connors_rsi_series(closes: &[f64], params: &ConnorsRsiParams) -> OptSeries {
    let n = closes.len();
    let rsi = compute_rsi_series(closes, params.rsi_period);

    let mut streak = vec![0.0f64; n];
    for i in 1..n {
        let d = closes[i] - closes[i - 1];
        streak[i] = if d > 0.0 {
            if streak[i - 1] > 0.0 {
                streak[i - 1] + 1.0
            } else {
                1.0
            }
        } else if d < 0.0 {
            if streak[i - 1] < 0.0 {
                streak[i - 1] - 1.0
            } else {
                -1.0
            }
        } else {
            0.0
        };
    }
    let streak_rsi = compute_rsi_series(&streak, params.streak_period);

    let roc = compute_roc_series(closes, 1);
    let lookback = params.rank_period;
    let mut rank: OptSeries = vec![None; n];
    if lookback > 0 {
        for i in lookback..n {
            let Some(cur) = roc[i] else { continue };
            let window = &roc[i - lookback..i];
            if window.iter().any(|v| v.is_none()) {
                continue;
            }
            let below = window
                .iter()
                .filter_map(|v| *v)
                .filter(|v| *v < cur)
                .count();
            rank[i] = Some((100.0 * below as f64) / lookback as f64);
        }
    }

    let mut out: OptSeries = vec![None; n];
    for i in 0..n {
        if let (Some(a), Some(b), Some(c)) = (rsi[i], streak_rsi[i], rank[i]) {
            out[i] = Some((a + b + c) / 3.0);
        }
    }
    out
}

/// Volume Ratio: `volume[i] / SMA(volume, period)[i]`.
pub fn compute_volume_ratio_series(volumes: &[f64], period: usize) -> Vec<Option<f64>> {
    let n = volumes.len();
    let sma = compute_sma_series(volumes, period);
    let mut out = vec![None; n];
    for i in 0..n {
        if let Some(avg) = sma[i] {
            if avg > f64::EPSILON {
                out[i] = Some(volumes[i] / avg);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp_5m_candles(count: usize) -> Vec<Candle> {
        (0..count)
            .map(|i| {
                let open = i as u64 * 300_000;
                let close = 100.0 + (i as f64) * 0.1;
                Candle {
                    open_time_ms: open,
                    close_time_ms: open + 299_999,
                    open: close - 0.05,
                    high: close + 0.2,
                    low: close - 0.2,
                    close,
                    volume: 10.0,
                    trades: 3,
                }
            })
            .collect()
    }

    #[test]
    fn htf_stoch_steps_in_only_when_the_htf_bar_closes() {
        // 100 five-minute bars = 8 closed 1h bars + a 4-bar partial.
        let candles = ramp_5m_candles(100);
        let cfg = IndicatorConfig {
            htf: Some(HtfParams {
                interval: "1h".to_string(),
                stoch_rsi: StochRsiParams {
                    rsi_period: 2,
                    stoch_period: 2,
                    k_smooth: 1,
                    d_smooth: 1,
                },
                source: None,
            }),
            ..IndicatorConfig::default()
        };
        let table = IndicatorTable::compute(&candles, &cfg);

        // 1h StochRSI(2,2) needs 4 closed 1h bars before it has a value, so the
        // first value lands on the bar that closes the fourth 1h bucket.
        assert!(table.htf_stoch_k[..47].iter().all(|v| v.is_none()));
        assert!(
            table.htf_stoch_k[47].is_some(),
            "first value at the 4th 1h close"
        );

        // Values may only change on a bar that closes an hour.
        let closes: Vec<usize> = (0..8).map(|k| k * 12 + 11).collect();
        for i in 1..candles.len() {
            if table.htf_stoch_k[i] != table.htf_stoch_k[i - 1] {
                assert!(
                    closes.contains(&i),
                    "value changed on bar {i}, not an hour close"
                );
            }
        }

        // The last four bars belong to an unclosed hour and must not move.
        let tail = table.htf_stoch_k[95].expect("value from the 8th hour");
        for i in 96..100 {
            assert_eq!(table.htf_stoch_k[i], Some(tail));
        }
    }

    #[test]
    fn htf_slope_is_a_state_held_for_the_whole_bucket() {
        let candles = ramp_5m_candles(200);
        let cfg = IndicatorConfig {
            htf: Some(HtfParams {
                interval: "1h".to_string(),
                stoch_rsi: StochRsiParams {
                    rsi_period: 2,
                    stoch_period: 2,
                    k_smooth: 1,
                    d_smooth: 1,
                },
                source: None,
            }),
            ..IndicatorConfig::default()
        };
        let table = IndicatorTable::compute(&candles, &cfg);

        // There is no slope before %K exists, and none on the very first 1h bar
        // (there is nothing to compare against yet).
        for i in 0..candles.len() {
            if table.htf_stoch_k[i].is_none() {
                assert!(table.htf_stoch_k_slope[i].is_none(), "bar {i}");
            }
        }

        // Slope is a state: inside one hour it holds the sign of the change into
        // that hour, measured between closed 1h bars. The first closed hour has
        // nothing to compare against, so the 5m bars in that bucket carry none.
        let closes: Vec<usize> = (0..candles.len() / 12).map(|k| k * 12 + 11).collect();
        let values: Vec<Option<f64>> = closes.iter().map(|&i| table.htf_stoch_k[i]).collect();
        let mut longest_run = 0usize;
        let mut run = 0usize;
        let mut run_value: Option<f64> = None;
        for (bucket, &open_bar) in closes.iter().enumerate() {
            let end = closes.get(bucket + 1).copied().unwrap_or(candles.len());
            let expected = bucket
                .checked_sub(1)
                .and_then(|prev| Some((values[bucket]?, values[prev]?)))
                .map(|(cur, prev)| {
                    if cur > prev {
                        1.0
                    } else if cur < prev {
                        -1.0
                    } else {
                        0.0
                    }
                });
            for i in open_bar..end {
                assert_eq!(
                    table.htf_stoch_k_slope[i], expected,
                    "slope must hold steady inside the hour closing at bar {open_bar} (bar {i})"
                );
                if expected.is_some() && table.htf_stoch_k_slope[i] == run_value {
                    run += 1;
                } else {
                    run_value = expected;
                    run = usize::from(expected.is_some());
                }
                longest_run = longest_run.max(run);
            }
        }
        assert!(
            longest_run > 10,
            "a slope must be held across a whole bucket, not set on one bar (got {longest_run})"
        );
    }

    #[test]
    fn htf_block_can_read_its_own_source_series() {
        let day_ms: u64 = 86_400_000;
        // Daily source spanning 300 days, so a weekly StochRSI is warm long before
        // the base chart starts.
        let source: Vec<Candle> = (0..300)
            .map(|d| {
                let t = (d as u64) * day_ms;
                Candle {
                    open_time_ms: t,
                    close_time_ms: t + day_ms - 1,
                    open: 100.0 + d as f64,
                    high: 102.0 + d as f64,
                    low: 99.0 + d as f64,
                    close: 101.0 + (d as f64 * 0.7),
                    volume: 1_000.0,
                    trades: 10,
                }
            })
            .collect();
        // Base chart: 5m bars over the last 3 days of that range only.
        let start = 297 * day_ms;
        let base: Vec<Candle> = (0..(3 * 288u64))
            .map(|i| {
                let t = start + i * 300_000;
                Candle {
                    open_time_ms: t,
                    close_time_ms: t + 299_999,
                    open: 100.0,
                    high: 100.5,
                    low: 99.5,
                    close: 100.2,
                    volume: 500.0,
                    trades: 5,
                }
            })
            .collect();

        let cfg = IndicatorConfig {
            htf2: Some(HtfParams {
                interval: "1w".to_string(),
                stoch_rsi: StochRsiParams {
                    rsi_period: 2,
                    stoch_period: 2,
                    k_smooth: 1,
                    d_smooth: 1,
                },
                source: Some("1d".to_string()),
            }),
            ..IndicatorConfig::default()
        };

        let table = IndicatorTable::compute_with_source(&base, &cfg, Some(&source));
        assert!(
            table.htf2_stoch_k.iter().all(|v| v.is_some()),
            "weekly values must be available on every 5m bar"
        );
        // A weekly series can only step at a week close: 3 days of base bars can
        // contain at most one boundary.
        let changes = table
            .htf2_stoch_k
            .windows(2)
            .filter(|w| w[0] != w[1])
            .count();
        assert!(changes <= 1, "weekly %K changed {changes} times in 3 days");

        // Without the extra series the block stays inert rather than silently
        // trying to resample 3 days of 5m bars into weeks.
        let inert = IndicatorTable::compute(&base, &cfg);
        assert!(inert.htf2_stoch_k.iter().all(|v| v.is_none()));
    }

    #[test]
    fn htf_requires_a_higher_multiple_interval() {
        let candles = ramp_5m_candles(120);
        let cfg = IndicatorConfig {
            htf: Some(HtfParams {
                interval: "5m".to_string(),
                stoch_rsi: default_stoch_rsi(),
                source: None,
            }),
            ..IndicatorConfig::default()
        };
        let table = IndicatorTable::compute(&candles, &cfg);
        assert!(table.htf_stoch_k.iter().all(|v| v.is_none()));
        assert!(table.htf_stoch_d.iter().all(|v| v.is_none()));
        assert!(table.htf_stoch_k_slope.iter().all(|v| v.is_none()));
    }

    #[test]
    fn wilder_rsi_matches_canonical_vector() {
        // Classic 15-bar Wilder RSI(14) textbook prices:
        let closes = [
            44.34, 44.09, 44.15, 43.61, 44.33, 44.83, 45.10, 45.42, 45.84, 46.08, 45.89, 46.03,
            45.61, 46.28, 46.28, 46.00, 46.03, 46.41, 46.22, 45.64,
        ];
        let rsi = compute_rsi_series(&closes, 14);
        assert!(rsi[13].is_none());
        // First RSI(14) at index 14 in Wilder's textbook is ~70.46
        let r14 = rsi[14].expect("rsi[14]");
        assert!((r14 - 70.46).abs() < 0.1, "expected ~70.46, got {r14}");
        // At index 19 in Wilder's textbook RSI(14) is ~57.97
        let r19 = rsi[19].expect("rsi[19]");
        assert!((r19 - 57.97).abs() < 0.1, "expected ~57.97, got {r19}");
    }

    #[test]
    fn rsi_ma_smooths_rsi_line() {
        let closes: Vec<f64> = (0..80)
            .map(|i| 100.0 + (i as f64 * 0.3).sin() * 5.0 + i as f64 * 0.1)
            .collect();
        let rsi = compute_rsi_series(&closes, 14);
        let sma = compute_rsi_ma_series(
            &rsi,
            &RsiParams {
                period: 14,
                ma_period: 14,
                ma_type: MaType::Sma,
            },
        );
        let ema = compute_rsi_ma_series(
            &rsi,
            &RsiParams {
                period: 14,
                ma_period: 14,
                ma_type: MaType::Ema,
            },
        );

        // First SMA needs the 14 RSI values starting at index 14 -> index 27.
        assert!(sma[26].is_none());
        assert!(sma[27].is_some());
        assert!(ema[27].is_some());
        assert!((sma[27].unwrap() - ema[27].unwrap()).abs() < 1e-9);

        let manual: f64 = rsi[14..=27].iter().map(|v| v.unwrap()).sum::<f64>() / 14.0;
        assert!((sma[27].unwrap() - manual).abs() < 1e-9);

        assert!(sma.iter().flatten().all(|v| (0.0..=100.0).contains(v)));
        assert!(ema.iter().flatten().all(|v| (0.0..=100.0).contains(v)));
    }

    #[test]
    fn supertrend_flips_with_direction() {
        let up: Vec<Candle> = (0..120)
            .map(|i| {
                let t = (i as u64) * 900_000;
                let close = 100.0 + i as f64 * 0.5;
                Candle {
                    open_time_ms: t,
                    close_time_ms: t + 900_000,
                    open: close - 0.2,
                    high: close + 0.4,
                    low: close - 0.4,
                    close,
                    volume: 1_000.0,
                    trades: 10,
                }
            })
            .collect();
        let down: Vec<Candle> = up
            .iter()
            .map(|c| Candle {
                open: 300.0 - c.open,
                high: 300.0 - c.low,
                low: 300.0 - c.high,
                close: 300.0 - c.close,
                ..c.clone()
            })
            .collect();
        let params = default_supertrend();

        let (up_line, up_dir) = compute_supertrend_series(&up, &params);
        let (down_line, down_dir) = compute_supertrend_series(&down, &params);

        let last = 119;
        assert_eq!(up_dir[last], Some(1.0));
        assert!(up_line[last].unwrap() < up[last].close);
        assert_eq!(down_dir[last], Some(-1.0));
        assert!(down_line[last].unwrap() > down[last].close);
    }

    #[test]
    fn connors_rsi_tracks_streak_and_stays_bounded() {
        let closes: Vec<f64> = (0..220)
            .map(|i| 100.0 + (i as f64 * 0.3).sin() * 4.0 + i as f64 * 0.02)
            .collect();
        let params = default_connors_rsi();
        let crsi = compute_connors_rsi_series(&closes, &params);

        // The percent-rank needs a full lookback window of 1-bar returns.
        assert!(crsi[..=params.rank_period].iter().all(|v| v.is_none()));

        let vals: Vec<f64> = crsi.iter().flatten().copied().collect();
        assert!(!vals.is_empty());
        assert!(vals.iter().all(|v| (0.0..=100.0).contains(v)));

        // A continuously rising series produces an all-positive streak.
        let rising: Vec<f64> = (0..120).map(|i| 100.0 + i as f64).collect();
        let rising_crsi = compute_connors_rsi_series(&rising, &params);
        let last = rising_crsi[119].expect("crsi after warmup");
        assert!(last > 60.0, "rising market should read strong, got {last}");
    }

    #[test]
    fn kst_computes_line_signal_and_histogram() {
        let closes: Vec<f64> = (0..100)
            .map(|i| 100.0 + (i as f64 * 0.5) + ((i as f64 * 0.2).sin() * 3.0))
            .collect();
        let params = default_kst();
        let (kst, sig, hist) = compute_kst_series(&closes, &params);

        // First valid KST is at index r4 + s4 - 1 = 30 + 15 - 1 = 44
        assert!(kst[43].is_none());
        assert!(kst[44].is_some());

        // First valid KST signal is at 44 + signal - 1 = 52
        assert!(sig[51].is_none());
        assert!(sig[52].is_some());
        assert!(hist[52].is_some());

        let k = kst[80].unwrap();
        let s = sig[80].unwrap();
        let h = hist[80].unwrap();
        assert!((k - s - h).abs() < 1e-9);
    }
}
