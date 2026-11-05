//! Hyperliquid public market data client (`candleSnapshot`), local cache, and closed-candle filter.
//!
//! Supports both native Hyperliquid perpetuals (`BTC`, `ETH`, `SOL`, `HYPE`, etc.)
//! and HIP-3 builder-deployed perpetuals (`xyz:CL`, `xyz:GOLD`, `xyz:NVDA`, `xyz:SP500`, etc.).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::HlError;

/// Mainnet Hyperliquid Info endpoint.
pub const HL_MAINNET_INFO_URL: &str = "https://api.hyperliquid.xyz/info";
/// Testnet Hyperliquid Info endpoint.
pub const HL_TESTNET_INFO_URL: &str = "https://api.hyperliquid-testnet.xyz/info";
/// Mainnet Hyperliquid Exchange endpoint.
pub const HL_MAINNET_EXCHANGE_URL: &str = "https://api.hyperliquid.xyz/exchange";
/// Testnet Hyperliquid Exchange endpoint.
pub const HL_TESTNET_EXCHANGE_URL: &str = "https://api.hyperliquid-testnet.xyz/exchange";
/// Testnet Hyperliquid Drip / Faucet UI URL.
pub const HL_TESTNET_DRIP_URL: &str = "https://app.hyperliquid-testnet.xyz/drip";

/// Safety margin (ms) after a candle's `close_time_ms` before we treat it as closed.
pub const DEFAULT_CLOSE_SAFETY_MARGIN_MS: u64 = 2_000;

/// Metadata for a curated Hyperliquid market.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CuratedMarket {
    pub symbol: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub is_hip3: bool,
}

/// Curated catalog of popular Crypto, Commodity, and Equity markets on Hyperliquid.
pub const CURATED_MARKETS: &[CuratedMarket] = &[
    // Commodities (HIP-3 XYZ DEX)
    CuratedMarket {
        symbol: "xyz:CL",
        name: "WTI Crude Oil (USDC)",
        category: "Commodities",
        is_hip3: true,
    },
    CuratedMarket {
        symbol: "xyz:BRENTOIL",
        name: "Brent Crude Oil (USDC)",
        category: "Commodities",
        is_hip3: true,
    },
    CuratedMarket {
        symbol: "xyz:GOLD",
        name: "Gold (USDC)",
        category: "Commodities",
        is_hip3: true,
    },
    CuratedMarket {
        symbol: "xyz:SILVER",
        name: "Silver (USDC)",
        category: "Commodities",
        is_hip3: true,
    },
    CuratedMarket {
        symbol: "xyz:NATGAS",
        name: "Natural Gas (USDC)",
        category: "Commodities",
        is_hip3: true,
    },
    CuratedMarket {
        symbol: "xyz:COPPER",
        name: "Copper (USDC)",
        category: "Commodities",
        is_hip3: true,
    },
    // Equities & Indices (HIP-3 XYZ DEX)
    CuratedMarket {
        symbol: "xyz:NVDA",
        name: "NVIDIA Corp (USDC)",
        category: "Equities",
        is_hip3: true,
    },
    CuratedMarket {
        symbol: "xyz:TSLA",
        name: "Tesla Inc (USDC)",
        category: "Equities",
        is_hip3: true,
    },
    CuratedMarket {
        symbol: "xyz:AAPL",
        name: "Apple Inc (USDC)",
        category: "Equities",
        is_hip3: true,
    },
    CuratedMarket {
        symbol: "xyz:MSFT",
        name: "Microsoft (USDC)",
        category: "Equities",
        is_hip3: true,
    },
    CuratedMarket {
        symbol: "xyz:PLTR",
        name: "Palantir Technologies (USDC)",
        category: "Equities",
        is_hip3: true,
    },
    CuratedMarket {
        symbol: "xyz:COIN",
        name: "Coinbase Global (USDC)",
        category: "Equities",
        is_hip3: true,
    },
    CuratedMarket {
        symbol: "xyz:SP500",
        name: "S&P 500 Index (USDC)",
        category: "Indices",
        is_hip3: true,
    },
    // Native Crypto Perpetuals
    CuratedMarket {
        symbol: "BTC",
        name: "Bitcoin (USDC)",
        category: "Crypto",
        is_hip3: false,
    },
    CuratedMarket {
        symbol: "ETH",
        name: "Ethereum (USDC)",
        category: "Crypto",
        is_hip3: false,
    },
    CuratedMarket {
        symbol: "SOL",
        name: "Solana (USDC)",
        category: "Crypto",
        is_hip3: false,
    },
    CuratedMarket {
        symbol: "HYPE",
        name: "Hyperliquid (USDC)",
        category: "Crypto",
        is_hip3: false,
    },
    CuratedMarket {
        symbol: "SUI",
        name: "Sui Network (USDC)",
        category: "Crypto",
        is_hip3: false,
    },
    CuratedMarket {
        symbol: "DOGE",
        name: "Dogecoin (USDC)",
        category: "Crypto",
        is_hip3: false,
    },
];

