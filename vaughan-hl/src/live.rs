//! Live execution runner (`HL-5`).
//!
//! Turns a [`CompiledStrategy`] into real Hyperliquid orders using the isolated,
//! network-bound agent key (`UnlockedAgentSigner`). The agent can only trade;
//! it can never withdraw. Safety rails, all enforced before any order is signed:
//!
//! - Only bars that **close after arming** may trigger an entry (no stale signal on start).
//! - Position sizing uses `min(account equity, allocation_usd)`, never the whole account.
//! - Hard daily-loss circuit breaker (halts the runner) and a per-trade risk cap.
//! - One position per coin; strategy `max_trades_per_window` / `max_losses_per_window`
//!   and `cooldown_bars` are honoured using real exchange fills.
//! - Isolated margin is set before the first entry on a coin.
//! - If the entry fills but its stop-loss is rejected, the position is flattened at once.
//!
//! The runner is shared by the TUI and the headless `vaughan hl run` command.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alloy::primitives::Address;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::engine::{evaluate_closed_candle_tick, BracketOrderPlan};
use crate::error::HlError;
use crate::indicators::IndicatorTable;
use crate::keystore::UnlockedAgentSigner;
use crate::market_data::{
    aggregate_candles, derived_source_interval, fetch_candles_from_hl, fetch_clearinghouse_state,
    fetch_spot_balance, fetch_user_account_telemetry, fetch_user_fills, interval_duration_ms,
    now_unix_ms, uk_day_number, Candle, HL_MAINNET_EXCHANGE_URL, HL_MAINNET_INFO_URL,
    HL_TESTNET_EXCHANGE_URL, HL_TESTNET_INFO_URL,
};
use crate::replay::Side;
use crate::strategy::CompiledStrategy;
use crate::wire::{
    format_px, format_sz, post_l1_action, resolve_asset_id, L1ActionWire, NonceAllocator,
    OrderGrouping, OrderTypeWire, OrderWire, WireTif,
};

const DAY_MS: u64 = 86_400_000;

/// Live state shown in the TUI / printed by the headless runner.
#[derive(Debug, Default, Clone)]
pub struct LiveStatus {
    pub msg: String,
    pub running: bool,
    pub halted: bool,
    pub orders_sent: u32,
}

impl LiveStatus {
    pub fn starting(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            running: true,
            halted: false,
            orders_sent: 0,
        }
    }
}

/// Handle to a running live loop.
#[derive(Clone)]
pub struct LiveHandle {
    pub stop: Arc<AtomicBool>,
    pub status: Arc<Mutex<LiveStatus>>,
    pub strategy_label: String,
    pub is_mainnet: bool,
    pub coin: String,
}