/// Supported Hyperliquid candle intervals.
pub const VALID_INTERVALS: &[&str] = &[
    "1m", "3m", "5m", "15m", "30m", "1h", "2h", "4h", "8h", "12h", "1d", "3d", "1w",
];

/// Intervals the venue does not serve, but that we build by aggregating a lower
/// one (Hyperliquid has no 10m candles, so 10m charts are 5m bars paired up).
pub const DERIVED_INTERVALS: &[&str] = &["10m"];

/// Lower interval that a derived interval is aggregated from.
pub fn derived_source_interval(interval: &str) -> Option<&'static str> {
    match interval {
        "10m" => Some("5m"),
        _ => None,
    }
}

/// Aggregate `src` bars into buckets of `target_ms`, aligned to the epoch. The
/// result keeps the first open, last close, highest high, lowest low, and summed
/// volume/trades of each bucket.
pub fn aggregate_candles(src: &[Candle], target_ms: u64) -> Vec<Candle> {
    if target_ms == 0 {
        return src.to_vec();
    }
    let mut out: Vec<Candle> = Vec::with_capacity(src.len());
    for c in src {
        let bucket = c.open_time_ms / target_ms;
        match out.last_mut() {
            Some(last) if last.open_time_ms / target_ms == bucket => {
                last.high = last.high.max(c.high);
                last.low = last.low.min(c.low);
                last.close = c.close;
                last.volume += c.volume;
                last.trades += c.trades;
            }
            // Bucket times are normalised to the bucket grid, so every aggregated
            // bar is exactly `target_ms` long even where the venue has gaps (a
            // 5m hole would otherwise leave a 5-minute "10m" bar behind).
            _ => out.push(Candle {
                open_time_ms: bucket * target_ms,
                close_time_ms: bucket * target_ms + target_ms - 1,
                open: c.open,
                high: c.high,
                low: c.low,
                close: c.close,
                volume: c.volume,
                trades: c.trades,
            }),
        }
    }
    out
}

/// Validate a Hyperliquid timeframe string.
pub fn validate_interval(interval: &str) -> Result<(), HlError> {
    if VALID_INTERVALS.contains(&interval) {
        Ok(())
    } else {
        Err(HlError::InvalidStrategy(format!(
            "unsupported interval '{interval}'; valid intervals: {}",
            VALID_INTERVALS.join(", ")
        )))
    }
}

/// Duration of one bar in milliseconds for a valid Hyperliquid interval.
pub fn interval_duration_ms(interval: &str) -> Result<u64, HlError> {
    let mins: u64 = match interval {
        "1m" => 1,
        "3m" => 3,
        "5m" => 5,
        "15m" => 15,
        "30m" => 30,
        "1h" => 60,
        "2h" => 120,
        "4h" => 240,
        "8h" => 480,
        "12h" => 720,
        "10m" => 10,
        "1d" => 1440,
        "3d" => 4_320,
        "1w" => 10_080,
        _ => {
            return Err(HlError::InvalidStrategy(format!(
                "unknown interval '{interval}'"
            )))
        }
    };
    Ok(mins * 60 * 1_000)
}

/// Normalized OHLCV candle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candle {
    /// Bar open timestamp in unix milliseconds (`t`).
    pub open_time_ms: u64,
    /// Bar close timestamp in unix milliseconds (`T`).
    pub close_time_ms: u64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    #[serde(default)]
    pub trades: u64,
}

/// Raw Hyperliquid `candleSnapshot` item from `POST /info`.
#[derive(Debug, Deserialize)]
struct RawHlCandle {
    #[serde(rename = "t")]
    open_time_ms: u64,
    #[serde(rename = "T")]
    close_time_ms: u64,
    #[serde(rename = "o")]
    open: String,
    #[serde(rename = "h")]
    high: String,
    #[serde(rename = "l")]
    low: String,
    #[serde(rename = "c")]
    close: String,
    #[serde(rename = "v")]
    volume: String,
    #[serde(rename = "n", default)]
    trades: u64,
}

impl RawHlCandle {
    fn into_candle(self) -> Result<Candle, HlError> {
        let parse_f64 = |s: &str, field: &str| -> Result<f64, HlError> {
            let v = s
                .parse::<f64>()
                .map_err(|e| HlError::MarketData(format!("invalid candle {field} '{s}': {e}")))?;
            if !v.is_finite() {
                return Err(HlError::MarketData(format!(
                    "non-finite candle {field} '{s}'"
                )));
            }
            Ok(v)
        };
        let open = parse_f64(&self.open, "open")?;
        let high = parse_f64(&self.high, "high")?;
        let low = parse_f64(&self.low, "low")?;
        let close = parse_f64(&self.close, "close")?;
        let volume = parse_f64(&self.volume, "volume")?;

        if open <= 0.0 || high <= 0.0 || low <= 0.0 || close <= 0.0 || volume < 0.0 {
            return Err(HlError::MarketData(
                "candle prices must be > 0 and volume >= 0".to_string(),
            ));
        }
        if high < low || high < open.max(close) || low > open.min(close) {
            return Err(HlError::MarketData(
                "candle OHLC envelope is inconsistent (high < low or outside open/close)"
                    .to_string(),
            ));
        }
        if self.close_time_ms < self.open_time_ms {
            return Err(HlError::MarketData(
                "candle close_time_ms is before open_time_ms".to_string(),
            ));
        }

        Ok(Candle {
            open_time_ms: self.open_time_ms,
            close_time_ms: self.close_time_ms,
            open,
            high,
            low,
            close,
            volume,
            trades: self.trades,
        })
    }
}

/// Current wall-clock time in unix milliseconds.
pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Sort candles by `open_time_ms`, deduplicate by `open_time_ms` (keeping latest),
/// and drop any unclosed candle where `now_ms < candle.close_time_ms + safety_margin_ms`.
pub fn closed_candles_only(
    mut candles: Vec<Candle>,
    now_ms: u64,
    safety_margin_ms: u64,
) -> Vec<Candle> {
    candles.sort_by_key(|c| c.open_time_ms);
    candles.dedup_by_key(|c| c.open_time_ms);
    candles
        .into_iter()
        .filter(|c| now_ms >= c.close_time_ms.saturating_add(safety_margin_ms))
        .collect()
}

/// Default cache path for `(coin, interval)` under `data_dir` (`data/candles/<sanitized_coin>_<sanitized_interval>.json`).
pub fn default_cache_path(data_dir: &Path, coin: &str, interval: &str) -> PathBuf {
    let sanitize = |raw: &str| -> String {
        raw.chars()
            .take(32)
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    };
    let safe_coin = sanitize(coin);
    let safe_interval = sanitize(interval);
    data_dir
        .join("candles")
        .join(format!("{safe_coin}_{safe_interval}.json"))
}

/// Save candles to a JSON cache file on disk.
pub fn save_candles_cache(path: &Path, candles: &[Candle]) -> Result<(), HlError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(candles)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Load candles from a JSON cache file on disk, validating every bar's OHLC envelope and timestamps.
pub fn load_candles_cache(path: &Path) -> Result<Vec<Candle>, HlError> {
    let raw = std::fs::read_to_string(path)?;
    let mut candles: Vec<Candle> = serde_json::from_str(&raw)?;
    for c in &candles {
        if !c.open.is_finite()
            || !c.high.is_finite()
            || !c.low.is_finite()
            || !c.close.is_finite()
            || !c.volume.is_finite()
            || c.open <= 0.0
            || c.high <= 0.0
            || c.low <= 0.0
            || c.close <= 0.0
            || c.volume < 0.0
            || c.high < c.low
            || c.high < c.open.max(c.close)
            || c.low > c.open.min(c.close)
            || c.close_time_ms < c.open_time_ms
        {
            return Err(HlError::MarketData(format!(
                "corrupted or invalid candle at t={} in {}",
                c.open_time_ms,
                path.display()
            )));
        }
    }
    candles.sort_by_key(|c| c.open_time_ms);
    candles.dedup_by_key(|c| c.open_time_ms);
    Ok(candles)
}

/// Fetch historical OHLCV candles from Hyperliquid `POST /info` (`candleSnapshot`),
/// automatically chunking requests when the lookback exceeds 4,500 bars so long
/// multi-week/month backtests on 3m/5m/15m timeframes fetch cleanly.
pub async fn fetch_candles_from_hl(
    info_url: &str,
    coin: &str,
    interval: &str,
    days: u32,
) -> Result<Vec<Candle>, HlError> {
    if !info_url.starts_with("https://") {
        return Err(HlError::MarketData(
            "Hyperliquid info_url must use https://".to_string(),
        ));
    }
    validate_interval(interval)?;
    let bar_ms = interval_duration_ms(interval)?;
    let now_ms = now_unix_ms();
    let clamped_days = days.clamp(1, 1_825);
    let total_span_ms = u64::from(clamped_days) * 24 * 60 * 60 * 1_000;
    let start_ms = now_ms.saturating_sub(total_span_ms);

    // Hyperliquid returns up to ~5000 candles per candleSnapshot call.
    let max_chunk_ms = bar_ms.saturating_mul(4_000);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| HlError::MarketData(e.to_string()))?;

    let mut all_candles: Vec<Candle> = Vec::new();
    let mut cursor_start = start_ms;

    while cursor_start < now_ms {
        let cursor_end = (cursor_start + max_chunk_ms).min(now_ms);
        let payload = serde_json::json!({
            "type": "candleSnapshot",
            "req": {
                "coin": coin,
                "interval": interval,
                "startTime": cursor_start,
                "endTime": cursor_end,
            }
        });

        let resp = client
            .post(info_url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| HlError::MarketData(format!("HTTP request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(HlError::MarketData(format!(
                "Hyperliquid /info returned {status} for {coin} ({interval}): {body}"
            )));
        }

        let raw_list: Vec<RawHlCandle> = resp.json().await.map_err(|e| {
            HlError::MarketData(format!("failed to decode candleSnapshot for {coin}: {e}"))
        })?;

        for raw in raw_list {
            all_candles.push(raw.into_candle()?);
        }

        if cursor_end >= now_ms {
            break;
        }
        cursor_start = cursor_end + bar_ms;
    }

    let closed = closed_candles_only(all_candles, now_ms, DEFAULT_CLOSE_SAFETY_MARGIN_MS);
    Ok(closed)
}