impl LiveHandle {
    pub fn snapshot(&self) -> LiveStatus {
        self.status.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

static ACTIVE_RUNS: Mutex<Vec<LiveHandle>> = Mutex::new(Vec::new());
static SIGNER: Mutex<Option<Arc<UnlockedAgentSigner>>> = Mutex::new(None);

/// All active live runs for this process (not stopped by user).
pub fn active_runs() -> Vec<LiveHandle> {
    let mut g = ACTIVE_RUNS.lock().unwrap_or_else(|e| e.into_inner());
    g.retain(|h| !h.stop.load(Ordering::SeqCst));
    g.clone()
}

/// The primary live run, if one is still running (survives screen changes).
pub fn active_run() -> Option<LiveHandle> {
    let mut g = ACTIVE_RUNS.lock().unwrap_or_else(|e| e.into_inner());
    g.retain(|h| !h.stop.load(Ordering::SeqCst));
    g.iter()
        .find(|h| h.snapshot().running)
        .cloned()
        .or_else(|| g.first().cloned())
}

/// Returns true if any live runner is actively running.
pub fn has_active_runs() -> bool {
    let mut g = ACTIVE_RUNS.lock().unwrap_or_else(|e| e.into_inner());
    g.retain(|h| !h.stop.load(Ordering::SeqCst));
    g.iter().any(|h| h.snapshot().running)
}

/// Register `h` as an active run.
pub fn register_run(h: LiveHandle) {
    ACTIVE_RUNS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(h);
}

/// Ask all active runs to stop. Existing exchange-side SL/TP orders stay in place.
pub fn stop_active() {
    let mut g = ACTIVE_RUNS.lock().unwrap_or_else(|e| e.into_inner());
    for h in g.iter() {
        h.stop.store(true, Ordering::SeqCst);
    }
    g.clear();
}

/// Cache the unlocked agent signer for this process so the password is asked once.
pub fn cache_signer(s: Arc<UnlockedAgentSigner>) {
    *SIGNER.lock().unwrap_or_else(|e| e.into_inner()) = Some(s);
}

/// The cached signer, if it matches the requested network and backing account.
pub fn cached_signer(is_mainnet: bool, backing: Address) -> Option<Arc<UnlockedAgentSigner>> {
    SIGNER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .filter(|s| s.is_mainnet() == is_mainnet && s.backing_account() == backing)
        .cloned()
}

/// Drop the cached signer (e.g. after re-connecting with a new agent key).
pub fn clear_signer() {
    *SIGNER.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Static configuration of one live run.
#[derive(Clone)]
pub struct LiveConfig {
    pub is_mainnet: bool,
    pub backing: Address,
    pub strategy: CompiledStrategy,
    pub strategy_label: String,
    /// Capital the strategy may size against (never the whole account unless equal).
    pub allocation_usd: f64,
    /// Halt the runner once realised PnL today (UTC) reaches this loss.
    pub max_daily_loss_usd: f64,
    pub poll_secs: u64,
}

impl LiveConfig {
    pub fn new(
        is_mainnet: bool,
        backing: Address,
        strategy: CompiledStrategy,
        strategy_label: String,
        allocation_usd: f64,
    ) -> Self {
        Self {
            is_mainnet,
            backing,
            strategy,
            strategy_label,
            allocation_usd,
            max_daily_loss_usd: allocation_usd * 0.10,
            poll_secs: 10,
        }
    }

    fn info_url(&self) -> &'static str {
        if self.is_mainnet {
            HL_MAINNET_INFO_URL
        } else {
            HL_TESTNET_INFO_URL
        }
    }

    fn exchange_url(&self) -> &'static str {
        if self.is_mainnet {
            HL_MAINNET_EXCHANGE_URL
        } else {
            HL_TESTNET_EXCHANGE_URL
        }
    }
}

/// Default capital allocation (USD) when the user has not set one.
pub const DEFAULT_ALLOCATION_USD: f64 = 100.0;

/// Allocation from `VAUGHAN_HL_ALLOCATION_USD`, else [`DEFAULT_ALLOCATION_USD`].
pub fn allocation_from_env() -> f64 {
    std::env::var("VAUGHAN_HL_ALLOCATION_USD")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 10.0)
        .unwrap_or(DEFAULT_ALLOCATION_USD)
}

// ---------------------------------------------------------------------------
// Exchange queries
// ---------------------------------------------------------------------------

async fn info_post(info_url: &str, payload: &Value) -> Result<Value, HlError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| HlError::MarketData(e.to_string()))?;
    let resp = client
        .post(info_url)
        .json(payload)
        .send()
        .await
        .map_err(|e| HlError::MarketData(format!("info request failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(HlError::MarketData(format!(
            "info returned HTTP {}",
            resp.status()
        )));
    }
    resp.json::<Value>()
        .await
        .map_err(|e| HlError::MarketData(format!("info decode failed: {e}")))
}

fn dex_of(coin: &str) -> Option<&str> {
    coin.split_once(':').map(|(d, _)| d)
}