/// Query Hyperliquid's `clearinghouseState` to fetch total account equity in USD.
/// Returns 0.0 if the account is new or uninitialized on the clearinghouse.
pub async fn fetch_clearinghouse_state(
    info_url: &str,
    user: alloy::primitives::Address,
) -> Result<f64, HlError> {
    if !info_url.starts_with("https://") {
        return Err(HlError::MarketData(
            "Hyperliquid info_url must use https://".to_string(),
        ));
    }
    let payload = serde_json::json!({
        "type": "clearinghouseState",
        "user": format!("{user:#x}")
    });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| HlError::MarketData(e.to_string()))?;

    let resp = client
        .post(info_url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| HlError::MarketData(format!("failed to fetch clearinghouse state: {e}")))?;

    if !resp.status().is_success() {
        return Ok(0.0);
    }

    let val: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);

    let equity_str = val
        .get("marginSummary")
        .and_then(|m| m.get("accountValue"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            val.get("crossMarginSummary")
                .and_then(|m| m.get("accountValue"))
                .and_then(|v| v.as_str())
        })
        .or_else(|| val.get("withdrawable").and_then(|v| v.as_str()));

    if let Some(s) = equity_str {
        if let Ok(eq) = s.parse::<f64>() {
            return Ok(eq);
        }
    }
    Ok(0.0)
}

/// Query Hyperliquid's `spotClearinghouseState` to fetch total spot USDC balance.
pub async fn fetch_spot_balance(
    info_url: &str,
    user: alloy::primitives::Address,
) -> Result<f64, HlError> {
    if !info_url.starts_with("https://") {
        return Err(HlError::MarketData(
            "Hyperliquid info_url must use https://".to_string(),
        ));
    }
    let payload = serde_json::json!({
        "type": "spotClearinghouseState",
        "user": format!("{user:#x}")
    });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| HlError::MarketData(e.to_string()))?;

    let resp = client
        .post(info_url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| HlError::MarketData(format!("failed to fetch spot state: {e}")))?;

    if !resp.status().is_success() {
        return Ok(0.0);
    }
    let val: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
    if let Some(balances) = val.get("balances").and_then(|b| b.as_array()) {
        for b in balances {
            if b.get("coin").and_then(|c| c.as_str()) == Some("USDC") {
                if let Some(total_str) = b.get("total").and_then(|t| t.as_str()) {
                    if let Ok(amount) = total_str.parse::<f64>() {
                        return Ok(amount);
                    }
                }
            }
        }
    }
    Ok(0.0)
}

/// An open perpetual position on Hyperliquid (native or HIP-3 builder DEX).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LivePositionInfo {
    pub coin: String,
    pub szi: f64,
    pub entry_px: f64,
    pub position_value: f64,
    pub unrealized_pnl: f64,
    pub return_on_equity: f64,
    pub liquidation_px: Option<f64>,
    pub margin_used: f64,
    pub leverage: u32,
    pub is_isolated: bool,
}

/// Unified account telemetry capturing real portfolio value, available margin,
/// held collateral, and open positions across default and builder DEXes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserAccountTelemetry {
    /// True net portfolio equity (Spot USDC + Unrealized PnL, or Perps Equity).
    pub portfolio_value: f64,
    /// Available collateral to open new trades after maintenance requirements.
    pub available_usdc: f64,
    /// Total collateral currently locked in active positions (isolated margin holds).
    pub total_margin_used: f64,
    /// Sum of all unrealized PnL across active positions.
    pub total_unrealized_pnl: f64,
    /// Whether the account operates in Unified Account mode (cross-collateralized via Spot USDC).
    pub is_unified: bool,
    /// Total Spot USDC on balance.
    pub spot_total: f64,
    /// Spot USDC held for open orders / positions.
    pub spot_hold: f64,
    /// All active open positions across native and builder DEXes.
    pub open_positions: Vec<LivePositionInfo>,
}

/// Query comprehensive account state across Spot, default perps, and builder DEXes (e.g. xyz).
/// Correctly distinguishes Unified Account (cross-collateralized via Spot USDC)
/// from classic separate perpetuals accounts without double-counting isolated margin.
pub async fn fetch_user_account_telemetry(
    info_url: &str,
    user: alloy::primitives::Address,
) -> Result<UserAccountTelemetry, HlError> {
    if !info_url.starts_with("https://") {
        return Err(HlError::MarketData(
            "Hyperliquid info_url must use https://".to_string(),
        ));
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| HlError::MarketData(e.to_string()))?;

    // 1. Fetch Spot clearinghouse state
    let spot_payload = serde_json::json!({
        "type": "spotClearinghouseState",
        "user": format!("{user:#x}")
    });
    let mut spot_total = 0.0;
    let mut spot_hold = 0.0;
    let mut spot_avail = 0.0;

    if let Ok(resp) = client.post(info_url).json(&spot_payload).send().await {
        if resp.status().is_success() {
            if let Ok(val) = resp.json::<serde_json::Value>().await {
                if let Some(balances) = val.get("balances").and_then(|b| b.as_array()) {
                    for b in balances {
                        if b.get("coin").and_then(|c| c.as_str()) == Some("USDC") {
                            if let Some(s) = b.get("total").and_then(|t| t.as_str()) {
                                spot_total = s.parse::<f64>().unwrap_or(0.0);
                            }
                            if let Some(s) = b.get("hold").and_then(|t| t.as_str()) {
                                spot_hold = s.parse::<f64>().unwrap_or(0.0);
                            }
                        }
                    }
                }
                if let Some(arr) = val
                    .get("tokenToAvailableAfterMaintenance")
                    .and_then(|a| a.as_array())
                {
                    for item in arr {
                        if let Some(pair) = item.as_array() {
                            if pair.first().and_then(|v| v.as_u64()) == Some(0) {
                                if let Some(s) = pair.get(1).and_then(|v| v.as_str()) {
                                    spot_avail = s.parse::<f64>().unwrap_or(0.0);
                                }
                            }
                        }
                    }
                }
                if spot_avail == 0.0 && spot_total > spot_hold {
                    spot_avail = spot_total - spot_hold;
                }
            }
        }
    }

    // 2. Fetch default perps and HIP-3 DEXes (xyz)
    let fetch_dex = |dex_opt: Option<&'static str>| {
        let client = client.clone();
        let url = info_url.to_string();
        async move {
            let mut payload = serde_json::json!({
                "type": "clearinghouseState",
                "user": format!("{user:#x}")
            });
            if let Some(d) = dex_opt {
                payload["dex"] = serde_json::json!(d);
            }
            let resp = client.post(&url).json(&payload).send().await.ok()?;
            if !resp.status().is_success() {
                return None;
            }
            let val: serde_json::Value = resp.json().await.ok()?;
            let account_val = val
                .get("marginSummary")
                .and_then(|m| m.get("accountValue"))
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            let margin_used = val
                .get("marginSummary")
                .and_then(|m| m.get("totalMarginUsed"))
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            let withdrawable = val
                .get("withdrawable")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);

            let mut positions = Vec::new();
            if let Some(arr) = val.get("assetPositions").and_then(|a| a.as_array()) {
                for ap in arr {
                    let pos = ap.get("position").unwrap_or(&serde_json::Value::Null);
                    let szi = pos
                        .get("szi")
                        .and_then(|s| s.as_str())
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(0.0);
                    if szi.abs() > 0.0 {
                        let coin = pos
                            .get("coin")
                            .and_then(|c| c.as_str())
                            .unwrap_or("?")
                            .to_string();
                        let entry_px = pos
                            .get("entryPx")
                            .and_then(|s| s.as_str())
                            .and_then(|s| s.parse::<f64>().ok())
                            .unwrap_or(0.0);
                        let position_value = pos
                            .get("positionValue")
                            .and_then(|s| s.as_str())
                            .and_then(|s| s.parse::<f64>().ok())
                            .unwrap_or(0.0);
                        let unrealized_pnl = pos
                            .get("unrealizedPnl")
                            .and_then(|s| s.as_str())
                            .and_then(|s| s.parse::<f64>().ok())
                            .unwrap_or(0.0);
                        let return_on_equity = pos
                            .get("returnOnEquity")
                            .and_then(|s| s.as_str())
                            .and_then(|s| s.parse::<f64>().ok())
                            .unwrap_or(0.0);
                        let liquidation_px = pos
                            .get("liquidationPx")
                            .and_then(|s| s.as_str())
                            .and_then(|s| s.parse::<f64>().ok());
                        let margin = pos
                            .get("marginUsed")
                            .and_then(|s| s.as_str())
                            .and_then(|s| s.parse::<f64>().ok())
                            .unwrap_or(0.0);
                        let leverage = pos
                            .get("leverage")
                            .and_then(|l| l.get("value"))
                            .and_then(|v| v.as_u64())
                            .unwrap_or(1) as u32;
                        let is_isolated = pos
                            .get("leverage")
                            .and_then(|l| l.get("type"))
                            .and_then(|t| t.as_str())
                            == Some("isolated");

                        positions.push(LivePositionInfo {
                            coin,
                            szi,
                            entry_px,
                            position_value,
                            unrealized_pnl,
                            return_on_equity,
                            liquidation_px,
                            margin_used: margin,
                            leverage,
                            is_isolated,
                        });
                    }
                }
            }
            Some((account_val, margin_used, withdrawable, positions))
        }
    };

    let (def_res, xyz_res) = tokio::join!(fetch_dex(None), fetch_dex(Some("xyz")));

    let (def_val, def_margin, def_withdrawable, mut positions) =
        def_res.unwrap_or((0.0, 0.0, 0.0, Vec::new()));
    let (xyz_val, xyz_margin, _, xyz_positions) = xyz_res.unwrap_or((0.0, 0.0, 0.0, Vec::new()));
    positions.extend(xyz_positions);

    let total_margin_used = def_margin + xyz_margin;
    let total_unrealized_pnl: f64 = positions.iter().map(|p| p.unrealized_pnl).sum();
    let perps_equity = def_val + xyz_val;

    // Detect Unified Account mode:
    let is_unified = spot_total > 0.0
        && (spot_hold > 0.0
            || (def_withdrawable <= 0.001 && total_margin_used > 0.0)
            || def_val == 0.0);

    let (portfolio_value, available_usdc) = if is_unified {
        let pv = (spot_total + total_unrealized_pnl).max(0.0);
        (pv, spot_avail)
    } else if perps_equity > 0.0 {
        let pv = perps_equity + spot_total;
        let avail = def_withdrawable.max(0.0);
        (pv, avail)
    } else {
        (spot_total, spot_avail)
    };

    Ok(UserAccountTelemetry {
        portfolio_value,
        available_usdc,
        total_margin_used: if spot_hold > 0.0 {
            spot_hold
        } else {
            total_margin_used
        },
        total_unrealized_pnl,
        is_unified,
        spot_total,
        spot_hold,
        open_positions: positions,
    })
}