/// Resolve `(asset_id, szDecimals)` for `coin` from the live `meta` (HIP-3 aware).
pub async fn fetch_asset_meta(info_url: &str, coin: &str) -> Result<(u32, u32), HlError> {
    let (dex_idx, meta_req) = if let Some(dex) = dex_of(coin) {
        let dexes = info_post(info_url, &json!({ "type": "perpDexs" })).await?;
        let arr = dexes
            .as_array()
            .ok_or_else(|| HlError::MarketData("perpDexs: expected array".into()))?;
        let idx = arr
            .iter()
            .position(|v| v.get("name").and_then(|n| n.as_str()) == Some(dex))
            .ok_or_else(|| HlError::MarketData(format!("unknown perp dex '{dex}'")))?;
        (idx as u32, json!({ "type": "meta", "dex": dex }))
    } else {
        (0, json!({ "type": "meta" }))
    };
    let meta = info_post(info_url, &meta_req).await?;
    let universe = meta
        .get("universe")
        .and_then(|u| u.as_array())
        .ok_or_else(|| HlError::MarketData("meta: missing universe".into()))?;
    let (i, entry) = universe
        .iter()
        .enumerate()
        .find(|(_, e)| e.get("name").and_then(|n| n.as_str()) == Some(coin))
        .ok_or_else(|| HlError::MarketData(format!("coin '{coin}' not found in meta")))?;
    let sz = entry
        .get("szDecimals")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| HlError::MarketData("meta: missing szDecimals".into()))?;
    Ok((resolve_asset_id(dex_idx, i as u32), sz as u32))
}

fn user_payload(kind: &str, user: Address, coin: &str) -> Value {
    let mut v = json!({ "type": kind, "user": format!("{user:#x}") });
    if let Some(d) = dex_of(coin) {
        v["dex"] = json!(d);
    }
    v
}

/// Signed position size of `coin` (positive long, negative short, 0 flat).
pub async fn fetch_position_size(
    info_url: &str,
    user: Address,
    coin: &str,
) -> Result<f64, HlError> {
    let st = info_post(info_url, &user_payload("clearinghouseState", user, coin)).await?;
    let mut szi = 0.0;
    if let Some(arr) = st.get("assetPositions").and_then(|a| a.as_array()) {
        for ap in arr {
            let pos = ap.get("position").unwrap_or(&Value::Null);
            if pos.get("coin").and_then(|c| c.as_str()) == Some(coin) {
                szi = pos
                    .get("szi")
                    .and_then(|s| s.as_str())
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(0.0);
            }
        }
    }
    Ok(szi)
}

async fn fetch_open_order_ids(
    info_url: &str,
    user: Address,
    coin: &str,
) -> Result<Vec<u64>, HlError> {
    let v = info_post(info_url, &user_payload("openOrders", user, coin)).await?;
    Ok(v.as_array()
        .map(|a| {
            a.iter()
                .filter(|o| o.get("coin").and_then(|c| c.as_str()) == Some(coin))
                .filter_map(|o| o.get("oid").and_then(|x| x.as_u64()))
                .collect()
        })
        .unwrap_or_default())
}

/// Fetch the closed candles needed to evaluate `interval` (aggregating derived intervals).
pub async fn fetch_live_candles(
    info_url: &str,
    coin: &str,
    interval: &str,
    min_bars: u64,
) -> Result<Vec<Candle>, HlError> {
    let interval_ms = interval_duration_ms(interval)?;
    let days = ((min_bars * interval_ms) / DAY_MS + 2).min(1_825) as u32;
    if let Some(src) = derived_source_interval(interval) {
        let base = fetch_candles_from_hl(info_url, coin, src, days).await?;
        let mut agg = aggregate_candles(&base, interval_ms);
        // A trailing bucket that is missing its last source bar is still forming.
        if let (Some(last), Some(base_last)) = (agg.last(), base.last()) {
            if base_last.close_time_ms < last.close_time_ms {
                agg.pop();
            }
        }
        return Ok(agg);
    }
    fetch_candles_from_hl(info_url, coin, interval, days).await
}

fn htf_source_interval(s: &CompiledStrategy) -> Option<String> {
    s.config
        .indicators
        .htf
        .as_ref()
        .and_then(|h| h.source.clone())
        .or_else(|| {
            s.config
                .indicators
                .htf2
                .as_ref()
                .and_then(|h| h.source.clone())
        })
}

// ---------------------------------------------------------------------------
// Order execution
// ---------------------------------------------------------------------------

async fn send_action(
    cfg: &LiveConfig,
    signer: &UnlockedAgentSigner,
    nonces: &NonceAllocator,
    action: &L1ActionWire,
) -> Result<Value, HlError> {
    let nonce = nonces.next_nonce(now_unix_ms());
    let sig = signer.sign_l1_action_checked(action, nonce, None, cfg.is_mainnet)?;
    post_l1_action(cfg.exchange_url(), action, nonce, &sig).await
}