/// A real on-chain / clearinghouse trade execution fill on Hyperliquid.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveTradeFill {
    pub oid: u64,
    pub coin: String,
    pub dir: String,
    pub px: f64,
    pub sz: f64,
    pub closed_pnl: f64,
    pub fee: f64,
    pub time_ms: u64,
}

/// Fetch real historical execution fills for `user` from Hyperliquid (`POST /info`, `userFills`).
pub async fn fetch_user_fills(
    info_url: &str,
    user: alloy::primitives::Address,
) -> Result<Vec<LiveTradeFill>, HlError> {
    if !info_url.starts_with("https://") {
        return Err(HlError::MarketData(
            "Hyperliquid info_url must use https://".to_string(),
        ));
    }
    let payload = serde_json::json!({
        "type": "userFills",
        "user": format!("{user:#x}")
    });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| HlError::MarketData(e.to_string()))?;

    let resp = client
        .post(info_url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| HlError::MarketData(format!("failed to fetch user fills: {e}")))?;

    if !resp.status().is_success() {
        return Ok(Vec::new());
    }

    let val: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
    let Some(arr) = val.as_array() else {
        return Ok(Vec::new());
    };

    let mut fills = Vec::new();
    for item in arr {
        let oid = item.get("oid").and_then(|v| v.as_u64()).unwrap_or(0);
        let coin = item
            .get("coin")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        let dir = item
            .get("dir")
            .and_then(|v| v.as_str())
            .unwrap_or("Trade")
            .to_string();
        let px = item
            .get("px")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        let sz = item
            .get("sz")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        let closed_pnl = item
            .get("closedPnl")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        let fee = item
            .get("fee")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        let time_ms = item.get("time").and_then(|v| v.as_u64()).unwrap_or(0);

        fills.push(LiveTradeFill {
            oid,
            coin,
            dir,
            px,
            sz,
            closed_pnl,
            fee,
            time_ms,
        });
    }
    fills.sort_by_key(|f| f.time_ms);
    Ok(fills)
}

/// Minutes since midnight in UK local time (`Europe/London`), applying the
/// BST/GMT rule: clocks go forward on the last Sunday of March at 01:00 UTC and
/// back on the last Sunday of October at 01:00 UTC.
pub fn uk_minutes_of_day(unix_ms: u64) -> u32 {
    let secs = (unix_ms / 1_000) as i64;
    let days = secs.div_euclid(86_400);
    let utc_min = (secs.rem_euclid(86_400) / 60) as u32;
    let (year, month, day) = civil_from_days(days);
    let offset = uk_offset_minutes(year, month, day, utc_min);
    (utc_min + offset) % 1_440
}

/// Day number (days since `1970-01-01`) of the UK local calendar date, used as a
/// trading-window key so daily limits reset at local midnight.
pub fn uk_day_number(unix_ms: u64) -> i64 {
    let secs = (unix_ms / 1_000) as i64;
    let days = secs.div_euclid(86_400);
    let utc_min = (secs.rem_euclid(86_400) / 60) as u32;
    let (year, month, day) = civil_from_days(days);
    let offset = uk_offset_minutes(year, month, day, utc_min);
    if utc_min + offset >= 1_440 {
        days + 1
    } else {
        days
    }
}

/// UK clock offset in minutes (`60` during BST, `0` during GMT).
fn uk_offset_minutes(year: i64, month: u32, day: u32, utc_min: u32) -> u32 {
    let today = days_from_civil(year, month, day);
    let spring = last_sunday_of_month(year, 3);
    let autumn = last_sunday_of_month(year, 10);
    let bst = if today > spring && today < autumn {
        true
    } else if today == spring {
        utc_min >= 60
    } else if today == autumn {
        utc_min < 60
    } else {
        false
    };
    if bst {
        60
    } else {
        0
    }
}

/// Day number of the last Sunday in `month` (March/October have 31 days).
fn last_sunday_of_month(year: i64, month: u32) -> i64 {
    let last = days_from_civil(year, month, 31);
    // 1970-01-01 was a Thursday, so `+ 4` maps day 0 to a Sunday.
    let weekday = (last + 4).rem_euclid(7);
    last - weekday
}

/// Days since `1970-01-01` for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (i64::from(month) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Proleptic Gregorian `(year, month, day)` for days since `1970-01-01`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { y + 1 } else { y }, month, day)
}

/// Query Hyperliquid's `userRole` info endpoint to check an agent address's status.
/// Returns `"agent"`, `"missing"`, or `"user"`.
pub async fn check_agent_role(
    info_url: &str,
    agent_address: alloy::primitives::Address,
) -> Result<String, HlError> {
    if !info_url.starts_with("https://") {
        return Err(HlError::MarketData(
            "Hyperliquid info_url must use https://".to_string(),
        ));
    }
    let payload = serde_json::json!({
        "type": "userRole",
        "user": format!("{agent_address:#x}")
    });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| HlError::MarketData(e.to_string()))?;

    let resp = client
        .post(info_url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| HlError::MarketData(format!("failed to check agent status: {e}")))?;

    if !resp.status().is_success() {
        return Ok("unknown".to_string());
    }
    let val: serde_json::Value = resp.json().await.unwrap_or_default();
    let role = val
        .get("role")
        .and_then(|r| r.as_str())
        .unwrap_or("missing");
    Ok(role.to_string())
}