/// Per-order result from an `order` response (`Err` carries the exchange error text).
fn order_statuses(resp: &Value) -> Vec<Result<Value, String>> {
    resp.pointer("/response/data/statuses")
        .and_then(|s| s.as_array())
        .map(|a| {
            a.iter()
                .map(|s| match s.get("error") {
                    Some(e) => Err(e.as_str().unwrap_or("unknown error").to_string()),
                    None => Ok(s.clone()),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn reduce_only_close(
    asset_id: u32,
    close_long: bool,
    size: &str,
    ref_px: f64,
    sz_decimals: u32,
) -> Result<L1ActionWire, HlError> {
    // Marketable IOC with a wide 5% band; reduce-only so it can never open a position.
    let px = if close_long {
        ref_px * 0.95
    } else {
        ref_px * 1.05
    };
    Ok(L1ActionWire::Order {
        orders: vec![OrderWire {
            a: asset_id,
            b: !close_long,
            p: format_px(px, sz_decimals)?,
            s: size.to_string(),
            r: true,
            t: OrderTypeWire::Limit { tif: WireTif::Ioc },
            c: None,
        }],
        grouping: OrderGrouping::Na,
        builder: None,
    })
}

/// Execution telemetry record written to `data/executions.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveExecutionRecord {
    pub timestamp_ms: u64,
    pub strategy: String,
    pub coin: String,
    pub side: String,
    pub signal_px: f64,
    pub fill_px: f64,
    pub slippage_bps: f64,
    pub size: String,
    pub notional_usd: f64,
    pub risk_1r_usd: f64,
    pub stop_px: String,
    pub tp_px: String,
}

pub fn record_execution(record: &LiveExecutionRecord) {
    if let Ok(line) = serde_json::to_string(record) {
        use std::io::Write;
        let _ = std::fs::create_dir_all("data");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("data/executions.jsonl")
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// Place the entry + SL/TP bracket and verify the protective orders were accepted.
async fn place_bracket(
    cfg: &LiveConfig,
    signer: &UnlockedAgentSigner,
    nonces: &NonceAllocator,
    plan: &BracketOrderPlan,
    asset_id: u32,
    sz_decimals: u32,
) -> Result<String, HlError> {
    let resp = send_action(cfg, signer, nonces, &plan.l1_action).await?;
    let statuses = order_statuses(&resp);
    let entry = statuses
        .first()
        .ok_or_else(|| HlError::Api(format!("order response had no statuses: {resp}")))?;
    let filled = match entry {
        Err(msg) => return Err(HlError::Api(format!("entry rejected: {msg}"))),
        Ok(s) => s.get("filled").cloned(),
    };
    let Some(filled) = filled else {
        return Err(HlError::Api(
            "entry did not fill (IOC expired); no position opened".into(),
        ));
    };
    let total_sz = filled
        .get("totalSz")
        .and_then(|v| v.as_str())
        .unwrap_or(&plan.size_str)
        .to_string();
    let avg_px: f64 = filled
        .get("avgPx")
        .and_then(|v| v.as_str())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);

    let slippage_bps = if plan.signal_px > 0.0 && avg_px > 0.0 {
        ((avg_px - plan.signal_px).abs() / plan.signal_px) * 10_000.0
    } else {
        0.0
    };

    // The stop-loss is the second order in every bracket; if it is not live we are naked.
    if let Some(Err(sl_err)) = statuses.get(1) {
        let close_long = plan.side == Side::Long;
        let close = reduce_only_close(asset_id, close_long, &total_sz, avg_px, sz_decimals)?;
        let flat = send_action(cfg, signer, nonces, &close).await;
        return Err(HlError::RiskViolation(format!(
            "stop-loss rejected ({sl_err}); emergency flatten {}",
            if flat.is_ok() {
                "sent"
            } else {
                "FAILED - CLOSE MANUALLY"
            }
        )));
    }
    let tp_note = if statuses.iter().skip(2).any(|s| s.is_err()) {
        " (a take-profit leg was rejected; stop-loss is live)"
    } else {
        ""
    };

    record_execution(&LiveExecutionRecord {
        timestamp_ms: now_unix_ms(),
        strategy: cfg.strategy_label.clone(),
        coin: plan.coin.clone(),
        side: plan.side.as_str().to_string(),
        signal_px: plan.signal_px,
        fill_px: avg_px,
        slippage_bps,
        size: total_sz.clone(),
        notional_usd: plan.notional_usd,
        risk_1r_usd: plan.risk_1r_usd,
        stop_px: plan.stop_px_str.clone(),
        tp_px: plan.tp_px_str.clone(),
    });

    Ok(format!(
        "{} {} {total_sz} @ {avg_px} (slip {:.1} bps) | SL {} TP {}{tp_note}",
        plan.side.as_str(),
        plan.coin,
        slippage_bps,
        plan.stop_px_str,
        plan.tp_px_str
    ))
}

async fn close_position(
    cfg: &LiveConfig,
    signer: &UnlockedAgentSigner,
    nonces: &NonceAllocator,
    asset_id: u32,
    sz_decimals: u32,
    szi: f64,
    ref_px: f64,
) -> Result<String, HlError> {
    let coin = &cfg.strategy.config.market.coin;
    let size = format_sz(szi.abs(), sz_decimals)?;
    let close = reduce_only_close(asset_id, szi > 0.0, &size, ref_px, sz_decimals)?;
    let resp = send_action(cfg, signer, nonces, &close).await?;
    if let Some(Err(e)) = order_statuses(&resp).first() {
        return Err(HlError::Api(format!("exit order rejected: {e}")));
    }
    // Remove the now-orphaned SL/TP legs.
    if let Ok(oids) = fetch_open_order_ids(cfg.info_url(), cfg.backing, coin).await {
        if !oids.is_empty() {
            let cancels = L1ActionWire::Cancel {
                cancels: oids.into_iter().map(|o| (asset_id, o)).collect(),
            };
            let _ = send_action(cfg, signer, nonces, &cancels).await;
        }
    }
    Ok(format!("EXIT {coin} {size} (indicator exit signal)"))
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

fn set_msg(status: &Arc<Mutex<LiveStatus>>, msg: String) {
    if let Ok(mut g) = status.lock() {
        g.msg = msg;
    }
}

async fn sleep_checked(stop: &AtomicBool, secs: u64) {
    for _ in 0..secs.max(1) {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Spawn-ready live loop. Always leaves `status.running == false` when it ends.
pub async fn run_live(
    cfg: LiveConfig,
    signer: Arc<UnlockedAgentSigner>,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<LiveStatus>>,
) {
    if let Ok(mut g) = status.lock() {
        g.running = true;
        g.halted = false;
        if g.msg.is_empty() || g.msg == "LIVE starting..." {
            g.msg = format!("LIVE [{}] starting...", cfg.strategy_label);
        }
    }
    let res = run_live_inner(&cfg, &signer, &stop, &status).await;
    if let Ok(mut g) = status.lock() {
        g.running = false;
        match res {
            Ok(()) => g.msg = "LIVE stopped. Open positions keep their exchange-side SL/TP.".into(),
            Err(e) => {
                g.halted = true;
                g.msg = format!("LIVE HALTED: {e}");
            }
        }
    }
}

async fn run_live_inner(
    cfg: &LiveConfig,
    signer: &UnlockedAgentSigner,
    stop: &AtomicBool,
    status: &Arc<Mutex<LiveStatus>>,
) -> Result<(), HlError> {
    if signer.is_mainnet() != cfg.is_mainnet || signer.backing_account() != cfg.backing {
        return Err(HlError::RiskViolation(
            "agent keystore does not match the selected network / backing account".into(),
        ));
    }
    if !cfg.allocation_usd.is_finite() || cfg.allocation_usd < 10.0 {
        return Err(HlError::RiskViolation(
            "allocation must be at least $10".into(),
        ));
    }
    let coin = cfg.strategy.config.market.coin.clone();
    let interval = cfg.strategy.config.market.interval.clone();
    let interval_ms = interval_duration_ms(&interval)?;
    let info_url = cfg.info_url();
    let leverage = cfg.strategy.config.market.leverage;

    let (asset_id, sz_decimals) = loop {
        match fetch_asset_meta(info_url, &coin).await {
            Ok(m) => break m,
            Err(e) => {
                set_msg(
                    status,
                    format!("LIVE: resolving {coin} market: {e} (retrying)"),
                );
                sleep_checked(stop, 5).await;
                if stop.load(Ordering::SeqCst) {
                    return Ok(());
                }
            }
        }
    };

    let armed_at = now_unix_ms();
    let nonces = NonceAllocator::new(0);
    let warmup = cfg.strategy.config.indicators.required_warmup_bars() as u64;
    let min_bars = warmup + 200;
    let htf_iv = htf_source_interval(&cfg.strategy);
    let mut last_eval_open = 0u64;
    let mut executed: HashSet<u64> = HashSet::new();
    let mut leverage_set = false;
    let mut event = String::from("waiting for the next bar close");

    set_msg(
        status,
        format!(
            "LIVE [{}] {coin} {interval} · allocation ${:.0} · {event}",
            cfg.strategy_label, cfg.allocation_usd
        ),
    );

    while !stop.load(Ordering::SeqCst) {
        sleep_checked(stop, cfg.poll_secs).await;
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let now = now_unix_ms();

        let candles = match fetch_live_candles(info_url, &coin, &interval, min_bars).await {
            Ok(c) if !c.is_empty() => c,
            Ok(_) => {
                event = "no candle data yet".into();
                continue;
            }
            Err(e) => {
                event = format!("data error: {e} (retrying)");
                set_msg(status, format!("LIVE [{}] {event}", cfg.strategy_label));
                continue;
            }
        };
        let Some(last) = candles.last().cloned() else {
            continue;
        };
        let heartbeat = format!(
            "LIVE [{}] {coin} {interval} · last close ${:.4}",
            cfg.strategy_label, last.close
        );
        if last.open_time_ms == last_eval_open {
            set_msg(status, format!("{heartbeat} · {event}"));
            continue;
        }
        last_eval_open = last.open_time_ms;

        if now.saturating_sub(last.close_time_ms) > interval_ms / 2 {
            event = "skipped a stale bar (late data)".into();
            set_msg(status, format!("{heartbeat} · {event}"));
            continue;
        }
        if last.close_time_ms <= armed_at {
            event = "waiting for the first bar that closes after arming".into();
            set_msg(status, format!("{heartbeat} · {event}"));
            continue;
        }

        let htf = match &htf_iv {
            Some(iv) => match fetch_live_candles(info_url, &coin, iv, 800).await {
                Ok(c) if c.len() > 1 => Some(c),
                _ => {
                    event = "higher-timeframe data unavailable; skipped bar".into();
                    set_msg(status, format!("{heartbeat} · {event}"));
                    continue;
                }
            },
            None => None,
        };

        let telem = fetch_user_account_telemetry(info_url, cfg.backing)
            .await
            .ok();
        let equity = if let Some(t) = telem {
            t.portfolio_value
        } else {
            let perps_eq = fetch_clearinghouse_state(info_url, cfg.backing)
                .await
                .unwrap_or(0.0);
            let spot_eq = fetch_spot_balance(info_url, cfg.backing)
                .await
                .unwrap_or(0.0);
            if perps_eq > 0.0 {
                perps_eq
            } else {
                spot_eq
            }
        };
        if equity <= 0.0 {
            event = "account query returned $0.00 balance".into();
            set_msg(status, format!("{heartbeat} · {event}"));
            continue;
        }
        let szi = match fetch_position_size(info_url, cfg.backing, &coin).await {
            Ok(s) => s,
            Err(e) => {
                event = format!("position query failed: {e}");
                set_msg(status, format!("{heartbeat} · {event}"));
                continue;
            }
        };

        let idx = candles.len() - 1;
        if szi.abs() > 0.0 {
            let table = IndicatorTable::compute_with_source(
                &candles,
                &cfg.strategy.config.indicators,
                htf.as_deref(),
            );
            let exit = if szi > 0.0 {
                cfg.strategy.should_signal_exit_long(&candles, &table, idx)
            } else {
                cfg.strategy.should_signal_exit_short(&candles, &table, idx)
            };
            if exit {
                event = match close_position(
                    cfg,
                    signer,
                    &nonces,
                    asset_id,
                    sz_decimals,
                    szi,
                    last.close,
                )
                .await
                {
                    Ok(s) => {
                        if let Ok(mut g) = status.lock() {
                            g.orders_sent += 1;
                        }
                        s
                    }
                    Err(e) => format!("EXIT FAILED: {e}"),
                };
            } else {
                event = format!("in position ({szi:+}); SL/TP resting on exchange");
            }
            set_msg(status, format!("{heartbeat} · {event}"));
            continue;
        }

        // ---- flat: daily-loss breaker + window limits from real fills ----
        let fills = match fetch_user_fills(info_url, cfg.backing).await {
            Ok(f) => f,
            Err(e) => {
                event = format!("cannot verify daily limits ({e}); no entry");
                set_msg(status, format!("{heartbeat} · {event}"));
                continue;
            }
        };
        let utc_day = now / DAY_MS;
        let pnl_today: f64 = fills
            .iter()
            .filter(|f| f.time_ms / DAY_MS == utc_day)
            .map(|f| f.closed_pnl - f.fee)
            .sum();
        if pnl_today <= -cfg.max_daily_loss_usd {
            return Err(HlError::RiskViolation(format!(
                "daily loss limit reached (${pnl_today:.2} <= -${:.2}); trading stopped for today",
                cfg.max_daily_loss_usd
            )));
        }
        let uk_today = uk_day_number(now);
        let mut open_oids = HashSet::new();
        let mut loss_oids = HashSet::new();
        let mut last_fill_ms = 0u64;
        let mut recent_closes: Vec<&crate::market_data::LiveTradeFill> = Vec::new();
        for f in fills.iter().filter(|f| f.coin == coin) {
            last_fill_ms = last_fill_ms.max(f.time_ms);
            if f.dir.starts_with("Close") {
                recent_closes.push(f);
            }
            if uk_day_number(f.time_ms) != uk_today {
                continue;
            }
            if f.dir.starts_with("Open") {
                open_oids.insert(f.oid);
            } else if f.dir.starts_with("Close") && f.closed_pnl < 0.0 {
                loss_oids.insert(f.oid);
            }
        }
        recent_closes.sort_by_key(|f| std::cmp::Reverse(f.time_ms));
        let consecutive_losses = recent_closes
            .iter()
            .take_while(|f| f.closed_pnl < 0.0)
            .count();

        if consecutive_losses >= 2 {
            let loss_cooldown_ms =
                (cfg.strategy.config.exit.cooldown_bars as u64 * 3).max(4) * interval_ms;
            if last_fill_ms > 0
                && last.close_time_ms.saturating_sub(last_fill_ms) < loss_cooldown_ms
            {
                event = format!("cooling down after {consecutive_losses} consecutive losses");
                set_msg(status, format!("{heartbeat} · {event}"));
                continue;
            }
        }
        if let Some(max) = cfg.strategy.config.entry.max_trades_per_window {
            if open_oids.len() >= max {
                event = format!("daily entry limit reached ({max})");
                set_msg(status, format!("{heartbeat} · {event}"));
                continue;
            }
        }
        if let Some(max) = cfg.strategy.config.entry.max_losses_per_window {
            if loss_oids.len() >= max {
                event = format!("daily loss-count limit reached ({max})");
                set_msg(status, format!("{heartbeat} · {event}"));
                continue;
            }
        }
        let cooldown_ms = cfg.strategy.config.exit.cooldown_bars as u64 * interval_ms;
        if last_fill_ms > 0 && last.close_time_ms.saturating_sub(last_fill_ms) < cooldown_ms {
            event = "cooling down after last trade".into();
            set_msg(status, format!("{heartbeat} · {event}"));
            continue;
        }
        if executed.contains(&last.open_time_ms) {
            continue;
        }

        let eff_equity = equity.min(cfg.allocation_usd);
        let plan = match evaluate_closed_candle_tick(
            &candles,
            htf.as_deref(),
            &cfg.strategy,
            eff_equity,
            asset_id,
            sz_decimals,
            None,
        ) {
            Ok(Some(p)) => p,
            Ok(None) => {
                event = format!("flat (no signal on closed bar) · equity ${equity:.2}");
                set_msg(status, format!("{heartbeat} · {event}"));
                continue;
            }
            Err(e) => {
                event = format!("no trade: {e}");
                set_msg(status, format!("{heartbeat} · {event}"));
                continue;
            }
        };

        if plan.risk_1r_usd > cfg.max_daily_loss_usd
            || plan.notional_usd > eff_equity * f64::from(leverage) + 1.0
        {
            event = format!(
                "signal rejected by risk guard (1R ${:.2}, notional ${:.2})",
                plan.risk_1r_usd, plan.notional_usd
            );
            set_msg(status, format!("{heartbeat} · {event}"));
            continue;
        }
        executed.insert(last.open_time_ms);

        if !leverage_set {
            let lev = L1ActionWire::UpdateLeverage {
                asset: asset_id,
                is_cross: false,
                leverage,
            };
            match send_action(cfg, signer, &nonces, &lev).await {
                Ok(_) => leverage_set = true,
                Err(e) => {
                    event = format!("could not set isolated {leverage}x leverage: {e}; no entry");
                    set_msg(status, format!("{heartbeat} · {event}"));
                    continue;
                }
            }
        }

        event = match place_bracket(cfg, signer, &nonces, &plan, asset_id, sz_decimals).await {
            Ok(s) => {
                if let Ok(mut g) = status.lock() {
                    g.orders_sent += 1;
                }
                format!("ENTERED {s}")
            }
            Err(e) => format!("ENTRY FAILED: {e}"),
        };
        set_msg(status, format!("{heartbeat} · {event}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_split_errors_from_fills() {
        let resp = json!({"status":"ok","response":{"type":"order","data":{"statuses":[
            {"filled":{"totalSz":"0.5","avgPx":"100.0","oid":1}},
            {"error":"Order has invalid price."},
            {"resting":{"oid":3}}
        ]}}});
        let st = order_statuses(&resp);
        assert_eq!(st.len(), 3);
        assert!(st[0].is_ok());
        assert_eq!(st[1].as_ref().unwrap_err(), "Order has invalid price.");
        assert!(st[2].is_ok());
    }

    #[test]
    fn reduce_only_close_never_opens_and_flips_side() {
        let a = reduce_only_close(7, true, "0.5", 100.0, 2).unwrap();
        let L1ActionWire::Order { orders, .. } = a else {
            panic!("expected order")
        };
        assert!(orders[0].r);
        assert!(!orders[0].b, "closing a long must sell");
        assert_eq!(orders[0].p, "95");
    }

    #[test]
    fn json_action_matches_exchange_schema() {
        let a = reduce_only_close(7, false, "1", 100.0, 2).unwrap();
        let j = a.to_json_action();
        assert_eq!(j["type"], "order");
        assert_eq!(j["orders"][0]["t"]["limit"]["tif"], "Ioc");
        assert_eq!(j["orders"][0]["r"], true);
        assert_eq!(j["grouping"], "na");
    }

    #[test]
    fn allocation_env_is_floored() {
        assert!(allocation_from_env() >= 10.0);
    }

    #[test]
    fn dex_prefix_is_detected() {
        assert_eq!(dex_of("xyz:CL"), Some("xyz"));
        assert_eq!(dex_of("BTC"), None);
    }

    #[test]
    fn live_execution_record_serializes_cleanly() {
        let rec = LiveExecutionRecord {
            timestamp_ms: 1730724000000,
            strategy: "test_fast".into(),
            coin: "BTC".into(),
            side: "LONG".into(),
            signal_px: 77400.0,
            fill_px: 77415.5,
            slippage_bps: 2.0,
            size: "0.00019".into(),
            notional_usd: 15.0,
            risk_1r_usd: 0.05,
            stop_px: "77200".into(),
            tp_px: "77800".into(),
        };
        let s = serde_json::to_string(&rec).unwrap();
        assert!(s.contains("\"slippage_bps\":2.0"));
        assert!(s.contains("\"strategy\":\"test_fast\""));
    }
}