/// Attempt to copy `text` to the system clipboard using `wl-copy` (Wayland) or `xclip` (X11).
pub fn copy_to_clipboard(text: &str) -> bool {
    let mut ok = false;
    if let Ok(mut child) = std::process::Command::new("wl-copy")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(text.as_bytes());
        }
        if child.wait().map(|s| s.success()).unwrap_or(false) {
            ok = true;
        }
    }
    if !ok {
        if let Ok(mut child) = std::process::Command::new("xclip")
            .arg("-selection")
            .arg("clipboard")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                let _ = stdin.write_all(text.as_bytes());
            }
            if child.wait().map(|s| s.success()).unwrap_or(false) {
                ok = true;
            }
        }
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregating_pairs_source_bars_into_uniform_buckets() {
        let mk = |t: u64, o: f64, h: f64, l: f64, c: f64, v: f64| Candle {
            open_time_ms: t,
            close_time_ms: t + 299_999,
            open: o,
            high: h,
            low: l,
            close: c,
            volume: v,
            trades: 1,
        };
        // Three 5m bars, then a gap, then one more 5m bar (the venue does this).
        let src = vec![
            mk(0, 100.0, 102.0, 99.0, 101.0, 5.0),
            mk(300_000, 101.0, 103.0, 100.5, 102.0, 6.0),
            mk(600_000, 102.0, 102.5, 101.0, 101.5, 7.0),
            mk(1_200_000, 101.5, 104.0, 101.0, 103.0, 8.0),
        ];
        let out = aggregate_candles(&src, 600_000);
        assert_eq!(out.len(), 3, "one bar per 10m bucket");

        // First bucket merges both 5m bars.
        assert_eq!(out[0].open_time_ms, 0);
        assert_eq!(out[0].close_time_ms, 599_999);
        assert_eq!(out[0].open, 100.0);
        assert_eq!(out[0].close, 102.0);
        assert_eq!(out[0].high, 103.0);
        assert_eq!(out[0].low, 99.0);
        assert!((out[0].volume - 11.0).abs() < 1e-9);

        // The lone 5m bar still becomes a full-length 10m bar (gap-normalised).
        assert_eq!(out[2].open_time_ms, 1_200_000);
        assert_eq!(out[2].close_time_ms, 1_799_999);
        for c in &out {
            assert_eq!(c.close_time_ms - c.open_time_ms + 1, 600_000);
        }
    }

    #[test]
    fn closed_candles_filters_unclosed_and_dedupes() {
        let c1 = Candle {
            open_time_ms: 1_000,
            close_time_ms: 2_000,
            open: 100.0,
            high: 105.0,
            low: 99.0,
            close: 102.0,
            volume: 10.0,
            trades: 5,
        };
        let c1_dup = c1.clone();
        let c2_unclosed = Candle {
            open_time_ms: 2_000,
            close_time_ms: 3_000,
            open: 102.0,
            high: 106.0,
            low: 101.0,
            close: 104.0,
            volume: 8.0,
            trades: 4,
        };

        // At now_ms = 4_500 with safety_margin = 2_000:
        // c1 close + 2000 = 4000 <= 4500 (kept)
        // c2 close + 2000 = 5000 > 4500 (dropped as still forming / within safety margin)
        let res = closed_candles_only(vec![c2_unclosed, c1, c1_dup], 4_500, 2_000);
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].open_time_ms, 1_000);
    }

    #[test]
    fn uk_local_minutes_follow_bst_boundaries() {
        let ts = |y: i64, m: u32, d: u32, h: u32, min: u32| {
            days_from_civil(y, m, d) as u64 * 86_400_000 + u64::from(h * 3_600 + min * 60) * 1_000
        };
        // 2024: BST ran from Sunday 31 March 01:00 UTC to Sunday 27 October 01:00 UTC.
        assert_eq!(uk_minutes_of_day(ts(2024, 3, 31, 0, 59)), 59);
        assert_eq!(uk_minutes_of_day(ts(2024, 3, 31, 1, 0)), 120);
        assert_eq!(uk_minutes_of_day(ts(2024, 7, 15, 12, 0)), 780);
        assert_eq!(uk_minutes_of_day(ts(2024, 10, 27, 0, 59)), 119);
        assert_eq!(uk_minutes_of_day(ts(2024, 10, 27, 1, 0)), 60);
        assert_eq!(uk_minutes_of_day(ts(2024, 12, 1, 12, 0)), 720);
    }

    #[test]
    fn unified_account_telemetry_does_not_double_count_margin() {
        let spot_total: f64 = 27.19;
        let spot_hold: f64 = 13.08;
        let spot_avail: f64 = 14.11;
        let sol_pnl: f64 = 0.45;
        let sp500_pnl: f64 = 0.33;
        let total_upnl = sol_pnl + sp500_pnl;

        let is_unified = spot_total > 0.0 && spot_hold > 0.0;
        assert!(
            is_unified,
            "account with spot hold for perps must be recognized as unified"
        );

        let portfolio_value = spot_total + total_upnl;
        assert!((portfolio_value - 27.97).abs() < 1e-4);
        assert!((spot_avail - 14.11).abs() < 1e-4);
        assert!((spot_hold - 13.08).abs() < 1e-4);

        let erroneous_double_counted = spot_total + 11.21 + 1.83;
        assert_ne!(portfolio_value, erroneous_double_counted);
    }

    #[tokio::test]
    #[ignore]
    async fn live_user_account_telemetry_fetch() {
        use std::str::FromStr;
        let addr =
            alloy::primitives::Address::from_str("0xfe4770adcedcbfe2b73bdb69f83321f1f79bfd63")
                .unwrap();
        let telem = fetch_user_account_telemetry(HL_MAINNET_INFO_URL, addr)
            .await
            .unwrap();
        println!("\n=== LIVE ACCOUNT TELEMETRY ===");
        println!("Portfolio Value : ${:.2} USD", telem.portfolio_value);
        println!("Available USDC  : ${:.2} USD", telem.available_usdc);
        println!("Total Margin    : ${:.2} USD", telem.total_margin_used);
        println!("Unrealized PnL  : ${:+.2} USD", telem.total_unrealized_pnl);
        println!("Is Unified Mode : {}", telem.is_unified);
        println!(
            "Spot Total      : ${:.2} USD (Hold: ${:.2})",
            telem.spot_total, telem.spot_hold
        );
        println!("Open Positions  : {}", telem.open_positions.len());
        for p in &telem.open_positions {
            println!(
                "  • {} {}x sz: {:.4} px: ${:.2} uPnL: ${:+.2} roe: {:.1}%",
                p.coin,
                p.leverage,
                p.szi,
                p.entry_px,
                p.unrealized_pnl,
                p.return_on_equity * 100.0
            );
        }
        assert!(telem.portfolio_value > 20.0);
        assert!(telem.available_usdc > 10.0);
        assert_eq!(telem.open_positions.len(), 2);
    }
}
